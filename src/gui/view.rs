//! Where the picture sits in the window: zooming, panning, fitting it to the screen, and the
//! canvas panel that all of that happens in.
//!
//! `View` keeps the state and does the arithmetic; `NonogramGui::canvas_panel` feeds it what egui
//! measured, and draws the canvas where it says.

use super::*;

/// Breathing room between the canvas and the panels around it.
const CANVAS_MARGIN: i8 = 16;

/// One click of the zoom buttons, in pixels per cell edge.
pub(crate) const ZOOM_STEP: f32 = 2.0;
/// How far the zoom controls can go, in pixels per cell edge.
pub(crate) const MIN_SCALE: f32 = 1.0;
pub(crate) const MAX_SCALE: f32 = 50.0;

/// Where the zoom starts out, in pixels per cell edge.
const STARTING_SCALE: f32 = 16.0;

/// How far ahead of the zoom the user set a triddler is drawn. A triangular cell has half a
/// square one's area, and its three clue gutters are rhombuses full of slanted numerals, so the
/// same scale buys a good deal less legibility than it does on a square grid.
const TRIDDLER_ZOOM_STEPS: f32 = 3.0;

/// How the canvas is placed in the window. Only `scale` is saved with the session; the rest is
/// measured afresh from each frame.
#[derive(Serialize, Deserialize)]
pub(super) struct View {
    /// What the zoom controls set, in pixels per cell edge (see `render_scale`).
    scale: f32,
    /// Whether the picture is too big for the space it's shown in, and so has somewhere to pan
    /// to. Measured from the scroll area as it was drawn last frame.
    #[serde(skip)]
    pannable: bool,
    /// Whether a middle-drag pan is in flight right now.
    #[serde(skip)]
    panning: bool,
    /// Where to scroll the canvas to on the next frame: what zooming (or fitting) last frame
    /// worked out, now that the canvas is drawn at the new scale.
    #[serde(skip)]
    pending_scroll: Option<Vec2>,
    /// Whether a fit (see `CanvasGui::needs_fit`) is under way: the first frame of one zooms
    /// straight to the scale it measures, and any after it only zoom out, until it fits.
    #[serde(skip)]
    fitting: bool,
    /// The render scale and the size of what was drawn at it, as of last frame, while fitting.
    #[serde(skip)]
    fit_measured: Option<(f32, Vec2)>,
    /// The size of the canvas's viewport as of last frame, and how big the drawing in it will
    /// come out this frame (which, after a zoom, is a guess), for centering it.
    #[serde(skip)]
    last_viewport: Vec2,
    #[serde(skip)]
    expected_drawing: Option<Vec2>,
    /// Mousewheel motion that hasn't yet added up to a whole step through the palette.
    #[serde(skip)]
    wheel_remainder: f32,
}

impl Default for View {
    fn default() -> Self {
        View {
            scale: STARTING_SCALE,
            pannable: false,
            panning: false,
            pending_scroll: None,
            fitting: false,
            fit_measured: None,
            last_viewport: Vec2::ZERO,
            expected_drawing: None,
            wheel_remainder: 0.0,
        }
    }
}

/// How many pixels per cell edge further in than the zoom a picture of `shape` is drawn.
fn head_start(shape: &crate::geometry::Shape) -> f32 {
    match shape {
        crate::geometry::Shape::Triangular(_) => TRIDDLER_ZOOM_STEPS * ZOOM_STEP,
        crate::geometry::Shape::Square { .. } => 0.0,
    }
}

impl View {
    /// What to actually draw a picture of `shape` at: `scale` is what the zoom controls set, but
    /// triddlers are drawn `TRIDDLER_ZOOM_STEPS` clicks further in than square puzzles.
    pub(super) fn render_scale(&self, shape: &crate::geometry::Shape) -> f32 {
        self.scale + head_start(shape)
    }

    /// One click of the zoom-in button.
    pub(super) fn zoom_in(&mut self) {
        self.scale = (self.scale + ZOOM_STEP).min(MAX_SCALE);
    }

    /// One click of the zoom-out button.
    pub(super) fn zoom_out(&mut self) {
        self.scale = (self.scale - ZOOM_STEP).max(MIN_SCALE);
    }

    /// Add this frame's wheel motion, in notches; returns how many whole steps through the
    /// palette that comes to, keeping the rest for later.
    fn palette_steps(&mut self, notches: f32) -> i32 {
        self.wheel_remainder += notches;
        let steps = self.wheel_remainder.trunc();
        self.wheel_remainder -= steps;
        steps as i32
    }

    /// Middle-drag pans, but only when there's somewhere to pan to; otherwise the middle button
    /// keeps its painting job. A pan that has started holds on until the button comes up, even if
    /// the pointer wanders off the canvas. Returns whether a pan is in flight.
    fn update_panning(
        &mut self,
        middle_pressed: bool,
        middle_down: bool,
        pointer_here: bool,
    ) -> bool {
        self.panning = middle_down && (self.panning || (middle_pressed && pointer_here));
        self.panning
    }

    /// The margin to put before the drawing this frame, to center it (see `centering_pad`).
    fn pad(&self, layout: ScreenLayout) -> Vec2 {
        match self.expected_drawing {
            Some(drawing) => centering_pad(layout, self.last_viewport, drawing),
            None => Vec2::ZERO,
        }
    }

    /// What the scroll area made of this frame: a `viewport` that big, showing `content` that
    /// big, of which the drawing itself (not counting the centering pad) is `drawing`.
    fn measured(&mut self, viewport: Vec2, content: Vec2, drawing: Vec2) {
        self.last_viewport = viewport;
        self.expected_drawing = Some(drawing);
        // This decides whether the *next* frame's middle-drag pans. A frame of lag is invisible
        // beside how long a zoom lasts.
        self.pannable = content.x > viewport.x + 0.5 || content.y > viewport.y + 0.5;
    }

    /// Zoom by `factor` (egui's `zoom_delta`), keeping whatever's at `anchor` (in points from the
    /// top-left of the viewport) where it is. `offset` is how far the scroll area is scrolled, and
    /// `pad` the centering pad it was drawn with.
    fn zoom(
        &mut self,
        factor: f32,
        shape: &crate::geometry::Shape,
        layout: ScreenLayout,
        anchor: Option<Vec2>,
        offset: Vec2,
        pad: Vec2,
    ) {
        let before = self.render_scale(shape);
        self.scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        let factor = self.render_scale(shape) / before;
        let Some(drawing) = self.expected_drawing else {
            return;
        };
        // Everything on the canvas is drawn in proportion to the zoom, so this is a good guess
        // at how big it'll come out, and where it'll be centered.
        let drawing = drawing * factor;
        self.expected_drawing = Some(drawing);
        if let Some(anchor) = anchor {
            let new_pad = centering_pad(layout, self.last_viewport, drawing);
            self.pending_scroll = Some(zoom_scroll_offset(offset, anchor, (pad, new_pad), factor));
        }
    }

    /// The desktop has zoom buttons, and keeps whatever zoom they set.
    fn stop_fitting(&mut self, needs_fit: &mut bool) {
        *needs_fit = false;
        self.fitting = false;
        self.fit_measured = None;
    }

    /// One frame's worth of zooming a picture of `shape` to fit `viewport`, if it wants to (see
    /// `CanvasGui::needs_fit`), given that what it drew last came out `content` big. Returns
    /// whether there's more to do, in which case another frame is wanted.
    ///
    /// Not everything in the drawing grows in proportion to the zoom (the borders don't, for
    /// one), so this measures, zooms, and measures again. The first measurement goes straight to
    /// the scale it works out; the ones after it only zoom out (by at least a pixel per cell),
    /// so it can't go back and forth.
    fn fit_step(
        &mut self,
        needs_fit: &mut bool,
        shape: &crate::geometry::Shape,
        viewport: Vec2,
        content: Vec2,
    ) -> bool {
        if !*needs_fit {
            self.stop_fitting(needs_fit);
            return false;
        }
        if content.x <= 0.0 || content.y <= 0.0 {
            return true; // Nothing drawn yet.
        }

        // The solver's clue gutters are laid out in an `egui::Grid`, which sizes its columns
        // from the frame before, so the first frame at a new zoom can come out any size at all.
        // A measurement only counts once it's come out the same twice running.
        let render = self.render_scale(shape);
        if self.fit_measured != Some((render, content)) {
            self.fit_measured = Some((render, content));
            return true;
        }

        let ratio = (viewport.x / content.x).min(viewport.y / content.y);
        // Whole points per cell, so that the grid lines land on pixels.
        let mut target = (render * ratio).floor();
        if self.fitting {
            target = target.min(render - 1.0);
            if ratio >= 1.0 {
                target = render; // It fits.
            }
        }
        // `scale` is what's clamped; the triddler's head start rides along.
        let scale = (target - head_start(shape)).clamp(MIN_SCALE, MAX_SCALE);

        if scale == self.scale {
            self.stop_fitting(needs_fit);
            false
        } else {
            // See `zoom`.
            self.expected_drawing = Some(content * (scale / self.scale));
            self.scale = scale;
            self.fitting = true;
            self.pending_scroll = Some(Vec2::ZERO);
            true
        }
    }
}

/// Where to scroll to after zooming by `factor`, so that whatever was `anchor` (in points from
/// the top-left of the scroll area's viewport) stays put, from `offset`. Everything on the canvas
/// is drawn from its top-left corner outwards, in proportion to the zoom, after a centering
/// margin (see `centering_pad`) that was `pads.0` and will be `pads.1`.
///
/// Along a direction in which the drawing will fit in the viewport, there's no scrolling, and
/// the anchor moves wherever centering puts it.
fn zoom_scroll_offset(offset: Vec2, anchor: Vec2, pads: (Vec2, Vec2), factor: f32) -> Vec2 {
    let (pad, new_pad) = pads;
    (offset + anchor - pad) * factor + new_pad - anchor
}

/// The margin that centers a drawing `drawing` big in a `viewport`, where it's smaller. In whole
/// points, so that the grid lines stay on pixels.
fn centering_pad(layout: ScreenLayout, viewport: Vec2, drawing: Vec2) -> Vec2 {
    match layout {
        ScreenLayout::Desktop => Vec2::ZERO,
        ScreenLayout::Compact => ((viewport - drawing) / 2.0).max(Vec2::ZERO).floor(),
    }
}

/// This frame's wheel motion over the canvas, in notches, with the events taken away from the
/// scroll area beneath — over the canvas the wheel steps through the palette instead of
/// scrolling. Positive is a notch rolled away from the user.
///
/// Read from the events rather than from the scroll delta so that one notch is one step
/// whatever the platform's scroll speed happens to be set to.
fn take_wheel_notches(ctx: &egui::Context) -> f32 {
    // Trackpads report points instead of lines; converting at egui's own rate makes a swipe
    // that would have scrolled one line's worth count as one notch.
    let points_per_line = ctx.options(|o| o.input_options.line_scroll_speed).max(1.0);

    ctx.input_mut(|i| {
        let mut notches = 0.0;
        i.events.retain(|event| match event {
            egui::Event::MouseWheel {
                unit,
                delta,
                modifiers,
                ..
            } => {
                // ctrl/cmd-wheel is egui's zoom gesture; that one belongs to `zoom_delta`.
                if modifiers.ctrl || modifiers.mac_cmd || modifiers.command {
                    return true;
                }
                // Whichever axis moved: shift-wheel arrives as horizontal motion on some
                // platforms, and there's only the one palette to step through either way.
                let amount = if delta.y != 0.0 { delta.y } else { delta.x };
                notches += match unit {
                    egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => amount,
                    egui::MouseWheelUnit::Point => amount / points_per_line,
                };
                false
            }
            _ => true,
        });
        // egui spreads a mouse notch over the next few frames, so the leftovers have to go as
        // well — and they arrive on frames that carry no event of their own.
        i.smooth_scroll_delta = Vec2::ZERO;
        notches
    })
}

impl NonogramGui {
    /// The canvas, in whatever space the panels around it have left.
    pub(super) fn canvas_panel(&mut self, ui: &mut egui::Ui, layout: ScreenLayout) {
        let ctx = &ui.ctx().clone();
        let margin = match layout {
            ScreenLayout::Desktop => CANVAS_MARGIN,
            ScreenLayout::Compact => compact::CANVAS_MARGIN,
        };
        let shape = self.canvas.document.solution().shape();
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.global_style()).inner_margin(margin))
            .show(ui, |ui| {
                // The compact layout's touch-sized spacing and bigger text would pad out the clue
                // gutters, which are laid out as widgets.
                *ui.spacing_mut() = desktop_spacing();
                // Nothing on the canvas scrolls by flinging it — the wheel, the middle button and
                // two fingers all have jobs of their own — so there's nothing to coast or
                // spring back, and a zoom that leaves the view past the edge should just stop
                // at it.
                ui.spacing_mut().scroll.kinetic.rubber_band = false;
                ui.spacing_mut().scroll.kinetic.decay_time = 0.0;
                ui.style_mut().text_styles = Style::default().text_styles;

                // Everything below only applies while the pointer is over the canvas: the wheel
                // and the middle button still mean what they usually do over the sidebar.
                let pointer_here = ui.rect_contains_pointer(ui.max_rect());

                // A plain wheel steps through the palette rather than scrolling — panning is the
                // middle button's job below — so the events have to be taken away from the
                // scroll area before it draws.
                if pointer_here {
                    let steps = self.view.palette_steps(take_wheel_notches(ctx));
                    if steps != 0 {
                        // A notch away from the user goes *up* the palette, the way it would move
                        // up any other list.
                        self.canvas.cycle_color(-steps);
                    }
                }

                let (middle_pressed, middle_down, pointer_delta) = ctx.input(|i| {
                    (
                        i.pointer.button_pressed(egui::PointerButton::Middle),
                        i.pointer.middle_down(),
                        i.pointer.delta(),
                    )
                });
                let panning = self
                    .view
                    .update_panning(middle_pressed, middle_down, pointer_here);
                let pannable = self.view.pannable;

                // In touch mode, a finger anywhere in this panel steers the canvas's cursor (see
                // `touch.rs`), so the panel's whole area is the canvas's to take touches from.
                let touch_area = (self.input_mode == InputMode::Touch).then(|| ui.max_rect());

                let scale = self.view.render_scale(&shape);

                // Where the drawing is smaller than the viewport, it's centered (on a phone; the
                // desktop keeps it beside the sidebar). That's done with a margin before it,
                // inside the scroll area.
                let pad = self.view.pad(layout);

                // A zoomed-in puzzle is routinely bigger than the window in both directions.
                // `animated(false)` because the only thing that scrolls this programmatically is
                // the pan below, which has to keep up with the pointer exactly. A finger's drag
                // belongs to the cursor, not to scrolling.
                let mut scroll_area = egui::ScrollArea::both().animated(false).scroll_source(
                    egui::scroll_area::ScrollSource {
                        drag: if touch_area.is_none() {
                            egui::scroll_area::DragScroll::Always
                        } else {
                            egui::scroll_area::DragScroll::Never
                        },
                        ..Default::default()
                    },
                );
                if let Some(offset) = self.view.pending_scroll.take() {
                    scroll_area = scroll_area.scroll_offset(offset);
                }
                let scrolled = scroll_area.show(ui, |ui| {
                    if panning {
                        ui.scroll_with_delta(pointer_delta);
                    }
                    // One finger steers the cursor, so two pan (as well as zooming, below).
                    if touch_area.is_some()
                        && let Some(touch) = ui.input(|i| i.multi_touch())
                    {
                        ui.scroll_with_delta(touch.translation_delta);
                    }
                    let drawing_area =
                        Rect::from_min_size(ui.cursor().min + pad, ui.available_size());
                    ui.scope_builder(egui::UiBuilder::new().max_rect(drawing_area), |ui| {
                        self.canvas.middle_pans = pannable;
                        self.canvas.touch_area = touch_area;
                        match &mut self.solve_gui {
                            Some(solve_gui) => solve_gui.body(ui, scale, &mut self.canvas),
                            None => {
                                self.canvas.canvas(ui, scale, RenderStyle::Experimental);
                            }
                        }
                    });
                });
                let viewport = scrolled.inner_rect.size();
                let drawing = scrolled.content_size - pad;
                self.view.measured(viewport, scrolled.content_size, drawing);

                // After the canvas, which sets a cursor of its own for whichever tool is up.
                if panning {
                    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                }

                // egui routes ctrl-scroll (and trackpad pinch) into `zoom_delta` rather than
                // into the scroll offset, and `take_wheel_notches` leaves those events alone.
                if pointer_here {
                    let (zoom, anchor) = ui.input(|i| {
                        // Between the fingers, or under the pointer.
                        let anchor = i
                            .multi_touch()
                            .map(|touch| touch.center_pos)
                            .or(i.pointer.hover_pos());
                        (i.zoom_delta(), anchor)
                    });
                    if zoom != 1.0 {
                        self.view.zoom(
                            zoom,
                            &shape,
                            layout,
                            anchor.map(|anchor| anchor - scrolled.inner_rect.min),
                            scrolled.state.clamped_offset(),
                            pad,
                        );
                    }
                }

                match layout {
                    ScreenLayout::Compact => {
                        let needs_fit = &mut self.canvas.needs_fit;
                        if self.view.fit_step(needs_fit, &shape, viewport, drawing) {
                            ctx.request_repaint();
                        }
                    }
                    ScreenLayout::Desktop => self.view.stop_fitting(&mut self.canvas.needs_fit),
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE: crate::geometry::Shape = crate::geometry::Shape::Square {
        width: 20,
        height: 10,
    };

    /// What a 20x10 picture comes out at: its cells, plus a border that doesn't grow with them.
    fn drawn(view: &View) -> Vec2 {
        let render = view.render_scale(&SQUARE);
        Vec2::new(20.0 * render, 10.0 * render) + Vec2::splat(30.0)
    }

    /// Fitting ends with the picture inside the viewport, as big as whole pixels per cell allow,
    /// give or take one (since its border doesn't scale, it can't be worked out exactly).
    #[test]
    fn fitting_fills_the_viewport() {
        for viewport in [
            Vec2::new(390.0, 600.0),
            Vec2::new(100.0, 100.0),
            Vec2::splat(2000.0),
        ] {
            let mut view = View::default();
            let mut needs_fit = true;
            for _ in 0..50 {
                if !view.fit_step(&mut needs_fit, &SQUARE, viewport, drawn(&view)) {
                    break;
                }
            }
            assert!(!needs_fit, "never finished fitting {viewport:?}");

            let fits = |content: Vec2| content.x <= viewport.x && content.y <= viewport.y;
            let best = (1..=MAX_SCALE as i32)
                .rev()
                .find(|scale| {
                    let s = *scale as f32;
                    fits(Vec2::new(20.0 * s, 10.0 * s) + Vec2::splat(30.0))
                })
                .unwrap_or(1) as f32;
            assert!(
                fits(drawn(&view)) || view.scale == MIN_SCALE,
                "{viewport:?}"
            );
            assert!(
                view.scale >= best - 1.0,
                "{viewport:?}: {} vs {best}",
                view.scale
            );
        }
    }

    /// Zooming keeps the point under the pointer under the pointer.
    #[test]
    fn zooming_keeps_the_anchor_still() {
        let mut view = View::default();
        view.measured(Vec2::new(300.0, 300.0), drawn(&view), drawn(&view));
        let (offset, anchor) = (Vec2::new(120.0, 40.0), Vec2::new(50.0, 70.0));
        let before = view.scale;

        view.zoom(
            1.5,
            &SQUARE,
            ScreenLayout::Desktop,
            Some(anchor),
            offset,
            Vec2::ZERO,
        );
        let factor = view.scale / before;
        let new_offset = view.pending_scroll.unwrap();
        // The point of the drawing that was under the anchor, before and after.
        assert_eq!((offset + anchor) * factor, new_offset + anchor);
    }

    /// A pan starts with a middle press over the canvas, and lasts until the button comes up,
    /// wherever the pointer goes.
    #[test]
    fn a_pan_holds_on_until_the_button_comes_up() {
        let mut view = View::default();
        assert!(
            !view.update_panning(false, true, true),
            "already down when it arrived"
        );
        assert!(view.update_panning(true, true, true));
        assert!(view.update_panning(false, true, false));
        assert!(!view.update_panning(false, false, false));
    }

    /// The wheel's notches, however finely they arrive, step through the palette whole.
    #[test]
    fn wheel_motion_adds_up_to_steps() {
        let mut view = View::default();
        assert_eq!(view.palette_steps(0.4), 0);
        assert_eq!(view.palette_steps(0.4), 0);
        assert_eq!(view.palette_steps(0.4), 1);
        assert_eq!(view.palette_steps(-2.0), -1);
    }
}
