//! Saving the whole GUI as a "session", so that nothing is lost when a phone kills the app in the
//! background, and restoring it the next time the app opens. Web only, for now.
//!
//! `NonogramGui` and the structs inside it derive serde themselves. Whatever is transient (open
//! dialogs, drags in progress, cached analyses) is `#[serde(skip)]`ped and comes back as its
//! default, and `NonogramGui::reconnect` puts back the few things that can't simply default. A
//! new field has to be one or the other, or the build fails.
//!
//! Documents are saved in WOVEN's serializable form (`formats/woven.rs`), but not as share
//! strings: the whole session is compressed once, at the end.
//!
//! There's no versioning for now. A session that an older build saved and this one can't read is
//! dropped, and the user is told so.

// Saving and loading only happen on the web, but the encoding is tested natively.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use super::*;
use crate::formats::woven::{compress_and_encode, decode_and_decompress};
use std::hash::{Hash as _, Hasher as _};

/// The `localStorage` key. Every GitHub Pages site of the same user shares one origin (and so one
/// `localStorage`), hence the prefix.
const SESSION_KEY: &str = "number-loom.session";

/// Lower than WOVEN's 11: this runs on every save, rather than once per share.
const BROTLI_QUALITY: u32 = 5;

/// A `Version` that no real one will ever equal, so whatever is tagged with it is never fresh.
pub(super) fn never() -> Version {
    Version::MAX
}

/// A receiver whose sender is already gone. The dialogs make a fresh channel when they start
/// their work, so this just has to be something to put there until then.
pub(super) fn dead_receiver<T>() -> mpsc::Receiver<T> {
    mpsc::channel().1
}

/// Stand-ins for `SolveGui::clues` and `intended_solution`, which deserializing needs a value for
/// but `NonogramGui::reconnect` replaces straight away.
pub(super) fn placeholder_solution() -> DynSolution {
    DynSolution::Square(Solution::blank_bw(1, 1))
}

pub(super) fn placeholder_puzzle() -> crate::puzzle::DynPuzzle {
    placeholder_solution().to_puzzle()
}

/// `#[serde(with = "session::document")]`: a `Document` as WOVEN's serializable form, plus the
/// filename, which WOVEN leaves out.
///
/// TODO: this isn't quite an exact round trip. When a document carries `in_progress`, WOVEN
/// reserves `?`, `[` and `]` for spelling it, and `make_chs_unique` renames any palette `ch`
/// that uses one. The solver canvas's own palette always has `UNSOLVED` as `?`, so a restored
/// session's palette `ch`s can differ from the ones that were saved.
pub(super) mod document {
    use crate::formats::woven::{self, WovenDocument};
    use crate::puzzle::Document;
    use serde::{
        Deserialize, Deserializer, Serialize, Serializer, de::Error as _, ser::Error as _,
    };

    #[derive(Serialize, Deserialize)]
    struct Saved {
        file: String,
        woven: WovenDocument,
    }

    pub fn serialize<S: Serializer>(document: &Document, s: S) -> Result<S::Ok, S::Error> {
        let woven = woven::to_woven_document(document).map_err(S::Error::custom)?;
        Saved {
            file: document.file.clone(),
            woven,
        }
        .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Document, D::Error> {
        let saved = Saved::deserialize(d)?;
        let mut document = woven::from_woven_document(saved.woven).map_err(D::Error::custom)?;
        document.file = saved.file;
        Ok(document)
    }
}

/// The same, for `Action::ReplaceDocument`'s `Box<Document>`.
pub(super) mod boxed_document {
    use crate::puzzle::Document;
    use serde::{Deserializer, Serializer};

    #[allow(clippy::borrowed_box)] // `with` hands us the field as it is.
    pub fn serialize<S: Serializer>(document: &Box<Document>, s: S) -> Result<S::Ok, S::Error> {
        super::document::serialize(document, s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Box<Document>, D::Error> {
        super::document::deserialize(d).map(Box::new)
    }
}

/// How saving has gone so far, so that an unchanged session isn't written again, and a warning
/// isn't repeated on every save.
#[derive(Default)]
pub(super) struct SaveState {
    /// Whether `save_session` has run at all yet. Until it has, the restored session isn't in
    /// storage (see `load`).
    pub(super) attempted: bool,
    /// Whether a frame has run since the last save. Nothing can change without one, so when
    /// this is `false` there's no need even to serialize the session to find out.
    pub(super) dirty: bool,
    /// A fingerprint of the session last written (or last found impossible to write).
    last_saved: Option<u64>,
    /// Whether the undo history had to be left out to make the session fit.
    history_left_out: bool,
}

fn fingerprint(json: &[u8]) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    json.hash(&mut hasher);
    hasher.finish()
}

/// The session as stored: the JSON, brotli-compressed, in base-64 (`localStorage` holds strings).
pub(super) fn encode(json: &[u8]) -> anyhow::Result<String> {
    compress_and_encode(json, BROTLI_QUALITY)
}

impl NonogramGui {
    /// The counterpart to `encode`, plus putting back what the session doesn't hold.
    pub(super) fn from_session_string(saved: &str) -> anyhow::Result<NonogramGui> {
        let mut gui: NonogramGui = serde_json::from_slice(&decode_and_decompress(saved)?)?;
        gui.reconnect()?;
        Ok(gui)
    }

    /// Put back what deserializing can't. Some of it isn't saved, because it's derived from
    /// something that is, and some of it is *shared*, which a deserialized copy wouldn't be.
    fn reconnect(&mut self) -> anyhow::Result<()> {
        if let Some(solve_gui) = &mut self.solve_gui {
            // Just as `enter_solve_mode` makes them: from the editor's document, which can't
            // change while solving. (The solver canvas's own document is no good: its cells are
            // the user's progress, not the answer.)
            let mut document = self.editor_gui.document.clone();
            solve_gui.clues = document.puzzle().clone();
            solve_gui.intended_solution = document.take_solution()?;

            // One status bar and one progress bar, whichever canvas is talking.
            solve_gui.canvas.status = Rc::clone(&self.editor_gui.status);
            solve_gui.canvas.progress = Rc::clone(&self.editor_gui.progress);
        }
        Ok(())
    }

    /// Run `f` on this GUI with both canvases' undo and redo stacks emptied, then put them back.
    fn without_history<R>(&mut self, f: impl FnOnce(&Self) -> R) -> R {
        use std::mem::take;
        let editor = (
            take(&mut self.editor_gui.undo_stack),
            take(&mut self.editor_gui.redo_stack),
        );
        let solver = self.solve_gui.as_mut().map(|solve_gui| {
            (
                take(&mut solve_gui.canvas.undo_stack),
                take(&mut solve_gui.canvas.redo_stack),
            )
        });

        let result = f(self);

        (self.editor_gui.undo_stack, self.editor_gui.redo_stack) = editor;
        if let (Some(solve_gui), Some(stacks)) = (&mut self.solve_gui, solver) {
            (solve_gui.canvas.undo_stack, solve_gui.canvas.redo_stack) = stacks;
        }
        result
    }
}

#[cfg(target_arch = "wasm32")]
use gloo_storage::{LocalStorage, Storage as _};

/// The session saved last time, if there is one.
///
/// It comes out of storage as it's loaded, and only goes back in with the first save, which
/// `eframe::App::update` makes as soon as a frame has been drawn. A session this build can't
/// handle might panic rather than fail politely (and on the web, a panic can't be caught), and
/// left in storage it would do that again on every load, leaving the app unable to start until
/// the user cleared the site's data. This way the next load starts afresh.
#[cfg(target_arch = "wasm32")]
pub(super) fn load() -> Option<anyhow::Result<NonogramGui>> {
    let saved = LocalStorage::raw().get_item(SESSION_KEY).ok().flatten()?;
    LocalStorage::delete(SESSION_KEY);
    Some(NonogramGui::from_session_string(&saved))
}

#[cfg(target_arch = "wasm32")]
fn store(json: &[u8]) -> anyhow::Result<()> {
    LocalStorage::raw()
        .set_item(SESSION_KEY, &encode(json)?)
        .map_err(|e| anyhow::anyhow!("{e:?}"))
}

#[cfg(target_arch = "wasm32")]
impl NonogramGui {
    pub(super) fn save_session(&mut self) {
        if self.saving.attempted && !self.saving.dirty {
            return;
        }
        self.saving.attempted = true;
        self.saving.dirty = false;

        let json = match serde_json::to_vec(self) {
            Ok(json) => json,
            Err(e) => {
                self.editor_gui
                    .status
                    .set(StatusMessage::error(format!("Unable to save: {e}")));
                return;
            }
        };
        let fingerprint = fingerprint(&json);
        if self.saving.last_saved == Some(fingerprint) {
            return;
        }
        // Whatever happens, don't try (and fail, and complain) again until something changes.
        self.saving.last_saved = Some(fingerprint);

        let Err(save_err) = store(&json) else {
            self.saving.history_left_out = false;
            return;
        };

        // Most likely out of room. The undo history is the part that grows without limit, so try
        // again without it.
        let without = self
            .without_history(|gui| serde_json::to_vec(gui))
            .map_err(anyhow::Error::from)
            .and_then(|json| store(&json));
        match without {
            Ok(()) => {
                if !self.saving.history_left_out {
                    self.editor_gui.status.set(StatusMessage::error(
                        "Undo history is very large; omitted from save file.",
                    ));
                }
                self.saving.history_left_out = true;
            }
            Err(_) => {
                self.editor_gui
                    .status
                    .set(StatusMessage::error(format!("Unable to save: {save_err}")));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::LaneIdx;

    fn round_trip(gui: &NonogramGui) -> NonogramGui {
        let json = serde_json::to_vec(gui).unwrap();
        NonogramGui::from_session_string(&encode(&json).unwrap()).unwrap()
    }

    fn cells(canvas: &CanvasGui) -> Vec<Color> {
        canvas.document.try_solution().unwrap().cells().raw.to_vec()
    }

    fn paint(canvas: &mut CanvasGui, cells: &[u32], color: Color) {
        canvas.perform(
            Action::ChangeColor {
                changes: cells.iter().map(|c| (CellIdx(*c), color)).collect(),
            },
            ActionMood::Normal,
        );
    }

    /// A 5×5 whose top row is filled: its clues have exactly one answer, so it can be solved.
    fn top_row() -> Document {
        let mut solution = Solution::blank_bw(5, 5);
        for x in 0..5 {
            solution.cells[CellIdx(x)] = Color(1);
        }
        Document::from_solution(DynSolution::Square(solution), "top-row.png".to_string())
    }

    /// Paint, then swap in a whole new (bigger) document, as a resize does. After a round trip,
    /// the picture, the filename and the undo stack are all intact, and undoing both steps lands
    /// in the same place as it does in the original.
    #[test]
    fn the_editor_and_its_history_survive() {
        let mut gui = NonogramGui::new(Document::from_solution(
            DynSolution::Square(Solution::blank_bw(5, 5)),
            "test.png".to_string(),
        ));
        paint(&mut gui.editor_gui, &[0, 6, 12], Color(1));
        let bigger = Document::from_solution(
            DynSolution::Square(Solution::blank_bw(7, 6)),
            "test.png".to_string(),
        );
        gui.editor_gui.perform(
            Action::ReplaceDocument {
                document: Box::new(bigger),
            },
            ActionMood::Normal,
        );
        paint(&mut gui.editor_gui, &[3], Color(1));

        let mut restored = round_trip(&gui);
        assert_eq!(cells(&restored.editor_gui), cells(&gui.editor_gui));
        assert_eq!(restored.editor_gui.document.file, "test.png");
        assert_eq!(restored.editor_gui.undo_stack.len(), 3);

        for _ in 0..3 {
            gui.editor_gui.un_or_re_do(true);
            restored.editor_gui.un_or_re_do(true);
            assert_eq!(cells(&restored.editor_gui), cells(&gui.editor_gui));
        }
        assert_eq!(cells(&restored.editor_gui), vec![BACKGROUND; 25]);

        // And redo, which has its own stack.
        restored.editor_gui.un_or_re_do(false);
        assert_eq!(
            cells(&restored.editor_gui)[..13]
                .iter()
                .filter(|c| **c == Color(1))
                .count(),
            3
        );
    }

    /// Partway through a solve: the progress (unknown cells included), a checked-off clue, the
    /// clues and the answer all come back, and the solver's canvas shares the editor's status
    /// bar again.
    #[test]
    fn a_solve_in_progress_survives() {
        let mut gui = NonogramGui::new_solving(top_row());
        let solve_gui = gui.solve_gui.as_mut().unwrap();
        paint(&mut solve_gui.canvas, &[0, 1], Color(1));
        paint(&mut solve_gui.canvas, &[5], BACKGROUND);
        solve_gui.canvas.perform(
            Action::ToggleClue {
                clue: (LaneIdx(0), 0),
            },
            ActionMood::Normal,
        );

        let restored = round_trip(&gui);
        let (before, after) = (
            gui.solve_gui.as_ref().unwrap(),
            restored.solve_gui.as_ref().unwrap(),
        );
        assert_eq!(cells(&after.canvas), cells(&before.canvas));
        assert!(cells(&after.canvas).contains(&UNSOLVED));
        assert_eq!(after.canvas.checked_clues, before.canvas.checked_clues);
        assert_eq!(after.canvas.undo_stack.len(), 3);
        assert_eq!(after.clues, before.clues);
        assert_eq!(after.intended_solution, before.intended_solution);
        assert!(after.canvas.solving);

        assert!(Rc::ptr_eq(
            &after.canvas.status,
            &restored.editor_gui.status
        ));
        assert!(Rc::ptr_eq(
            &after.canvas.progress,
            &restored.editor_gui.progress
        ));
        after.canvas.status.set(StatusMessage::info("hello"));
        assert_eq!(restored.editor_gui.status.get().unwrap().text, "hello");
    }

    /// What `save_session` falls back to when storage is full: the session without undo or redo
    /// history. The history in memory is untouched.
    #[test]
    fn history_can_be_left_out() {
        let mut gui = NonogramGui::new_solving(top_row());
        paint(&mut gui.editor_gui, &[24], Color(1));
        paint(&mut gui.solve_gui.as_mut().unwrap().canvas, &[0], Color(1));
        gui.solve_gui.as_mut().unwrap().canvas.un_or_re_do(true);

        let json = gui.without_history(|gui| serde_json::to_vec(gui).unwrap());
        let restored = NonogramGui::from_session_string(&encode(&json).unwrap()).unwrap();
        assert!(restored.editor_gui.undo_stack.is_empty());
        let solver = &restored.solve_gui.as_ref().unwrap().canvas;
        assert!(solver.undo_stack.is_empty() && solver.redo_stack.is_empty());
        assert_eq!(cells(&restored.editor_gui), cells(&gui.editor_gui));

        assert_eq!(gui.editor_gui.undo_stack.len(), 1);
        assert_eq!(gui.solve_gui.as_ref().unwrap().canvas.redo_stack.len(), 1);
    }

    /// A link opened over a restored session doesn't throw the session away: the earlier puzzle
    /// is one undo away in the editor (though its solving progress isn't).
    #[test]
    fn a_link_leaves_the_earlier_puzzle_behind_undo() {
        let mut gui = round_trip(&NonogramGui::new_solving(top_row()));
        paint(
            &mut gui.solve_gui.as_mut().unwrap().canvas,
            &[0, 1],
            Color(1),
        );

        let mut left_column = Solution::blank_bw(5, 5);
        for y in 0..5 {
            left_column.cells[CellIdx(y * 5)] = Color(1);
        }
        let left_column = DynSolution::Square(left_column);
        let link = Document::from_solution(left_column.clone(), "linked.woven".to_string());
        assert!(gui.open_linked(link));

        let solver = &gui.solve_gui.as_ref().unwrap().canvas;
        assert!(cells(solver).iter().all(|c| *c == UNSOLVED));
        assert_eq!(
            gui.solve_gui.as_ref().unwrap().intended_solution,
            left_column
        );

        gui.exit_solve_mode();
        gui.editor_gui.un_or_re_do(true);
        assert_eq!(
            gui.editor_gui.document.try_solution().unwrap(),
            top_row().try_solution().unwrap()
        );
        gui.editor_gui.un_or_re_do(false);
        assert_eq!(
            gui.editor_gui.document.try_solution().unwrap(),
            &left_column
        );
    }

    #[test]
    fn a_damaged_session_is_an_error_not_a_panic() {
        assert!(NonogramGui::from_session_string("not base-64 at all!").is_err());
        assert!(
            NonogramGui::from_session_string(&encode(b"{\"not\": \"a session\"}").unwrap())
                .is_err()
        );
        assert!(NonogramGui::from_session_string("").is_err());
    }
}
