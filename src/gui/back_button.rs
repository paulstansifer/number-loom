//! The browser's back button (and Android's): it closes whatever is open on top — a dialog, the
//! drawer — rather than leaving the app.
//!
//! There's no way to hear the back button directly. What a page can do is push entries onto the
//! browser's history, and hear about it (`popstate`) when the back button pops one off. So there's
//! one entry for each thing that's open: opening one pushes an entry, and the back button pops
//! it, which closes it. Closing one any other way (its own close button, say) steps history back
//! to match, and the `popstate` that causes is ignored.
//!
//! (A reload leaves any entries pushed before it in the history, where they no longer stand for
//! anything: pressing back over one does nothing visible.)
//!
//! Natively, there's no back button, and none of this does anything.

#[cfg(target_arch = "wasm32")]
use std::{cell::Cell, rc::Rc};

#[derive(Default)]
pub(super) struct BackButton {
    #[cfg(target_arch = "wasm32")]
    web: Option<web::Listening>,
}

impl BackButton {
    /// How many times the back button has been pressed since last asked, each of which should
    /// close the topmost thing that's open.
    pub(super) fn take_presses(&mut self, ctx: &egui::Context) -> usize {
        #[cfg(target_arch = "wasm32")]
        {
            let web = self.web.get_or_insert_with(|| web::Listening::new(ctx));
            let presses = web.presses.replace(0);
            // Each press has already taken its entry off the history.
            web.pushed = web.pushed.saturating_sub(presses);
            presses
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = ctx;
            0
        }
    }

    /// `open` things are open now: push or pop history entries to match.
    pub(super) fn sync(&mut self, open: usize) {
        #[cfg(target_arch = "wasm32")]
        if let Some(web) = &mut self.web {
            web.sync(open);
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = open;
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use super::*;
    use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};

    pub(super) struct Listening {
        /// How many history entries are this app's.
        pub(super) pushed: usize,
        /// Back presses not yet acted on.
        pub(super) presses: Rc<Cell<usize>>,
        /// `popstate`s still to come from stepping back on purpose, which aren't presses.
        expected: Rc<Cell<usize>>,
        /// Kept so the listener isn't dropped (which would leave the browser calling a dead
        /// function).
        _listener: Option<Closure<dyn FnMut()>>,
    }

    impl Listening {
        pub(super) fn new(ctx: &egui::Context) -> Listening {
            let presses = Rc::new(Cell::new(0));
            let expected = Rc::new(Cell::new(0));
            let listener = web_sys::window().map(|window| {
                let (presses, expected, ctx) = (presses.clone(), expected.clone(), ctx.clone());
                let listener = Closure::<dyn FnMut()>::new(move || {
                    if expected.get() > 0 {
                        expected.set(expected.get() - 1);
                    } else {
                        presses.set(presses.get() + 1);
                        // Nothing else would make egui draw a frame to act on it.
                        ctx.request_repaint();
                    }
                });
                let _ = window.add_event_listener_with_callback(
                    "popstate",
                    listener.as_ref().unchecked_ref(),
                );
                listener
            });
            Listening {
                pushed: 0,
                presses,
                expected,
                _listener: listener,
            }
        }

        pub(super) fn sync(&mut self, open: usize) {
            let Some(history) = web_sys::window().and_then(|window| window.history().ok()) else {
                return;
            };
            if open > self.pushed {
                for _ in self.pushed..open {
                    let _ = history.push_state(&JsValue::NULL, "");
                }
            } else if open < self.pushed {
                // One `popstate`, however far back it goes.
                self.expected.set(self.expected.get() + 1);
                let _ = history.go_with_delta(-((self.pushed - open) as i32));
            }
            self.pushed = open;
        }
    }
}
