//! The palette editor: the swatch list in the sidebar, and the number-key shortcuts that pick
//! from it.

use super::*;

/// Find a color that's definitely safe, at least.
pub fn default_color(palette: &Palette) -> Color {
    if palette.contains_key(&Color(1)) {
        Color(1)
    } else {
        BACKGROUND
    }
}

/// The number-key shortcut for the `index`th palette entry, in the order the palette editor
/// shows them, along with the character to name it in a tooltip.
fn palette_shortcut(index: usize) -> Option<(egui::Key, char)> {
    use egui::Key::*;
    const KEYS: [(egui::Key, char); 10] = [
        (Num1, '1'),
        (Num2, '2'),
        (Num3, '3'),
        (Num4, '4'),
        (Num5, '5'),
        (Num6, '6'),
        (Num7, '7'),
        (Num8, '8'),
        (Num9, '9'),
        (Num0, '0'),
    ];
    KEYS.get(index).copied()
}

impl CanvasGui {
    pub(super) fn palette_editor(&mut self, ui: &mut egui::Ui, read_only: bool) {
        let mut picked_color = self.current_color;
        let mut removed_color = None;
        let mut add_color = false;

        // Same story as in `common_sidebar_items`: these shortcuts have no modifier, so they
        // have to stand down by hand while a `TextEdit` has the keyboard.
        let typing = ui.ctx().wants_keyboard_input();

        use itertools::Itertools;

        for (index, (color, color_info)) in self
            .document
            .solution_mut()
            .palette_mut()
            .iter_mut()
            .sorted_by_key(|(color, _)| *color)
            // TODO: actually paint a palette entry for unsolved,
            // in case the user doesn't have a middle button.
            .filter(|(color, _)| !(**color == UNSOLVED && read_only))
            .enumerate()
        {
            let shortcut = palette_shortcut(index);
            let (r, g, b) = color_info.rgb;
            let button_text = if color_info.corner.is_some() {
                color_info.ch.to_string()
            } else {
                "■".to_string()
            };

            ui.horizontal(|ui| {
                ui.label(RichText::new(icons::ICON_CHEVRON_FORWARD).size(24.0).color(
                    Color32::from_black_alpha(if *color == picked_color { 255 } else { 0 }),
                ));

                let color_text = RichText::new(button_text)
                    .monospace()
                    .size(24.0)
                    .color(egui::Color32::from_rgb(r, g, b));
                let hover_text = match shortcut {
                    Some((_, ch)) => format!("Paint with {} (press {})", color_info.name, ch),
                    None => format!("Paint with {}", color_info.name),
                };
                if ui
                    .add(egui::Button::new(color_text))
                    .on_hover_text(hover_text)
                    .clicked()
                    || (!typing
                        && shortcut.is_some_and(|(key, _)| ui.input(|i| i.key_pressed(key))))
                {
                    picked_color = *color;
                };

                if !read_only {
                    let mut edited_color = [r as f32 / 256.0, g as f32 / 256.0, b as f32 / 256.0];

                    let edit = ui.color_edit_button_rgb(&mut edited_color);
                    // `egui` only allows rectangular-swatch-of-current-color as the palette marker,
                    // which doesn't look good in this case. (In fact, the color is also somewhat wrong)
                    // HACK: draw a pencil icon over it.
                    let visuals = *ui.style().interact(&edit);
                    let painter = ui.painter();
                    painter.rect(
                        edit.rect,
                        visuals.corner_radius,
                        visuals.bg_fill,
                        visuals.bg_stroke,
                        egui::StrokeKind::Inside,
                    );
                    painter.text(
                        edit.rect.center(),
                        egui::Align2::CENTER_CENTER,
                        icons::ICON_EDIT,
                        egui::FontId::proportional(edit.rect.height() * 0.7),
                        visuals.fg_stroke.color,
                    );
                    if edit.on_hover_text("Edit this color").changed() {
                        // TODO: this should probably also be undoable
                        picked_color = *color;
                        color_info.rgb = (
                            (edited_color[0] * 256.0) as u8,
                            (edited_color[1] * 256.0) as u8,
                            (edited_color[2] * 256.0) as u8,
                        );
                    }
                    if *color != BACKGROUND && ui.button(icons::ICON_DELETE).clicked() {
                        removed_color = Some(*color);
                    }
                }
            });
        }
        if !read_only && ui.button("New color").clicked() {
            add_color = true;
        }
        self.current_color = picked_color;

        if Some(self.current_color) == removed_color {
            self.current_color = BACKGROUND;
        }

        if let Some(removed_color) = removed_color {
            let mut new_document = self.document.clone();
            let new_picture = new_document.solution_mut();
            for cell in new_picture.cells_mut().iter_mut() {
                if *cell == removed_color {
                    *cell = self.current_color;
                }
            }
            new_picture.palette_mut().remove(&removed_color);
            self.perform(
                Action::ReplaceDocument {
                    document: Box::new(new_document),
                },
                ActionMood::Normal,
            );
        }
        if add_color {
            let mut new_document = self.document.clone();
            let new_picture = new_document.solution_mut();
            let next_color = Color(new_picture.palette().keys().map(|k| k.0).max().unwrap() + 1);
            new_picture.palette_mut().insert(
                next_color,
                ColorInfo {
                    ch: (next_color.0 + 65) as char, // TODO: will break chargrid export
                    name: "New color".to_string(),
                    rgb: (128, 128, 128),
                    color: next_color,
                    corner: None,
                },
            );
            self.perform(
                Action::ReplaceDocument {
                    document: Box::new(new_document),
                },
                ActionMood::Normal,
            );
        }
    }
}

#[cfg(test)]
mod palette_tests {
    use super::*;
    use crate::puzzle::Solution;

    fn doc(sol: Solution<crate::geometry::Square>) -> Document {
        Document::from_solution(DynSolution::Square(sol), "test".to_string())
    }

    #[test]
    fn swapping_the_document_cant_leave_the_color_dangling() {
        let mut fancy = Solution::blank_bw(3, 3);
        fancy
            .palette
            .insert(Color(3), ColorInfo::default_fg(Color(3)));

        let mut gui = NonogramGui::new(doc(fancy)).editor_gui;
        gui.current_color = Color(3);
        gui.drag_start_color = Color(3);

        // The black-and-white palette has no `Color(3)`.
        gui.perform(
            Action::ReplaceDocument {
                document: Box::new(doc(Solution::blank_bw(3, 3))),
            },
            ActionMood::Normal,
        );

        let palette = gui.document.try_solution().unwrap().palette();
        assert!(palette.contains_key(&gui.current_color));
        assert!(palette.contains_key(&gui.drag_start_color));
    }

    #[test]
    fn a_color_the_new_palette_still_has_is_left_alone() {
        let mut gui = NonogramGui::new(doc(Solution::blank_bw(3, 3))).editor_gui;
        gui.current_color = BACKGROUND;
        gui.drag_start_color = BACKGROUND;

        gui.perform(
            Action::ReplaceDocument {
                document: Box::new(doc(Solution::blank_bw(4, 4))),
            },
            ActionMood::Normal,
        );

        assert_eq!(gui.current_color, BACKGROUND);
        assert_eq!(gui.drag_start_color, BACKGROUND);
    }
}
