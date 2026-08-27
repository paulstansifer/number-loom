//! Growing and shrinking the picture.
//!
//! Two separate resizers, because the two shapes resize along different axes: a square puzzle
//! adds and removes whole rows and columns, while a triddler nudges one of its outline's six
//! bounds. They share only the "how many lines" box, parsed by `resize_lines`.

use super::*;

impl NonogramGui {
    /// Parses `lines_to_affect_string` (the resizer's "how many lines" box). On a bad value,
    /// marks the field visibly wrong (appends "??") rather than showing a separate error — the
    /// existing convention both resizers use.
    fn resize_lines(&mut self) -> Option<usize> {
        match self.lines_to_affect_string.parse::<usize>() {
            Ok(lines) => Some(lines),
            Err(_) => {
                self.lines_to_affect_string += "??";
                None
            }
        }
    }

    fn square_resize(&mut self, top: Option<bool>, left: Option<bool>, add: bool) {
        // This resizer is inherently square: it adds and removes whole rows and columns.
        // Triangular puzzles resize by nudging one of six bounds instead — see `tri_resize`.
        let Some(picture) = self.editor_gui.document.square_solution_mut() else {
            self.editor_gui
                .status
                .set(StatusMessage::error(TRIDDLER_UNSUPPORTED));
            return;
        };
        let mut g = picture.to_columns();
        let Some(lines) = self.resize_lines() else {
            return;
        };
        if let Some(left) = left {
            let lines = if add {
                lines
            } else {
                lines.min(g.len().saturating_sub(1)) // don't remove the last column!
            };
            if add {
                g.resize(g.len() + lines, vec![BACKGROUND; g.first().unwrap().len()]);
                if left {
                    g.rotate_right(lines);
                }
            } else {
                if left {
                    g.rotate_left(lines);
                }
                g.truncate(g.len() - lines);
            }
        } else if let Some(top) = top {
            let lines = if add {
                lines
            } else {
                lines.min(g.first().unwrap().len().saturating_sub(1)) // don't remove the last row!
            };
            if add {
                for row in g.iter_mut() {
                    row.resize(row.len() + lines, BACKGROUND);
                    if top {
                        row.rotate_right(lines);
                    }
                }
            } else {
                for row in g.iter_mut() {
                    if top {
                        row.rotate_left(lines);
                    }
                    row.truncate(row.len() - lines);
                }
            }
        }

        let mut new_doc = Box::new(self.editor_gui.document.clone());
        {
            let solution = new_doc.solution_mut();
            *solution = DynSolution::Square(Solution::from_columns(
                solution.clue_style(),
                solution.palette().clone(),
                g,
            ));
        }
        self.editor_gui.perform(
            Action::ReplaceDocument { document: new_doc },
            ActionMood::Normal,
        );
    }

    /// Triangular counterpart to `resize`: grows or shrinks one of the outline's six bounds
    /// (see `Solution::resized`/`geometry::Side`) instead of adding/removing rows or columns.
    fn tri_resize(&mut self, side: crate::geometry::Side, add: bool) {
        let Some(lines) = self.resize_lines() else {
            return;
        };
        let Some(tri) = self
            .editor_gui
            .document
            .try_solution()
            .and_then(|s| s.as_tri())
        else {
            self.editor_gui
                .status
                .set(StatusMessage::error(TRIDDLER_UNSUPPORTED));
            return;
        };
        let delta = if add { lines as i32 } else { -(lines as i32) };
        let Some(resized) = tri.resized(side, delta) else {
            let message = if add {
                "can't grow any further"
            } else {
                "can't shrink any further"
            };
            self.editor_gui.status.set(StatusMessage::error(message));
            return;
        };

        let mut new_doc = Box::new(self.editor_gui.document.clone());
        *new_doc.solution_mut() = DynSolution::Tri(resized);
        self.editor_gui.perform(
            Action::ReplaceDocument { document: new_doc },
            ActionMood::Normal,
        );
    }

    pub(super) fn resizer(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.label(format!(
                "Canvas size: {}",
                self.editor_gui.document.dims_label()
            ));
        });

        centered_row(ui, "resizer_row", |ui| {
            egui::Grid::new("resizer").show(ui, |ui| {
                ui.label("");
                ui.horizontal(|ui| {
                    if ui.button(icons::ICON_ADD).clicked() {
                        self.square_resize(Some(true), None, true);
                    }
                    if ui.button(icons::ICON_REMOVE).clicked() {
                        self.square_resize(Some(true), None, false);
                    }
                });
                ui.label("");
                ui.end_row();

                ui.vertical(|ui| {
                    if ui.button(icons::ICON_ADD).clicked() {
                        self.square_resize(None, Some(true), true);
                    }
                    if ui.button(icons::ICON_REMOVE).clicked() {
                        self.square_resize(None, Some(true), false);
                    }
                });
                ui.text_edit_singleline(&mut self.lines_to_affect_string);

                ui.vertical(|ui| {
                    if ui.button(icons::ICON_ADD).clicked() {
                        self.square_resize(None, Some(false), true);
                    }
                    if ui.button(icons::ICON_REMOVE).clicked() {
                        self.square_resize(None, Some(false), false);
                    }
                });
                ui.end_row();

                ui.label("");
                ui.horizontal(|ui| {
                    if ui.button(icons::ICON_ADD).clicked() {
                        self.square_resize(Some(false), None, true);
                    }
                    if ui.button(icons::ICON_REMOVE).clicked() {
                        self.square_resize(Some(false), None, false);
                    }
                });
                ui.label("");
            });
        });
    }

    /// Triangular counterpart to `resizer`: a hexagon holding the same "how many lines" box,
    /// with a +/- pair on each of its six sides — one per `geometry::Side` — grown/shrunk via
    /// `tri_resize`. Each pair sits flush against, and rotated parallel to, its own hexagon edge.
    pub(super) fn tri_resizer(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.label(format!(
                "Outline size: {}",
                self.editor_gui.document.dims_label()
            ));
        });

        // Used to grey out a "+" that would have no effect at all (see `Outline::can_grow`) —
        // not to decide what clicking it does, `tri_resize` re-checks that against a fresh
        // `Solution` on its own.
        let outline = self
            .editor_gui
            .document
            .try_solution()
            .and_then(|s| s.as_tri())
            .map(|t| *t.geometry.dims());

        // Layout constants, all in screen pixels.
        const APOTHEM: f32 = 22.0; // centre-to-edge distance of the hexagon.
        const CIRCUMRADIUS: f32 = APOTHEM / 0.866_025_4; // centre-to-vertex distance.
        const EDIT_SIZE: Vec2 = Vec2::new(30.0, 18.0);
        const BUTTON_ALONG: f32 = 12.0; // button extent parallel to its edge.
        const BUTTON_ACROSS: f32 = 12.0; // button extent along the outward normal.
        const BUTTON_GAP: f32 = 3.0; // gap between the +/- pair, along the edge.
        const BUTTON_DISTANCE: f32 = APOTHEM + BUTTON_ACROSS / 2.0 + 3.0;
        const MARK_RADIUS: f32 = 3.0; // half-length of the hand-drawn +/- strokes.
        const PAD: f32 = 4.0; // headroom for stroke width / hover expansion at the edges.

        let to_vec2 = |v: crate::layout::Vec2| Vec2::new(v.x, v.y);
        let sides = crate::geometry::Side::all();
        let outward_dirs: Vec<Vec2> = sides
            .iter()
            .map(|s| to_vec2(s.outward_and_edge_dir().0))
            .collect();

        // All the widget's geometry, as a pure function of where its centre ends up. Called
        // once below against a placeholder centre just to measure how much room the widget
        // actually needs, then again against the real centre once that's known — rather than
        // guessing a fixed canvas size up front and hoping the content fits inside it (which
        // previously left it either clipped or sitting in an oversized, wastefully padded box).
        let layout = |center: Pos2| {
            // Hexagon vertices: `Side::all()` is in cyclic (adjacent-sharing-a-vertex) order,
            // and adjacent outward normals are exactly 60° apart, so their sum bisects the angle
            // between them — the direction of the vertex they share. No trig needed.
            let hex_points: Vec<Pos2> = (0..6)
                .map(|i| {
                    let a = outward_dirs[i];
                    let b = outward_dirs[(i + 1) % 6];
                    center + (a + b).normalized() * CIRCUMRADIUS
                })
                .collect();

            let mut buttons = Vec::with_capacity(12);
            for side in sides {
                let (outward, edge_dir) = side.outward_and_edge_dir();
                let (outward, edge_dir) = (to_vec2(outward), to_vec2(edge_dir));
                let base = center + outward * BUTTON_DISTANCE;
                for add in [true, false] {
                    let sign = if add { 1.0 } else { -1.0 };
                    let button_center =
                        base + edge_dir * (sign * BUTTON_GAP / 2.0 + sign * BUTTON_ALONG / 2.0);
                    // A small rectangle with one axis along the edge and the other along the
                    // outward normal — the same "centre ± dir*half ± perp*half" construction
                    // `layout::tri_clue_rhombus` uses for the (differently-shaped) clue boxes.
                    let along = edge_dir * (BUTTON_ALONG / 2.0);
                    let across = outward * (BUTTON_ACROSS / 2.0);
                    let corners = [
                        button_center + along + across,
                        button_center - along + across,
                        button_center - along - across,
                        button_center + along - across,
                    ];
                    buttons.push((side, add, button_center, outward, edge_dir, corners));
                }
            }
            (hex_points, buttons)
        };

        let (probe_hex, probe_buttons) = layout(Pos2::ZERO);
        let mut bounds = Rect::from_center_size(Pos2::ZERO, EDIT_SIZE);
        for p in probe_hex
            .iter()
            .chain(probe_buttons.iter().flat_map(|b| b.5.iter()))
        {
            bounds.extend_with(*p);
        }
        let canvas = bounds.size() + Vec2::splat(2.0 * PAD);

        // Take the full width, so the hexagon ends up centred in the sidebar the way the square
        // resizer's cross does — everything below is placed relative to `response.rect`'s centre,
        // so widening the reservation is all it takes.
        let strip = Vec2::new(ui.available_width().max(canvas.x), canvas.y);
        let (response, painter) = ui.allocate_painter(strip, egui::Sense::hover());
        // Place the bounding box computed above flush inside the reserved rect (plus `PAD`), so
        // there's no dead space on any side and nothing is clipped.
        let center = response.rect.center() - bounds.center().to_vec2();
        let (hex_points, buttons) = layout(center);

        painter.add(egui::Shape::convex_polygon(
            hex_points,
            ui.visuals().extreme_bg_color,
            ui.visuals().window_stroke(),
        ));

        ui.put(
            Rect::from_center_size(center, EDIT_SIZE),
            egui::TextEdit::singleline(&mut self.lines_to_affect_string),
        );

        let mut clicked: Option<(crate::geometry::Side, bool)> = None;
        for (side, add, button_center, outward, edge_dir, corners) in buttons {
            // Only "+" can ever be pointless enough to grey out — see `Outline::can_grow`. If
            // "+5" would be capped to "+1" it's still worth doing, so this only fires when *no*
            // growth at all is possible, not when the requested amount would be capped.
            let enabled = !add || outline.is_none_or(|o| o.can_grow(side));

            let id = ui.id().with((side as u8, add));
            let button_rect = Rect::from_points(&corners);
            let button_response = ui.interact(button_rect, id, egui::Sense::click());
            let visuals = if enabled {
                if button_response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                *ui.style().interact(&button_response)
            } else {
                // Grey out exactly the way `Ui::disable` does (`Visuals::gray_out`), and — to
                // match plain `egui::Button`, whose outline only appears on hover — force no
                // border at all rather than reusing `noninteractive`'s (which is meant for
                // window/separator outlines and is always visible).
                let base = ui.visuals().widgets.inactive;
                egui::style::WidgetVisuals {
                    bg_fill: ui.visuals().gray_out(base.bg_fill),
                    weak_bg_fill: ui.visuals().gray_out(base.weak_bg_fill),
                    bg_stroke: egui::Stroke::NONE,
                    fg_stroke: egui::Stroke::new(
                        base.fg_stroke.width,
                        ui.visuals().gray_out(base.fg_stroke.color),
                    ),
                    corner_radius: base.corner_radius,
                    expansion: base.expansion,
                }
            };
            painter.add(egui::Shape::convex_polygon(
                corners.to_vec(),
                visuals.bg_fill,
                // `bg_stroke`, not `fg_stroke`: egui only draws a button's outline when it's
                // hovered or active (`inactive.bg_stroke` is `Stroke::NONE`), so using it here
                // gets that same "no outline at rest" look for free.
                visuals.bg_stroke,
            ));

            // Hand-drawn +/- (a rotated cross/dash), rather than rotated text: simpler and more
            // robust than centring a rotated glyph, and matches how `draw_analysis_mark` in
            // solver.rs already draws a rotated mark with plain line segments.
            let stroke = egui::Stroke::new(1.5, visuals.fg_stroke.color);
            painter.line_segment(
                [
                    button_center - edge_dir * MARK_RADIUS,
                    button_center + edge_dir * MARK_RADIUS,
                ],
                stroke,
            );
            if add {
                painter.line_segment(
                    [
                        button_center - outward * MARK_RADIUS,
                        button_center + outward * MARK_RADIUS,
                    ],
                    stroke,
                );
            }

            if enabled && button_response.clicked() {
                clicked = Some((side, add));
            }
        }

        if let Some((side, add)) = clicked {
            self.tri_resize(side, add);
        }
        // Hack: we need more space to not overlap the next section:
        ui.add_space(28.0);
    }
}
