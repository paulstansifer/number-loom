use super::{
    Action, ActionMood, BacktrackSolver, CanvasGui, ClueId, Disambiguator, Staleable, Tool,
    auto_button, default_color, outline_text,
};
use crate::{
    puzzle::{Color, DynPuzzle, PuzzleDynOps, UNSOLVED},
    solve::grid_solve::LineStatus,
    user_settings::{UserSettings, consts},
};
use egui::{Color32, Pos2, Rect, RichText, Vec2, text::Fonts};
use std::collections::HashSet;
use web_time::Instant;

use crate::puzzle::{Document, DynSolution};
pub struct SolveGui {
    pub canvas: CanvasGui,
    pub clues: DynPuzzle,
    pub intended_solution: DynSolution,
    pub analyze_lines: bool,
    pub detect_errors: bool,
    pub infer_background: bool,
    /// Per clue family, per line: can line-logic fully solve any cells?
    pub line_analysis: Staleable<Option<Vec<Vec<LineStatus>>>>,
    pub mark_fixed_clues: bool,
    /// Per clue family, per line: which of that line's clues are "done"
    pub fixed_clues: Staleable<Option<Vec<Vec<Vec<usize>>>>>,
    pub render_style: RenderStyle,
    last_inferred_version: u32,
    pub hovered_cell: Option<u32>,
    /// The solve, played back in the sidebar. Captured the first time the picture comes out
    /// right, and frozen there: whatever the user does to the grid afterwards isn't part of the
    /// solve they just finished.
    pub replay: Option<Replay>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderStyle {
    TraditionalDots,
    TraditionalXes,
    Experimental,
}

impl RenderStyle {
    /// The name this style is saved under. Stored rather than derived, so that renaming a variant
    /// doesn't silently reset everyone's saved choice.
    fn setting_name(self) -> &'static str {
        match self {
            RenderStyle::TraditionalDots => "traditional_dots",
            RenderStyle::TraditionalXes => "traditional_xes",
            RenderStyle::Experimental => "experimental",
        }
    }

    fn from_setting_name(name: &str) -> Option<RenderStyle> {
        match name {
            "traditional_dots" => Some(RenderStyle::TraditionalDots),
            "traditional_xes" => Some(RenderStyle::TraditionalXes),
            "experimental" => Some(RenderStyle::Experimental),
            _ => None,
        }
    }
}

impl SolveGui {
    pub fn new(
        mut document: Document,
        status: super::SharedStatus,
        progress: super::SharedProgress,
    ) -> Self {
        let mut working_doc = document.clone();
        for cell in working_doc.solution_mut().cells_mut() {
            *cell = UNSOLVED;
        }
        working_doc.solution_mut().palette_mut().insert(
            UNSOLVED,
            crate::puzzle::ColorInfo {
                ch: '?',
                name: "unknown".to_owned(),
                rgb: (128, 128, 128),
                color: UNSOLVED,
                corner: None,
            },
        );
        let current_color = default_color(working_doc.solution_mut().palette());

        let clues = document.puzzle().clone();
        let solved_mask = vec![true; document.solution_mut().cells().len()];

        SolveGui {
            canvas: CanvasGui {
                document: working_doc,
                version: 0,
                current_color,
                drag_start_color: current_color,
                undo_stack: vec![],
                redo_stack: vec![],
                current_tool: Tool::LineAlongLane,
                previous_tool: Tool::LineAlongLane,
                line_tool_state: None,
                selection: None,
                annotations: vec![],
                annotate_drag: None,
                checked_clues: HashSet::new(),
                solving: true,
                middle_pans: false,
                picture_rect: None,
                solved_mask: Staleable {
                    val: ("".to_string(), solved_mask),
                    version: 0,
                },
                disambiguator: Staleable {
                    val: Disambiguator::new(),
                    version: 0,
                },
                backtrack_solver: Staleable {
                    val: BacktrackSolver::new(),
                    version: 0,
                },
                id: Staleable {
                    val: "".to_string(),
                    version: 0,
                },
                status,
                progress,
            },
            clues,
            intended_solution: document.take_solution().unwrap(),
            analyze_lines: UserSettings::get_bool(consts::SOLVER_ANALYZE_LINES),
            detect_errors: UserSettings::get_bool(consts::SOLVER_DETECT_ERRORS),
            infer_background: UserSettings::get_bool(consts::SOLVER_INFER_BACKGROUND),
            line_analysis: Staleable {
                val: None,
                version: u32::MAX,
            },
            mark_fixed_clues: UserSettings::get_bool(consts::SOLVER_MARK_FIXED_CLUES),
            fixed_clues: Staleable {
                val: None,
                version: u32::MAX,
            },
            render_style: UserSettings::get(consts::SOLVER_RENDER_STYLE)
                .and_then(|name| RenderStyle::from_setting_name(&name))
                .unwrap_or(RenderStyle::Experimental),
            last_inferred_version: u32::MAX,
            hovered_cell: None,
            replay: None,
        }
    }

    /// Check a clue off, or un-check it. Clues the solver has checked off itself never get here:
    /// the gutters swallow those clicks, since un-checking one would last only until the next
    /// repaint.
    fn toggle_checked_clue(&mut self, id: ClueId) {
        self.canvas
            .perform(Action::ToggleClue { clue: id }, ActionMood::Normal);
    }

    fn detect_any_errors(&self) -> bool {
        let picture = self.canvas.document.try_solution().unwrap();
        for (cell, intended) in picture.cells().iter().zip(self.intended_solution.cells()) {
            if *cell != *intended && *cell != crate::puzzle::UNSOLVED {
                return true;
            }
        }
        false
    }

    fn is_correctly_solved(&self) -> bool {
        self.canvas.document.try_solution().unwrap().cells() == self.intended_solution.cells()
    }

    fn infer_background(&mut self) {
        let mut grid = self.canvas.document.solution_mut().to_partial();

        if self.clues.settle_solution(&mut grid).is_ok() {
            let mut changes = std::collections::HashMap::new();
            let picture = self.canvas.document.try_solution().unwrap();
            for (index, cell) in grid.iter().enumerate() {
                let current_color = picture.cells()[index];
                if cell.is_known() && cell.known_or() != Some(current_color) {
                    changes.insert(index as u32, cell.known_or().unwrap());
                }
            }

            if !changes.is_empty() {
                self.canvas
                    .perform(Action::ChangeColor { changes }, ActionMood::Merge);
            }
        }
    }

    pub fn sidebar(&mut self, ui: &mut egui::Ui) {
        // The first time the picture comes out right, freeze the solve for playback. Later
        // completions (after an undo and a redo, say) reuse this one rather than recapturing.
        if self.replay.is_none() && self.is_correctly_solved() {
            self.replay = Some(Replay::new(&self.canvas));
        }

        ui.vertical(|ui| {
            if !self.canvas.document.title.is_empty() {
                ui.label(RichText::new(&self.canvas.document.title).strong());
            }
            if !self.canvas.document.author.is_empty() {
                ui.label(format!("by {}", &self.canvas.document.author));
            }

            if let Some(replay) = &mut self.replay {
                replay.show(ui, &self.canvas);
            }

            if self.is_correctly_solved() {
                ui.colored_label(egui::Color32::DARK_GREEN, "Correctly solved");

                if !self.canvas.document.description.is_empty() {
                    ui.label(&self.canvas.document.description);
                }
            }

            self.canvas.common_sidebar_items(ui);

            ui.separator();
            let scale = 20.0;
            // A triddler's rosette has six rhombus arms instead of a square grid's four square
            // ones, and a rhombus is wider across its short diagonal than it is long — so it
            // needs more room to keep adjacent arms from overlapping.
            let triangular = matches!(self.clues.shape(), crate::geometry::Shape::Triangular(_));
            let plus_size = if triangular {
                scale * super::triddler::ROSETTE_SIZE
            } else {
                scale * 3.0
            };

            if let Some(cell) = self.hovered_cell {
                let picture = self.canvas.document.try_solution().unwrap();

                let (resp, painter) =
                    ui.allocate_painter(Vec2::new(plus_size, plus_size), egui::Sense::empty());
                let rect = resp.rect;

                if triangular {
                    super::triddler::draw_rosette(
                        ui,
                        &painter,
                        picture,
                        cell,
                        rect.center(),
                        scale,
                    );
                } else {
                    let color = picture.cells()[cell as usize];
                    let rgb = picture.palette()[&color].rgb;
                    let text = if color == UNSOLVED { "?" } else { " " };

                    // One run per clue family: two arms each for a square grid.
                    // `arm_directions` lists each family's two directions adjacently, matching
                    // the `(backward, forward)` pairs `runs_at_cell` returns.
                    let runs = picture.runs_at_cell(cell);
                    let dirs = picture.arm_directions();
                    let size = Vec2::new(20.0, 20.0);
                    let center = rect.min + Vec2::new(scale, scale);

                    for (family, (back, forward)) in runs.iter().enumerate() {
                        for (i, count) in [back, forward].into_iter().enumerate() {
                            let Some(dir) = dirs.get(family * 2 + i) else {
                                continue;
                            };
                            if *count == 0 {
                                continue;
                            }
                            let at = center + Vec2::new(dir.x * scale, dir.y * scale);
                            draw_string_in_box(
                                ui,
                                &painter,
                                Rect::from_min_size(at, size),
                                &count.to_string(),
                                scale,
                                rgb,
                            );
                        }
                    }

                    let mid_rect = Rect::from_min_size(center, size);
                    draw_string_in_box(ui, &painter, mid_rect, text, scale, rgb);
                }
            } else {
                ui.add_space(plus_size);
            }

            ui.separator();

            ui.label("Render style");
            // Radio buttons report `changed()` one at a time; comparing against the style we came
            // in with saves whichever one of the three the user landed on.
            let was = self.render_style;
            ui.radio_value(
                &mut self.render_style,
                RenderStyle::TraditionalDots,
                "traditional (dots)",
            );
            ui.radio_value(
                &mut self.render_style,
                RenderStyle::TraditionalXes,
                "traditional (Xes)",
            );
            ui.radio_value(
                &mut self.render_style,
                RenderStyle::Experimental,
                "experimental",
            );
            if self.render_style != was {
                let _ = UserSettings::set(
                    consts::SOLVER_RENDER_STYLE,
                    self.render_style.setting_name(),
                );
            }

            ui.separator();

            let analyze = auto_button(ui, "Analyze Lines", &mut self.analyze_lines);
            if analyze.auto.changed() {
                let _ = UserSettings::set(
                    consts::SOLVER_ANALYZE_LINES,
                    &self.analyze_lines.to_string(),
                );
                if !self.analyze_lines {
                    // Turning the aid off has to take its marks with it; otherwise the last
                    // analysis sits in the gutters, going staler with every move.
                    self.line_analysis.update(None, u32::MAX);
                }
            }
            if analyze.button.clicked() || self.analyze_lines {
                let clues = &self.clues;
                let picture = self.canvas.document.try_solution().unwrap();
                let grid = picture.to_partial();
                self.line_analysis
                    .get_or_refresh(self.canvas.version, || Some(clues.analyze_lines(&grid)));
            }

            let mark = auto_button(ui, "Mark resolved clues", &mut self.mark_fixed_clues);
            if mark.auto.changed() {
                let _ = UserSettings::set(
                    consts::SOLVER_MARK_FIXED_CLUES,
                    &self.mark_fixed_clues.to_string(),
                );
                if !self.mark_fixed_clues {
                    // As with the analysis marks: turning the aid off takes its marks with it.
                    self.fixed_clues.update(None, u32::MAX);
                }
            }
            if mark.button.clicked() || self.mark_fixed_clues {
                let clues = &self.clues;
                let picture = self.canvas.document.try_solution().unwrap();
                let grid = picture.to_partial();
                self.fixed_clues
                    .get_or_refresh(self.canvas.version, || Some(clues.fixed_clues(&grid)));
            }

            let detect = auto_button(ui, "Detect errors", &mut self.detect_errors);
            if detect.auto.changed() {
                let _ = UserSettings::set(
                    consts::SOLVER_DETECT_ERRORS,
                    &self.detect_errors.to_string(),
                );
            }
            if (detect.button.clicked() || self.detect_errors) && self.detect_any_errors() {
                ui.colored_label(egui::Color32::DARK_RED, "Error detected");
            }

            let infer = auto_button(ui, "Infer background", &mut self.infer_background);
            if infer.auto.changed() {
                let _ = UserSettings::set(
                    consts::SOLVER_INFER_BACKGROUND,
                    &self.infer_background.to_string(),
                );
            }
            if (infer.button.clicked() || self.infer_background)
                && self.last_inferred_version != self.canvas.version
            {
                self.infer_background();
                self.last_inferred_version = self.canvas.version;
            }
        });
    }

    /// The block of one color under the pointer, for the gutters to report. `None` when the
    /// pointer isn't over the picture.
    ///
    /// This lags the pointer by a frame — `hovered_cell` is set by the canvas, which is drawn
    /// after the gutters — but so does the sidebar's rosette, and egui repaints on every pointer
    /// move anyway.
    fn hover_blocks(&self) -> Option<super::HoverBlocks> {
        let cell = self.hovered_cell?;
        let picture = self.canvas.document.try_solution()?;
        Some(super::HoverBlocks {
            by_family: picture.blocks_at_cell(cell),
            rgb: picture.palette()[&picture.cells()[cell as usize]].rgb,
        })
    }

    pub fn body(&mut self, ui: &mut egui::Ui, scale: f32) {
        let is_stale = !self.line_analysis.fresh(self.canvas.version);
        let hover = self.hover_blocks();

        // A hexagon's three clue blocks run along the lane directions, so they can't be laid out
        // as panels beside the grid; they share the picture's painter instead.
        if matches!(self.clues.shape(), crate::geometry::Shape::Triangular(_)) {
            let overlay = super::ClueOverlay {
                puzzle: &self.clues,
                analysis: self.line_analysis.val.as_ref(),
                fixed: self.fixed_clues.val.as_ref(),
                is_stale,
                hover,
            };
            let (hovered, clicked) =
                self.canvas
                    .canvas_with_clues(ui, scale, self.render_style, Some(overlay));
            self.hovered_cell = hovered;
            if let Some(id) = clicked {
                self.toggle_checked_clue(id);
            }
            return;
        }

        // The square gutters number their lines within a clue family, while `hover` names whole
        // lanes; a family's first lane is the offset between the two.
        let hint = |family: usize| -> Option<BlockHint> {
            let hover = hover.as_ref()?;
            let &(lane, len) = hover.by_family.get(family)?;
            let start = self
                .canvas
                .document
                .try_solution()?
                .lane_map()
                .family(family)
                .start;
            Some(BlockHint {
                line: lane - start,
                len,
                rgb: hover.rgb,
            })
        };
        // Family 0 is the rows, drawn by the horizontal gutter; family 1 is the columns.
        let (row_hint, col_hint) = (hint(0), hint(1));

        let mut clicked_clue = None;
        ui.vertical(|ui| {
            // No spacing between the cells: each clue gutter reserves its own gap against the
            // grid (`CLUE_PAD`, in cell units), and egui's default few pixels on top of that
            // would be a zoom-independent gap that swamps the gutter when zoomed out.
            egui::Grid::new("solve_grid")
                .spacing(Vec2::ZERO)
                .show(ui, |ui| {
                    ui.label(""); // Top-left is empty
                    let line_analysis = self.line_analysis.val.as_ref();
                    let fixed_clues = self.fixed_clues.val.as_ref();
                    // Family 0 is the rows, family 1 the columns, in both analyses.
                    let checked = &self.canvas.checked_clues;
                    let marks = |family: usize, hint: Option<BlockHint>| GutterMarks {
                        analysis: line_analysis.and_then(|la| la.get(family)).map(|v| &v[..]),
                        fixed: fixed_clues.and_then(|fc| fc.get(family)).map(|v| &v[..]),
                        checked,
                        is_stale,
                        hover: hint,
                    };
                    let col_clicked = draw_dyn_clues(
                        ui,
                        &self.clues,
                        scale,
                        Orientation::Vertical,
                        &marks(1, col_hint),
                    );
                    ui.end_row();

                    let row_clicked = draw_dyn_clues(
                        ui,
                        &self.clues,
                        scale,
                        Orientation::Horizontal,
                        &marks(0, row_hint),
                    );
                    // A click lands in one gutter or the other, never both.
                    clicked_clue = col_clicked.or(row_clicked);
                    self.hovered_cell = self.canvas.canvas(ui, scale, self.render_style);
                    ui.end_row();
                });
        });

        if let Some(id) = clicked_clue {
            self.toggle_checked_clue(id);
        }
    }
}

/// How fast the sidebar replays a finished solve.
const REPLAY_STEPS_PER_SECOND: f32 = 20.0;

/// Pixels per cell for a replay `across` cells wide in a sidebar `available` pixels wide.
fn replay_scale(across: f32, available: f32) -> f32 {
    let whole = (available / across).floor();
    if whole >= 1.0 {
        whole
    } else {
        available / across // Gotta use fractional pictures to fit at all
    }
}

/// A finished solve, played back a step at a time.
///
/// This uses the undo stack, skipping over clue-check-off actions
pub struct Replay {
    /// How many steps the solve took, frozen when it came out right, so that whatever the user
    /// does to the grid afterwards isn't replayed as part of it.
    steps: usize,
    /// When this run of the animation started.
    started: Instant,
    /// Where the replay was drawn, as of the last frame that drew it. `pub` for tests/gui.rs,
    /// which needs somewhere to aim a click; see `CanvasGui::picture_rect`.
    pub rect: Option<Rect>,
}

impl Replay {
    fn new(canvas: &CanvasGui) -> Replay {
        Replay {
            steps: canvas.undo_stack.len(),
            started: Instant::now(),
            rect: None,
        }
    }

    /// How far into the solve the animation has got, in steps. Uncapped, so it runs off the end
    /// of the replay once it's finished.
    pub fn step(&self) -> usize {
        (self.started.elapsed().as_secs_f32() * REPLAY_STEPS_PER_SECOND) as usize
    }

    /// Where in the undo stack each frame of the replay stands: the picture before the solve
    /// started, then one frame after each step that painted. Checking a clue off is on the stack
    /// too but changes no cell, so it gets no frame of its own and the replay steps past it.
    fn frames(&self, canvas: &CanvasGui) -> Vec<usize> {
        // Undoing back past where the solve finished really does take those steps off the stack,
        // leaving less to replay.
        let stack = &canvas.undo_stack[..self.steps.min(canvas.undo_stack.len())];
        let mut frames = vec![0];
        for (i, action) in stack.iter().enumerate() {
            if matches!(action, Action::ChangeColor { .. }) {
                frames.push(i + 1);
            }
        }
        frames
    }

    /// How many steps there are left to show.
    fn len(&self, canvas: &CanvasGui) -> usize {
        self.frames(canvas).len() - 1
    }

    /// The picture as it stood after `step` painting steps of the solve.
    ///
    /// Each undo entry holds the colors its step painted over, so starting from the picture as it
    /// stands *now* and applying them newest-first walks backwards through the solve. Anything
    /// the user painted after finishing gets unwound on the way past.
    fn frame(&self, canvas: &CanvasGui, step: usize) -> Vec<Color> {
        let frames = self.frames(canvas);
        let position = frames[step.min(frames.len() - 1)];
        let mut cells = canvas.document.try_solution().unwrap().cells().to_vec();
        for undone in canvas.undo_stack[position..].iter().rev() {
            // Painting is the only thing that reaches the solver's undo stack: its palette editor
            // is read-only, and every `ReplaceDocument` in the app goes to the editor's canvas.
            if let Action::ChangeColor { changes } = undone {
                for (cell, was) in changes {
                    cells[*cell as usize] = *was;
                }
            }
        }
        cells
    }

    /// Draw the current frame, and start over if it's clicked.
    ///
    /// The canvas supplies the geometry, the palette, and — through its undo stack — the solve
    /// itself.
    fn show(&mut self, ui: &mut egui::Ui, canvas: &CanvasGui) {
        let picture = canvas.document.try_solution().unwrap();
        let extent = picture.extent();
        let available = ui.available_width();
        let scale = replay_scale(extent.x, available);

        let (response, painter) = ui.allocate_painter(
            // `min`, because a fractional scale is a division that can land a hair over.
            Vec2::new((scale * extent.x).min(available), scale * extent.y),
            egui::Sense::click(),
        );
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        self.rect = Some(response.rect);
        if response.clicked() {
            self.started = Instant::now();
        }

        let len = self.len(canvas);
        let step = self.step().min(len);
        let running = step < len;

        let to_screen = egui::emath::RectTransform::from_to(
            Rect::from_min_size(Pos2::ZERO, Vec2::new(extent.x, extent.y)),
            response.rect,
        );

        let palette = picture.palette();
        let cells = self.frame(canvas, step);
        let mut shapes = Vec::with_capacity(cells.len());
        for (index, color) in cells.iter().enumerate() {
            // Unknown is gray whatever the render style is: the traditional styles leave it the
            // same white as the background, which would make most of a replay invisible.
            let fill = if *color == UNSOLVED {
                Color32::from_rgb(128, 128, 128)
            } else {
                let (r, g, b) = palette[color].rgb;
                Color32::from_rgb(r, g, b)
            };
            let cell = index as u32;
            let origin = picture.cell_origin(cell);
            // A `Corner` color is a half-square — a trianogram's diagonal — and the canvas draws
            // it as one, so the replay has to as well. (A triddler's triangular *cells* are a
            // different thing, and come out of the geometry below.)
            shapes.push(match palette[color].corner {
                Some(corner) => super::triano::half_cell(
                    corner,
                    fill,
                    &to_screen,
                    to_screen * Pos2::new(origin.x, origin.y),
                ),
                None => {
                    let (verts, n) = picture.cell_shape(cell).vertices(origin);
                    egui::Shape::convex_polygon(
                        verts[..n]
                            .iter()
                            .map(|p| to_screen * Pos2::new(p.x, p.y))
                            .collect(),
                        fill,
                        egui::Stroke::default(),
                    )
                }
            });
        }
        painter.extend(shapes);

        // Only while it's actually moving, and only in time for the next step, so a replay
        // doesn't drag the whole app up to the monitor's frame rate and a finished one leaves it
        // idle altogether.
        if running {
            let next = (step + 1) as f32 / REPLAY_STEPS_PER_SECOND;
            let wait = (next - self.started.elapsed().as_secs_f32()).max(0.0);
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f32(wait));
        }
    }
}

#[derive(Clone, Copy)]
pub enum Orientation {
    Horizontal,
    Vertical,
}

/// What one square gutter shows in place of an analysis mark: the length of the block under the
/// pointer, on the one line of that gutter's family the pointer is on.
#[derive(Clone, Copy)]
pub struct BlockHint {
    /// Which line of the family, indexed like the gutter's own clue lists.
    pub line: usize,
    pub len: usize,
    pub rgb: (u8, u8, u8),
}

/// What a square clue gutter draws besides the clues themselves: the indicator strip's contents,
/// and which clues have been resolved (and so are drawn without their boxes).
#[derive(Clone, Copy)]
pub struct GutterMarks<'a> {
    /// This family's per-line analysis marks, if the analysis is being shown.
    pub analysis: Option<&'a [LineStatus]>,
    /// Per line of this family, the indices of that line's fully-resolved clues.
    pub fixed: Option<&'a [Vec<usize>]>,
    /// The clues the user has checked off by hand, keyed by `(lane, index within the lane)`.
    pub checked: &'a HashSet<ClueId>,
    /// Whether the analysis predates the picture as it now stands.
    pub is_stale: bool,
    pub hover: Option<BlockHint>,
}

use crate::solve::line_solve::SolveMode;

/// The little mark showing which technique will crack a line: a dot for skimming, a diamond for
/// scrubbing, a red cross for a contradiction.
pub(crate) fn draw_analysis_mark(
    painter: &egui::Painter,
    center: Pos2,
    scale: f32,
    status: &LineStatus,
    is_stale: bool,
) {
    let radius = scale * crate::layout::ANALYSIS_MARK_RADIUS;
    let color = if is_stale {
        Color32::from_gray(192)
    } else {
        Color32::BLACK
    };

    match status {
        Ok(Some(SolveMode::Skim)) => {
            painter.circle_filled(center, radius, color);
        }
        Ok(Some(SolveMode::Scrub)) => {
            let points = vec![
                center + Vec2::new(0.0, -radius),
                center + Vec2::new(radius, 0.0),
                center + Vec2::new(0.0, radius),
                center + Vec2::new(-radius, 0.0),
            ];
            painter.add(egui::Shape::convex_polygon(
                points,
                color,
                egui::Stroke::NONE,
            ));
        }
        Err(_) => {
            let stroke = egui::Stroke::new((radius * 0.4).max(1.0), Color32::RED);
            painter.line_segment(
                [
                    center + Vec2::new(-radius, -radius),
                    center + Vec2::new(radius, radius),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + Vec2::new(radius, -radius),
                    center + Vec2::new(-radius, radius),
                ],
                stroke,
            );
        }
        _ => {}
    }
}

/// The font to write a clue in: `scale * font_scale` tall, but squeezed narrower as the number
/// gets longer, so a three-digit clue takes up no more width than a one-digit one.
pub(crate) fn clue_font(
    ui: &egui::Ui,
    clue_txt: &str,
    scale: f32,
    font_scale: f32,
) -> egui::FontId {
    let base_font = egui::FontId::monospace(scale * font_scale);
    let text_width = |fonts: &Fonts, t: &str| {
        fonts
            .layout_no_wrap(t.to_string(), base_font.clone(), Color32::BLACK)
            .rect
            .width()
    };

    let (width_2, width_3) = ui.fonts(|f| {
        (
            f32::max(text_width(f, "00") / (scale * font_scale), 1.0),
            f32::max(text_width(f, "000") / (scale * font_scale), 1.0),
        )
    });
    let fonts_by_digit = [
        base_font.clone(),
        base_font,
        egui::FontId::monospace(scale * font_scale / width_2),
        egui::FontId::monospace(scale * font_scale / width_3),
    ];

    fonts_by_digit[clue_txt.len().min(fonts_by_digit.len() - 1)].clone()
}

pub(super) fn draw_string_at(
    ui: &egui::Ui,
    painter: &egui::Painter,
    center: Pos2,
    clue_txt: &str,
    scale: f32,
    (r, g, b): (u8, u8, u8),
    font_scale: f32,
) {
    let text_color = if r as u16 + g as u16 + b as u16 > 384 {
        Color32::BLACK
    } else {
        Color32::WHITE
    };

    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        clue_txt,
        clue_font(ui, clue_txt, scale, font_scale),
        text_color,
    );
}

/// How light a color looks, from 0.0 (black) to 1.0 (white).
fn luminance(c: Color32) -> f32 {
    (0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32) / 255.0
}

/// The color to outline something drawn in `rgb` with, so that it stays legible against the panel
/// it sits on — `None` when the color already stands out on its own.
pub(crate) fn contrast_outline(ui: &egui::Ui, (r, g, b): (u8, u8, u8)) -> Option<Color32> {
    /// Below this much difference in lightness, a color needs an outline to be legible.
    const MIN_CONTRAST: f32 = 0.4;

    let fill = Color32::from_rgb(r, g, b);
    if (luminance(fill) - luminance(ui.visuals().panel_fill)).abs() >= MIN_CONTRAST {
        return None;
    }
    Some(if luminance(fill) > 0.5 {
        Color32::BLACK
    } else {
        Color32::WHITE
    })
}

/// How wide to stroke the halo around a bare number. Doubled because half of a stroke hides under
/// the glyphs it outlines, so the border shows `scale * 0.05` wide.
pub(crate) fn outline_width(scale: f32) -> f32 {
    2.0 * (scale * 0.05).max(1.0)
}

/// How wide to stroke a resolved clue's own outline. Thinner than a number's halo: all of this
/// stroke shows, rather than half of it hiding under the glyphs.
pub(super) fn clue_outline_width(scale: f32) -> f32 {
    (scale * 0.04).max(1.0)
}

/// A number written straight onto the canvas in its own color, with no clue box behind it: the
/// gutters' hover readout.
///
/// A pale color (or, in a dark theme, a dark one) would be all but invisible against the plain
/// background, so a number that doesn't stand out on its own gets an outline in the opposite
/// extreme, stroked along the digits themselves (see `outline_text`).
pub(crate) fn draw_bare_number(
    ui: &egui::Ui,
    painter: &egui::Painter,
    center: Pos2,
    txt: &str,
    scale: f32,
    rgb: (u8, u8, u8),
) {
    /// The size of a square clue's own label — the indicator strip is a clue box wide, so a
    /// number fills it the same way a clue fills its box.
    const FONT_SCALE: f32 = 0.7;

    draw_bare_number_sized(ui, painter, center, txt, scale, rgb, FONT_SCALE);
}

/// As `draw_bare_number`, but for a box whose label isn't a square clue's size — a rhombus's is
/// smaller, so the number that replaces it has to be too.
pub(super) fn draw_bare_number_sized(
    ui: &egui::Ui,
    painter: &egui::Painter,
    center: Pos2,
    txt: &str,
    scale: f32,
    (r, g, b): (u8, u8, u8),
    font_scale: f32,
) {
    let font = clue_font(ui, txt, scale, font_scale);
    let fill = Color32::from_rgb(r, g, b);

    if let Some(outline) = contrast_outline(ui, (r, g, b)) {
        painter.extend(outline_text::halo_shapes(
            ui,
            center,
            txt,
            &font,
            outline,
            outline_width(scale),
        ));
    }

    painter.text(center, egui::Align2::CENTER_CENTER, txt, font, fill);
}

pub(crate) fn draw_string_in_box(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: Rect,
    clue_txt: &str,
    scale: f32,
    rgb: (u8, u8, u8),
) {
    painter.rect_filled(rect, 0.0, Color32::from_rgb(rgb.0, rgb.1, rgb.2));
    draw_string_at(ui, painter, rect.center(), clue_txt, scale, rgb, 0.7);
}

/// The mean of a convex polygon's vertices: the true centre for a parallelogram, and a triangle's
/// centroid — exactly where `CellShape::center` puts it.
pub(super) fn polygon_centroid(points: &[Pos2]) -> Pos2 {
    let sum = points.iter().fold(Vec2::ZERO, |acc, p| acc + p.to_vec2());
    Pos2::ZERO + sum / points.len() as f32
}

pub(super) fn fill_polygon(painter: &egui::Painter, points: &[Pos2], (r, g, b): (u8, u8, u8)) {
    painter.add(egui::Shape::convex_polygon(
        points.to_vec(),
        Color32::from_rgb(r, g, b),
        egui::Stroke::NONE,
    ));
}

/// As `draw_string_in_box`, but the clue box is an arbitrary convex polygon — the solver
/// sidebar's rosette centre is whatever shape the hovered cell is, rather than a rect.
pub(crate) fn draw_string_in_polygon(
    ui: &egui::Ui,
    painter: &egui::Painter,
    points: &[Pos2],
    clue_txt: &str,
    scale: f32,
    rgb: (u8, u8, u8),
) {
    fill_polygon(painter, points, rgb);
    draw_string_at(
        ui,
        painter,
        polygon_centroid(points),
        clue_txt,
        scale,
        rgb,
        0.7,
    );
}

fn draw_clues<C: crate::puzzle::Clue>(
    ui: &mut egui::Ui,
    puzzle: &crate::puzzle::Puzzle<C, crate::geometry::Square>,
    scale: f32,
    orientation: Orientation,
    marks: &GutterMarks<'_>,
) -> Option<ClueId> {
    // The strip between the grid and the first clue box, where the per-line analysis mark goes.
    // It's in cell units like everything else here, so it holds its proportions at any zoom (and
    // stays wide enough for a bare number to be drawn there instead of the mark).
    let puzz_padding = scale * crate::layout::CLUE_PAD;
    let between_clues = scale * 0.5;
    let box_side = scale * 0.9;
    let box_margin = (scale - box_side) / 2.0;

    let clues_vec = match orientation {
        Orientation::Horizontal => puzzle.row_clues(),
        Orientation::Vertical => puzzle.col_clues(),
    };

    let mut max_size: f32 = 0.0;
    for line_clues in clues_vec {
        let mut this_size = 0.0;
        for clue in line_clues {
            this_size += box_side * (clue.express(&puzzle.palette).len() as f32) + between_clues;
        }
        max_size = max_size.max(this_size);
    }
    max_size += puzz_padding;

    let (response, painter) = ui.allocate_painter(
        match orientation {
            Orientation::Horizontal => Vec2::new(max_size, scale * puzzle.row_clues().len() as f32),
            Orientation::Vertical => Vec2::new(scale * puzzle.col_clues().len() as f32, max_size),
        } + Vec2::new(2.0, 2.0),
        // Clicking a clue box checks it off by hand; see `SolveGui::toggle_checked_clue`.
        egui::Sense::click(),
    );

    // Where a click landed, if this gutter took one this frame, and which clue it fell on.
    let click_pos = response
        .interact_pointer_pos()
        .filter(|_| response.clicked());
    let hover_pos = response.hover_pos();
    let mut clicked = None;

    // Rows are family 0 and columns family 1, so this gutter's lanes start here.
    let family = match orientation {
        Orientation::Horizontal => 0,
        Orientation::Vertical => 1,
    };
    let family_start = puzzle.geometry.lane_map().family(family).start;

    for i in 0..clues_vec.len() {
        // The indicator strip against the grid: the hovered line's block length if there is one,
        // and otherwise the analysis mark the number is deliberately covering up.
        let center = match orientation {
            Orientation::Horizontal => Pos2::new(
                response.rect.max.x - puzz_padding / 2.0,
                response.rect.min.y + (i as f32 + 0.5) * scale,
            ),
            Orientation::Vertical => Pos2::new(
                response.rect.min.x + (i as f32 + 0.5) * scale,
                response.rect.max.y - puzz_padding / 2.0,
            ),
        };
        match marks.hover.filter(|h| h.line == i) {
            Some(h) => {
                draw_bare_number(ui, &painter, center, &h.len.to_string(), scale, h.rgb);
            }
            None => {
                if let Some(analysis) = marks.analysis {
                    draw_analysis_mark(&painter, center, scale, &analysis[i], marks.is_stale);
                }
            }
        }

        let line_clues = &clues_vec[i];
        let mut current_pos = match orientation {
            Orientation::Horizontal => response.rect.max.x - puzz_padding,
            Orientation::Vertical => response.rect.max.y - puzz_padding,
        };

        // A resolved clue is drawn as a bare number: its box has nothing left to tell the solver,
        // so it stops competing for attention with the ones that do.
        let fixed_here = marks.fixed.and_then(|f| f.get(i));
        for (clue_idx, clue) in line_clues.iter().enumerate().rev() {
            let auto_fixed = fixed_here.is_some_and(|fixed| fixed.contains(&clue_idx));
            let fixed = auto_fixed || marks.checked.contains(&(family_start + i, clue_idx));
            // A triano clue is a shape as much as a number — the caps say which way its ends
            // slant — so it's drawn as one silhouette: filled while there's still work in it, and
            // outlined once it's resolved. A nonogram clue is a lone box, and needs neither.
            let shaped = C::style() == crate::puzzle::ClueStyle::Triano;

            // Lay the clue's boxes out before painting any of them, since the silhouette has to
            // go down before the boxes that sit on it.
            let mut boxes: Vec<(&crate::puzzle::ColorInfo, Option<u16>, Rect)> = vec![];
            for (color_info, len) in clue.express(&puzzle.palette).into_iter().rev() {
                let corner = match orientation {
                    Orientation::Horizontal => Pos2::new(
                        current_pos - box_side,
                        response.rect.min.y + (i as f32) * scale + box_margin,
                    ),
                    Orientation::Vertical => Pos2::new(
                        response.rect.min.x + (i as f32) * scale + box_margin,
                        current_pos - box_side,
                    ),
                };
                boxes.push((
                    color_info,
                    len,
                    Rect::from_min_size(corner, Vec2::new(box_side, box_side)),
                ));
                current_pos -= box_side;
            }
            current_pos -= between_clues;

            // Every box of a clue answers for the whole clue, caps included. A clue the solver
            // checked off itself takes no clicks, though: unchecking it would last only until the
            // next repaint.
            if !auto_fixed {
                let on_clue = |p: Pos2| boxes.iter().any(|(_, _, rect)| rect.contains(p));
                if hover_pos.is_some_and(on_clue) {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if click_pos.is_some_and(on_clue) {
                    clicked = Some((family_start + i, clue_idx));
                }
            }

            // The body's color speaks for the whole clue; a clue that is nothing but caps falls
            // back to the first of those.
            let clue_rgb = boxes
                .iter()
                .find(|(_, len, _)| len.is_some())
                .or(boxes.first())
                .map(|(color_info, _, _)| color_info.rgb);
            if shaped && let Some(rgb) = clue_rgb {
                super::triano::draw_clue_silhouette(ui, &painter, &boxes, fixed, scale, rgb);
            }

            for (color_info, len, rect) in boxes {
                if let Some(len) = len {
                    assert!(len > 0);

                    if fixed {
                        draw_bare_number(
                            ui,
                            &painter,
                            rect.center(),
                            &len.to_string(),
                            scale,
                            color_info.rgb,
                        );
                    } else {
                        draw_string_in_box(
                            ui,
                            &painter,
                            rect,
                            &len.to_string(),
                            scale,
                            color_info.rgb,
                        );
                    }
                } else if !fixed {
                    // A resolved clue's caps are left to the silhouette outline drawn above.
                    super::triano::draw_cap(&painter, color_info, rect);
                }
            }
        }
    }

    clicked
}

pub fn draw_dyn_clues(
    ui: &mut egui::Ui,
    puzzle: &DynPuzzle,
    scale: f32,
    orientation: Orientation,
    marks: &GutterMarks<'_>,
) -> Option<ClueId> {
    // Clue gutters are still laid out as two axis-aligned rectangles, so only square puzzles
    // can be drawn. `Geometry::gutters` has the per-lane anchors a six-way version needs.
    match puzzle {
        DynPuzzle::SquareNono(puzzle) => draw_clues(ui, puzzle, scale, orientation, marks),
        DynPuzzle::SquareTriano(puzzle) => draw_clues(ui, puzzle, scale, orientation, marks),
        DynPuzzle::TriNono(_) => None,
    }
}

#[cfg(test)]
mod replay_tests {
    use super::*;
    use crate::gui::NonogramGui;
    use crate::puzzle::{BACKGROUND, Solution};

    fn canvas() -> CanvasGui {
        NonogramGui::new(Document::from_solution(
            DynSolution::Square(Solution::blank_bw(3, 3)),
            "test".to_string(),
        ))
        .editor_gui
    }

    fn paint(canvas: &mut CanvasGui, cells: &[u32], color: Color) {
        canvas.perform(
            Action::ChangeColor {
                changes: cells.iter().map(|c| (*c, color)).collect(),
            },
            ActionMood::Normal,
        );
    }

    fn cells(canvas: &CanvasGui) -> Vec<Color> {
        canvas.document.try_solution().unwrap().cells().to_vec()
    }

    /// The replay's `n`th frame is the picture as it stood after `n` actions — including the
    /// steps that repaint a cell an earlier step already touched.
    #[test]
    fn frames_retrace_the_solve() {
        let mut canvas = canvas();
        let mut expected = vec![cells(&canvas)];
        paint(&mut canvas, &[0, 1, 2], Color(1));
        expected.push(cells(&canvas));
        paint(&mut canvas, &[1, 4], BACKGROUND);
        expected.push(cells(&canvas));
        paint(&mut canvas, &[4, 8], Color(1));
        expected.push(cells(&canvas));

        let replay = Replay::new(&canvas);
        assert_eq!(replay.len(&canvas), 3);
        for (step, want) in expected.iter().enumerate() {
            assert_eq!(replay.frame(&canvas, step), *want, "at step {step}");
        }
    }

    /// Undone work is not part of the solve: the undo stack no longer holds it, so the replay
    /// doesn't show it either.
    #[test]
    fn undone_steps_do_not_appear() {
        let mut canvas = canvas();
        let start = cells(&canvas);
        paint(&mut canvas, &[0], Color(1));
        paint(&mut canvas, &[8], Color(1));
        canvas.un_or_re_do(true);

        let replay = Replay::new(&canvas);
        assert_eq!(replay.len(&canvas), 1);
        assert_eq!(replay.frame(&canvas, 0), start);
        assert_eq!(replay.frame(&canvas, 1), cells(&canvas));
    }

    /// Reading the stack live means the picture keeps moving under the replay. Painting after
    /// finishing is unwound rather than replayed, because the step count was frozen at the moment
    /// the solve came out right.
    #[test]
    fn painting_after_the_solve_is_not_replayed() {
        let mut canvas = canvas();
        paint(&mut canvas, &[0], Color(1));
        paint(&mut canvas, &[8], Color(1));
        let solved = cells(&canvas);

        let replay = Replay::new(&canvas);
        paint(&mut canvas, &[4], Color(1));
        paint(&mut canvas, &[5], Color(1));

        assert_eq!(replay.len(&canvas), 2);
        // The last frame is still the solve's own, not the doodled-on picture.
        assert_eq!(replay.frame(&canvas, 2), solved);
        assert_ne!(cells(&canvas), solved);
    }

    /// Undoing back past where the solve finished really does take those steps off the stack, so
    /// the replay gets shorter rather than reading off the end of it.
    #[test]
    fn undoing_past_the_end_shortens_the_replay() {
        let mut canvas = canvas();
        let start = cells(&canvas);
        paint(&mut canvas, &[0], Color(1));
        paint(&mut canvas, &[8], Color(1));

        let replay = Replay::new(&canvas);
        canvas.un_or_re_do(true);
        canvas.un_or_re_do(true);

        assert_eq!(replay.len(&canvas), 0);
        assert_eq!(replay.frame(&canvas, 0), start);
    }

    /// Checking a clue off goes on the undo stack, but it isn't a step of the *solve*: the replay
    /// has one frame per painting step and steps straight past the rest.
    #[test]
    fn check_offs_are_not_replayed() {
        let mut canvas = canvas();
        let start = cells(&canvas);
        paint(&mut canvas, &[0], Color(1));
        let after_painting = cells(&canvas);
        canvas.perform(Action::ToggleClue { clue: (0, 0) }, ActionMood::Normal);
        paint(&mut canvas, &[8], Color(1));

        let replay = Replay::new(&canvas);
        assert_eq!(canvas.undo_stack.len(), 3, "the check-off is on the stack");
        assert_eq!(replay.len(&canvas), 2, "but it isn't a step of the solve");
        assert_eq!(replay.frame(&canvas, 0), start);
        assert_eq!(replay.frame(&canvas, 1), after_painting);
        assert_eq!(replay.frame(&canvas, 2), cells(&canvas));
    }

    /// Cells get whole pixels whenever the puzzle fits at one pixel per cell, and the replay
    /// never spills out of the sidebar.
    #[test]
    fn replay_scale_prefers_whole_pixels() {
        // 180px of sidebar, 25 cells across: 7.2px each, so 7.
        assert_eq!(replay_scale(25.0, 180.0), 7.0);
        // An exact fit isn't rounded down.
        assert_eq!(replay_scale(20.0, 180.0), 9.0);
        // Too wide for one pixel per cell: fractional, but still inside the sidebar.
        assert_eq!(replay_scale(360.0, 180.0), 0.5);
        // Never wider than the sidebar (up to the rounding a division can't avoid, which
        // `show` clamps away), and never so small there's nothing to see.
        for across in 1..500 {
            let scale = replay_scale(across as f32, 180.0);
            assert!(
                scale > 0.0 && scale * across as f32 <= 180.001,
                "{across} across"
            );
        }
    }
}
