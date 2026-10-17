//! Whether the user is working with a mouse or a touchscreen, which decides how the canvas takes
//! its input (see `touch.rs`). It follows whatever was used last, not the
//! size of the screen: a laptop can have both.

/// How the user is pointing at things right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputMode {
    /// A mouse, a trackpad or a stylus: something that can hover.
    #[default]
    Mouse,
    /// A finger.
    Touch,
}

impl InputMode {
    /// The mode after this frame's input.
    ///
    /// egui turns a touch into an imitation pointer as well, so pointer events alone can't tell
    /// the two apart. A touch event means a finger; a pointer moving with nothing pressed and
    /// nothing touching means something that can hover, which a finger can't. Anything else
    /// leaves the mode as it was.
    pub fn after(self, input: &egui::InputState) -> InputMode {
        let hovering = !input.pointer.any_down() && !input.any_touches();
        input.events.iter().fold(self, |mode, event| match event {
            egui::Event::Touch { .. } => InputMode::Touch,
            egui::Event::PointerMoved(_) if hovering => InputMode::Mouse,
            _ => mode,
        })
    }
}
