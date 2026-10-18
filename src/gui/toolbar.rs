//! The document-wide controls across the top of the window, and the three dialogs they open.
//!
//! The New, Library, and Save/share windows are opened from here (or from the compact layout's
//! menu), and drawn and dismissed by `dialogs`, which has a method for each.

use super::*;
use crate::formats::partial_cells::has_progress;
use crate::puzzle::UncertainDocument;

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

/// Hexagon side length for a new triddler.
///
/// TODO: this said "5 is equivalent to a", and stopped. Finish the comparison (to a square
/// puzzle's size, presumably).
const NEW_TRI_SIDE: i32 = 5;

impl NewPuzzleDialog {
    /// A blank puzzle, of the shape and with the clues chosen.
    fn new_document(&self) -> Document {
        let new_solution = match self.shape {
            NewPuzzleShape::Square => {
                let (x_size, y_size) = NEW_SQUARE_SIZE;
                DynSolution::Square(Solution::new(
                    self.clue_style,
                    match self.clue_style {
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
                let geometry =
                    crate::geometry::Geometry::new(crate::geometry::Outline::hexagon(NEW_TRI_SIDE));
                let cells = vec![BACKGROUND; geometry.cell_count()].into();
                DynSolution::Tri(Solution::new(
                    ClueStyle::Nono,
                    import::bw_palette(),
                    geometry,
                    cells,
                ))
            }
        };
        Document::from_solution(new_solution, "blank.xml".to_owned())
    }
}

/// The Save/share window, while it's open.
pub(super) struct SaveShareDialog {
    /// The solve progress there was when the window was opened, if there was any.
    progress: Option<PartialSolution>,
    /// Whether to put `progress` in what this saves and shares.
    save_progress: bool,
    /// Whether to share a link, rather than a bare share string.
    as_link: bool,
    /// What there is to share, as things stand: empty when it needs working out again.
    shared: String,
    /// A share string pasted in, to load.
    pasted: String,
    quality_warnings: Vec<String>,
}

impl SaveShareDialog {
    /// `document`, as this saves and shares it: with `progress` in it if `save_progress` (and
    /// none otherwise, even if it was loaded with some).
    fn document_to_save(&self, document: &Document) -> Document {
        let mut document = document.clone();
        document.set_in_progress(self.progress.clone().filter(|_| self.save_progress));
        document
    }

    /// What to share: a share string, or a link with one in it.
    fn shared(&self, document: &Document) -> String {
        let woven = crate::formats::woven::to_woven(&self.document_to_save(document)).unwrap();
        if self.as_link {
            // A share string is broken into lines, which a link can't be. (Reading it back
            // ignores whitespace either way.)
            let woven: String = woven.split_whitespace().collect();
            format!("https://paulstansifer.github.io/number-loom/#{woven}")
        } else {
            woven
        }
    }
}

impl NonogramGui {
    /// The document-wide controls across the top: zoom, the New/Library/Open/Save dialogs, and
    /// the Edit/Puzzle mode toggle. Runs before the sidebar and canvas each frame, so a mode
    /// switched here takes effect on the same frame.
    pub(super) fn toolbar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        // See the matching note in `CanvasGui::undo_redo`: bare-key shortcuts have to opt out of
        // firing while a text field has the focus.
        let typing = ctx.egui_wants_keyboard_input();

        ui.horizontal(|ui| {
            if ui.button(icons::ICON_ZOOM_IN).clicked()
                || (!typing && ui.input(|i| i.key_pressed(egui::Key::Equals)))
            {
                self.view.zoom_in();
            }
            if ui.button(icons::ICON_ZOOM_OUT).clicked()
                || (!typing && ui.input(|i| i.key_pressed(egui::Key::Minus)))
            {
                self.view.zoom_out();
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
                self.open_save_share(false);
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
        let clue_style = self.canvas.document.solution().clue_style();
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

    /// Open the Save/share window, offering a link to start with if `as_link`, and otherwise a
    /// bare share string.
    pub(super) fn open_save_share(&mut self, as_link: bool) {
        self.save_share = Some(SaveShareDialog {
            progress: self.solve_progress(),
            save_progress: false,
            as_link,
            shared: String::new(),
            pasted: String::new(),
            quality_warnings: self.canvas.document.quality_check(),
        });
    }

    /// The New, Library and Save/share dialogs, whichever are open, and what comes back from the
    /// file and library loads they (and Open) started. Floating windows on the desktop; the
    /// whole screen in the compact layout.
    pub(super) fn dialogs(&mut self, ctx: &egui::Context, layout: ScreenLayout) {
        // Each of these can come back with a document to open. It lands in the editor (see
        // `replace_document`), and the Library's and pasted ones are then opened for solving.
        let new_puzzle = self.new_puzzle_dialog(ctx, layout).map(|doc| (doc, false));
        self.receive_library();
        let from_library = self.library_dialog(ctx, layout).map(|doc| (doc, true));
        self.receive_opened_file();
        let pasted = self.save_share_dialog(ctx, layout).map(|doc| (doc, true));
        self.receive_save_result();

        if let Some((document, for_solving)) = pasted.or(from_library).or(new_puzzle) {
            self.replace_document(document);
            self.new_dialog = None;
            self.library_dialog = None;
            self.save_share = None;
            if for_solving {
                self.enter_solve_mode();
            }
        }
    }

    /// The New puzzle dialog, if it's open. Returns the new puzzle, once it's been asked for.
    fn new_puzzle_dialog(&mut self, ctx: &egui::Context, layout: ScreenLayout) -> Option<Document> {
        let dialog = self.new_dialog.as_mut()?;
        let mut new_document = None;
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

                // Trianogram clues on a triddler are rejected at construction, so the clue style
                // is only a choice for a square puzzle.
                if dialog.shape == NewPuzzleShape::Square {
                    ui.radio_value(&mut dialog.clue_style, ClueStyle::Nono, "Nonogram");
                    ui.radio_value(&mut dialog.clue_style, ClueStyle::Triano, "Trianogram");
                }

                if ui.button("Ok").clicked() {
                    new_document = Some(dialog.new_document());
                }
            },
        );
        if back {
            self.new_dialog = None;
        }
        new_document
    }

    /// Take in the library, if it has arrived since the Library button asked for it.
    fn receive_library(&mut self) {
        if let Ok(result) = self.library_receiver.try_recv() {
            self.library_dialog = Some(match result {
                Ok(library) => LibraryStatus::Loaded(library),
                // `{:#}` rather than `to_string`, so the dialog shows the whole context chain
                // ("couldn't fetch https://...: HTTP status 404") and not just its last link.
                Err(e) => LibraryStatus::Failed(format!("{e:#}")),
            });
        }
    }

    /// The Library dialog, if it's open. Returns the puzzle chosen from it, if one was.
    fn library_dialog(&mut self, ctx: &egui::Context, layout: ScreenLayout) -> Option<Document> {
        let status = self.library_dialog.as_ref()?;
        let columns = 2;
        let mut chosen = None;
        let mut close = false;
        let (_, back) = compact::dialog(
            ctx,
            layout,
            egui::Window::new("Puzzle Library").max_size(ctx.content_rect().size() * 0.9),
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
                            // A phone's previews shrink to fill the screen; a desktop's don't.
                            let width = match layout {
                                ScreenLayout::Desktop => gallery::DESKTOP_PREVIEW_WIDTH,
                                ScreenLayout::Compact => {
                                    gallery::preview_width_for(ui, ui.available_width(), columns)
                                }
                            };
                            egui::Grid::new("library_grid").show(ui, |ui| {
                                for (i, doc) in docs.iter().enumerate() {
                                    if gallery::gallery_puzzle_preview(ui, doc, width).clicked() {
                                        chosen = Some(doc.clone());
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
                                    .color(Color32::DARK_RED),
                            );
                        });
                    }
                }
                // The compact layout's dialogs close with their back button.
                if layout == ScreenLayout::Desktop {
                    ui.separator();
                    close = ui.button("Cancel").clicked();
                }
            },
        );
        if close || back {
            self.library_dialog = None;
        }
        chosen
    }

    /// The Save/share window, if it's open. Returns a puzzle pasted into it to load, if one was.
    fn save_share_dialog(&mut self, ctx: &egui::Context, layout: ScreenLayout) -> Option<Document> {
        let dialog = self.save_share.as_mut()?;
        let canvas = &mut self.canvas;
        // How wide the text boxes are.
        let text_width = match layout {
            ScreenLayout::Desktop => 730.0,
            ScreenLayout::Compact => ctx.content_rect().width() - 2.0 * compact::DIALOG_MARGIN,
        };
        if dialog.shared.is_empty() {
            dialog.shared = dialog.shared(&canvas.document);
        }

        let mut open = true;
        let mut loaded = None;
        let mut save = false;
        let (_, back) = compact::dialog(
            ctx,
            layout,
            egui::Window::new("Save/share")
                .open(&mut open)
                .default_width(780.0),
            "Save/share",
            |ui| {
                if !dialog.quality_warnings.is_empty() {
                    if dialog.quality_warnings.len() == 1 {
                        ui.label("Warning:");
                    } else {
                        ui.label("Warnings:");
                    }
                    for warning in &dialog.quality_warnings {
                        ui.label(warning);
                    }
                    ui.separator();
                }
                let sh_ui_1 = ui.add_enabled(
                    dialog.progress.is_some(),
                    egui::Checkbox::new(&mut dialog.save_progress, "save solve progress"),
                );
                let sh_ui_2 = ui.radio_value(&mut dialog.as_link, false, "Share string:");
                let sh_ui_3 = ui.radio_value(&mut dialog.as_link, true, "Share link:");

                if sh_ui_1.changed() || sh_ui_2.changed() || sh_ui_3.changed() {
                    dialog.shared.clear();
                }

                let chosen = if dialog.as_link { &sh_ui_3 } else { &sh_ui_2 };
                ui.add(
                    // Read-only: an `&str` can be selected and copied, but not edited.
                    egui::TextEdit::multiline(&mut dialog.shared.as_str())
                        .font(TextStyle::Monospace)
                        .desired_width(text_width),
                )
                .labelled_by(chosen.id);
                if compact::can_share() {
                    if ui.button(icons::ICON_SHARE).clicked() {
                        compact::share(&canvas.document.title, &dialog.shared, dialog.as_link);
                    }
                } else if ui.button("Copy to clipboard").clicked() {
                    ctx.copy_text(dialog.shared.clone());
                    // There's nothing else to show that it happened, on a phone.
                    if layout == ScreenLayout::Compact {
                        canvas.status.set(StatusMessage::info("Copied"));
                    }
                }

                if canvas.document.license == CC_BY {
                    if canvas.document.author.trim().is_empty() {
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

                let paste_label = ui.label("Paste a 'WOVEN' string to load:");
                ui.add(
                    egui::TextEdit::multiline(&mut dialog.pasted)
                        .font(TextStyle::Monospace)
                        .desired_width(text_width),
                )
                .labelled_by(paste_label.id);

                if ui.button("Load").clicked() {
                    match crate::formats::woven::from_woven(
                        &dialog.pasted,
                        "unknown.woven".to_string(),
                    ) {
                        Ok(doc) => loaded = Some(doc),
                        Err(e) => {
                            canvas.status.set(StatusMessage::error(format!(
                                "Error loading WOVEN puzzle: {e:#}"
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
                    let filename_label = ui.label("Filename:");
                    ui.add(
                        egui::TextEdit::singleline(&mut canvas.document.file)
                            .desired_width(text_width.min(450.0)),
                    )
                    .labelled_by(filename_label.id);
                });
                save = ui.button("Save").clicked();
            },
        );

        if save {
            let document = dialog.document_to_save(&canvas.document);
            self.save_file(document.into());
        }
        if back || !open {
            self.save_share = None;
        }
        loaded
    }

    /// Ask where to save `document`, and save it there. How that went turns up in
    /// `receive_save_result`.
    fn save_file(&mut self, mut document: UncertainDocument) {
        let (sender, receiver) = mpsc::channel();
        self.save_result_receiver = receiver;

        spawn_async(async move {
            let formats = [&READABLE_FORMATS[..], &[PRINTABLE_FORMAT]].concat();
            let handle = with_filters(rfd::AsyncFileDialog::new(), &formats)
                .set_file_name(document.file.clone())
                .save_file()
                .await;

            if let Some(handle) = handle {
                let result = async {
                    let bytes = to_bytes(&mut document, Some(handle.file_name()), None)?;
                    handle.write(&bytes).await?;
                    Ok(())
                }
                .await;
                let _ = sender.send(result); // Don't panic if it's already gone!
            }
        });
    }

    /// Report a save that went wrong. (Whether or not the Save/share window is still open: it
    /// may well have been closed while the file dialog was up.)
    fn receive_save_result(&mut self) {
        if let Ok(Err(e)) = self.save_result_receiver.try_recv() {
            self.canvas
                .status
                .set(StatusMessage::error(format!("Error saving file: {e:#}")));
        }
    }

    /// The progress towards solving the puzzle, if anything is known.
    fn solve_progress(&mut self) -> Option<PartialSolution> {
        let document = &self.canvas.document;
        let progress = document.in_progress()?;
        let palette = document.solution().palette();
        has_progress(&progress.raw, palette.keys().copied()).then(|| progress.clone())
    }

    /// Ask for a file to open. It turns up in `receive_opened_file`.
    pub(super) fn open_file(&mut self) {
        let (sender, receiver) = mpsc::channel();
        self.opened_file_receiver = receiver;

        spawn_async(async move {
            let handle = with_filters(rfd::AsyncFileDialog::new(), &READABLE_FORMATS)
                .pick_file()
                .await;

            if let Some(handle) = handle {
                // We need a picture to edit it:
                let document =
                    match crate::import::load(&handle.file_name(), handle.read().await, None) {
                        Ok(document) => document.into_document_with_deadline().await,
                        Err(e) => Err(e),
                    };

                let _ = sender.send(document); // Don't panic if it's already gone!
            }
        });
    }

    fn receive_opened_file(&mut self) {
        if let Ok(result) = self.opened_file_receiver.try_recv() {
            match result {
                Ok(document) => self.replace_document(document),
                Err(e) => {
                    self.canvas
                        .status
                        .set(StatusMessage::error(format!("Error loading file: {e:#}")));
                }
            }
        }
    }
}

/// The file types Open can read (and Save can write), as the file dialog names them.
const READABLE_FORMATS: [(&str, &[&str]); 5] = [
    ("image", &["png", "gif", "bmp"]),
    ("PBN", &["xml", "pbn"]),
    ("chargrid", &["txt"]),
    ("Olšák", &["g"]),
    ("woven", &["woven"]),
];

/// What Save can write, but Open can't read.
const PRINTABLE_FORMAT: (&str, &[&str]) = ("HTML (for printing)", &["html"]);

/// `dialog`, offering `formats`, each on its own and all of them at once (first, as the default).
fn with_filters(dialog: rfd::AsyncFileDialog, formats: &[(&str, &[&str])]) -> rfd::AsyncFileDialog {
    let all: Vec<&str> = formats
        .iter()
        .flat_map(|(_, exts)| *exts)
        .copied()
        .collect();
    formats.iter().fold(
        dialog.add_filter("all recognized formats", &all),
        |dialog, (name, exts)| dialog.add_filter(*name, exts),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A link has to survive being shared, which a line break in the middle of it wouldn't.
    #[test]
    fn a_link_is_one_unbroken_line_that_opens_the_puzzle() {
        let woven = include_str!("../../examples/woven/square_color_metadata.0.woven");
        let doc =
            crate::formats::woven::from_woven(woven.trim(), "linked.woven".to_string()).unwrap();
        let mut gui = NonogramGui::new(doc.clone());
        gui.open_save_share(true);

        let link = gui.save_share.unwrap().shared(&gui.canvas.document);
        assert!(!link.contains(char::is_whitespace), "{link:?}");
        let (_, hash) = link.split_once('#').unwrap();
        let opened = crate::formats::woven::from_woven(hash, "linked.woven".to_string()).unwrap();
        assert_eq!(opened.title, doc.title);
        assert_eq!(opened.solution().cells(), doc.solution().cells());
    }

    /// Progress that came with a puzzle opened in the editor is still there to save after the
    /// editor has drawn the picture.
    #[test]
    fn loaded_progress_survives_the_editor() {
        use crate::solve::line_solve::Cell;

        let woven = include_str!("../../examples/woven/square_bw.1.woven");
        let mut doc =
            crate::formats::woven::from_woven(woven.trim(), "test.woven".to_string()).unwrap();
        let answer = doc.solution().cells().raw.to_vec();
        let mut progress: PartialSolution = vec![Cell::new_anything(); answer.len()].into();
        progress[CellIdx(0)] = Cell::from_color(answer[0]);
        doc.set_in_progress(Some(progress));

        let mut gui = NonogramGui::new(doc);
        egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| gui.main_ui(ui))
            .drop_without_applying_deltas();
        assert!(gui.solve_progress().is_some());
    }
}
