//! The editing tools: which one is selected, how it's reached, and what each does to the grid.
//!
//! The lasso is the exception — it lives in `selection`, since it tracks the pointer off the
//! grid and carries a whole floating layer with it. Everything here acts on a single cell that
//! the pointer is already over.

use super::*;

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
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

/// The icon, bare-key shortcut, and tooltip key-name for each tool.
fn tool_appearance(tool: Tool) -> (&'static str, egui::Key, char) {
    match tool {
        Tool::Pencil => (icons::ICON_BRUSH, egui::Key::P, 'P'),
        Tool::LineAlongLane => (icons::ICON_LINE_START, egui::Key::L, 'L'),
        Tool::FloodFill => (icons::ICON_FORMAT_COLOR_FILL, egui::Key::F, 'F'),
        // `L` is spoken for by the line tool, so the lasso gets "select" instead.
        Tool::Lasso => (icons::ICON_LASSO_SELECT, egui::Key::S, 'S'),
        Tool::Annotate => (icons::ICON_SQUARE_FOOT, egui::Key::A, 'A'),
    }
}

/// One entry in the tool row: a toggle button that its key also reaches. `typing` suppresses the
/// key while a `TextEdit` has the keyboard.
fn tool_button(
    ui: &mut egui::Ui,
    current_tool: &mut Tool,
    tool: Tool,
    typing: bool,
    description: &str,
) {
    let (icon, key, ch) = tool_appearance(tool);
    ui.selectable_value(current_tool, tool, egui::RichText::new(icon).size(24.0))
        .on_hover_text(format!("{description} (press {ch})"));
    if !typing && ui.input(|i| i.key_pressed(key)) {
        *current_tool = tool;
    }
}

impl CanvasGui {
    pub(super) fn tool_selector(&mut self, ui: &mut egui::Ui, editing: bool) {
        let was = self.current_tool;

        // Same story as in `common_sidebar_items`: no modifiers here either.
        let typing = ui.ctx().wants_keyboard_input();

        centered_row(ui, "tools", |ui| {
            tool_button(ui, &mut self.current_tool, Tool::Pencil, typing, "Pencil");
            tool_button(
                ui,
                &mut self.current_tool,
                Tool::LineAlongLane,
                typing,
                "Line along a row, column or diagonal",
            );
            // Flood fill and the lasso are editor-only, so their keys are dead in the solver
            // rather than silently switching to a tool with no button.
            if editing {
                tool_button(
                    ui,
                    &mut self.current_tool,
                    Tool::FloodFill,
                    typing,
                    "Flood Fill",
                );

                tool_button(
                    ui,
                    &mut self.current_tool,
                    Tool::Lasso,
                    typing,
                    "Lasso select: draw a loop, then drag to move what's inside",
                );
            } else {
                // Likewise, annotate is solve-only
                tool_button(
                    ui,
                    &mut self.current_tool,
                    Tool::Annotate,
                    typing,
                    "Annotate a border: click to mark one, drag to measure a span (or hold shift)",
                );
            }
        });

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
        } else if self.allow_annotations && ui.input(|i| i.modifiers.shift) {
            Tool::Annotate
        } else {
            self.current_tool
        }
    }

    fn flood_fill(&mut self, start: u32) {
        let picture = self.document.solution_mut();
        let target_color = picture.cells()[start as usize];
        if target_color == self.current_color {
            return; // Nothing to do
        }

        let mut changes = HashMap::new();
        let mut q = std::collections::VecDeque::new();
        let mut visited = std::collections::HashSet::new();

        q.push_back(start);
        visited.insert(start);

        while let Some(cell) = q.pop_front() {
            changes.insert(cell, self.current_color);

            for neighbor in picture.neighbor_cells(cell) {
                if picture.cells()[neighbor as usize] == target_color && visited.insert(neighbor) {
                    q.push_back(neighbor);
                }
            }
        }

        if !changes.is_empty() {
            self.perform(Action::ChangeColor { changes }, ActionMood::Normal);
        }
    }

    /// Apply the selected tool at `cell`, in response to a press or drag over the grid.
    ///
    /// The lasso never arrives here: it has to keep tracking the pointer once it leaves the
    /// grid, so `canvas_with_clues` deals with it before there is a cell to speak of.
    pub(super) fn pointer_tool_input(&mut self, cell: u32, pointer: &egui::PointerState) {
        let picture = self.document.solution_mut();
        let paint_color = if pointer.middle_down() {
            if picture.palette().contains_key(&UNSOLVED) {
                UNSOLVED
            } else {
                BACKGROUND
            }
        } else if pointer.secondary_down() {
            BACKGROUND
        } else if picture.cells()[cell as usize] != self.current_color {
            self.current_color
        } else {
            BACKGROUND
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

        match self.current_tool {
            Tool::Pencil => {
                let mood = if pointer.any_pressed() {
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
                if pointer.any_click() {
                    let original_color = self.current_color;
                    self.current_color = paint_color;
                    self.flood_fill(cell);
                    self.current_color = original_color;
                }
            }
            Tool::LineAlongLane => {
                if pointer.any_pressed() {
                    self.drag_start_color = paint_color;
                    self.line_tool_state = Some(cell);

                    self.perform(
                        Action::ChangeColor {
                            changes: [(cell, self.drag_start_color)].into(),
                        },
                        ActionMood::Normal,
                    );
                } else if pointer.any_down() {
                    if let Some(start) = self.line_tool_state {
                        let changes = self.line_between(start, cell);
                        self.perform(Action::ChangeColor { changes }, ActionMood::ReplaceAction);
                    }
                } else if pointer.any_released() {
                    self.line_tool_state = None;
                }
            }
            // Both are handled above: the lasso because the pointer is still allowed to be off
            // the grid, and annotating because it snaps to a border rather than to a cell.
            Tool::Lasso | Tool::Annotate => {}
        }
    }

    /// The cells between two points along whichever lane best matches the drag.
    ///
    /// A square grid offers two directions through a cell; a triddler offers three. Picking the
    /// family whose lane actually contains both endpoints generalizes the old "is this drag more
    /// horizontal than vertical?" test.
    fn line_between(&mut self, start: u32, end: u32) -> HashMap<u32, Color> {
        let picture = self.document.solution_mut();
        let lanes = picture.lane_map();

        let mut changes = HashMap::new();
        if start == end {
            changes.insert(end, self.drag_start_color);
            return changes;
        }

        // Cell *centres*, not raw origins: a triangle's centroid sits off-corner and at a
        // different offset for ▲ than ▼, so mixing origins would misjudge lane direction
        // whenever a lane's cells alternate orientation.
        let center = |cell: u32| picture.cell_shape(cell).center(picture.cell_origin(cell));

        let start_center = center(start);
        let end_center = center(end);
        let drag =
            crate::layout::Vec2::new(end_center.x - start_center.x, end_center.y - start_center.y);
        let drag_len = (drag.x * drag.x + drag.y * drag.y).sqrt();

        // A lane's cells zigzag between ▲ and ▼ centroids on a triangular grid, so the step to
        // an immediate neighbor is not representative of the lane's direction — e.g. from a ▲,
        // the very next step is purely vertical even on a "/" lane. Use the span from the lane's
        // first cell to its last instead, which averages the zigzag out into the lane's true
        // on-screen direction, and gives a stable average per-cell spacing along it.
        let mut best: Option<(usize, f32)> = None; // (lane, |cos angle| to drag)
        for membership in lanes.memberships(start) {
            let lane = lanes.lane(membership.lane as usize);
            if lane.cells.len() < 2 {
                continue; // No direction to compare against.
            }
            let first = center(lane.cells[0]);
            let last = center(*lane.cells.last().unwrap());
            let span = crate::layout::Vec2::new(last.x - first.x, last.y - first.y);
            let span_len = (span.x * span.x + span.y * span.y).sqrt();
            // Angle between the lane's direction and the drag, ignoring which way along the
            // lane it points, so dragging toward either end still snaps to that lane.
            let cos_angle = ((span.x * drag.x + span.y * drag.y) / (span_len * drag_len)).abs();
            if best.is_none_or(|(_, best_cos)| cos_angle > best_cos) {
                best = Some((membership.lane as usize, cos_angle));
            }
        }

        match best {
            Some((lane_idx, _)) => {
                let lane = lanes.lane(lane_idx);
                let from_pos = lanes
                    .memberships(start)
                    .iter()
                    .find(|m| m.lane as usize == lane_idx)
                    .unwrap()
                    .position as usize;

                let first = center(lane.cells[0]);
                let last = center(*lane.cells.last().unwrap());
                let span = crate::layout::Vec2::new(last.x - first.x, last.y - first.y);
                let span_len = (span.x * span.x + span.y * span.y).sqrt();
                let avg_spacing = span_len / (lane.cells.len() - 1) as f32;

                // Distance travelled along the lane, in cell steps, found by projecting the
                // drag onto the lane's own (first-to-last) direction.
                let signed_distance = (drag.x * span.x + drag.y * span.y) / span_len;
                let delta = (signed_distance / avg_spacing).round() as isize;

                let to_pos =
                    (from_pos as isize + delta).clamp(0, lane.cells.len() as isize - 1) as usize;
                let (from, to) = (from_pos.min(to_pos), from_pos.max(to_pos));
                for cell in &lane.cells[from..=to] {
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

#[cfg(test)]
mod line_tool_tests {
    use super::*;
    use crate::puzzle::Solution;

    fn at(x: usize, y: usize) -> u32 {
        (y * 6 + x) as u32
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
        let mut gui = canvas();
        // From (1,1), drag toward (4,2): mostly rightward with a little drop, so the row
        // (angle 0) is closer than the column (angle 90) to the drag direction.
        let changes = gui.line_between(at(1, 1), at(4, 2));

        let mut got: Vec<u32> = changes.keys().copied().collect();
        got.sort();
        assert_eq!(got, vec![at(1, 1), at(2, 1), at(3, 1), at(4, 1)]);
        assert!(changes.values().all(|c| *c == Color(1)));
    }

    /// Symmetric case: a near-vertical drag snaps to the column instead.
    #[test]
    fn a_near_vertical_drag_snaps_to_the_column() {
        let mut gui = canvas();
        // From (1,1), drag toward (2,4): mostly downward with a little sideways drift.
        let changes = gui.line_between(at(1, 1), at(2, 4));

        let mut got: Vec<u32> = changes.keys().copied().collect();
        got.sort();
        assert_eq!(got, vec![at(1, 1), at(1, 2), at(1, 3), at(1, 4)]);
    }

    /// A drag that goes backward along the lane (toward its start) still snaps and paints the
    /// span behind the origin, not just the endpoint.
    #[test]
    fn dragging_backward_along_a_lane_still_draws_the_span() {
        let mut gui = canvas();
        let changes = gui.line_between(at(4, 2), at(1, 3));

        let mut got: Vec<u32> = changes.keys().copied().collect();
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
            vec![BACKGROUND; Geometry::<Tri>::new(Outline::hexagon(2)).cell_count()],
        );
        let lane_map = sol.geometry.lane_map().clone();
        let mut gui = NonogramGui::new(Document::from_solution(
            DynSolution::Tri(sol),
            "test".to_string(),
        ))
        .editor_gui;
        gui.current_tool = Tool::LineAlongLane;
        gui.drag_start_color = Color(1);

        // Every "/" and "\" lane (families 1 and 2) with more than one cell: dragging end to end
        // should paint every cell in it, no matter which orientation each end happens to be.
        for family in [1usize, 2usize] {
            for lane_idx in lane_map.family(family) {
                let lane = lane_map.lane(lane_idx);
                if lane.cells.len() < 2 {
                    continue;
                }
                let first = lane.cells[0];
                let last = *lane.cells.last().unwrap();

                let mut got: Vec<u32> = gui.line_between(first, last).keys().copied().collect();
                got.sort();
                let mut want = lane.cells.clone();
                want.sort();
                assert_eq!(
                    got,
                    want,
                    "family {family} lane {lane_idx} (len {}) didn't paint end to end",
                    lane.cells.len()
                );
            }
        }
    }
}
