//! The touch cursor: how the canvas takes input from a finger.
//!
//! A fingertip hides whatever it's pointing at, and it's far blunter than a cell a few
//! millimetres across. So in touch mode the canvas doesn't care where the finger *is*, only how it
//! *moves*: a cursor on the grid follows the finger's movements the way a trackpad's pointer does,
//! from wherever on the canvas the finger happens to be, and the tools act at the cursor.
//!
//! - **Drag:** moves the cursor, smoothly rather than a cell at a time. When the finger lifts,
//!   the cursor settles into the middle of whichever cell it's in.
//! - **Tap:** a click of the current tool at the cursor.
//! - **Hold still, then drag:** the tool goes down at the cursor, and is dragged along with it
//!   until the finger lifts.
//! - **A second finger:** abandons the gesture, along with any stroke it had started. (Two fingers
//!   are for zooming.)
//!
//! `Gesture` is the state machine, which knows nothing of egui's input or of the grid;
//! `CanvasGui::touch_frame` connects it to both.

use super::annotate::AnnotatePointer;
use super::selection::LassoPointer;
use super::tools::{PaintIntent, PaintPointer};
use super::*;

// Tunable numbers, which want settling on a real phone.

/// How far a finger may wander, in points, and still count as staying put (for a tap or a hold).
const TAP_SLOP: f32 = 8.0;
/// How long a finger has to stay put, in seconds, before it puts the tool down.
const HOLD_DELAY: f64 = 0.35;
/// How far the cursor moves for each point the finger moves. At 1, it keeps pace with the finger
/// on screen.
const CURSOR_GAIN: f32 = 1.0;

/// How far inside the picture's far edges the cursor stops, in abstract units: the edge itself
/// belongs to no cell.
const EDGE: f32 = 1e-3;

/// Where a finger's gesture has got to.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum Gesture {
    /// No finger down, or none that went down on the canvas.
    #[default]
    Idle,
    /// A finger is down and hasn't strayed: this could still turn out to be a tap, a hold or a
    /// drag.
    Undecided { start: Pos2, since: f64 },
    /// The finger strayed before the hold delay was up, so it's moving the cursor.
    Steering { last: Pos2 },
    /// The finger stayed put long enough to put the tool down, and is dragging it along.
    Holding { last: Pos2 },
    /// A second finger arrived. Nothing more happens until they've all been lifted.
    Abandoned,
}

/// What the canvas should do about a gesture, in the order the fields are listed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Step {
    /// Abandon the stroke in progress, and take back whatever it did.
    pub cancel: bool,
    /// Put the tool down at the cursor.
    pub press: bool,
    /// Move the cursor this far, in points (and the tool with it, if it's down).
    pub moved: Vec2,
    /// Lift the tool. A tap is a press and a release in the same step.
    pub release: bool,
    /// Settle the cursor into the middle of its cell.
    pub settle: bool,
}

impl Step {
    /// This step, followed by `later`.
    fn and(self, later: Step) -> Step {
        Step {
            cancel: self.cancel || later.cancel,
            press: self.press || later.press,
            moved: self.moved + later.moved,
            release: self.release || later.release,
            settle: self.settle || later.settle,
        }
    }
}

impl Gesture {
    /// A finger went down on the canvas.
    pub(super) fn down(&mut self, at: Pos2, time: f64) {
        if *self == Gesture::Idle {
            *self = Gesture::Undecided {
                start: at,
                since: time,
            };
        }
    }

    /// When the hold delay will be up, if a finger is waiting on it. (egui only runs a frame when
    /// something happens, and a finger holding still isn't something happening.)
    pub(super) fn hold_due(&self) -> Option<f64> {
        match self {
            Gesture::Undecided { since, .. } => Some(since + HOLD_DELAY),
            _ => None,
        }
    }

    /// Time passing, with the finger down.
    pub(super) fn tick(&mut self, time: f64) -> Step {
        match *self {
            Gesture::Undecided { start, since } if time - since >= HOLD_DELAY => {
                *self = Gesture::Holding { last: start };
                Step {
                    press: true,
                    ..Step::default()
                }
            }
            _ => Step::default(),
        }
    }

    /// The finger is at `to`.
    pub(super) fn moved(&mut self, to: Pos2, time: f64) -> Step {
        // If the hold delay ran out before this movement, the tool went down first.
        let held = self.tick(time);
        let step = match *self {
            Gesture::Undecided { start, .. } if (to - start).length() > TAP_SLOP => {
                *self = Gesture::Steering { last: to };
                // Counted from where the finger went down, so the slop isn't lost.
                Step {
                    moved: to - start,
                    ..Step::default()
                }
            }
            Gesture::Steering { last } => {
                *self = Gesture::Steering { last: to };
                Step {
                    moved: to - last,
                    ..Step::default()
                }
            }
            Gesture::Holding { last } => {
                *self = Gesture::Holding { last: to };
                Step {
                    moved: to - last,
                    ..Step::default()
                }
            }
            _ => Step::default(),
        };
        held.and(step)
    }

    /// The finger lifted.
    pub(super) fn up(&mut self, time: f64) -> Step {
        let held = self.tick(time);
        let step = match *self {
            // Never strayed, and lifted before the hold delay was up: a tap.
            Gesture::Undecided { .. } => Step {
                press: true,
                release: true,
                ..Step::default()
            },
            Gesture::Steering { .. } | Gesture::Abandoned => Step {
                settle: true,
                ..Step::default()
            },
            Gesture::Holding { .. } => Step {
                release: true,
                settle: true,
                ..Step::default()
            },
            Gesture::Idle => Step::default(),
        };
        *self = Gesture::Idle;
        held.and(step)
    }

    /// Another finger is down too.
    pub(super) fn second_finger(&mut self) -> Step {
        let step = match self {
            Gesture::Holding { .. } => Step {
                cancel: true,
                ..Step::default()
            },
            _ => Step::default(),
        };
        if *self != Gesture::Idle {
            *self = Gesture::Abandoned;
        }
        step
    }
}

/// One frame of finger input: as egui reports it (`from_egui`), or as a test makes it up.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct TouchSample {
    /// Where a finger went down this frame, if one did somewhere the canvas answers to.
    pub pressed_at: Option<Pos2>,
    /// Whether a finger is down as the frame ends.
    pub down: bool,
    /// Where the finger is (or was last).
    pub pos: Option<Pos2>,
    /// Whether more than one finger is down.
    pub several: bool,
    pub time: f64,
}

impl TouchSample {
    /// egui reports a finger through the same pointer a mouse uses, so that's where this reads
    /// it from. `area` is where the canvas takes touches from.
    pub(super) fn from_egui(input: &egui::InputState, area: Rect) -> TouchSample {
        let pointer = &input.pointer;
        TouchSample {
            pressed_at: pointer
                .any_pressed()
                .then(|| pointer.press_origin())
                .flatten()
                .filter(|at| area.contains(*at)),
            // When the browser cancels a touch (`touchcancel`), eframe tells egui about the touch
            // but never lets go of the pointer, so ask both.
            down: pointer.any_down() && input.any_touches(),
            pos: pointer.latest_pos(),
            several: input.multi_touch().is_some(),
            time: input.time,
        }
    }
}

/// A canvas's touch cursor, and the gesture steering it. Not saved with the session.
#[derive(Default)]
pub(crate) struct TouchState {
    pub(super) gesture: Gesture,
    /// Where the cursor is, in abstract units: always on a cell, once it has been placed. It
    /// moves smoothly, and is drawn where it is (see `touch_cursor_outline`), but the painting
    /// tools act on whichever cell it's in, and it settles into the middle of that cell when the
    /// finger lifts.
    cursor: Option<Point>,
    /// Whether the tool has moved since it went down. A hold that's let go of without moving is
    /// still a click.
    dragged: bool,
    /// How long the undo stack was when the tool went down, so that a cancelled stroke can be
    /// taken back.
    undo_len_at_press: usize,
}

/// What the tool is told, for a press, a drag or a release.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Contact {
    Press,
    Drag,
    Release,
}

fn cell_center(picture: &DynSolution, cell: CellIdx) -> Point {
    picture.cell_shape(cell).center(picture.cell_origin(cell))
}

/// How far from the middle of `cell` to draw its outline, for a cursor at `at` (somewhere in
/// `cell`): as far as `at` is, except that the outline doesn't leave the picture's bounds.
fn outline_offset(picture: &DynSolution, cell: CellIdx, at: Point) -> (f32, f32) {
    let (corners, n) = picture.cell_shape(cell).vertices(picture.cell_origin(cell));
    let corners = &corners[..n];
    let center = cell_center(picture, cell);
    let extent = picture.extent();
    let low = |pick: fn(&Point) -> f32| corners.iter().map(pick).fold(f32::INFINITY, f32::min);
    let high = |pick: fn(&Point) -> f32| corners.iter().map(pick).fold(f32::NEG_INFINITY, f32::max);
    // The cell itself is inside the picture, so these ranges always contain 0.
    (
        (at.x - center.x).clamp(-low(|p| p.x), extent.x - high(|p| p.x)),
        (at.y - center.y).clamp(-low(|p| p.y), extent.y - high(|p| p.y)),
    )
}

/// A short buzz, where the device can do that, to say that a hold has put the tool down.
fn buzz() {
    // Not `Navigator::vibrate_with_duration`: in a browser with no `vibrate` (Firefox and Safari
    // have none), that throws, and an exception thrown out through eframe leaves it unable to
    // handle any event ever again. `Function::call1` catches it instead.
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast as _;
        let Some(navigator) = web_sys::window().map(|window| window.navigator()) else {
            return;
        };
        if let Ok(vibrate) = js_sys::Reflect::get(&navigator, &"vibrate".into())
            && let Ok(vibrate) = vibrate.dyn_into::<js_sys::Function>()
        {
            let _ = vibrate.call1(&navigator, &15.into());
        }
    }
}

impl CanvasGui {
    /// The canvas's input in touch mode, in place of everything it does with a mouse. `area` is
    /// where it takes touches from. Returns the hovered cell, which in touch mode is the
    /// cursor's.
    pub(super) fn touch_input(
        &mut self,
        ui: &mut egui::Ui,
        area: Rect,
        to_screen: &egui::emath::RectTransform,
        tool: Tool,
    ) -> Option<CellIdx> {
        let mut sample = ui.input(|i| TouchSample::from_egui(i, area));
        // A press on something floating over the canvas (the menu, a dialog, a window) belongs
        // to that, not to the cursor. The panels, the canvas's among them, are all in the
        // background.
        sample.pressed_at = sample.pressed_at.filter(|at| {
            ui.ctx()
                .layer_id_at(*at)
                .is_none_or(|layer| layer.order == egui::Order::Background)
        });
        let where_was = |gui: &CanvasGui| gui.touch.cursor.map(|p| (p.x, p.y));
        let before = where_was(self);
        let step = self.touch_frame(sample, to_screen.scale().x, tool);

        // A finger steering the cursor doesn't scroll anything, so keep the cursor in view, with
        // a cell to spare, whenever it moves (not when the view does: two fingers can pan away
        // from it). Scrolling only happens once it's that close to the edge.
        if where_was(self) != before
            && let Some(rect) = self.touch_cursor_rect(to_screen)
        {
            ui.scroll_to_rect(rect.expand(to_screen.scale().x), None);
        }
        if let Some(due) = self.touch.gesture.hold_due() {
            ui.ctx()
                .request_repaint_after(Duration::from_secs_f64((due - sample.time).max(0.0)));
        }
        if step.press && !step.release {
            buzz();
        }
        if tool == Tool::Lasso {
            self.lasso_keys(ui);
        }
        self.touch_cursor()
    }

    /// The cell the touch cursor is on, if it's anywhere.
    pub(super) fn touch_cursor(&self) -> Option<CellIdx> {
        self.document.solution().cell_containing(self.touch.cursor?)
    }

    /// As `touch_cursor`, but first putting the cursor in the middle of the picture if it isn't
    /// on a cell: it hasn't been placed yet, or the picture has changed under it.
    fn place_touch_cursor(&mut self) -> Option<CellIdx> {
        if let Some(cell) = self.touch_cursor() {
            return Some(cell);
        }
        let picture = self.document.solution();
        let extent = picture.extent();
        let cell = picture
            .cell_containing(Point::new(extent.x / 2.0, extent.y / 2.0))
            .or((!picture.cells().is_empty()).then_some(CellIdx(0)))?;
        self.touch.cursor = Some(cell_center(picture, cell));
        Some(cell)
    }

    /// One frame of touch input. `points_per_unit` is the canvas's scale; `tool` is the tool in
    /// hand. Returns what the gesture did, after doing it.
    pub(super) fn touch_frame(
        &mut self,
        sample: TouchSample,
        points_per_unit: f32,
        tool: Tool,
    ) -> Step {
        if self.place_touch_cursor().is_none() {
            return Step::default();
        }

        let gesture = &mut self.touch.gesture;
        if let Some(at) = sample.pressed_at {
            gesture.down(at, sample.time);
        }
        let mut step = Step::default();
        if sample.several {
            step = step.and(gesture.second_finger());
        }
        step = step.and(match (sample.down, sample.pos) {
            (true, Some(pos)) => gesture.moved(pos, sample.time),
            (true, None) => gesture.tick(sample.time),
            (false, _) => gesture.up(sample.time),
        });

        if step.cancel {
            self.cancel_touch_stroke(tool);
        }
        if step.press {
            self.touch.dragged = false;
            self.touch.undo_len_at_press = self.undo_stack.len();
            self.touch_tool(tool, Contact::Press);
        }
        if step.moved != Vec2::ZERO
            && self.move_touch_cursor(step.moved * (CURSOR_GAIN / points_per_unit))
            && matches!(self.touch.gesture, Gesture::Holding { .. })
        {
            self.touch.dragged = true;
            self.touch_tool(tool, Contact::Drag);
        }
        if step.release {
            self.touch_tool(tool, Contact::Release);
        }
        if step.settle
            && let Some(cell) = self.touch_cursor()
        {
            self.touch.cursor = Some(cell_center(self.document.solution(), cell));
        }
        step
    }

    /// Move the cursor by `delta` abstract units, as far as the grid allows. Returns whether it
    /// moved at all.
    fn move_touch_cursor(&mut self, delta: Vec2) -> bool {
        let picture = self.document.solution();
        let Some(from) = self.touch.cursor else {
            return false;
        };
        // Pushed past the edge of the picture, the cursor stops at it.
        let extent = picture.extent();
        let to = Point::new(
            (from.x + delta.x).clamp(0.0, extent.x - EDGE),
            (from.y + delta.y).clamp(0.0, extent.y - EDGE),
        );
        // Where that still isn't on a cell (around the corners of a triddler), slide along the
        // edge rather than stopping dead: try each direction of the move alone.
        let landing = [to, Point::new(to.x, from.y), Point::new(from.x, to.y)]
            .into_iter()
            .find_map(|p| Some((p, picture.cell_containing(p)?)))
            // And no further than the cursor as it's drawn goes (see `outline_offset`), so that
            // coming back from the edge moves it straight away.
            .map(|(p, cell)| {
                let center = cell_center(picture, cell);
                let (dx, dy) = outline_offset(picture, cell, p);
                let kept = Point::new(center.x + dx, center.y + dy);
                if picture.cell_containing(kept) == Some(cell) {
                    kept
                } else {
                    p
                }
            });
        match landing {
            Some(p) if (p.x, p.y) != (from.x, from.y) => {
                self.touch.cursor = Some(p);
                true
            }
            _ => false,
        }
    }

    /// Drive the tool at the cursor, as a mouse would drive it at the pointer.
    fn touch_tool(&mut self, tool: Tool, contact: Contact) {
        let Some(at) = self.touch.cursor else {
            return;
        };
        let (pressed, down, released) = match contact {
            Contact::Press => (true, true, false),
            Contact::Drag => (false, true, false),
            Contact::Release => (false, false, true),
        };
        match tool {
            Tool::Lasso => self.lasso_input(
                LassoPointer {
                    pressed,
                    down,
                    released,
                },
                at,
            ),
            Tool::Annotate => self.annotate_input(
                AnnotatePointer {
                    pressed,
                    down,
                    released,
                    dragging: self.touch.dragged,
                },
                at,
            ),
            Tool::Pencil | Tool::LineAlongLane | Tool::FloodFill => {
                if let Some(cell) = self.touch_cursor() {
                    let pointer = PaintPointer {
                        pressed,
                        down,
                        released,
                        clicked: released && !self.touch.dragged,
                        // There's no second button to rule a cell out with; picking the
                        // background color from the palette is how a finger does that.
                        intent: PaintIntent::Current,
                    };
                    self.pointer_tool_input(cell, pointer, tool);
                }
            }
        }
    }

    /// A second finger arrived mid-stroke: let go of the tool, and take back what the stroke did.
    ///
    /// Except for the lasso, which is only let go of: taking back a lift would leave the
    /// selection floating over cells it no longer came from.
    fn cancel_touch_stroke(&mut self, tool: Tool) {
        if tool == Tool::Annotate {
            self.annotate_drag = None;
        }
        // Not a click, however little the tool moved.
        self.touch.dragged = true;
        self.touch_tool(tool, Contact::Release);
        if tool != Tool::Lasso && self.undo_stack.len() > self.touch.undo_len_at_press {
            self.un_or_re_do(true);
            // Nor is it something to redo, as though the user had chosen to undo it.
            self.redo_stack.pop();
        }
    }

    /// The cursor, outlining its cell: in a color that the palette doesn't use, so that it can't
    /// be mistaken for a cell, inside a halo of black or white (whichever sets it off better).
    pub(super) fn touch_cursor_shapes(
        &self,
        to_screen: &egui::emath::RectTransform,
    ) -> Vec<egui::Shape> {
        let picture = self.document.solution();
        let ((r, g, b), _) = super::palette::suggested_color(picture.palette());
        let luma = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
        let halo = if luma > 150.0 {
            Color32::BLACK
        } else {
            Color32::WHITE
        };
        let points: Vec<Pos2> = self
            .touch_cursor_outline()
            .into_iter()
            .map(|p| to_screen * Pos2::new(p.x, p.y))
            .collect();
        if points.is_empty() {
            return vec![];
        }
        vec![
            egui::Shape::closed_line(points.clone(), egui::Stroke::new(5.0, halo)),
            egui::Shape::closed_line(points, egui::Stroke::new(2.5, Color32::from_rgb(r, g, b))),
        ]
    }

    /// Where on screen the cursor is, for keeping it in view.
    pub(super) fn touch_cursor_rect(&self, to_screen: &egui::emath::RectTransform) -> Option<Rect> {
        let points: Vec<Pos2> = self
            .touch_cursor_outline()
            .into_iter()
            .map(|p| to_screen * Pos2::new(p.x, p.y))
            .collect();
        (!points.is_empty()).then(|| Rect::from_points(&points))
    }

    /// The cursor as it's drawn: the outline of the cell it's in, but centered on where it really
    /// is, which while a finger is moving it is anywhere at all (and is the middle of a cell
    /// again once it settles). Pushed back inside the picture where it would stick out of it.
    /// In abstract units; empty if there's no cursor.
    fn touch_cursor_outline(&self) -> Vec<Point> {
        let picture = self.document.solution();
        let (Some(cell), Some(at)) = (self.touch_cursor(), self.touch.cursor) else {
            return vec![];
        };
        let (corners, n) = picture.cell_shape(cell).vertices(picture.cell_origin(cell));
        let (dx, dy) = outline_offset(picture, cell, at);
        corners[..n]
            .iter()
            .map(|p| Point::new(p.x + dx, p.y + dy))
            .collect()
    }
}

#[cfg(test)]
mod gesture_tests {
    use super::*;

    fn at(x: f32, y: f32) -> Pos2 {
        Pos2::new(x, y)
    }

    #[test]
    fn a_tap() {
        let mut g = Gesture::default();
        g.down(at(100.0, 100.0), 0.0);
        assert_eq!(g.moved(at(100.0, 100.0), 0.05), Step::default());
        let step = g.up(0.1);
        assert!(step.press && step.release);
        assert_eq!(step.moved, Vec2::ZERO);
        assert_eq!(g, Gesture::Idle);
    }

    /// A finger never lands perfectly still.
    #[test]
    fn a_shaky_tap_is_still_a_tap() {
        let mut g = Gesture::default();
        g.down(at(100.0, 100.0), 0.0);
        assert_eq!(g.moved(at(104.0, 97.0), 0.05), Step::default());
        let step = g.up(0.1);
        assert!(step.press && step.release);
    }

    /// Movement past the slop steers the cursor (counting the slop too), and nothing is pressed.
    #[test]
    fn a_drag_steers() {
        let mut g = Gesture::default();
        g.down(at(100.0, 100.0), 0.0);
        assert_eq!(g.moved(at(120.0, 100.0), 0.05).moved, Vec2::new(20.0, 0.0));
        assert_eq!(g.moved(at(130.0, 95.0), 0.5).moved, Vec2::new(10.0, -5.0));
        let step = g.up(0.6);
        assert_eq!(
            step,
            Step {
                settle: true,
                ..Step::default()
            }
        );
    }

    #[test]
    fn a_hold_then_a_drag() {
        let mut g = Gesture::default();
        g.down(at(100.0, 100.0), 0.0);
        assert_eq!(g.hold_due(), Some(HOLD_DELAY));
        assert_eq!(g.tick(0.2), Step::default());
        assert!(g.tick(0.4).press);
        assert_eq!(g.hold_due(), None);
        let step = g.moved(at(100.0, 130.0), 0.5);
        assert!(!step.press);
        assert_eq!(step.moved, Vec2::new(0.0, 30.0));
        let step = g.up(0.6);
        assert!(step.release && step.settle && !step.press);
    }

    /// Held, then lifted without moving: the tool went down at the hold, and comes up now. (The
    /// canvas treats that as a click.)
    #[test]
    fn a_hold_without_moving() {
        let mut g = Gesture::default();
        g.down(at(100.0, 100.0), 0.0);
        assert!(g.tick(0.5).press);
        let step = g.up(0.9);
        assert!(step.release && !step.press);
    }

    /// No frame ran while the finger was holding still, so the hold only shows up with the next
    /// movement: the press still comes before it.
    #[test]
    fn a_hold_noticed_late() {
        let mut g = Gesture::default();
        g.down(at(100.0, 100.0), 0.0);
        let step = g.moved(at(120.0, 100.0), 0.5);
        assert!(step.press);
        assert_eq!(step.moved, Vec2::new(20.0, 0.0));
        assert!(matches!(g, Gesture::Holding { .. }));
    }

    #[test]
    fn a_second_finger_cancels_a_hold() {
        let mut g = Gesture::default();
        g.down(at(100.0, 100.0), 0.0);
        g.tick(0.5);
        g.moved(at(110.0, 100.0), 0.6);
        assert!(g.second_finger().cancel);
        assert_eq!(g.moved(at(150.0, 100.0), 0.7), Step::default());
        let step = g.up(0.8);
        assert!(!step.release && step.settle);
    }

    /// Before the tool is down, a second finger just means the first one's tap or drag is off.
    #[test]
    fn a_second_finger_spoils_a_tap() {
        let mut g = Gesture::default();
        g.down(at(100.0, 100.0), 0.0);
        assert_eq!(g.second_finger(), Step::default());
        let step = g.up(0.1);
        assert!(!step.press && !step.release);
    }
}

#[cfg(test)]
mod canvas_tests {
    use super::*;
    use crate::puzzle::Solution;

    /// Points per cell edge, for these tests.
    const SCALE: f32 = 20.0;

    fn at(x: usize, y: usize) -> CellIdx {
        CellIdx((y * 5 + x) as u32)
    }

    fn editor(tool: Tool) -> CanvasGui {
        let mut gui = NonogramGui::new(Document::from_solution(
            DynSolution::Square(Solution::blank_bw(5, 5)),
            "test".to_string(),
        ))
        .canvas;
        gui.current_tool = tool;
        gui.current_color = Color(1);
        gui
    }

    fn cell(gui: &CanvasGui, cell: CellIdx) -> Color {
        gui.cell(cell)
    }

    fn painted(gui: &CanvasGui) -> Vec<CellIdx> {
        gui.document
            .solution()
            .cells()
            .iter_enumerated()
            .filter(|(_, c)| **c == Color(1))
            .map(|(i, _)| i)
            .collect()
    }

    /// A finger, somewhere on the canvas (but nowhere in particular: the cursor doesn't care).
    struct Finger {
        pos: Pos2,
        time: f64,
    }

    impl Finger {
        fn down(gui: &mut CanvasGui) -> Finger {
            let finger = Finger {
                pos: Pos2::new(300.0, 400.0),
                time: 10.0,
            };
            finger.frame(gui, true, true);
            finger
        }

        fn frame(&self, gui: &mut CanvasGui, pressed: bool, down: bool) -> Step {
            let tool = gui.current_tool;
            gui.touch_frame(
                TouchSample {
                    pressed_at: pressed.then_some(self.pos),
                    down,
                    pos: Some(self.pos),
                    several: false,
                    time: self.time,
                },
                SCALE,
                tool,
            )
        }

        /// Move by `cells` cell edges, in a few frames.
        fn slide(&mut self, gui: &mut CanvasGui, cells: (f32, f32)) {
            for _ in 0..4 {
                self.pos += Vec2::new(cells.0, cells.1) * SCALE / 4.0;
                self.time += 0.02;
                self.frame(gui, false, true);
            }
        }

        fn hold(&mut self, gui: &mut CanvasGui) {
            self.time += HOLD_DELAY + 0.05;
            assert!(self.frame(gui, false, true).press);
        }

        fn lift(mut self, gui: &mut CanvasGui) {
            self.time += 0.02;
            self.frame(gui, false, false);
        }
    }

    fn tap(gui: &mut CanvasGui) {
        Finger::down(gui).lift(gui);
    }

    /// The cursor starts in the middle of the picture, and a tap paints there.
    #[test]
    fn a_tap_paints_at_the_cursor() {
        let mut gui = editor(Tool::Pencil);
        tap(&mut gui);
        assert_eq!(painted(&gui), vec![at(2, 2)]);
        tap(&mut gui);
        assert_eq!(painted(&gui), vec![], "a second tap takes it back off");
    }

    /// A drag moves the cursor by however far the finger went, wherever the finger is, and paints
    /// nothing; then the cursor settles into the middle of its cell.
    #[test]
    fn a_drag_moves_the_cursor() {
        let mut gui = editor(Tool::Pencil);
        let mut finger = Finger::down(&mut gui);
        finger.slide(&mut gui, (2.0, -1.0));
        finger.lift(&mut gui);
        assert_eq!(gui.touch_cursor(), Some(at(4, 1)));
        let cursor = gui.touch.cursor.unwrap();
        assert_eq!((cursor.x, cursor.y), (4.5, 1.5));
        assert!(painted(&gui).is_empty());

        tap(&mut gui);
        assert_eq!(painted(&gui), vec![at(4, 1)]);
    }

    /// While the finger moves it, the cursor is drawn wherever it really is, not on the cell it's
    /// in; it's only once the finger lifts that it snaps to a cell. Either way, it's drawn
    /// inside the picture.
    #[test]
    fn the_cursor_moves_smoothly_until_it_settles() {
        // The outline's top-left corner, in cell edges.
        let corner = |gui: &CanvasGui| {
            let outline = gui.touch_cursor_outline();
            let x = outline.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
            let y = outline.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
            (x, y)
        };
        let mut gui = editor(Tool::Pencil);
        let mut finger = Finger::down(&mut gui);
        // (Far enough to be more than a shaky tap.)
        finger.slide(&mut gui, (1.25, 0.0));
        let (x, y) = corner(&gui);
        assert!((x - 3.25).abs() < 1e-4 && y == 2.0, "drawn at ({x}, {y})");

        // Into the last column, and on past the edge: it stops with its edge on the picture's.
        finger.slide(&mut gui, (2.0, 0.0));
        assert!((corner(&gui).0 - 4.0).abs() < 1e-4);

        finger.slide(&mut gui, (-0.6, 0.0));
        let (x, _) = corner(&gui);
        assert!(x > 3.0 && x < 4.0, "drawn at {x}, between cells");
        finger.lift(&mut gui);
        assert_eq!(corner(&gui), (3.0, 2.0));
    }

    /// The cursor can't leave the grid: pushed past an edge, it slides along it.
    #[test]
    fn the_cursor_stays_on_the_grid() {
        let mut gui = editor(Tool::Pencil);
        let mut finger = Finger::down(&mut gui);
        finger.slide(&mut gui, (10.0, 1.0));
        finger.lift(&mut gui);
        assert_eq!(gui.touch_cursor(), Some(at(4, 3)));
    }

    /// Hold, then drag: the pencil goes down where the cursor was and paints along the way, as a
    /// single undo step.
    #[test]
    fn a_hold_then_a_drag_paints_a_stroke() {
        let mut gui = editor(Tool::Pencil);
        let mut finger = Finger::down(&mut gui);
        finger.hold(&mut gui);
        assert_eq!(painted(&gui), vec![at(2, 2)]);
        finger.slide(&mut gui, (-2.0, 0.0));
        finger.lift(&mut gui);
        assert_eq!(painted(&gui), vec![at(0, 2), at(1, 2), at(2, 2)]);
        assert_eq!(gui.undo_stack.len(), 1);
    }

    /// A second finger mid-stroke takes the stroke back, and leaves nothing to redo.
    #[test]
    fn a_second_finger_takes_a_stroke_back() {
        let mut gui = editor(Tool::LineAlongLane);
        tap(&mut gui); // Something to undo that the cancelled stroke mustn't take with it.
        let mut finger = Finger::down(&mut gui);
        finger.slide(&mut gui, (-2.0, 0.0));
        finger.lift(&mut gui);

        let mut finger = Finger::down(&mut gui);
        finger.hold(&mut gui);
        finger.slide(&mut gui, (0.0, 2.0));
        assert_eq!(painted(&gui), vec![at(0, 2), at(2, 2), at(0, 3), at(0, 4)]);

        let tool = gui.current_tool;
        gui.touch_frame(
            TouchSample {
                down: true,
                pos: Some(finger.pos),
                several: true,
                time: finger.time + 0.02,
                ..TouchSample::default()
            },
            SCALE,
            tool,
        );
        finger.lift(&mut gui);
        assert_eq!(painted(&gui), vec![at(2, 2)]);
        assert_eq!(gui.undo_stack.len(), 1);
        assert!(gui.redo_stack.is_empty());
        assert!(gui.line_tool_state.is_none());
    }

    /// A tap is a click, which is what flood fill waits for.
    #[test]
    fn a_tap_floods() {
        let mut gui = editor(Tool::FloodFill);
        tap(&mut gui);
        assert_eq!(painted(&gui).len(), 25);
    }

    /// While solving, the line tool marks with whatever color is picked, the background
    /// included (there's no second button to do that with).
    #[test]
    fn solving_by_touch() {
        let mut solution = Solution::blank_bw(5, 5);
        solution.cells[at(0, 0)] = Color(1);
        let doc = Document::from_solution(DynSolution::Square(solution), "test".to_string());
        let mut gui = NonogramGui::new_solving(doc).canvas;

        gui.current_color = BACKGROUND;
        tap(&mut gui);
        assert_eq!(cell(&gui, at(2, 2)), BACKGROUND);
        tap(&mut gui);
        assert_eq!(cell(&gui, at(2, 2)), UNSOLVED);

        gui.current_tool = Tool::Annotate;
        tap(&mut gui);
        assert_eq!(gui.annotations.len(), 1);
    }
}
