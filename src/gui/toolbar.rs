//! The document-wide controls across the top of the window, and the three dialogs they open.
//!
//! The New, Library, and Save/share windows are opened, drawn, and dismissed entirely within
//! `toolbar`, so their state lives no wider than this module needs.

use super::*;

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
    x_size: usize,
    y_size: usize,
    /// Hexagon side length, used only when `shape` is `Triangular`. Doesn't cover every possible
    /// triddler outline (see `Outline`) — just a reasonable default shape to start editing from.
    tri_side: i32,
}

impl NonogramGui {
    /// The document-wide controls across the top: zoom, the New/Library/Open/Save dialogs, and
    /// the Edit/Puzzle mode toggle. Runs before the sidebar and canvas each frame, so a mode
    /// switched here takes effect on the same frame.
    pub(super) fn toolbar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        // See the matching note in `common_sidebar_items`: bare-key shortcuts have to opt out of
        // firing while a text field has the focus.
        let typing = ctx.wants_keyboard_input();

        ui.horizontal(|ui| {
            if ui.button(icons::ICON_ZOOM_IN).clicked()
                || (!typing && ui.input(|i| i.key_pressed(egui::Key::Equals)))
            {
                self.scale = (self.scale + 2.0).min(50.0);
            }
            if ui.button(icons::ICON_ZOOM_OUT).clicked()
                || (!typing && ui.input(|i| i.key_pressed(egui::Key::Minus)))
            {
                self.scale = (self.scale - 2.0).max(1.0);
            }
            if ui.button("New").clicked() {
                let clue_style = self.editor_gui.document.solution_mut().clue_style();
                // Only a square puzzle has a width/height to seed the dialog's (square-shaped)
                // defaults from; a triddler's own dimensions don't map onto this at all.
                let (x_size, y_size) = self
                    .editor_gui
                    .document
                    .try_solution()
                    .and_then(|s| s.as_square())
                    .map(|sq| (sq.x_size(), sq.y_size()))
                    .unwrap_or((10, 10));
                self.new_dialog = Some(NewPuzzleDialog {
                    shape: NewPuzzleShape::Square,
                    clue_style,
                    x_size,
                    y_size,
                    tri_side: 3,
                });
            }
            let mut new_document = None;
            if let Some(dialog) = self.new_dialog.as_mut() {
                egui::Window::new("New puzzle").show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut dialog.shape, NewPuzzleShape::Square, "Square");
                        ui.radio_value(&mut dialog.shape, NewPuzzleShape::Triangular, "Triddler");
                    });

                    match dialog.shape {
                        NewPuzzleShape::Square => {
                            ui.add(
                                egui::Slider::new(&mut dialog.x_size, 5..=100)
                                    .step_by(5.0)
                                    .text("x size"),
                            );
                            ui.add(
                                egui::Slider::new(&mut dialog.y_size, 5..=100)
                                    .step_by(5.0)
                                    .text("y size"),
                            );
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
                        NewPuzzleShape::Triangular => {
                            // Trianogram clues on a triddler are rejected at construction, so
                            // there's nothing to choose here — a triddler is always a nonogram.
                            ui.add(
                                egui::Slider::new(&mut dialog.tri_side, 1..=10)
                                    .text("hexagon side"),
                            );
                        }
                    }

                    if ui.button("Ok").clicked() {
                        let new_solution = match dialog.shape {
                            NewPuzzleShape::Square => DynSolution::Square(Solution::new(
                                dialog.clue_style,
                                match dialog.clue_style {
                                    ClueStyle::Nono => import::bw_palette(),
                                    ClueStyle::Triano => import::triano_palette(),
                                },
                                crate::geometry::Geometry::new(crate::geometry::Rect {
                                    width: dialog.x_size,
                                    height: dialog.y_size,
                                }),
                                vec![BACKGROUND; dialog.x_size * dialog.y_size],
                            )),
                            NewPuzzleShape::Triangular => {
                                let geometry = crate::geometry::Geometry::new(
                                    crate::geometry::Outline::hexagon(dialog.tri_side),
                                );
                                let cells = vec![BACKGROUND; geometry.cell_count()];
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
                        self.solve_mode = false;
                    }
                });
            }

            if ui.button("Library").clicked() {
                let (sender, receiver) = mpsc::channel();
                self.library_receiver = receiver;
                self.library_dialog = Some(LibraryStatus::Loading);

                spawn_async(async move {
                    let result = crate::import::puzzles_from_github().await;
                    let _ = sender.send(result);
                });
            }

            if let Ok(result) = self.library_receiver.try_recv() {
                match result {
                    Ok(library) => self.library_dialog = Some(LibraryStatus::Loaded(library)),
                    Err(e) => self.library_dialog = Some(LibraryStatus::Failed(e.to_string())),
                }
            }

            let mut next_enter_solve_mode = false;
            let mut close_library = false;
            if let Some(status) = &self.library_dialog {
                egui::Window::new("Puzzle Library")
                    .max_size(ctx.screen_rect().size() * 0.9)
                    .show(ctx, |ui| {
                        match status {
                            LibraryStatus::Loading => {
                                ui.vertical_centered(|ui| {
                                    ui.add(egui::Spinner::new());
                                    ui.label("Loading library...");
                                });
                            }
                            LibraryStatus::Loaded(docs) => {
                                egui::ScrollArea::vertical().show(ui, |ui| {
                                    egui::Grid::new("library_grid").show(ui, |ui| {
                                        for (i, doc) in docs.iter().enumerate() {
                                            if gallery::gallery_puzzle_preview(ui, doc).clicked() {
                                                new_document = Some(doc.clone());
                                                next_enter_solve_mode = true;
                                                close_library = true;
                                            }
                                            if i % 2 == 1 {
                                                ui.end_row();
                                            }
                                        }
                                    });
                                });
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
                        ui.separator();
                        if ui.button("Cancel").clicked() {
                            close_library = true;
                        }
                    });
            }
            if close_library {
                self.library_dialog = None;
            }
            self.loader(ui);

            if ui.button("Save/share").clicked() {
                self.share_string =
                    crate::formats::woven::to_woven(&mut self.editor_gui.document).unwrap();
                self.quality_warnings = self.editor_gui.document.quality_check();
                self.show_save_share_window = true;
            }

            if self.show_save_share_window {
                egui::Window::new("Save/share")
                    .open(&mut self.show_save_share_window)
                    .default_width(780.0)
                    .show(ctx, |ui| {
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
                        ui.label("Share String:");
                        ui.add(
                            egui::TextEdit::multiline(&mut self.share_string.clone())
                                .font(TextStyle::Monospace)
                                .desired_width(730.0),
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
                                .desired_width(730.0),
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
                                    .desired_width(450.0),
                            );
                        });
                        if ui.button("Save").clicked() {
                            let mut document_copy = self.editor_gui.document.clone();

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
                    });
            }

            if let Some(new_document) = new_document {
                let document = Box::new(new_document);
                self.editor_gui
                    .perform(Action::ReplaceDocument { document }, ActionMood::Normal);
                self.new_dialog = None;
                self.library_dialog = None;
                self.show_save_share_window = false;
            }

            ui.separator();
            if ui
                .selectable_value(&mut self.solve_mode, false, "Edit")
                .clicked()
            {
                self.solve_gui = None;
            }
            if ui
                .selectable_value(&mut self.solve_mode, true, "Puzzle")
                .clicked()
                || next_enter_solve_mode
            {
                self.enter_solve_mode();
            }
        });
    }

    fn loader(&mut self, ui: &mut egui::Ui) {
        if ui.button("Open").clicked() {
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
                    let document =
                        crate::import::load(&handle.file_name(), handle.read().await, None);

                    sender.send(document).unwrap();
                }
            });
        }

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
