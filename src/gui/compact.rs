//! The compact layout, for phones: a bar of buttons across the top, the tools and the palette
//! across the bottom, the canvas between them, and everything else in a drawer from the left.
//!
//! It's a different arrangement of the same pieces the desktop's sidebars are built from (see
//! `NonogramGui::edit_sidebar` and `SolveGui::sidebar`), so a fix to one of those reaches both.
//! The desktop layout doesn't come through here at all.

use super::solver::Aid;
use super::*;

/// Which arrangement of the window to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenLayout {
    Desktop,
    Compact,
}

/// Narrower than this, in points, gets the compact layout. egui on the web uses one point per
/// CSS pixel. (`pages.html` gives the app the whole window from 100px wider than this, so the
/// page's own framing never pushes the app into this layout.)
const COMPACT_WIDTH: f32 = 600.0;

impl ScreenLayout {
    pub fn of(ctx: &egui::Context) -> ScreenLayout {
        if ctx.content_rect().width() < COMPACT_WIDTH {
            ScreenLayout::Compact
        } else {
            ScreenLayout::Desktop
        }
    }
}

/// The size of anything a finger has to hit, in points. Always this big in the compact layout,
/// whatever the input mode: sizing buttons by the input would make them change size on a
/// touchscreen laptop.
pub(super) const TARGET: f32 = 48.0;

/// How big an icon on a button is.
pub(super) const ICON_SIZE: f32 = 24.0;

/// Between the canvas and the bars. Smaller than the desktop's: a phone has no room to spare.
pub(super) const CANVAS_MARGIN: i8 = 8;

/// Around the contents of a full-screen dialog.
pub(super) const DIALOG_MARGIN: f32 = 12.0;

/// How wide the drawer is, at most. It always leaves a strip of the screen showing beside it,
/// for tapping to close it.
const DRAWER_WIDTH: f32 = 312.0;

/// How long a message that isn't an error stays up.
const TOAST_DURATION: Duration = Duration::from_secs(4);

/// The thin progress bar under the top bar.
const PROGRESS_HEIGHT: f32 = 3.0;

/// The unsolved-cell count's pill-shaped button in the editor's top bar.
const PILL_HEIGHT: f32 = 40.0;

/// Between the sections of the drawer.
const SECTION_GAP: f32 = 8.0;

/// How wide the Resize window is.
const RESIZE_WIDTH: f32 = 300.0;

/// The compact layout's spacing: everything that can be tapped is at least `TARGET` tall.
pub(super) fn spacing() -> egui::Spacing {
    egui::Spacing {
        interact_size: Vec2::splat(TARGET),
        // Exactly enough for an icon button to be `TARGET` wide, which brings a short word
        // ("Ok") close.
        button_padding: Vec2::new(
            (TARGET - ICON_SIZE) / 2.0,
            egui::Spacing::default().button_padding.y,
        ),
        ..egui::Spacing::default()
    }
}

/// The compact layout's look: buttons drawn a pixel inside their (finger-sized) targets, so that
/// rows of them have room to breathe, and a little rounder.
pub(super) fn visuals(mut visuals: Visuals) -> Visuals {
    let widgets = &mut visuals.widgets;
    for state in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        state.expansion -= 1.0;
        state.corner_radius = egui::CornerRadius::same(CORNER_RADIUS);
    }
    visuals
}

/// Buttons that are on or off, like the tools.
pub(super) trait Toggled {
    fn toggled(self, ui: &egui::Ui, on: bool) -> Self;
}

impl Toggled for egui::Button<'_> {
    /// `Button::selected` would do, except that egui draws a selected button square, and at its
    /// full size, unlike every other button. This gives it the selection's colors and nothing
    /// else.
    fn toggled(self, ui: &egui::Ui, on: bool) -> Self {
        if on {
            let selection = ui.visuals().selection;
            self.fill(selection.bg_fill).stroke(selection.stroke)
        } else {
            self
        }
    }
}

/// How round the compact layout's buttons are.
const CORNER_RADIUS: u8 = 4;

/// How much bigger the compact layout's text is than egui's (and the desktop's). Phones' own
/// interfaces use bigger text than desktops' at the same size in CSS pixels, to go with their
/// bigger targets.
const TEXT_SCALE: f32 = 1.2;

pub(super) fn text_styles() -> std::collections::BTreeMap<TextStyle, egui::FontId> {
    let mut styles = Style::default().text_styles;
    for font in styles.values_mut() {
        font.size *= TEXT_SCALE;
    }
    styles
}

/// A button with nothing on it but an icon: the desktop's ordinary size, or a finger-sized
/// square.
pub(super) fn icon_button(icon: &str, layout: ScreenLayout) -> egui::Button<'static> {
    match layout {
        ScreenLayout::Desktop => egui::Button::new(icon.to_owned()),
        ScreenLayout::Compact => egui::Button::new(RichText::new(icon.to_owned()).size(ICON_SIZE))
            .min_size(Vec2::splat(TARGET)),
    }
}

/// A dialog: on the desktop, `window`, which has whatever size, position and close button it
/// wants; in the compact layout, the whole screen, under a bar with a back button and `title`.
/// Returns what `add_contents` returned, if it ran, and whether the back button was pressed.
pub(super) fn dialog<R>(
    ctx: &egui::Context,
    layout: ScreenLayout,
    window: egui::Window<'_>,
    title: &str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> (Option<R>, bool) {
    match layout {
        ScreenLayout::Desktop => (
            window.show(ctx, add_contents).and_then(|shown| shown.inner),
            false,
        ),
        ScreenLayout::Compact => {
            let screen = ctx.content_rect();
            let id = egui::Id::unique(("full_screen_dialog", title));
            let mut back = false;
            // A modal, so that nothing underneath takes the touches meant for it.
            let shown = egui::Modal::new(id)
                .area(
                    egui::Modal::default_area(id)
                        .anchor(egui::Align2::LEFT_TOP, Vec2::ZERO)
                        .constrain(false),
                )
                .frame(egui::Frame::central_panel(&ctx.global_style()).inner_margin(0.0))
                .backdrop_color(Color32::TRANSPARENT)
                .show(ctx, |ui| {
                    ui.set_min_size(screen.size());
                    ui.set_max_size(screen.size());
                    ui.spacing_mut().item_spacing.y = 0.0;
                    egui::Frame::side_top_panel(ui.style()).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            back = ui
                                .add(icon_button(icons::ICON_ARROW_BACK, layout))
                                .clicked();
                            ui.heading(title);
                        });
                    });
                    ui.separator();
                    ui.spacing_mut().item_spacing.y = egui::Spacing::default().item_spacing.y;
                    egui::ScrollArea::vertical()
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            egui::Frame::new()
                                .inner_margin(DIALOG_MARGIN)
                                .show(ui, add_contents)
                                .inner
                        })
                        .inner
                });
            (Some(shown.inner), back)
        }
    }
}

/// A switch, as on a phone's settings screen. `name` is for screen readers.
fn toggle_switch(ui: &mut egui::Ui, on: &mut bool, name: &str) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(Vec2::splat(TARGET), egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let is_on = *on;
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), is_on, name)
    });

    if ui.is_rect_visible(rect) {
        let how_on = ui.ctx().animate_bool_responsive(response.id, is_on);
        let visuals = ui.style().interact_selectable(&response, is_on);
        let track = Rect::from_center_size(rect.center(), Vec2::new(40.0, 24.0));
        let radius = track.height() / 2.0;
        let fill = if is_on {
            ui.visuals().selection.bg_fill
        } else {
            visuals.bg_fill
        };
        ui.painter().rect(
            track,
            radius,
            fill,
            visuals.bg_stroke,
            egui::StrokeKind::Inside,
        );
        let x = egui::lerp((track.left() + radius)..=(track.right() - radius), how_on);
        ui.painter().circle(
            Pos2::new(x, track.center().y),
            radius - 3.0,
            Color32::WHITE,
            visuals.fg_stroke,
        );
    }
    response
}

/// A full-width button in the drawer.
fn menu_item(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add(egui::Button::new(label).min_size(Vec2::new(ui.available_width(), TARGET)))
        .clicked()
}

/// The heading over a section of the drawer: body text, but bold.
///
/// egui's own fonts have no bold weight (`RichText::strong` only darkens the color), so the text
/// is drawn twice, `BOLDNESS` apart.
pub(super) struct SectionHeading<'a>(pub(super) &'a str);

/// How far apart the two copies of a `SectionHeading` are drawn.
const BOLDNESS: f32 = 0.6;

impl egui::Widget for SectionHeading<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let galley = egui::WidgetText::from(self.0).into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        );
        let (rect, response) = ui.allocate_exact_size(
            galley.size() + Vec2::new(BOLDNESS, 0.0),
            egui::Sense::hover(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), galley.text())
        });
        if ui.is_rect_visible(rect) {
            let color = ui.visuals().strong_text_color();
            for dx in [0.0, BOLDNESS] {
                ui.painter()
                    .galley(rect.min + Vec2::new(dx, 0.0), galley.clone(), color);
            }
        }
        response
    }
}

/// A row with `label` on the left, and `add_controls` laid out from the right.
fn row_with_controls(ui: &mut egui::Ui, label: &str, add_controls: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            add_controls,
        );
    });
}

/// What the compact layout has open, that the desktop's doesn't have. Not saved with the
/// session.
#[derive(Default)]
pub(super) struct CompactState {
    drawer_open: bool,
    metadata_open: bool,
    resize_open: bool,
    /// The color whose dialog is open, after a long press on its swatch.
    color_dialog: Option<Color>,
}

/// Whether the browser can hand things to the system's share sheet (`navigator.share`).
pub(super) fn can_share() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        share_function().is_some()
    }
    #[cfg(not(target_arch = "wasm32"))]
    false
}

#[cfg(target_arch = "wasm32")]
fn share_function() -> Option<(web_sys::Navigator, js_sys::Function)> {
    use wasm_bindgen::JsCast as _;
    let navigator = web_sys::window()?.navigator();
    let share = js_sys::Reflect::get(&navigator, &"share".into())
        .ok()?
        .dyn_into::<js_sys::Function>()
        .ok()?;
    Some((navigator, share))
}

/// Open the system's share sheet, with `shared` as a link if `is_link`, and otherwise as text.
/// Does nothing where there isn't one (see `can_share`).
pub(super) fn share(title: &str, shared: &str, is_link: bool) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast as _;
        let Some((navigator, share)) = share_function() else {
            return;
        };
        let data = js_sys::Object::new();
        let set = |key: &str, value: &str| {
            let _ = js_sys::Reflect::set(&data, &key.into(), &value.into());
        };
        if !title.is_empty() {
            set("title", title);
        }
        set(if is_link { "url" } else { "text" }, shared);
        // Called through `Function::call1`, which catches what it throws, like `touch::buzz`.
        // What it returns is a promise, which is rejected when the user changes their mind;
        // that's nothing to complain about.
        if let Ok(promise) = share.call1(&navigator, &data)
            && let Ok(promise) = promise.dyn_into::<js_sys::Promise>()
        {
            wasm_bindgen_futures::spawn_local(async move {
                let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
            });
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (title, shared, is_link);
}

/// Something open on top of the canvas, which the back button closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Overlay {
    Solved,
    ColorDialog,
    Metadata,
    SaveShare,
    Library,
    New,
    Drawer,
    Resize,
}

/// Something chosen from the drawer's "File" section.
#[derive(Clone, Copy)]
enum FileItem {
    New,
    Library,
    Open,
    SaveShare,
}

impl NonogramGui {
    /// What's open on top of the canvas, topmost first: the order they're drawn in, backwards.
    pub(super) fn overlays(&self, layout: ScreenLayout) -> Vec<Overlay> {
        let compact = &self.compact;
        // Only the compact layout announces a solve with a window.
        let solved = layout == ScreenLayout::Compact
            && self.solve_gui.as_ref().is_some_and(|solve_gui| {
                !solve_gui.solved_announced
                    && solve_gui.replay.is_some()
                    && solve_gui.is_correctly_solved()
            });
        [
            (Overlay::Solved, solved),
            (Overlay::ColorDialog, compact.color_dialog.is_some()),
            (Overlay::Metadata, compact.metadata_open),
            (Overlay::SaveShare, self.show_save_share_window),
            (Overlay::Library, self.library_dialog.is_some()),
            (Overlay::New, self.new_dialog.is_some()),
            (Overlay::Drawer, compact.drawer_open),
            (Overlay::Resize, compact.resize_open),
        ]
        .into_iter()
        .filter_map(|(overlay, open)| open.then_some(overlay))
        .collect()
    }

    pub(super) fn close_overlay(&mut self, overlay: Overlay) {
        match overlay {
            Overlay::Solved => {
                if let Some(solve_gui) = &mut self.solve_gui {
                    solve_gui.solved_announced = true;
                }
            }
            Overlay::ColorDialog => self.compact.color_dialog = None,
            Overlay::Metadata => self.compact.metadata_open = false,
            Overlay::SaveShare => self.show_save_share_window = false,
            Overlay::Library => self.library_dialog = None,
            Overlay::New => self.new_dialog = None,
            Overlay::Drawer => self.compact.drawer_open = false,
            Overlay::Resize => self.compact.resize_open = false,
        }
    }

    /// The whole window, in the compact layout.
    pub(super) fn compact_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        let editing = self.solve_gui.is_none();
        if !editing {
            // The editor's dialogs belong to the editor's document.
            self.compact.metadata_open = false;
            self.compact.resize_open = false;
            self.compact.color_dialog = None;
        }

        // The drawer floats above everything else, so it can go first, which it has to: what
        // it asks the solving aids to do has to happen before the canvas is drawn.
        let mut aid_requests = [false; Aid::ALL.len()];
        if self.compact.drawer_open {
            self.drawer(ctx, &mut aid_requests);
        }
        // The drawer isn't always open, but the aids that run after every move still have to.
        match &mut self.solve_gui {
            Some(solve_gui) => {
                for (aid, requested) in Aid::ALL.into_iter().zip(aid_requests) {
                    solve_gui.run_aid(aid, requested);
                }
            }
            None => self.update_line_solve(false),
        }

        let top_bar = egui::Panel::top("compact_top_bar")
            .show(ui, |ui| self.top_bar(ui))
            .response
            .rect;
        let bottom_bar = egui::Panel::bottom("compact_bottom_bar")
            .show(ui, |ui| self.bottom_bar(ui))
            .response
            .rect;

        if self.compact.resize_open {
            self.resize_window(ctx, bottom_bar.top());
        }

        self.canvas_panel(ui, ScreenLayout::Compact);

        self.progress_strip(ctx, top_bar.bottom());
        self.dialogs(ctx, ScreenLayout::Compact);
        if self.compact.metadata_open {
            self.metadata_dialog(ctx);
        }
        if let Some(color) = self.compact.color_dialog {
            self.color_dialog(ctx, color);
        }
        self.solved_modal(ctx);
        self.toasts(ctx, bottom_bar.top());
    }

    /// ☰, undo and redo; then, while solving, the button to clear annotations and whether
    /// there's an error, or while editing, the unsolved-cell count.
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add(icon_button(icons::ICON_MENU, ScreenLayout::Compact))
                .clicked()
            {
                self.compact.drawer_open = true;
            }
            self.current_canvas().undo_redo(ui, ScreenLayout::Compact);

            let right = egui::Layout::right_to_left(egui::Align::Center);
            match &mut self.solve_gui {
                Some(solve_gui) => {
                    solve_gui
                        .canvas
                        .clear_annotations_button(ui, ScreenLayout::Compact);
                    if solve_gui.errors_detected() {
                        ui.with_layout(right, |ui| {
                            egui::Frame::new()
                                .fill(Color32::from_rgb(0xfb, 0xe3, 0xe4))
                                .corner_radius(PILL_HEIGHT / 2.0)
                                .inner_margin(egui::Margin::symmetric(12, 6))
                                .show(ui, |ui| {
                                    ui.colored_label(Color32::DARK_RED, "Error detected");
                                });
                        });
                    }
                }
                None => {
                    ui.with_layout(right, |ui| self.unsolved_pill(ui));
                }
            }
        });
    }

    /// How many cells the line solve leaves unsolved. Tapping it disambiguates, and tapping it
    /// again while that runs stops it. While the line solve doesn't run after every change and
    /// is out of date, it says "Solve" instead, and runs it.
    fn unsolved_pill(&mut self, ui: &mut egui::Ui) {
        let fresh = self.solve_report_version == Some(self.editor_gui.version);
        let count = self.solve_cells_left.filter(|_| fresh);
        let running = self
            .editor_gui
            .disambiguator
            .val
            .running_progress()
            .is_some();

        let label = match count {
            Some(count) => format!("{count} unsolved"),
            None => "Solve".to_owned(),
        };
        let label = if running {
            format!("{label} {}", icons::ICON_STOP)
        } else {
            label
        };
        let pill = egui::Button::new(label)
            .corner_radius(PILL_HEIGHT / 2.0)
            .min_size(Vec2::new(0.0, PILL_HEIGHT));

        let clicked = ui
            .scope(|ui| {
                ui.spacing_mut().button_padding.x = PILL_HEIGHT / 3.0;
                ui.add(pill).clicked()
            })
            .inner;
        if clicked {
            let canvas = &mut self.editor_gui;
            if running {
                canvas.disambiguator.val.stop();
            } else if count.is_some() {
                let picture = canvas.document.try_solution().unwrap();
                canvas.disambiguator.val.start(picture);
            } else {
                self.update_line_solve(true);
            }
        }
    }

    /// The tools and the palette: in one row while solving, in two while editing, where the
    /// tool row also has buttons for the Metadata and Resize dialogs.
    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        let held = match &mut self.solve_gui {
            Some(solve_gui) => {
                centered_row(ui, "compact_tools", |ui| {
                    solve_gui.canvas.tool_selector(ui, ScreenLayout::Compact);
                    ui.separator();
                    let render_style = solve_gui.render_style;
                    solve_gui.canvas.palette_strip(ui, render_style);
                });
                None
            }
            None => {
                centered_row(ui, "compact_tools", |ui| {
                    self.editor_gui.tool_selector(ui, ScreenLayout::Compact);
                    ui.separator();
                    let compact = &mut self.compact;
                    let metadata =
                        egui::Button::new(RichText::new(icons::ICON_EDIT_NOTE).size(ICON_SIZE))
                            .toggled(ui, compact.metadata_open)
                            .min_size(Vec2::splat(TARGET));
                    if ui.add(metadata).clicked() {
                        compact.metadata_open = true;
                    }
                    let resize =
                        egui::Button::new(RichText::new(icons::ICON_RESIZE).size(ICON_SIZE))
                            .toggled(ui, compact.resize_open)
                            .min_size(Vec2::splat(TARGET));
                    if ui.add(resize).clicked() {
                        compact.resize_open = !compact.resize_open;
                    }
                });
                centered_row(ui, "compact_palette", |ui| {
                    // What the editor's canvas is drawn in.
                    self.editor_gui.palette_strip(ui, RenderStyle::Experimental)
                })
            }
        };
        if held.is_some() {
            self.compact.color_dialog = held;
        }
    }

    /// The menu: a drawer from the left, over the dimmed screen.
    fn drawer(&mut self, ctx: &egui::Context, aid_requests: &mut [bool; Aid::ALL.len()]) {
        let screen = ctx.content_rect();
        let width = DRAWER_WIDTH.min(screen.width() - TARGET);
        let id = egui::Id::unique("drawer");
        let margin = 12.0;

        let shown = egui::Modal::new(id)
            .area(
                egui::Modal::default_area(id)
                    .anchor(egui::Align2::LEFT_TOP, Vec2::ZERO)
                    .constrain(false),
            )
            .frame(egui::Frame::side_top_panel(&ctx.global_style()).inner_margin(margin))
            .show(ctx, |ui| {
                ui.set_width(width - 2.0 * margin);
                ui.set_height(screen.height() - 2.0 * margin);
                egui::ScrollArea::vertical()
                    .show(ui, |ui| self.drawer_contents(ui, aid_requests))
                    .inner
            });
        if shown.inner || shown.should_close() {
            self.compact.drawer_open = false;
        }
    }

    /// Returns whether the drawer should close.
    fn drawer_contents(
        &mut self,
        ui: &mut egui::Ui,
        aid_requests: &mut [bool; Aid::ALL.len()],
    ) -> bool {
        let mut close = false;

        ui.horizontal(|ui| {
            ui.heading("Number Loom");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                close = ui
                    .add(icon_button(icons::ICON_CLOSE, ScreenLayout::Compact))
                    .clicked();
            });
        });

        // Unlike the desktop's toolbar, choosing the mode that's already on does nothing:
        // starting the solve over is too easy to do by accident on a phone.
        let solving = self.solve_gui.is_some();
        let (mut edit, mut puzzle) = (false, false);
        ui.columns(2, |columns| {
            let mode = |ui: &mut egui::Ui, label: &str, selected: bool| {
                ui.add(
                    egui::Button::new(label)
                        .toggled(ui, selected)
                        .min_size(Vec2::new(ui.available_width(), 0.0)),
                )
                .clicked()
                    && !selected
            };
            edit = mode(&mut columns[0], "Edit", !solving);
            puzzle = mode(&mut columns[1], "Puzzle", solving);
        });
        if edit {
            self.exit_solve_mode();
            close = true;
        }
        if puzzle {
            self.enter_solve_mode();
            close = true;
        }

        ui.add_space(SECTION_GAP);
        ui.add(SectionHeading("File"));
        for (label, item) in [
            ("New", FileItem::New),
            ("Library", FileItem::Library),
            ("Open", FileItem::Open),
            ("Save/share", FileItem::SaveShare),
        ] {
            if menu_item(ui, label) {
                match item {
                    FileItem::New => self.open_new_dialog(),
                    FileItem::Library => self.open_library(),
                    FileItem::Open => self.open_file(),
                    FileItem::SaveShare => {
                        self.open_save_share();
                        // A link is the easiest thing to share from a phone.
                        self.share_as_link = true;
                    }
                }
                close = true;
            }
        }

        match &mut self.solve_gui {
            Some(solve_gui) => {
                ui.add_space(SECTION_GAP);
                ui.horizontal(|ui| {
                    ui.add(SectionHeading("Solve aids"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Centered over the switches, which are the last thing in each row.
                        let height = ui.text_style_height(&egui::TextStyle::Body);
                        ui.add_sized(Vec2::new(TARGET, height), SectionHeading("Auto"));
                    });
                });
                for (aid, requested) in Aid::ALL.into_iter().zip(aid_requests.iter_mut()) {
                    row_with_controls(ui, aid.label(), |ui| {
                        let name = format!("{} automatically", aid.label());
                        if toggle_switch(ui, solve_gui.auto_mut(aid), &name).changed() {
                            solve_gui.auto_changed(aid);
                        }
                        let auto = *solve_gui.auto_mut(aid);
                        *requested = ui.add_enabled(!auto, egui::Button::new("once")).clicked();
                    });
                }

                ui.add_space(SECTION_GAP);
                solve_gui.render_style_picker(ui, ScreenLayout::Compact);
            }
            None => {
                ui.add_space(SECTION_GAP);
                let label = "Auto-solve";
                let mut auto_solve = self.auto_solve;
                row_with_controls(ui, label, |ui| {
                    toggle_switch(ui, &mut auto_solve, label);
                });
                if auto_solve != self.auto_solve {
                    self.set_auto_solve(auto_solve);
                }
                self.backtrack_controls(ui);
            }
        }

        close
    }

    /// Under the top bar, while something is running.
    fn progress_strip(&self, ctx: &egui::Context, top: f32) {
        let Some(progress) = *self.editor_gui.progress.borrow() else {
            return;
        };
        let screen = ctx.content_rect();
        let track = Rect::from_min_max(
            Pos2::new(screen.left(), top),
            Pos2::new(screen.right(), top + PROGRESS_HEIGHT),
        );
        let done = Rect::from_min_size(
            track.min,
            Vec2::new(track.width() * progress.clamp(0.0, 1.0), track.height()),
        );
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("progress_strip"),
        ));
        let color = ctx.global_style().visuals.selection.bg_fill;
        painter.rect_filled(track, 0.0, color.gamma_multiply(0.3));
        painter.rect_filled(done, 0.0, color);
        // The search reports its progress over a channel, which egui doesn't know to watch.
        ctx.request_repaint();
    }

    /// Status messages, floating just above `bottom`. Errors stay until they're dismissed; the
    /// rest go away on their own.
    fn toasts(&mut self, ctx: &egui::Context, bottom: f32) {
        let Some((message, age)) = self.editor_gui.status.get_with_age() else {
            return;
        };
        if !message.is_error {
            if age >= TOAST_DURATION {
                return;
            }
            ctx.request_repaint_after(TOAST_DURATION - age);
        }

        let screen = ctx.content_rect();
        let fill = if message.is_error {
            Color32::from_rgb(0x8a, 0x1c, 0x22)
        } else {
            Color32::from_gray(0x32)
        };
        let mut dismissed = false;
        egui::Area::new(egui::Id::unique("toast"))
            .order(egui::Order::Foreground)
            // Fingers on the canvas go straight through one that can't be dismissed anyway.
            .interactable(message.is_error)
            .pivot(egui::Align2::CENTER_BOTTOM)
            .fixed_pos(Pos2::new(screen.center().x, bottom - DIALOG_MARGIN))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(fill)
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::symmetric(16, 8))
                    .show(ui, |ui| {
                        ui.set_max_width(screen.width() - 4.0 * DIALOG_MARGIN);
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&message.text).color(Color32::WHITE),
                                )
                                .wrap(),
                            );
                            if message.is_error {
                                dismissed = ui
                                    .add(
                                        icon_button(icons::ICON_CLOSE, ScreenLayout::Compact)
                                            .frame(false),
                                    )
                                    .clicked();
                            }
                        });
                    });
            });
        if dismissed {
            self.editor_gui.status.clear();
        }
    }

    /// The Resize controls, in a window just above the bottom bar (`bottom`), which leaves the
    /// grid in sight. It doesn't block anything, and the Resize button closes it.
    fn resize_window(&mut self, ctx: &egui::Context, bottom: f32) {
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("resize_window"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::CENTER_BOTTOM)
            .fixed_pos(Pos2::new(screen.center().x, bottom - DIALOG_MARGIN))
            .show(ctx, |ui| {
                egui::Frame::window(ui.style()).show(ui, |ui| {
                    ui.set_width(RESIZE_WIDTH.min(screen.width() - 4.0 * DIALOG_MARGIN));
                    self.resize_controls(ui, ScreenLayout::Compact);
                });
            });
    }

    fn metadata_dialog(&mut self, ctx: &egui::Context) {
        let (_, back) = dialog(
            ctx,
            ScreenLayout::Compact,
            egui::Window::new("Metadata"),
            "Metadata",
            |ui| self.metadata_form(ui),
        );
        if back {
            self.compact.metadata_open = false;
        }
    }

    /// A palette color's picker, its name, and (unless it's the background) a button to delete
    /// it. Opened by a long press on its swatch.
    fn color_dialog(&mut self, ctx: &egui::Context, color: Color) {
        let canvas = &mut self.editor_gui;
        if !canvas
            .document
            .try_solution()
            .is_some_and(|picture| picture.palette().contains_key(&color))
        {
            self.compact.color_dialog = None;
            return;
        }

        let width = (ctx.content_rect().width() - 4.0 * DIALOG_MARGIN).min(RESIZE_WIDTH);
        let (mut close, mut delete) = (false, false);
        let shown = egui::Modal::new(egui::Id::unique("color_dialog")).show(ctx, |ui| {
            ui.set_width(width);
            let color_info = canvas
                .document
                .solution_mut()
                .palette_mut()
                .get_mut(&color)
                .expect("just checked");

            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = ui
                        .add(icon_button(icons::ICON_CLOSE, ScreenLayout::Compact))
                        .clicked();
                    if color != BACKGROUND {
                        delete = ui
                            .add(icon_button(icons::ICON_DELETE, ScreenLayout::Compact))
                            .clicked();
                    }
                    // Not undoable, any more than the desktop's palette editor is.
                    ui.add(
                        egui::TextEdit::singleline(&mut color_info.name)
                            .desired_width(ui.available_width()),
                    )
                    .accessible_name("Rename this color");
                });
            });

            let (r, g, b) = color_info.rgb;
            let mut picked = Color32::from_rgb(r, g, b);
            // The picker is as wide as its sliders are.
            ui.spacing_mut().slider_width = width;
            if egui::widgets::color_picker::color_picker_color32(
                ui,
                &mut picked,
                egui::widgets::color_picker::Alpha::Opaque,
            ) {
                color_info.rgb = (picked.r(), picked.g(), picked.b());
            }
        });

        if delete {
            canvas.remove_color(color);
        }
        if close || delete || shown.should_close() {
            self.compact.color_dialog = None;
        }
    }

    /// Once the puzzle comes out right, a window saying so, with the solve played back and the
    /// puzzle's description. Only once per puzzle: it stays dismissed.
    fn solved_modal(&mut self, ctx: &egui::Context) {
        let Some(solve_gui) = &mut self.solve_gui else {
            return;
        };
        if solve_gui.solved_announced
            || solve_gui.replay.is_none()
            || !solve_gui.is_correctly_solved()
        {
            return;
        }

        let width = (ctx.content_rect().width() - 4.0 * DIALOG_MARGIN).min(360.0);
        let shown = egui::Modal::new(egui::Id::unique("solved")).show(ctx, |ui| {
            ui.set_width(width);
            let mut close = false;
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Correctly solved")
                        .heading()
                        .color(Color32::DARK_GREEN),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = ui
                        .add(icon_button(icons::ICON_CLOSE, ScreenLayout::Compact))
                        .clicked();
                });
            });
            solve_gui.title_and_author(ui);
            solve_gui.replay(ui);
            solve_gui.description(ui);
            close
        });
        if shown.inner || shown.should_close() {
            solve_gui.solved_announced = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gui() -> NonogramGui {
        NonogramGui::new(Document::from_solution(
            DynSolution::Square(Solution::blank_bw(5, 5)),
            "test".to_string(),
        ))
    }

    /// The back button closes whatever's on top, one thing at a time.
    #[test]
    fn back_closes_the_topmost_thing() {
        let mut gui = gui();
        assert!(gui.overlays(ScreenLayout::Compact).is_empty());

        gui.compact.drawer_open = true;
        gui.compact.resize_open = true;
        gui.open_save_share();
        assert_eq!(
            gui.overlays(ScreenLayout::Compact),
            vec![Overlay::SaveShare, Overlay::Drawer, Overlay::Resize]
        );

        for expected in [Overlay::SaveShare, Overlay::Drawer, Overlay::Resize] {
            let topmost = gui.overlays(ScreenLayout::Compact)[0];
            assert_eq!(topmost, expected);
            gui.close_overlay(topmost);
        }
        assert!(gui.overlays(ScreenLayout::Compact).is_empty());
    }

    /// Only the compact layout announces a solve with a window, so only there does back have
    /// it to close.
    #[test]
    fn the_solved_window_is_closed_by_back() {
        let mut gui = gui();
        gui.enter_solve_mode();
        let solve_gui = gui.solve_gui.as_mut().unwrap();
        let changes = solve_gui
            .intended_solution
            .cells()
            .iter_enumerated()
            .map(|(cell, color)| (cell, *color))
            .collect();
        solve_gui
            .canvas
            .perform(Action::ChangeColor { changes }, ActionMood::Normal);
        solve_gui.solver_frame();

        assert!(gui.overlays(ScreenLayout::Desktop).is_empty());
        assert_eq!(gui.overlays(ScreenLayout::Compact), vec![Overlay::Solved]);
        gui.close_overlay(Overlay::Solved);
        assert!(gui.overlays(ScreenLayout::Compact).is_empty());
    }
}
