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

/// The size the selected-color chevron is drawn at, and how much width its column is given.
///
/// The icon font's em box is three times as wide as this glyph's ink, and all that slack is
/// width the name field beside it would rather have — so the chevron is painted into a column
/// cut down to fit the ink, rather than laid out as a label at its natural width.
/// `the_chevron_fits_its_column` keeps the two in step.
const CHEVRON_SIZE: f32 = 24.0;
const CHEVRON_COLUMN: f32 = 12.0;

/// How bright a color has to be to count as white, how colorless to count as white rather than
/// a pale tint of something, and how dark to count as black — as a fraction of each scale.
const NEUTRAL_TOLERANCE: f32 = 0.1;

/// The saturation and value a color needs before it counts as one of the palette's *hues*
/// rather than a neutral or a muddy shade. Anything under either bar has no say in which hues
/// the palette already covers.
const VIVID: f32 = 0.5;

/// How far from a primary's hue still counts as having that primary, as a fraction of the hue
/// circle — so an orange is close enough to red that there's no point adding another red.
const PRIMARY_TOLERANCE: f32 = 0.2;

/// The distance between two hues, as a fraction of the circle. Never more than half of it, since
/// going round the other way is always an option.
fn hue_distance(a: f32, b: f32) -> f32 {
    let apart = (a - b).abs();
    apart.min(1.0 - apart)
}

/// How much empty circle follows `hues[i]` before the next hue round, as a fraction of the
/// circle. `hues` must be sorted, and hold at least two entries.
fn gap_after(hues: &[f32], i: usize) -> f32 {
    let next = hues[(i + 1) % hues.len()];
    (next - hues[i] + 1.0).fract()
}

/// The most vivid color at `hue`.
fn full_saturation(hue: f32) -> (u8, u8, u8) {
    let [r, g, b] = egui::ecolor::rgb_from_hsv((hue, 1.0, 1.0));
    let byte = |c: f32| (c * 255.0).round() as u8;
    (byte(r), byte(g), byte(b))
}

/// What to call a hue, out of the twelve the color wheel is usually cut into.
/// Partially based on https://bitfume.com/tools/hue-names/
fn hue_name(hue: f32) -> &'static str {
    const NAMES: [&str; 12] = [
        "red",
        "orange",
        "yellow",
        "lime",
        "green",
        "emerald",
        "cyan",
        "azure",
        "blue",
        "purple",
        "magenta",
        "raspberry",
    ];
    NAMES[(hue.fract() * 12.0).round() as usize % 12]
}

/// The color the "New color" button should reach for, given what the palette already holds, and
/// what to call it.
///
/// Repeated clicks should walk through colors that are easy to tell apart both from each other
/// and from the rest of the puzzle — a solver has to read them off a grid of small cells. So:
/// the two ends of the range first, then the three primaries, and after that whichever hue the
/// picture has the most room left for.
fn suggested_color(palette: &Palette) -> ((u8, u8, u8), &'static str) {
    let hsv = |(r, g, b): (u8, u8, u8)| {
        egui::ecolor::hsv_from_rgb([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0])
    };

    // Nonograms typically want white and black!
    if !palette
        .values()
        .any(|ci| matches!(hsv(ci.rgb), (_, s, v) if v >= 1.0 - NEUTRAL_TOLERANCE && s <= NEUTRAL_TOLERANCE))
    {
        return ((255, 255, 255), "white");
    }
    if !palette
        .values()
        .any(|ci| hsv(ci.rgb).2 <= NEUTRAL_TOLERANCE)
    {
        return ((0, 0, 0), "black");
    }

    // From here on this is only about hue, so anything washed out or dark enough to read as a
    // shade rather than a color has nothing to say about which hues are taken.
    let mut hues: Vec<f32> = palette
        .values()
        .map(|ci| hsv(ci.rgb))
        .filter(|(_, s, v)| *s >= VIVID && *v >= VIVID)
        .map(|(h, _, _)| h)
        .collect();

    for primary in [0.0, 1.0 / 3.0, 2.0 / 3.0] {
        if !hues
            .iter()
            .any(|h| hue_distance(*h, primary) <= PRIMARY_TOLERANCE)
        {
            return (full_saturation(primary), hue_name(primary));
        }
    }

    // All three primaries are covered, so fill in wherever the circle is emptiest.
    hues.sort_by(f32::total_cmp);
    let mut widest = 0;
    for i in 1..hues.len() {
        // Ties go to the earliest gap, which on a fresh red/green/blue palette means yellow.
        if gap_after(&hues, i) > gap_after(&hues, widest) {
            widest = i;
        }
    }
    let hue = (hues[widest] + gap_after(&hues, widest) / 2.0).fract();
    (full_saturation(hue), hue_name(hue))
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
    /// The palette entries the sidebar offers, in the order it shows them — which is the order
    /// the number keys and the wheel step through as well.
    ///
    /// TODO: actually paint a palette entry for unsolved, in case the user doesn't have a middle
    /// button.
    fn palette_order(&self) -> Vec<Color> {
        use itertools::Itertools;

        let Some(picture) = self.document.try_solution() else {
            return vec![];
        };
        picture
            .palette()
            .keys()
            .copied()
            .filter(|color| !(*color == UNSOLVED && self.solving))
            .sorted()
            .collect()
    }

    /// Move `steps` entries along the palette, wrapping around at either end.
    pub(super) fn cycle_color(&mut self, steps: i32) {
        let order = self.palette_order();
        if order.is_empty() {
            return;
        }
        let at = order
            .iter()
            .position(|color| *color == self.current_color)
            .unwrap_or(0) as i32;
        self.current_color = order[(at + steps).rem_euclid(order.len() as i32) as usize];
    }

    pub(super) fn palette_editor(&mut self, ui: &mut egui::Ui) {
        // The solver's palette is the puzzle's, so it's shown but not edited.
        let read_only = self.solving;

        let mut picked_color = self.current_color;
        let mut removed_color = None;
        let mut add_color = false;

        // Same story as in `common_sidebar_items`: these shortcuts have no modifier, so they
        // have to stand down by hand while a `TextEdit` has the keyboard.
        let typing = ui.ctx().wants_keyboard_input();

        // How wide the delete button turned out, so that the background's row — which has no
        // such button — can leave a hole the same size and keep the name fields lined up. Same
        // measure-last-frame trick as `centered_row`, and off by a frame in the same harmless way.
        let delete_width_id = ui.id().with("palette_delete_width");

        for (index, color) in self.palette_order().into_iter().enumerate() {
            let color_info = self
                .document
                .solution_mut()
                .palette_mut()
                .get_mut(&color)
                .expect("just read out of this palette");

            let shortcut = palette_shortcut(index);
            let (r, g, b) = color_info.rgb;
            let button_text = if color_info.corner.is_some() {
                color_info.ch.to_string()
            } else {
                "■".to_string()
            };

            ui.horizontal(|ui| {
                let (marker, _) = ui.allocate_exact_size(
                    Vec2::new(CHEVRON_COLUMN, CHEVRON_SIZE),
                    egui::Sense::hover(),
                );
                ui.painter().text(
                    marker.center(),
                    egui::Align2::CENTER_CENTER,
                    icons::ICON_CHEVRON_FORWARD,
                    egui::FontId::proportional(CHEVRON_SIZE),
                    Color32::from_black_alpha(if color == picked_color { 255 } else { 0 }),
                );

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
                    picked_color = color;
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
                        picked_color = color;
                        color_info.rgb = (
                            (edited_color[0] * 256.0) as u8,
                            (edited_color[1] * 256.0) as u8,
                            (edited_color[2] * 256.0) as u8,
                        );
                    }

                    if color == BACKGROUND {
                        // No deleting the background, so pad the gap where its button would be.
                        let width: f32 = ui
                            .data(|d| d.get_temp(delete_width_id))
                            .unwrap_or_else(|| ui.spacing().interact_size.x);
                        // Plus the gap a real widget would leave after itself: `add_space`
                        // advances the cursor and nothing more.
                        ui.add_space(width + ui.spacing().item_spacing.x);
                    } else {
                        let delete = ui.button(icons::ICON_DELETE);
                        ui.data_mut(|d| d.insert_temp(delete_width_id, delete.rect.width()));
                        if delete.clicked() {
                            removed_color = Some(color);
                        }
                    }

                    // A color's name shows up in its tooltip, in the clue gutters of a puzzle
                    // whose clues are lettered, and in every format that stores names — so it's
                    // worth being able to fix. (Not undoable either; see the TODO above.)
                    // Last in the row, so it can have whatever width is left.
                    ui.add(
                        egui::TextEdit::singleline(&mut color_info.name)
                            .desired_width(ui.available_width()),
                    )
                    .on_hover_text("Rename this color");
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
            let (rgb, name) = suggested_color(new_picture.palette());
            new_picture.palette_mut().insert(
                next_color,
                ColorInfo {
                    ch: (next_color.0 + 65) as char, // TODO: will break chargrid export
                    name: name.to_string(),
                    rgb,
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

    /// The chevron's column is narrower than the icon font's em box, on the grounds that most
    /// of that box is empty. If the glyph or its size ever changes, the column has to keep up.
    #[test]
    fn the_chevron_fits_its_column() {
        let ctx = egui::Context::default();
        egui_material_icons::initialize(&ctx);
        // The fonts don't exist until a pass has run.
        let _ = ctx.run(egui::RawInput::default(), |_| {});

        let ink = ctx
            .fonts(|f| {
                f.layout_no_wrap(
                    icons::ICON_CHEVRON_FORWARD.to_string(),
                    egui::FontId::proportional(CHEVRON_SIZE),
                    Color32::BLACK,
                )
            })
            .mesh_bounds
            .width();

        assert!(
            ink <= CHEVRON_COLUMN,
            "the chevron's ink is {ink} wide, but its column is only {CHEVRON_COLUMN}"
        );
    }

    /// Build a palette out of nothing but colors, for the suggester to chew on.
    fn palette_of(colors: &[(u8, u8, u8)]) -> Palette {
        colors
            .iter()
            .enumerate()
            .map(|(i, rgb)| {
                let color = Color(i as u8);
                (
                    color,
                    ColorInfo {
                        ch: (i as u8 + 65) as char,
                        name: "test".to_string(),
                        rgb: *rgb,
                        color,
                        corner: None,
                    },
                )
            })
            .collect()
    }

    const WHITE: (u8, u8, u8) = (255, 255, 255);
    const BLACK: (u8, u8, u8) = (0, 0, 0);
    const RED: (u8, u8, u8) = (255, 0, 0);
    const GREEN: (u8, u8, u8) = (0, 255, 0);
    const BLUE: (u8, u8, u8) = (0, 0, 255);

    /// The two ends of the range come before any hue, white before black.
    #[test]
    fn the_neutrals_come_first() {
        assert_eq!(suggested_color(&palette_of(&[])).0, WHITE);
        assert_eq!(suggested_color(&palette_of(&[BLACK])).0, WHITE);
        assert_eq!(suggested_color(&palette_of(&[WHITE])).0, BLACK);
        // "Within ~10%" of each, not exactly each.
        assert_eq!(suggested_color(&palette_of(&[(245, 250, 240)])).0, BLACK);
        assert_eq!(
            suggested_color(&palette_of(&[(245, 250, 240), (10, 6, 12)])).0,
            RED
        );
    }

    /// A black-and-white puzzle — the usual starting point — walks red, green, blue.
    #[test]
    fn the_primaries_come_next() {
        let mut palette = palette_of(&[WHITE, BLACK]);
        for expected in [RED, GREEN, BLUE] {
            let (rgb, _) = suggested_color(&palette);
            assert_eq!(rgb, expected);
            palette.insert(
                Color(palette.len() as u8),
                ColorInfo {
                    ch: 'x',
                    name: "test".to_string(),
                    rgb,
                    color: Color(palette.len() as u8),
                    corner: None,
                },
            );
        }
    }

    /// A hue near enough to a primary stands in for it, so the palette doesn't collect two reds.
    #[test]
    fn a_near_primary_counts_as_that_primary() {
        // Orange is a twelfth of the circle from red, inside the tolerance.
        let orange = full_saturation(1.0 / 12.0);
        assert_eq!(
            suggested_color(&palette_of(&[WHITE, BLACK, orange])).0,
            GREEN
        );
    }

    /// Only vivid colors get a say in which hues are taken: a pale or muddy red doesn't stop the
    /// palette from wanting a real one.
    #[test]
    fn washed_out_colors_dont_claim_a_hue() {
        // A pale red (low saturation) and a dark red (low value), neither of them vivid.
        let pale = (255, 200, 200);
        let dark = (60, 0, 0);
        assert_eq!(
            suggested_color(&palette_of(&[WHITE, BLACK, pale, dark])).0,
            RED
        );
    }

    /// Once all three primaries are there, the next color goes into the middle of whatever arc
    /// of the circle is emptiest.
    #[test]
    fn later_colors_fill_the_widest_hue_gap() {
        // Red, green and blue divide the circle evenly, so the first of the three equal gaps
        // wins: halfway from red to green is yellow.
        let mut palette = palette_of(&[WHITE, BLACK, RED, GREEN, BLUE]);
        let (yellow, name) = suggested_color(&palette);
        assert_eq!(yellow, (255, 255, 0));
        assert_eq!(name, "yellow");

        // With yellow in place, the widest gaps are green-to-blue and blue-to-red; the earlier
        // one wins, which is cyan.
        palette.insert(
            Color(9),
            ColorInfo {
                ch: 'y',
                name: "yellow".to_string(),
                rgb: yellow,
                color: Color(9),
                corner: None,
            },
        );
        assert_eq!(suggested_color(&palette).0, (0, 255, 255));
    }

    /// The wheel walks the palette in the order the sidebar lists it, and comes back around at
    /// either end.
    #[test]
    fn cycling_wraps_around_the_palette() {
        let mut fancy = Solution::blank_bw(3, 3);
        fancy
            .palette
            .insert(Color(2), ColorInfo::default_fg(Color(2)));

        // In color order: the background, then 1, then 2.
        let mut gui = NonogramGui::new(doc(fancy)).editor_gui;
        assert_eq!(gui.current_color, Color(1));

        gui.cycle_color(1);
        assert_eq!(gui.current_color, Color(2));
        gui.cycle_color(1);
        assert_eq!(gui.current_color, BACKGROUND);
        gui.cycle_color(-1);
        assert_eq!(gui.current_color, Color(2));

        // More than a lap around still lands where a single step would.
        gui.cycle_color(-4);
        assert_eq!(gui.current_color, Color(1));
    }

    /// "Unknown" is a state a solver's cell can be in, not a color to paint with — the palette
    /// doesn't offer it, so neither does the wheel.
    #[test]
    fn cycling_steps_past_unknown_while_solving() {
        let mut solving = Solution::blank_bw(3, 3);
        solving.palette.insert(
            UNSOLVED,
            ColorInfo {
                ch: '?',
                name: "unknown".to_string(),
                rgb: (128, 128, 128),
                color: UNSOLVED,
                corner: None,
            },
        );

        let mut gui = NonogramGui::new(doc(solving)).editor_gui;
        gui.solving = true;

        gui.cycle_color(1);
        assert_eq!(gui.current_color, BACKGROUND);
        gui.cycle_color(1);
        assert_eq!(gui.current_color, Color(1));
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
