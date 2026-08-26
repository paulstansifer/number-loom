use super::{
    Action, ActionMood, CanvasGui, Disambiguator, Staleable, Tool, default_color, outline_text,
};
use crate::{
    grid_solve::LineStatus,
    puzzle::{Color, DynPuzzle, PuzzleDynOps, UNSOLVED},
    user_settings::{UserSettings, consts},
};
use egui::{Color32, Pos2, Rect, RichText, Vec2, text::Fonts};
use web_time::Instant;

use crate::puzzle::{Document, DynSolution};
pub struct SolveGui {
    pub canvas: CanvasGui,
    pub clues: DynPuzzle,
    pub intended_solution: DynSolution,
    pub analyze_lines: bool,
    pub detect_errors: bool,
    pub infer_background: bool,
    pub line_analysis: Staleable<Option<Vec<Vec<LineStatus>>>>,
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
            render_style: UserSettings::get(consts::SOLVER_RENDER_STYLE)
                .and_then(|name| RenderStyle::from_setting_name(&name))
                .unwrap_or(RenderStyle::Experimental),
            last_inferred_version: u32::MAX,
            hovered_cell: None,
            replay: None,
        }
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
            let plus_size = if triangular { scale * 4.4 } else { scale * 3.0 };

            if let Some(cell) = self.hovered_cell {
                let picture = self.canvas.document.try_solution().unwrap();
                let color = picture.cells()[cell as usize];
                let rgb = picture.palette()[&color].rgb;

                // One run per clue family: two arms each for a square grid, three for a triddler.
                // `arm_directions` places them, so this becomes a hexagonal rosette by itself.
                let runs = picture.runs_at_cell(cell);
                let dirs = picture.arm_directions();

                let (resp, painter) =
                    ui.allocate_painter(Vec2::new(plus_size, plus_size), egui::Sense::empty());

                let rect = resp.rect;
                let text = if color == UNSOLVED { "?" } else { " " };

                if triangular {
                    let true_center = rect.center();
                    // The centre swatch matches the hovered triangle's own shape (▲ or ▼)
                    // instead of a generic square.
                    let cell_shape = picture.cell_shape(cell);
                    let mid_size =
                        crate::layout::Vec2::new(scale, scale * crate::layout::TRI_ROW_HEIGHT);
                    let (verts, n) = cell_shape.vertices_sized(
                        crate::layout::Point::new(
                            true_center.x - mid_size.x / 2.0,
                            true_center.y - mid_size.y / 2.0,
                        ),
                        mid_size,
                    );
                    let mid_points: Vec<Pos2> =
                        verts[..n].iter().map(|p| Pos2::new(p.x, p.y)).collect();
                    draw_string_in_polygon(ui, &painter, &mid_points, text, scale, rgb);

                    // Arms are shaped like the rhombus that lane's clue boxes use, so they read
                    // as belonging to that lane, and pushed further out than a square grid's
                    // arms so adjacent rhombuses (60° apart, wide across their short diagonal)
                    // don't overlap each other or the centre swatch.
                    let arm_size = scale * 0.68;
                    let arm_distance = scale * 1.7;
                    for (family, (back, forward)) in runs.iter().enumerate() {
                        for (i, count) in [back, forward].into_iter().enumerate() {
                            let Some(dir) = dirs.get(family * 2 + i) else {
                                continue;
                            };
                            if *count == 0 {
                                continue;
                            }
                            let arm_center = true_center + Vec2::new(dir.x, dir.y) * arm_distance;
                            // The arm's short side should sit flush against this cell's real
                            // edge for the *other* family it borders — "near" (leading to the
                            // previous cell) for the back arm, "far" for the forward arm.
                            let (ea, eb) = super::annotate::lane_step_edge(
                                cell_shape,
                                crate::layout::Point::new(0.0, 0.0),
                                family,
                                i == 0,
                            );
                            let (edx, edy) = (eb.x - ea.x, eb.y - ea.y);
                            let elen = (edx * edx + edy * edy).sqrt();
                            let edge_dir = if elen > 0.0 {
                                crate::layout::Vec2::new(edx / elen, edy / elen)
                            } else {
                                crate::layout::Vec2::new(0.0, 1.0)
                            };
                            // Same long/short proportion as a real clue box, so this preview
                            // actually looks like the gutter boxes it's previewing.
                            let arm_short = arm_size
                                * (crate::layout::CLUE_BOX_SHORT / crate::layout::CLUE_BOX);
                            let points: Vec<Pos2> = crate::layout::tri_clue_rhombus(
                                crate::layout::Point::new(arm_center.x, arm_center.y),
                                family,
                                edge_dir,
                                arm_size,
                                arm_short,
                            )
                            .iter()
                            .map(|p| Pos2::new(p.x, p.y))
                            .collect();
                            draw_string_in_rhombus(
                                ui,
                                &painter,
                                &points,
                                &count.to_string(),
                                scale,
                                rgb,
                            );
                        }
                    }
                } else {
                    let size = Vec2::new(20.0, 20.0);
                    let center = rect.min + Vec2::new(scale, scale);

                    // `arm_directions` lists each family's two directions adjacently, matching
                    // the `(backward, forward)` pairs `runs_at_cell` returns.
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

            if ui.checkbox(&mut self.analyze_lines, "[auto]").changed() {
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
            if ui.button("Analyze Lines").clicked() || self.analyze_lines {
                let clues = &self.clues;
                let picture = self.canvas.document.try_solution().unwrap();
                let grid = picture.to_partial();
                self.line_analysis
                    .get_or_refresh(self.canvas.version, || Some(clues.analyze_lines(&grid)));
            }

            ui.separator();

            if ui.checkbox(&mut self.detect_errors, "[auto]").changed() {
                let _ = UserSettings::set(
                    consts::SOLVER_DETECT_ERRORS,
                    &self.detect_errors.to_string(),
                );
            }
            if (ui.button("Detect errors").clicked() || self.detect_errors)
                && self.detect_any_errors()
            {
                ui.colored_label(egui::Color32::DARK_RED, "Error detected");
            }
            ui.separator();

            if ui.checkbox(&mut self.infer_background, "[auto]").changed() {
                let _ = UserSettings::set(
                    consts::SOLVER_INFER_BACKGROUND,
                    &self.infer_background.to_string(),
                );
            }
            if (ui.button("Infer background").clicked() || self.infer_background)
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
                is_stale,
                hover,
            };
            self.hovered_cell =
                self.canvas
                    .canvas_with_clues(ui, scale, self.render_style, Some(overlay));
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

        ui.vertical(|ui| {
            // No spacing between the cells: each clue gutter reserves its own gap against the
            // grid (`CLUE_PAD`, in cell units), and egui's default few pixels on top of that
            // would be a zoom-independent gap that swamps the gutter when zoomed out.
            egui::Grid::new("solve_grid")
                .spacing(Vec2::ZERO)
                .show(ui, |ui| {
                    ui.label(""); // Top-left is empty
                    let line_analysis = self.line_analysis.val.as_ref();
                    draw_dyn_clues(
                        ui,
                        &self.clues,
                        scale,
                        Orientation::Vertical,
                        line_analysis.and_then(|la| la.get(1)).map(|v| &v[..]),
                        is_stale,
                        col_hint,
                    );
                    ui.end_row();

                    draw_dyn_clues(
                        ui,
                        &self.clues,
                        scale,
                        Orientation::Horizontal,
                        line_analysis.and_then(|la| la.first()).map(|v| &v[..]),
                        is_stale,
                        row_hint,
                    );
                    self.hovered_cell = self.canvas.canvas(ui, scale, self.render_style);
                    ui.end_row();
                });
        });
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
/// The undo stack *is* the recording: it holds one entry per step, each naming the cells that
/// step painted and the colors they had before it. So this stores no picture data of its own —
/// just how long the solve was and when the animation started — and rebuilds each frame on
/// demand from the stack as it stands. Frames cost a grid copy and a walk over the steps above
/// the one being shown, which at a few frames a second is nothing.
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

    /// How many steps there are left to show: the solve's length, except that undoing back past
    /// where it finished really does take those steps off the stack, leaving less to replay.
    fn len(&self, canvas: &CanvasGui) -> usize {
        self.steps.min(canvas.undo_stack.len())
    }

    /// The picture as it stood after `step` steps of the solve.
    ///
    /// Each undo entry holds the colors its step painted over, so starting from the picture as it
    /// stands *now* and applying them newest-first walks backwards through the solve. Anything
    /// the user painted after finishing gets unwound on the way past.
    fn frame(&self, canvas: &CanvasGui, step: usize) -> Vec<Color> {
        let mut cells = canvas.document.try_solution().unwrap().cells().to_vec();
        for undone in canvas.undo_stack[step..].iter().rev() {
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
            let (verts, n) = picture.cell_shape(cell).vertices(picture.cell_origin(cell));
            shapes.push(egui::Shape::convex_polygon(
                verts[..n]
                    .iter()
                    .map(|p| to_screen * Pos2::new(p.x, p.y))
                    .collect(),
                fill,
                egui::Stroke::default(),
            ));
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

use crate::line_solve::SolveMode;

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

fn draw_string_at(
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
    (r, g, b): (u8, u8, u8),
) {
    /// The size of a square clue's own label — the indicator strip is a clue box wide, so a
    /// number fills it the same way a clue fills its box.
    const FONT_SCALE: f32 = 0.7;
    /// Below this much difference in lightness, the number needs an outline to be legible.
    const MIN_CONTRAST: f32 = 0.4;

    let font = clue_font(ui, txt, scale, FONT_SCALE);
    let fill = Color32::from_rgb(r, g, b);

    if (luminance(fill) - luminance(ui.visuals().panel_fill)).abs() < MIN_CONTRAST {
        let outline = if luminance(fill) > 0.5 {
            Color32::BLACK
        } else {
            Color32::WHITE
        };
        // Half the stroke hides under the number, so the border shows `scale * 0.05` wide.
        let width = 2.0 * (scale * 0.05).max(1.0);
        painter.extend(outline_text::halo_shapes(
            ui, center, txt, &font, outline, width,
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
fn polygon_centroid(points: &[Pos2]) -> Pos2 {
    let sum = points.iter().fold(Vec2::ZERO, |acc, p| acc + p.to_vec2());
    Pos2::ZERO + sum / points.len() as f32
}

fn fill_polygon(painter: &egui::Painter, points: &[Pos2], (r, g, b): (u8, u8, u8)) {
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

/// As `draw_string_in_polygon`, but for a rhombus clue box specifically: the label is smaller
/// than a square box's (the rhombus is narrower top-to-bottom than it is wide) and nudged up a
/// bit from the geometric middle, since the rhombus's mass sits toward its bottom half.
pub(crate) fn draw_string_in_rhombus(
    ui: &egui::Ui,
    painter: &egui::Painter,
    points: &[Pos2],
    clue_txt: &str,
    scale: f32,
    rgb: (u8, u8, u8),
) {
    fill_polygon(painter, points, rgb);
    const FONT_SCALE: f32 = 0.5;
    let center = polygon_centroid(points);
    draw_string_at(ui, painter, center, clue_txt, scale, rgb, FONT_SCALE);
}

fn draw_clues<C: crate::puzzle::Clue>(
    ui: &mut egui::Ui,
    puzzle: &crate::puzzle::Puzzle<C, crate::geometry::Square>,
    scale: f32,
    orientation: Orientation,
    line_analysis: Option<&[LineStatus]>,
    is_stale: bool,
    hover: Option<BlockHint>,
) {
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
        egui::Sense::empty(),
    );

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
        match hover.filter(|h| h.line == i) {
            Some(h) => {
                draw_bare_number(ui, &painter, center, &h.len.to_string(), scale, h.rgb);
            }
            None => {
                if let Some(analysis) = line_analysis {
                    draw_analysis_mark(&painter, center, scale, &analysis[i], is_stale);
                }
            }
        }

        let line_clues = &clues_vec[i];
        let mut current_pos = match orientation {
            Orientation::Horizontal => response.rect.max.x - puzz_padding,
            Orientation::Vertical => response.rect.max.y - puzz_padding,
        };

        for clue in line_clues.iter().rev() {
            let expressed_clues = clue.express(&puzzle.palette);

            for (color_info, len) in expressed_clues.into_iter().rev() {
                let (r, g, b) = color_info.rgb;
                let bg_color = egui::Color32::from_rgb(r, g, b);

                let corner = match orientation {
                    Orientation::Horizontal => Pos2::new(
                        current_pos,
                        response.rect.min.y + (i as f32) * scale + box_margin,
                    ),
                    Orientation::Vertical => Pos2::new(
                        response.rect.min.x + (i as f32) * scale + box_margin,
                        current_pos,
                    ),
                };

                if let Some(len) = len {
                    assert!(len > 0);

                    let translated_corner = corner
                        + match orientation {
                            Orientation::Horizontal => Vec2::new(-box_side, 0.0),
                            Orientation::Vertical => Vec2::new(0.0, -box_side),
                        };

                    let rect =
                        Rect::from_min_size(translated_corner, Vec2::new(box_side, box_side));
                    draw_string_in_box(ui, &painter, rect, &len.to_string(), scale, color_info.rgb);
                    current_pos -= box_side;
                } else {
                    let mut triangle = super::triangle_shape(
                        color_info.corner.expect("must be a corner"),
                        bg_color,
                        Vec2::new(box_side, box_side),
                    );
                    let translated_corner = corner
                        + match orientation {
                            Orientation::Horizontal => Vec2::new(-box_side, 0.0),
                            Orientation::Vertical => Vec2::new(0.0, -box_side),
                        };
                    triangle.translate(translated_corner.to_vec2());
                    current_pos -= box_side;

                    painter.add(triangle);
                }
            }
            current_pos -= between_clues;
        }
    }
}

pub fn draw_dyn_clues(
    ui: &mut egui::Ui,
    puzzle: &DynPuzzle,
    scale: f32,
    orientation: Orientation,
    line_analysis: Option<&[LineStatus]>,
    is_stale: bool,
    hover: Option<BlockHint>,
) {
    // Clue gutters are still laid out as two axis-aligned rectangles, so only square puzzles
    // can be drawn. `Geometry::gutters` has the per-lane anchors a six-way version needs.
    match puzzle {
        DynPuzzle::SquareNono(puzzle) => {
            draw_clues(
                ui,
                puzzle,
                scale,
                orientation,
                line_analysis,
                is_stale,
                hover,
            );
        }
        DynPuzzle::SquareTriano(puzzle) => {
            draw_clues(
                ui,
                puzzle,
                scale,
                orientation,
                line_analysis,
                is_stale,
                hover,
            );
        }
        DynPuzzle::TriNono(_) => {}
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
