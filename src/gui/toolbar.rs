//! The document-wide controls across the top of the window, and the three dialogs they open.
//!
//! The New, Library, and Save/share windows are opened from here (or from the compact layout's
//! menu), and drawn and dismissed by `dialogs`.

use super::*;
use crate::formats::partial_cells::has_progress;

/// How far the puzzle library has got, once the Library button has asked for it.
pub enum LibraryStatus {
    Loading,
    Loaded(Vec<Document>),
    Failed(String),
}

#[derive(PartialEq, Eq)]
enum NewPuzzleShape {
    Square,
    Triangular,
}

pub(super) struct NewPuzzleDialog {
    shape: NewPuzzleShape,
    clue_style: crate::puzzle::ClueStyle,
}

/// A new square puzzle's dimensions. The dialog offers no size controls at all; resizing from
/// the sidebar is the way to get to any other shape.
const NEW_SQUARE_SIZE: (usize, usize) = (20, 20);

/// Hexagon side length for a new triddler: 5 is equivalent to a
const NEW_TRI_SIDE: i32 = 5;

impl NonogramGui {
    fn share_string(&self) -> String {
        let woven = share_string(
            &self.editor_gui.document,
            self.share_progress.as_ref(),
            self.save_solve_progress,
        );

        if self.share_as_link {
            format!("https://paulstansifer.github.io/number-loom/#{woven}")
        } else {
            woven
        }
    }

    /// The document-wide controls across the top: zoom, the New/Library/Open/Save dialogs, and
    /// the Edit/Puzzle mode toggle. Runs before the sidebar and canvas each frame, so a mode
    /// switched here takes effect on the same frame.
    pub(super) fn toolbar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        // See the matching note in `CanvasGui::undo_redo`: bare-key shortcuts have to opt out of
        // firing while a text field has the focus.
        let typing = ctx.wants_keyboard_input();

        ui.horizontal(|ui| {
            if ui.button(icons::ICON_ZOOM_IN).clicked()
                || (!typing && ui.input(|i| i.key_pressed(egui::Key::Equals)))
            {
                self.scale = (self.scale + ZOOM_STEP).min(MAX_SCALE);
            }
            if ui.button(icons::ICON_ZOOM_OUT).clicked()
                || (!typing && ui.input(|i| i.key_pressed(egui::Key::Minus)))
            {
                self.scale = (self.scale - ZOOM_STEP).max(MIN_SCALE);
            }
            if ui.button("New").clicked() {
                self.open_new_dialog();
            }
            if ui.button("Library").clicked() {
                self.open_library();
            }
            if ui.button("Open").clicked() {
                self.open_file();
            }
            if ui.button("Save/share").clicked() {
                self.open_save_share();
            }

            ui.separator();

            let solving = self.solve_gui.is_some();
            if ui.selectable_label(!solving, "Edit").clicked() {
                self.exit_solve_mode();
            }
            if ui.selectable_label(solving, "Puzzle").clicked() {
                self.enter_solve_mode();
            }
        });
    }

    pub(super) fn open_new_dialog(&mut self) {
        let clue_style = self.editor_gui.document.solution_mut().clue_style();
        self.new_dialog = Some(NewPuzzleDialog {
            shape: NewPuzzleShape::Square,
            clue_style,
        });
    }

    pub(super) fn open_library(&mut self) {
        let (sender, receiver) = mpsc::channel();
        self.library_receiver = receiver;
        self.library_dialog = Some(LibraryStatus::Loading);

        spawn_async(async move {
            let result = crate::import::load_library().await;
            let _ = sender.send(result);
        });
    }

    pub(super) fn open_save_share(&mut self) {
        self.share_progress = self.solve_progress();
        self.quality_warnings = self.editor_gui.document.quality_check();
        self.show_save_share_window = true;
        self.share_string.clear(); // needs to be regenerated
    }

    /// The New, Library and Save/share dialogs, whichever are open, and what comes back from the
    /// file and library loads they (and Open) started. Floating windows on the desktop; the
    /// whole screen in the compact layout.
    pub(super) fn dialogs(&mut self, ctx: &egui::Context, layout: ScreenLayout) {
        // How wide the Save/share window's text boxes are.
        let text_width = match layout {
            ScreenLayout::Desktop => 730.0,
            ScreenLayout::Compact => ctx.screen_rect().width() - 2.0 * compact::DIALOG_MARGIN,
        };

        let mut new_document = None;
        // A new puzzle is something to draw, so it always lands in the editor. Deferred,
        // rather than done in the button handler, because `dialog` borrows out of `self`.
        let mut leave_solve_mode = false;
        if let Some(dialog) = self.new_dialog.as_mut() {
            let (_, back) = compact::dialog(
                ctx,
                layout,
                egui::Window::new("New puzzle"),
                "New puzzle",
                |ui| {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut dialog.shape, NewPuzzleShape::Square, "Square");
                        ui.radio_value(&mut dialog.shape, NewPuzzleShape::Triangular, "Triddler");
                    });

                    // Trianogram clues on a triddler are rejected at construction, so the clue
                    // style is only a choice for a square puzzle.
                    if dialog.shape == NewPuzzleShape::Square {
                        ui.radio_value(
                            &mut dialog.clue_style,
                            crate::puzzle::ClueStyle::Nono,
                            "Nonogram",
                        );
                        ui.radio_value(
                            &mut dialog.clue_style,
                            crate::puzzle::ClueStyle::Triano,
                            "Trianogram",
                        );
                    }

                    if ui.button("Ok").clicked() {
                        let new_solution = match dialog.shape {
                            NewPuzzleShape::Square => {
                                let (x_size, y_size) = NEW_SQUARE_SIZE;
                                DynSolution::Square(Solution::new(
                                    dialog.clue_style,
                                    match dialog.clue_style {
                                        ClueStyle::Nono => import::bw_palette(),
                                        ClueStyle::Triano => import::triano_palette(),
                                    },
                                    crate::geometry::Geometry::new(crate::geometry::Rect {
                                        width: x_size,
                                        height: y_size,
                                    }),
                                    vec![BACKGROUND; x_size * y_size].into(),
                                ))
                            }
                            NewPuzzleShape::Triangular => {
                                let geometry = crate::geometry::Geometry::new(
                                    crate::geometry::Outline::hexagon(NEW_TRI_SIDE),
                                );
                                let cells = vec![BACKGROUND; geometry.cell_count()].into();
                                DynSolution::Tri(Solution::new(
                                    ClueStyle::Nono,
                                    import::bw_palette(),
                                    geometry,
                                    cells,
                                ))
                            }
                        };
                        new_document = Some(Document::from_solution(
                            new_solution,
                            "blank.xml".to_owned(),
                        ));
                        leave_solve_mode = true;
                    }
                },
            );
            if back {
                self.new_dialog = None;
            }
        }

        if let Ok(result) = self.library_receiver.try_recv() {
            match result {
                Ok(library) => self.library_dialog = Some(LibraryStatus::Loaded(library)),
                // `{:#}` rather than `to_string`, so the dialog shows the whole context chain
                // ("couldn't fetch https://...: HTTP status 404") and not just its last link.
                Err(e) => self.library_dialog = Some(LibraryStatus::Failed(format!("{e:#}"))),
            }
        }

        let mut next_enter_solve_mode = false;
        let mut close_library = false;
        if let Some(status) = &self.library_dialog {
            // The previews are a fixed width, so a phone only has room for one across.
            let columns = match layout {
                ScreenLayout::Desktop => 2,
                ScreenLayout::Compact => 1,
            };
            let (_, back) = compact::dialog(
                ctx,
                layout,
                egui::Window::new("Puzzle Library").max_size(ctx.screen_rect().size() * 0.9),
                "Puzzle Library",
                |ui| {
                    match status {
                        LibraryStatus::Loading => {
                            ui.vertical_centered(|ui| {
                                ui.add(egui::Spinner::new());
                                ui.label("Loading library...");
                            });
                        }
                        LibraryStatus::Loaded(docs) => {
                            let mut grid = |ui: &mut egui::Ui| {
                                egui::Grid::new("library_grid").show(ui, |ui| {
                                    for (i, doc) in docs.iter().enumerate() {
                                        if gallery::gallery_puzzle_preview(ui, doc).clicked() {
                                            new_document = Some(doc.clone());
                                            next_enter_solve_mode = true;
                                            close_library = true;
                                        }
                                        if i % columns == columns - 1 {
                                            ui.end_row();
                                        }
                                    }
                                });
                            };
                            match layout {
                                ScreenLayout::Desktop => {
                                    egui::ScrollArea::vertical().show(ui, grid);
                                }
                                // The whole screen scrolls already.
                                ScreenLayout::Compact => grid(ui),
                            }
                        }
                        LibraryStatus::Failed(e) => {
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new(format!("Failed to load library: {}", e))
                                        .color(Color32::RED),
                                );
                            });
                        }
                    }
                    // The compact layout's dialogs close with their back button.
                    if layout == ScreenLayout::Desktop {
                        ui.separator();
                        if ui.button("Cancel").clicked() {
                            close_library = true;
                        }
                    }
                },
            );
            close_library |= back;
        }
        if close_library {
            self.library_dialog = None;
        }

        self.receive_opened_file();

        if self.show_save_share_window {
            if self.share_string.is_empty() {
                self.share_string = self.share_string();
            }
            let (_, back) = compact::dialog(
                ctx,
                layout,
                egui::Window::new("Save/share")
                    .open(&mut self.show_save_share_window)
                    .default_width(780.0),
                "Save/share",
                |ui| {
                    if !self.quality_warnings.is_empty() {
                        if self.quality_warnings.len() == 1 {
                            ui.label("Warning:");
                        } else {
                            ui.label("Warnings:");
                        }
                        for warning in &self.quality_warnings {
                            ui.label(warning);
                        }
                        ui.separator();
                    }
                    let sh_ui_1 = ui.add_enabled(
                        self.share_progress.is_some(),
                        egui::Checkbox::new(&mut self.save_solve_progress, "save solve progress"),
                    );
                    let sh_ui_2 = ui.radio_value(&mut self.share_as_link, false, "Share string:");
                    let sh_ui_3 = ui.radio_value(&mut self.share_as_link, true, "Share link:");

                    if sh_ui_1.changed() || sh_ui_2.changed() || sh_ui_3.changed() {
                        self.share_string.clear();
                    }

                    ui.add(
                        egui::TextEdit::multiline(&mut self.share_string.clone())
                            .font(TextStyle::Monospace)
                            .desired_width(text_width),
                    );
                    if ui.button("Copy to clipboard").clicked() {
                        ctx.copy_text(self.share_string.clone());
                    }

                    if self.editor_gui.document.license == "CC BY 4.0" {
                        if self.editor_gui.document.author.trim().is_empty() {
                            ui.label(
                                "(The author field in your puzzle is empty; please use \
                                'anonymous' if that's what you want.)",
                            );
                        }
                        ui.add(
                            egui::Hyperlink::from_label_and_url(
                                "Contribute this puzzle to Number Loom",
                                "https://forms.gle/WXxWVsEMqy3NHXmK9",
                            )
                            .open_in_new_tab(true),
                        );
                    } else {
                        ui.label(
                            "If you'd like to contribute your puzzle to Number Loom's \
                            library, please set the license to 'CC BY 4.0'",
                        );
                    }

                    ui.separator();

                    ui.label("Paste a 'WOVEN' string to load:");
                    ui.add(
                        egui::TextEdit::multiline(&mut self.pasted_string)
                            .font(TextStyle::Monospace)
                            .desired_width(text_width),
                    );

                    if ui.button("Load").clicked() {
                        match crate::formats::woven::from_woven(
                            &self.pasted_string,
                            "unknown.woven".to_string(),
                        ) {
                            Ok(doc) => {
                                new_document = Some(doc);
                                next_enter_solve_mode = true;
                            }
                            Err(e) => {
                                self.editor_gui.status.set(StatusMessage::error(format!(
                                    "Error loading WOVEN puzzle: {:?}",
                                    e
                                )));
                            }
                        }
                    }

                    ui.separator();

                    ui.label("Supported file types:");
                    ui.label("  .png (or other image formats): solution image");
                    ui.label("  .xml/.pbn: the format used by the \"pbnsolve\" solver");
                    ui.label("  .txt: grid of characters");
                    ui.label("  .g: the format used by the Olšák solver");
                    ui.label("  .woven: Number Loom's custom format");
                    ui.label("  .html: printable puzzle");

                    ui.horizontal(|ui| {
                        ui.label("Filename:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.editor_gui.document.file)
                                .desired_width(text_width.min(450.0)),
                        );
                    });
                    if ui.button("Save").clicked() {
                        let mut document_copy = document_to_save(
                            &self.editor_gui.document,
                            self.share_progress.as_ref(),
                            self.save_solve_progress,
                        );

                        let (sender, receiver) = mpsc::channel();
                        self.save_result_receiver = receiver;

                        spawn_async(async move {
                            let handle = rfd::AsyncFileDialog::new()
                                .add_filter(
                                    "all recognized formats",
                                    &["png", "gif", "bmp", "xml", "pbn", "txt", "g", "html"],
                                )
                                .add_filter("image", &["png", "gif", "bmp"])
                                .add_filter("PBN", &["xml", "pbn"])
                                .add_filter("chargrid", &["txt"])
                                .add_filter("Olšák", &["g"])
                                .add_filter("woven", &["woven"])
                                .add_filter("HTML (for printing)", &["html"])
                                .set_file_name(document_copy.file.clone())
                                .save_file()
                                .await;

                            if let Some(handle) = handle {
                                let result = async {
                                    let bytes = to_bytes(
                                        &mut document_copy,
                                        Some(handle.file_name()),
                                        None,
                                    )?;
                                    handle.write(&bytes).await?;
                                    Ok(())
                                }
                                .await;
                                sender.send(result).unwrap();
                            }
                        });
                    }

                    if let Ok(Err(e)) = self.save_result_receiver.try_recv() {
                        self.editor_gui
                            .status
                            .set(StatusMessage::error(format!("Error saving file: {:?}", e)));
                    }
                },
            );
            if back {
                self.show_save_share_window = false;
            }
        }

        if let Some(new_document) = new_document {
            let document = Box::new(new_document);
            self.editor_gui
                .perform(Action::ReplaceDocument { document }, ActionMood::Normal);
            self.new_dialog = None;
            self.library_dialog = None;
            self.show_save_share_window = false;
        }
        if leave_solve_mode {
            self.exit_solve_mode();
        }
        if next_enter_solve_mode {
            self.enter_solve_mode();
        }
    }

    /// The grid as it's being solved (or, outside of solve mode, as it was loaded), if anything
    /// on it is known.
    fn solve_progress(&mut self) -> Option<PartialSolution> {
        let progress = match &mut self.solve_gui {
            Some(solve_gui) => solve_gui.canvas.document.solution().ok()?.to_partial(),
            None => self.editor_gui.document.in_progress()?.clone(),
        };
        let palette = self.editor_gui.document.solution().ok()?.palette();
        has_progress(&progress.raw, palette.keys().copied()).then_some(progress)
    }

    /// Ask for a file to open. It turns up in `receive_opened_file`.
    pub(super) fn open_file(&mut self) {
        let (sender, receiver) = mpsc::channel();
        self.opened_file_receiver = receiver;

        spawn_async(async move {
            let handle = rfd::AsyncFileDialog::new()
                .add_filter(
                    "all recognized formats",
                    &["png", "gif", "bmp", "xml", "pbn", "txt", "g"],
                )
                .add_filter("image", &["png", "gif", "bmp"])
                .add_filter("PBN", &["xml", "pbn"])
                .add_filter("chargrid", &["txt"])
                .add_filter("Olsak", &["g"])
                .add_filter("woven", &["woven"])
                .pick_file()
                .await;

            if let Some(handle) = handle {
                let document = crate::import::load(&handle.file_name(), handle.read().await, None);
                // We need a solution to edit it:
                let document = match document {
                    Ok(mut document) => document.find_solution().await.map(|()| document),
                    Err(e) => Err(e),
                };

                sender.send(document).unwrap();
            }
        });
    }

    fn receive_opened_file(&mut self) {
        if let Ok(result) = self.opened_file_receiver.try_recv() {
            match result {
                Ok(document) => {
                    let document = Box::new(document);
                    self.editor_gui
                        .perform(Action::ReplaceDocument { document }, ActionMood::Normal);
                }
                Err(e) => {
                    self.editor_gui
                        .status
                        .set(StatusMessage::error(format!("Error loading file: {:?}", e)));
                }
            }
        }
    }
}

/// `document`, with `progress` in it if `save_progress` (and none otherwise, even if it was loaded
/// with some).
fn document_to_save(
    document: &Document,
    progress: Option<&PartialSolution>,
    save_progress: bool,
) -> Document {
    let mut document = document.clone();
    document.set_in_progress(progress.filter(|_| save_progress).cloned());
    document
}

fn share_string(
    document: &Document,
    progress: Option<&PartialSolution>,
    save_progress: bool,
) -> String {
    crate::formats::woven::to_woven(&mut document_to_save(document, progress, save_progress))
        .unwrap()
}
