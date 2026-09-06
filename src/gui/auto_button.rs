//! A button with an "auto" toggle tucked underneath it.
//!
//! Several of the solving aids can either be run once, on demand, or left switched on to re-run
//! after every move. Rather than a checkbox sitting next to the button and merely *implying* that
//! it governs it, this draws a second, fake button beneath the real one, poking out on the left.
//! The gear on that exposed sliver is the toggle: pressing it holds the under-button down for as
//! long as the aid is automatic, and the main button greys out, because there's nothing left to
//! ask for.

use egui::{
    Align2, Color32, Rect, Response, Sense, Shape, StrokeKind, TextStyle, WidgetInfo, WidgetType,
    epaint::RectShape, pos2,
};
use egui_material_icons::icons;

/// How far the under-button slides right — in under the main button, out of sight — while held
/// down. Small, but it's what sells "depressed" rather than merely "dark".
const DEPRESS: f32 = 2.0;

/// How far the under-button reaches past the main button's left edge. Only its rounded corners
/// let any of this show; the rest is what keeps the two from reading as separate widgets.
const TUCK: f32 = 6.0;

pub struct AutoButton {
    /// The main button. `clicked()` means "do it once, right now".
    pub button: Response,
    /// The gear on the under-button. `changed()` means the flag just flipped.
    pub auto: Response,
}

/// A `label` button with a gear-shaped "do this automatically" toggle for `auto` under it.
pub fn auto_button(ui: &mut egui::Ui, label: &str, auto: &mut bool) -> AutoButton {
    let font = TextStyle::Button.resolve(ui.style());
    let gear_width = ui.fonts(|fonts| {
        fonts
            .layout_no_wrap(
                icons::ICON_SETTINGS.to_owned(),
                font.clone(),
                Color32::PLACEHOLDER,
            )
            .size()
            .x
    });
    let sliver_width = gear_width + ui.spacing().button_padding.x * 2.0;

    // The under-button has to be painted *behind* the main button, but its position isn't known
    // until the main button has been laid out. So claim a slot in the paint list now — everything
    // drawn from here on lands on top of whatever ends up in it — and fill it in at the end.
    let painter = ui.painter().clone();
    let under_slot = painter.add(Shape::Noop);

    let shift = if *auto { DEPRESS } else { 0.0 };

    let (button, gear) = ui
        .horizontal(|ui| {
            // Sibling rows share a `ui.id()`, so that can't be salted into an id of our own —
            // two of these in one sidebar would collide. The auto-id counter is what actually
            // distinguishes them, and it has to be read before anything else in the row is laid
            // out, so that toggling (which changes what the row allocates) can't move it.
            let gear_id = ui.auto_id_with(("auto", label));

            // The sliver and the main button are meant to read as one object, so no gap.
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add_space(sliver_width + shift);
            let button = ui.add_enabled(!*auto, egui::Button::new(label));

            let sliver = Rect::from_min_max(
                pos2(button.rect.left() - sliver_width, button.rect.top()),
                pos2(button.rect.left(), button.rect.bottom()),
            );
            // The whole sliver is the target, not just the glyph on it.
            let mut gear = ui.interact(sliver, gear_id, Sense::click());
            if gear.clicked() {
                *auto = !*auto;
                gear.mark_changed();
            }
            let is_auto = *auto;
            // Not just `label`: that's the main button's name, and a screen reader (or a
            // `get_by_label` in tests/gui.rs) has to be able to tell the two apart.
            let auto_label = format!("{label} automatically");
            gear.widget_info(|| {
                WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), is_auto, &auto_label)
            });
            if gear.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }

            // `active` is what egui paints a button being pressed with, which is exactly the
            // look wanted for one that stays pressed.
            let mut visuals = if is_auto {
                ui.visuals().widgets.active
            } else {
                *ui.style().interact(&gear)
            };
            // Real buttons swell a pixel when hovered or held. This one is the thing the main
            // button sits on, so it holds still; growing would poke out above and below it.
            visuals.expansion = 0.0;

            let under = Rect::from_min_max(
                pos2(sliver.left(), button.rect.top()),
                pos2(button.rect.left() + TUCK, button.rect.bottom()),
            );
            // Exactly what `egui::Button` paints for itself, so the fake one is indistinguishable
            // from the real one.
            painter.set(
                under_slot,
                RectShape::new(
                    under,
                    visuals.corner_radius,
                    visuals.weak_bg_fill,
                    visuals.bg_stroke,
                    StrokeKind::Inside,
                ),
            );
            painter.text(
                sliver.center(),
                Align2::CENTER_CENTER,
                icons::ICON_SETTINGS,
                font,
                visuals.fg_stroke.color,
            );

            // A bare gear needs words.
            let gear = gear.on_hover_text(&auto_label);
            (button, gear)
        })
        .inner;

    AutoButton { button, auto: gear }
}
