//! The editing tools: which one is selected, how it's reached, and what each does to the grid.
//!
//! The lasso is the exception — it lives in `selection`, since it tracks the pointer off the
//! grid and carries a whole floating layer with it. Everything here acts on a single cell that
//! the pointer is already over.

use super::*;
use compact::Toggled as _;

#[derive(PartialEq, Eq, Clone, Copy, Debug, Serialize, Deserialize)]
pub enum Tool {
    Pencil,
    /// Editor-only
    FloodFill,
    LineAlongLane,
    /// Editor only
    Lasso,
    /// Solver only
    Annotate,
}

/// For a mouse, this is what the mouse buttons mean; other kinds of input have to specify some other way.
/// There may also be fallback behavior if the current cell is already the color that matches the intent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum PaintIntent {
    /// Currently selected palette color (primary button)
    #[default]
    Current,
    Background,
    /// UNSOLVED, if we're in solve mode, or BACKGROUND if we're in edit mode
    Erase,
}

/// What pencil, line and flood fill need to know about the pointer in a frame.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct PaintPointer {
    pub(super) pressed: bool,
    pub(super) down: bool,
    pub(super) released: bool,
    pub(super) clicked: bool,
    pub(super) intent: PaintIntent,
}

impl PaintPointer {
    pub(super) fn from_egui(pointer: &egui::PointerState) -> PaintPointer {
        PaintPointer {
            pressed: pointer.any_pressed(),
            down: pointer.any_down(),
            released: pointer.any_released(),
            clicked: pointer.any_click(),
            intent: if pointer.middle_down() {
                PaintIntent::Erase
            } else if pointer.secondary_down() {
                PaintIntent::Background
            } else {
                PaintIntent::Current
            },
        }
    }
}

impl Tool {
    /// The tool row, in order.
    const ALL: [Tool; 5] = [
        Tool::Pencil,
        Tool::LineAlongLane,
        Tool::FloodFill,
        Tool::Lasso,
        Tool::Annotate,
    ];

    /// Whether the editor (`!solving`) or the solver (`solving`) offers this tool.
    fn offered(self, solving: bool) -> bool {
        match self {
            Tool::LineAlongLane => true,
            Tool::Pencil | Tool::FloodFill | Tool::Lasso => !solving,
            Tool::Annotate => solving,
        }
    }
}

/// The icon, bare-key shortcut, tooltip key-name, and description for each tool.
fn tool_appearance(tool: Tool) -> (&'static str, egui::Key, char, &'static str) {
    match tool {
        Tool::Pencil => (icons::ICON_BRUSH, egui::Key::P, 'P', "Pencil"),
        Tool::LineAlongLane => (
            icons::ICON_LINE_START,
            egui::Key::L,
            'L',
            "Line along a lane",
        ),
        Tool::FloodFill => (
            icons::ICON_FORMAT_COLOR_FILL,
            egui::Key::F,
            'F',
            "Flood Fill",
        ),
        Tool::Lasso => (
            icons::ICON_LASSO_SELECT,
            egui::Key::S,
            'S',
            "Lasso select: draw a loop, then drag to move what's inside",
        ),
        Tool::Annotate => (
            icons::ICON_SQUARE_FOOT,
            egui::Key::A,
            'A',
            "Annotate: click to mark one cell, drag to measure a span",
        ),
    }
}

/// One entry in the tool row: a toggle button that its key also reaches. `typing` suppresses the
/// key while a `TextEdit` has the keyboard. Grouped like this so that hiding a tool also disables
/// its shortcut.
///
/// Returns whether the tool's key was pressed. The button picks the tool itself, but the key
/// means "toggle", which only the caller can act on — it's the one that knows what came before.
fn tool_button(
    ui: &mut egui::Ui,
    current_tool: &mut Tool,
    tool: Tool,
    typing: bool,
    layout: ScreenLayout,
) -> bool {
    let (icon, key, ch, description) = tool_appearance(tool);
    let response = match layout {
        ScreenLayout::Desktop => ui.selectable_value(
            current_tool,
            tool,
            egui::RichText::new(icon).size(compact::ICON_SIZE),
        ),
        ScreenLayout::Compact => {
            let response =
                ui.add(compact::icon_button(icon, layout).toggled(ui, *current_tool == tool));
            if response.clicked() {
                *current_tool = tool;
            }
            response
        }
    };
    response.on_hover_text(if tool == Tool::Annotate {
        format!("{description} (press {ch} or hold Shift, press again to go back)")
    } else {
        format!("{description} (press {ch}, again to go back)")
    });
    !typing && ui.input(|i| i.key_pressed(key))
}

impl CanvasGui {
    pub(super) fn tool_selector(&mut self, ui: &mut egui::Ui, layout: ScreenLayout) {
        let was = self.current_tool;
        let solving = self.solving;

        // Same story as in `common_sidebar_items`: no modifiers here either.
        let typing = ui.ctx().egui_wants_keyboard_input();

        // Which tool a key asked for this frame, acted on once the row is done: asking for the
        // tool you're already in means going back to the one before it, and only `self` knows
        // which that was.
        let mut requested = None;

        let mut row = |ui: &mut egui::Ui| {
            for tool in Tool::ALL {
                if tool.offered(solving)
                    && tool_button(ui, &mut self.current_tool, tool, typing, layout)
                {
                    requested = Some(tool);
                }
            }
        };
        match layout {
            ScreenLayout::Desktop => centered_row(ui, "tools", row),
            ScreenLayout::Compact => row(ui),
        }

        if let Some(tool) = requested {
            self.current_tool = if was == tool {
                self.previous_tool
            } else {
                tool
            };
        }
        // `previous_tool` starts out equal to `current_tool`, so the toggle above is a no-op
        // until there has actually been something to go back to.
        if was != self.current_tool {
            self.previous_tool = was;
        }

        // Leaving the lasso commits whatever it was holding, so no other tool ever has to think
        // about a floating layer.
        if was == Tool::Lasso && self.current_tool != Tool::Lasso {
            self.clear_selection();
        }
    }

    /// The tool the pointer is actually driving this frame.
    ///
    /// Holding shift is a momentary switch to the annotate tool, so a solver can measure a run
    /// without giving up the line tool. A drag already in flight keeps whatever tool started it:
    /// letting go of shift halfway through must not hand that drag to something else.
    pub(super) fn effective_tool(&self, ui: &egui::Ui) -> Tool {
        if self.annotate_drag.is_some() {
            Tool::Annotate
        } else if self.line_tool_state.is_some() {
            Tool::LineAlongLane
        } else if self.solving && ui.input(|i| i.modifiers.shift) {
            Tool::Annotate
        } else {
            self.current_tool
        }
    }

    fn flood_fill(&mut self, start: CellIdx, color: Color) {
        let picture = self.document.solution();
        let target_color = picture.cells()[start];
        if target_color == color {
            return; // Nothing to do
        }

        let mut changes = HashMap::new();
        let mut q = std::collections::VecDeque::new();
        let mut visited = std::collections::HashSet::new();

        q.push_back(start);
        visited.insert(start);

        while let Some(cell) = q.pop_front() {
            changes.insert(cell, color);

            for neighbor in picture.neighbor_cells(cell) {
                if picture.cells()[neighbor] == target_color && visited.insert(neighbor) {
                    q.push_back(neighbor);
                }
            }
        }

        self.perform(Action::ChangeColor { changes }, ActionMood::Normal);
    }

    /// Apply `tool` (which is `effective_tool`, not necessarily `current_tool`) at `cell`, in
    /// response to a press or drag over the grid.
    ///
    /// The lasso never arrives here: it has to keep tracking the pointer once it leaves the
    /// grid, so `canvas_with_clues` deals with it before there is a cell to speak of.
    pub(super) fn pointer_tool_input(&mut self, cell: CellIdx, pointer: PaintPointer, tool: Tool) {
        let picture = self.document.solution();
        // The meaning of "erase" differs depending on the mode:
        let erased = if picture.palette().contains_key(&UNSOLVED) {
            UNSOLVED
        } else {
            BACKGROUND
        };
        let under_pointer = picture.cells()[cell];
        let paint_color = match pointer.intent {
            PaintIntent::Erase => erased,
            // While solving, a cell that's already been ruled out goes back to undecided, so the
            // same button both makes and unmakes the mark. In the editor there's nothing to go
            // back to: `unknown` is background there, which is what this said all along.
            PaintIntent::Background if self.solving && under_pointer == BACKGROUND => erased,
            PaintIntent::Background => BACKGROUND,
            PaintIntent::Current if under_pointer != self.current_color => self.current_color,
            // Likewise for the color the user is painting with — undecided, not ruled out.
            PaintIntent::Current if self.solving => erased,
            PaintIntent::Current => BACKGROUND,
        };
        // Paranoia, since it would cause a crash.
        debug_assert!(
            picture.palette().contains_key(&paint_color),
            "painting with {paint_color:?}, which is not in the palette"
        );
        let paint_color = if picture.palette().contains_key(&paint_color) {
            paint_color
        } else {
            BACKGROUND
        };

        match tool {
            Tool::Pencil => {
                let mood = if pointer.pressed {
                    self.drag_start_color = paint_color;
                    ActionMood::Normal
                } else {
                    ActionMood::Merge
                };

                self.perform(
                    Action::ChangeColor {
                        changes: [(cell, self.drag_start_color)].into(),
                    },
                    mood,
                );
            }
            Tool::FloodFill => {
                if pointer.clicked {
                    self.flood_fill(cell, paint_color);
                }
            }
            Tool::LineAlongLane => {
                if pointer.pressed {
                    self.drag_start_color = paint_color;
                    self.line_tool_state = Some(cell);

                    self.perform(
                        Action::ChangeColor {
                            changes: [(cell, self.drag_start_color)].into(),
                        },
                        ActionMood::Normal,
                    );
                } else if pointer.down {
                    if let Some(start) = self.line_tool_state {
                        let changes = self.line_between(start, cell);
                        self.perform(Action::ChangeColor { changes }, ActionMood::ReplaceAction);
                    }
                } else if pointer.released {
                    self.line_tool_state = None;
                }
            }
            // Both are handled above: the lasso because the pointer is still allowed to be off
            // the grid, and annotating because it snaps to a border rather than to a cell.
            Tool::Lasso | Tool::Annotate => {}
        }
    }

    /// The cells between two points along whichever lane best matches the drag.
    fn line_between(&self, start: CellIdx, end: CellIdx) -> HashMap<CellIdx, Color> {
        let picture = self.document.solution();

        let mut changes = HashMap::new();
        if start == end {
            changes.insert(end, self.drag_start_color);
            return changes;
        }

        // Cell *centres*, not raw origins: a triangle's centroid sits off-corner and at a
        // different offset for ▲ than ▼, so mixing origins would misjudge lane direction
        // whenever a lane's cells alternate orientation.
        let center = |cell: CellIdx| picture.cell_shape(cell).center(picture.cell_origin(cell));
        let (start_center, end_center) = (center(start), center(end));
        let drag =
            crate::layout::Vec2::new(end_center.x - start_center.x, end_center.y - start_center.y);

        match lane_along_drag(picture, start, drag) {
            Some(along) => {
                let lane = &picture.lane_map().lanes[along.lane];
                let to = along.target(lane.cells.len());
                let (from, to) = (along.from.min(to), along.from.max(to));
                for cell in lane.cells[LanePos::from(from)..=LanePos::from(to)].iter() {
                    changes.insert(*cell, self.drag_start_color);
                }
            }
            // `start` sits in no lane with a usable direction; just paint the endpoint.
            None => {
                changes.insert(end, self.drag_start_color);
            }
        }
        changes
    }
}

/// Which lane a drag away from a cell means, and how far along it the drag got.
#[derive(Clone, Copy, Debug)]
pub(super) struct DragAlongLane {
    /// Index into `LaneMap::lanes`.
    pub lane: LaneIdx,
    /// Where the cell the drag started from sits in that lane.
    pub from: usize,
    /// How far along the lane the drag reached, in cells: signed, so negative runs back toward
    /// the lane's start, and *unrounded*, so that a drag too small to reach the next cell still
    /// says which way it was heading. Nor is it clamped to the lane — see `target`.
    pub steps: f32,
}

impl DragAlongLane {
    /// Where the drag ended up, as a cell position in a lane of `len` cells.
    pub fn target(&self, len: usize) -> usize {
        (self.from as isize + self.steps.round() as isize).clamp(0, len as isize - 1) as usize
    }

    /// Whether the drag ran along the lane's own direction rather than back against it.
    pub fn forward(&self) -> bool {
        self.steps >= 0.0
    }
}

/// The lane through `cell` whose direction best matches `drag`, and how far along it the drag
/// reached. `None` if the drag has no direction, or if no lane through `cell` has one.
///
/// A square grid offers two directions through a cell; a triddler offers three. Picking the lane
/// whose direction is closest to the drag generalizes the old "is this drag more horizontal than
/// vertical?" test. The line tool and the annotate tool ask exactly this same question of a drag,
/// so they ask it here.
///
/// A lane's cells zigzag between ▲ and ▼ centroids on a triangular grid, so the step to an
/// immediate neighbor is not representative of the lane's direction — e.g. from a ▲, the very
/// next step is purely vertical even on a "/" lane. Use the span from the lane's first cell to
/// its last instead, which averages the zigzag out into the lane's true on-screen direction, and
/// gives a stable average per-cell spacing along it.
pub(super) fn lane_along_drag(
    picture: &DynSolution,
    cell: CellIdx,
    drag: crate::layout::Vec2,
) -> Option<DragAlongLane> {
    let drag_len = (drag.x * drag.x + drag.y * drag.y).sqrt();
    if drag_len <= f32::EPSILON {
        return None;
    }

    let lane_map = picture.lane_map();
    let center = |cell: CellIdx| picture.cell_shape(cell).center(picture.cell_origin(cell));

    let mut best: Option<(DragAlongLane, f32)> = None; // (candidate, |cos angle| to the drag)
    for membership in &lane_map.memberships[cell] {
        let lane = &lane_map.lanes[membership.lane];
        if lane.cells.len() < 2 {
            continue; // No direction to compare against.
        }
        let first = center(lane.cells[LanePos(0)]);
        let last = center(*lane.cells.last().unwrap());
        let span = crate::layout::Vec2::new(last.x - first.x, last.y - first.y);
        let span_len = (span.x * span.x + span.y * span.y).sqrt();
        let avg_spacing = span_len / (lane.cells.len() - 1) as f32;

        // Angle between the lane's direction and the drag, ignoring which way along the lane it
        // points, so dragging toward either end still snaps to that lane.
        let cos_angle = ((span.x * drag.x + span.y * drag.y) / (span_len * drag_len)).abs();
        // Distance travelled along the lane, in cell steps, found by projecting the drag onto
        // the lane's own (first-to-last) direction.
        let steps = (drag.x * span.x + drag.y * span.y) / span_len / avg_spacing;

        if best.is_none_or(|(_, best_cos)| cos_angle > best_cos) {
            best = Some((
                DragAlongLane {
                    lane: membership.lane,
                    from: usize::from(membership.position),
                    steps,
                },
                cos_angle,
            ));
        }
    }
    best.map(|(along, _)| along)
}

#[cfg(test)]
mod line_tool_tests {
    use super::*;
    use crate::puzzle::Solution;

    fn at(x: usize, y: usize) -> CellIdx {
        CellIdx((y * 6 + x) as u32)
    }

    fn canvas() -> CanvasGui {
        let sol = Solution::blank_bw(6, 6);
        let mut gui = NonogramGui::new(Document::from_solution(
            DynSolution::Square(sol),
            "test".to_string(),
        ))
        .editor_gui;
        gui.current_tool = Tool::LineAlongLane;
        gui.drag_start_color = Color(1);
        gui
    }

    /// Dragging mostly-horizontally, but not to a cell that shares the start's row, still snaps
    /// to the row: the nearest lane by angle, not an exact row/column match.
    #[test]
    fn a_near_horizontal_drag_snaps_to_the_row() {
        let gui = canvas();
        // From (1,1), drag toward (4,2): mostly rightward with a little drop, so the row
        // (angle 0) is closer than the column (angle 90) to the drag direction.
        let changes = gui.line_between(at(1, 1), at(4, 2));

        let mut got: Vec<CellIdx> = changes.keys().copied().collect();
        got.sort();
        assert_eq!(got, vec![at(1, 1), at(2, 1), at(3, 1), at(4, 1)]);
        assert!(changes.values().all(|c| *c == Color(1)));
    }

    /// Symmetric case: a near-vertical drag snaps to the column instead.
    #[test]
    fn a_near_vertical_drag_snaps_to_the_column() {
        let gui = canvas();
        // From (1,1), drag toward (2,4): mostly downward with a little sideways drift.
        let changes = gui.line_between(at(1, 1), at(2, 4));

        let mut got: Vec<CellIdx> = changes.keys().copied().collect();
        got.sort();
        assert_eq!(got, vec![at(1, 1), at(1, 2), at(1, 3), at(1, 4)]);
    }

    /// A drag that goes backward along the lane (toward its start) still snaps and paints the
    /// span behind the origin, not just the endpoint.
    #[test]
    fn dragging_backward_along_a_lane_still_draws_the_span() {
        let gui = canvas();
        let changes = gui.line_between(at(4, 2), at(1, 3));

        let mut got: Vec<CellIdx> = changes.keys().copied().collect();
        got.sort();
        assert_eq!(got, vec![at(1, 2), at(2, 2), at(3, 2), at(4, 2)]);
    }

    /// A triangular grid's `/` and `\` lanes zigzag between ▲ and ▼ cell centroids, so a lane's
    /// direction can't be judged from a single neighboring cell: from a ▲, the very next cell
    /// along a "/" lane sits directly *below* it (a purely vertical step), which used to fool the
    /// snapping into thinking that lane wasn't diagonal at all. Regardless of which orientation
    /// a lane starts (or ends) on, dragging along its full length should paint the whole thing.
    #[test]
    fn diagonal_lanes_are_reachable_from_either_triangle_orientation() {
        use crate::geometry::{Geometry, Outline, Tri};
        use crate::puzzle::ClueStyle;

        let geometry = Geometry::<Tri>::new(Outline::hexagon(2));
        let sol: Solution<Tri> = Solution::new(
            ClueStyle::Nono,
            HashMap::from([(BACKGROUND, ColorInfo::default_bg())]),
            geometry,
            vec![BACKGROUND; Geometry::<Tri>::new(Outline::hexagon(2)).cell_count()].into(),
        );
        let lane_map = sol.geometry.lane_map.clone();
        let mut gui = NonogramGui::new(Document::from_solution(
            DynSolution::Tri(sol),
            "test".to_string(),
        ))
        .editor_gui;
        gui.current_tool = Tool::LineAlongLane;
        gui.drag_start_color = Color(1);

        // Every "/" and "\" lane (families 1 and 2) with more than one cell: dragging end to end
        // should paint every cell in it, no matter which orientation each end happens to be.
        for family in [FamilyIdx(1), FamilyIdx(2)] {
            for lane_idx in lane_map.family(family) {
                let lane = &lane_map.lanes[lane_idx];
                if lane.cells.len() < 2 {
                    continue;
                }
                let first = lane.cells[LanePos(0)];
                let last = *lane.cells.last().unwrap();

                let mut got: Vec<CellIdx> = gui.line_between(first, last).keys().copied().collect();
                got.sort();
                let mut want = lane.cells.raw.clone();
                want.sort();
                assert_eq!(
                    got,
                    want,
                    "family {family:?} lane {lane_idx:?} (len {}) didn't paint end to end",
                    lane.cells.len()
                );
            }
        }
    }
}

/// The painting tools driven through `PaintPointer` directly, with no egui in between: what a
/// press, a drag, a release and a click each do, and what each `PaintIntent` paints.
#[cfg(test)]
mod paint_pointer_tests {
    use super::*;
    use crate::puzzle::Solution;

    fn at(x: usize, y: usize) -> CellIdx {
        CellIdx((y * 5 + x) as u32)
    }

    fn press(intent: PaintIntent) -> PaintPointer {
        PaintPointer {
            pressed: true,
            down: true,
            intent,
            ..Default::default()
        }
    }

    fn drag(intent: PaintIntent) -> PaintPointer {
        PaintPointer {
            down: true,
            intent,
            ..Default::default()
        }
    }

    /// What a click ends with. (The button is already up, so there's no intent left to report.)
    fn release() -> PaintPointer {
        PaintPointer {
            released: true,
            clicked: true,
            ..Default::default()
        }
    }

    fn editor(tool: Tool) -> CanvasGui {
        let mut gui = NonogramGui::new(Document::from_solution(
            DynSolution::Square(Solution::blank_bw(5, 5)),
            "test".to_string(),
        ))
        .editor_gui;
        gui.current_tool = tool;
        gui.current_color = Color(1);
        gui
    }

    /// A 5×5 whose top row is filled, being solved, with nothing known yet.
    fn solver() -> CanvasGui {
        let mut solution = Solution::blank_bw(5, 5);
        for x in 0..5 {
            solution.cells[at(x, 0)] = Color(1);
        }
        let doc = Document::from_solution(DynSolution::Square(solution), "test".to_string());
        let mut canvas = NonogramGui::new_solving(doc).solve_gui.unwrap().canvas;
        canvas.current_color = Color(1);
        canvas
    }

    fn cell(gui: &CanvasGui, cell: CellIdx) -> Color {
        gui.document.solution().cells()[cell]
    }

    /// The pencil paints with whatever the press found to do, for the whole drag, and a press on
    /// a cell that already has the current color takes it back off.
    #[test]
    fn the_pencil_paints_a_stroke_and_toggles() {
        let mut gui = editor(Tool::Pencil);
        gui.pointer_tool_input(at(0, 0), press(PaintIntent::Current), gui.current_tool);
        gui.pointer_tool_input(at(1, 0), drag(PaintIntent::Current), gui.current_tool);
        gui.pointer_tool_input(at(1, 0), release(), gui.current_tool);
        assert_eq!(cell(&gui, at(0, 0)), Color(1));
        assert_eq!(cell(&gui, at(1, 0)), Color(1));
        assert_eq!(gui.undo_stack.len(), 1, "one stroke is one undo step");

        gui.pointer_tool_input(at(0, 0), press(PaintIntent::Current), gui.current_tool);
        assert_eq!(cell(&gui, at(0, 0)), BACKGROUND);
    }

    /// While solving, ruling out a cell and doing it again goes back to undecided, as does
    /// pressing with the current color on a cell that already has it. `Erase` always does.
    #[test]
    fn solving_marks_toggle_back_to_unknown() {
        let mut gui = solver();
        let corner = at(4, 4);
        assert_eq!(cell(&gui, corner), UNSOLVED);

        for (intent, want) in [
            (PaintIntent::Background, BACKGROUND),
            (PaintIntent::Background, UNSOLVED),
            (PaintIntent::Current, Color(1)),
            (PaintIntent::Current, UNSOLVED),
            (PaintIntent::Current, Color(1)),
            (PaintIntent::Erase, UNSOLVED),
        ] {
            gui.pointer_tool_input(corner, press(intent), gui.current_tool);
            gui.pointer_tool_input(corner, release(), gui.current_tool);
            assert_eq!(cell(&gui, corner), want, "after {intent:?}");
        }
    }

    /// Flood fill acts on the click, not on the press, and `Background` fills with background.
    #[test]
    fn flood_fill_waits_for_the_click() {
        let mut gui = editor(Tool::FloodFill);
        gui.pointer_tool_input(at(2, 2), press(PaintIntent::Current), gui.current_tool);
        assert_eq!(cell(&gui, at(2, 2)), BACKGROUND);
        gui.pointer_tool_input(at(2, 2), release(), gui.current_tool);
        assert!(
            gui.document
                .solution()
                .cells()
                .iter()
                .all(|c| *c == Color(1))
        );

        let mut ruling_out = release();
        ruling_out.intent = PaintIntent::Background;
        gui.pointer_tool_input(at(0, 0), ruling_out, gui.current_tool);
        assert_eq!(cell(&gui, at(4, 4)), BACKGROUND);
    }

    /// The line tool paints from where the press landed to wherever the drag has got to, and the
    /// release ends the line.
    #[test]
    fn the_line_tool_follows_the_drag() {
        let mut gui = editor(Tool::LineAlongLane);
        gui.pointer_tool_input(at(0, 1), press(PaintIntent::Current), gui.current_tool);
        gui.pointer_tool_input(at(2, 1), drag(PaintIntent::Current), gui.current_tool);
        gui.pointer_tool_input(at(3, 1), drag(PaintIntent::Current), gui.current_tool);
        gui.pointer_tool_input(at(3, 1), release(), gui.current_tool);
        let row: Vec<Color> = (0..5).map(|x| cell(&gui, at(x, 1))).collect();
        assert_eq!(row, [Color(1), Color(1), Color(1), Color(1), BACKGROUND]);
        assert!(gui.line_tool_state.is_none());
        assert_eq!(gui.undo_stack.len(), 1);
    }

    /// Picking another tool partway through a line (with its key, say) leaves the line in charge
    /// until the drag is over — and then lets go of it, rather than leaving the line tool stuck.
    #[test]
    fn switching_tools_mid_line_still_finishes_the_line() {
        let ctx = egui::Context::default();
        // As the canvas does it: whichever tool `effective_tool` says is driving the pointer.
        let step = |gui: &mut CanvasGui, at: CellIdx, pointer: PaintPointer| {
            let mut tool = gui.current_tool;
            ctx.run_ui(egui::RawInput::default(), |ui| {
                tool = gui.effective_tool(ui)
            })
            .drop_without_applying_deltas();
            gui.pointer_tool_input(at, pointer, tool);
        };

        let mut gui = editor(Tool::LineAlongLane);
        step(&mut gui, at(0, 1), press(PaintIntent::Current));
        gui.current_tool = Tool::Lasso;
        step(&mut gui, at(3, 1), drag(PaintIntent::Current));
        step(&mut gui, at(3, 1), release());

        let row: Vec<Color> = (0..5).map(|x| cell(&gui, at(x, 1))).collect();
        assert_eq!(row, [Color(1), Color(1), Color(1), Color(1), BACKGROUND]);
        assert!(gui.line_tool_state.is_none());
    }
}
