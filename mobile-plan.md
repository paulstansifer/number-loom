# Number Loom on phones: a PWA

A checklist for making the web build work well on a phone, as an installable PWA served from GitHub
Pages. Both solving and editing are in scope. The desktop layout stays as it is: the phone layout
is a separate arrangement, not a reorganization of the existing one (though the two should share
widget code; see 5.1).

The phases are ordered so that each can be checked on its own before the next depends on it.
Phases 1 and 2 are independent of the rest and worth landing early.

A mockup of the intended screens (solve mode, edit mode, and the menu open in each) is at
https://claude.ai/artifact/5ezcy1v9dZXxpHGDyY79wg (private to Paul). It predates two decisions
below: edit mode's top tool row also gets Metadata and Resize buttons, and the menu loses its
"Picture" section accordingly.

---

## Decisions already made

- **PWA first.** If a Play Store listing is wanted later, wrap the PWA as a Trusted Web Activity
  (Bubblewrap). A native Android or iOS build is not planned.
- **Compact layout when the screen is narrower than 600 points.** egui on the web uses one point
  per CSS pixel, so this is the same 600px a media query would use. The compact layout always uses
  touch-sized targets (48 points), whatever the input device: phones are always touch, and sizing
  buttons by the live input mode would make them change size on touchscreen laptops.
- **Portrait only**, via the manifest's `orientation`. Landscape can wait.
- **Input mode follows the input**, independently of layout: an `egui::Event::Touch` switches to
  touch mode; a `PointerMoved` while no button is down and no touch is active switches to mouse
  mode (only a hovering mouse or stylus produces that). egui turns touches into an imitation
  pointer, so pointer events alone can't tell the two apart.
- **The touch cursor** (the core of touch mode; see Phase 4):
  - A cursor drawn on the grid, which a one-finger drag *anywhere* on the canvas moves
    *relatively*, like a trackpad. It never jumps to where the finger is.
  - It is locked to the grid: always on a cell, snapping to the nearest one on release.
  - A tap acts as a click of the current tool at the cursor. Press-and-hold, then drag, drives
    the current tool as a drag starting at the cursor.
  - Its color comes from the same generator as new palette colors (`suggested_color` in
    `gui/palette.rs`), so that it stands out from the palette, plus a white halo so that it stands
    out from the cells.
- **No manual clue check-off in touch mode.** Taps go to the cursor, which can't enter the clue
  gutters. (Letting the cursor into the gutters is a possible later addition.) Mouse mode is
  unchanged, even in the compact layout.
- **The solving aids stay fully functional**, with both "Run" and "Auto", in the menu. They are
  not reduced to auto-only icons.
- **When the puzzle is solved, a modal window announces it**, showing the solve replay (`Replay`
  in `gui/solver.rs`) and the puzzle's description. It replaces the sidebar's replay in the
  compact layout.
- **Dropped from the compact layout:** the zoom buttons (pinch replaces them), the rosette, the
  title and author in the solve sidebar (they move to the solved modal), the undo/redo counts,
  and the status bar (replaced by toasts and a progress bar).
- **The palette scrolls by dragging** when it doesn't fit across.
- **The undo and redo stacks are saved along with the puzzle** (Phase 2). There are plans for
  them to become load-bearing, so don't truncate them to save space without asking.

### The compact screens

**Top bar**, both modes: ☰ (opens the menu), undo, redo. Then:

- *Solve mode:* a "clear annotations" button (an icon), present only while there are annotations
  (it replaces the sidebar button and the Escape key). On the right, a status label while there's
  something to say: "Error detected" (from the Detect Errors aid).
- *Edit mode:* on the right, a pill button showing the unsolved-cell count from the line solve
  ("6 unsolved"). Tapping it runs the disambiguator; while it runs, the pill shows a stop icon and
  tapping it again stops it. When line-solving after every change is off, the pill says "Solve"
  instead of a count and runs the line solve.

**Under the top bar:** a thin progress bar, only while something is running (disambiguation, the
backtracking solve). It replaces the status bar's `ProgressBar`, and sits at the top because
thumbs cover the bottom of the screen.

**Toasts:** `StatusMessage`s become brief messages floating above the bottom bar, which go away
on their own. Errors (`is_error`) stay until dismissed. Ongoing state ("Error detected") belongs in
the top bar, not in a toast.

**Bottom bar:**

- *Solve mode*, one row: the line and annotate tools, a separator, then the palette: background
  (drawn as ✕), the puzzle's colors, and a dashed "?" for unsolved. `palette_order` currently
  leaves unsolved out while solving; there's a TODO there asking for exactly this entry. The
  palette part scrolls by dragging if it overflows.
- *Edit mode*, two rows. Top: pencil, line, flood fill, lasso, a separator, then a Metadata button
  and a Resize button. Bottom: the palette, then a "+" to add a color, scrolling by dragging if it
  overflows. Long-pressing a color (`Response::long_touched`) opens its edit dialog: the color
  picker, its name, and Delete (the only place delete lives in the compact layout).

**The menu** is a drawer from the left, over a dimmed screen:

- An Edit / Puzzle switch (the mode toggle from the toolbar).
- *File:* New…, Library, Open file…, Save / share….
- *Solve mode:* the four aids (Analyze lines, Mark resolved clues, Detect errors, Infer
  background), each a row with a Run button and an Auto switch, Run disabled while Auto is on (as
  `auto_button` does now). Then the render style, as a three-way switch.
- *Edit mode:* "Line-solve after every change" (the `auto_solve` setting) and "Solve by
  backtracking".

**Dialogs:**

- Library, New and Save/share are full-screen in the compact layout, not floating windows.
- Metadata is a full-screen dialog too.
- Resize is a *non-modal* window that leaves the grid visible, since you need to see the grid
  while resizing. Docking it just above the bottom bar is probably simplest.

---

## Phase 0: tapping works at all

### [x] 0.1 Stop eframe from seeing every tap twice

**Done** (in both `index.html` and `pages.html`), as described below. Tested in headless Chrome,
driven over the DevTools protocol with touch emulation at 390×844:

- Before the fix, one tap with the pencil left the cell white and the undo count at 2.
- After it, one tap paints the cell (undo count 1), one tap expands "Metadata", and tapping a
  text field focuses eframe's hidden text input. Mouse clicks behave the same as before.

Still to check on a real phone. The script also focuses the canvas on a touch `pointerup`, as
eframe's own handler does. Chrome focused the text field without that, but eframe says iOS needs
it. Also lost for touches: eframe runs a frame inside its pointer handlers so that Safari will
allow clipboard writes and downloads. If those break from a tap on iOS, this is why.

eframe 0.31's web backend turns a touch into a Primary press twice: once from `pointerdown`
(Pointer Events fire for touches too) and once from `touchstart`. Likewise the release comes from
both `pointerup` and `touchend`. See `install_pointerdown` and `install_touchstart` in eframe's
`src/web/events.rs`. Upstream `main` still doesn't check `pointerType`, so upgrading won't fix it.

The effect: `pointerdown` runs a frame immediately, and `touchstart`'s press lands in the next
frame. The pencil (`pointer_tool_input` in `gui/tools.rs`) sees `any_pressed()` twice, and the
second time the cell is already `current_color`, so it toggles back. Flood fill undoes itself the
same way. A checkbox toggles twice.

The likely fix is a few lines of JavaScript in both `index.html` and `pages.html`: a capture-phase
listener on `window` that calls `stopPropagation()` on `pointerdown` and `pointerup` events whose
`pointerType` is `"touch"`. Capturing at `window` runs before eframe's listeners, which are on the
canvas and the document, and the `touch*` events still get through.

This diagnosis comes from reading the code, not from a device. Confirm it before fixing it.

**Verify:** in Chrome DevTools' device emulation (which fires both kinds of event), tapping a cell
with the pencil should paint it, and the undo stack should grow by one, not two. Then on a real
phone.

---

## Phase 1: installable

### [x] 1.1 Manifest, icons and service worker

**Done**, in `web/`. The placeholder icons are `icon.png` scaled up 3× and 8× with
nearest-neighbour scaling, so they're pixelated rather than blurry. Two things beyond plain
network-first in `sw.js`:

- On install, it caches the page and whatever trunk links from it. The page that registered the
  worker loaded before the worker existed, so otherwise going offline right after the first visit
  would find nothing cached.
- When a newly built file arrives, it deletes the old build's copy. Trunk puts a hash of the build
  in the names of the wasm, the JS glue and the snippet directories, so old builds would
  otherwise pile up in the cache.

`index.html` (itch.io) has all of this too. It can't be installed from inside itch.io's iframe,
but the offline caching still applies.

Both HTML files register `sw.js`, which doesn't exist (it's left over from the eframe template).

- `manifest.json`: `name`, `short_name`, `start_url: "./"`, `scope: "./"`,
  `display: "standalone"`, `orientation: "portrait"`, `theme_color`, `background_color`, icons.
- Icons at 192×192 and 512×512. For now, use placeholders: `icon.png` scaled up (it's only
  64×64, so they'll be blurry). Paul will make real ones later.
- Leave out the maskable icon until the real icons exist. A maskable icon (`"purpose":
  "maskable"` in the manifest) is a full-bleed, opaque square that Android crops to whatever
  shape the launcher uses (circle, rounded square…), so anything important has to sit inside the
  central circle, 80% of the icon's width across. Without one, Android shrinks the ordinary icon
  onto a white shape, which is fine for placeholders.
- `sw.js`: cache the app shell (the HTML, the JS glue, the wasm) for offline use. Prefer
  network-first for those, so a new deploy reaches people without a hard refresh. Pages redeploys
  on every push to `main`. Keep the existing `#dev` bypass.
- Copy all of these into the build with `<link data-trunk rel="copy-file" ...>`, and add
  `<link rel="manifest">` to both HTML files.

It has to be the Pages build: itch.io serves the app in an iframe on its own domain, which can't
be installed.

### [x] 1.2 Full-screen on narrow screens in `pages.html`

**Done.** Below 600px, `pages.html` also switches its `theme-color` to egui's panel color, so the
browser's bar matches the app's. The share links are `https://` now.

Phase 1 was tested in headless Chrome over the DevTools protocol, at 390×844:

- Chrome reported no manifest errors and no installability errors.
- The service worker was active and controlled the page on the first visit, and the page, JS,
  wasm and snippet were cached by then.
- With the server stopped, the app loaded and ran.
- After a second build with new hashes was served from the same address, only the new build's
  files were left in the cache.
- The desktop layout of `pages.html` is unchanged.

Still to check on a real phone: installing from Pages, opening in portrait, and loading offline.

Pages serves `pages.html`, which frames the app beside some explanatory text. Below the 600px
breakpoint, make the app frame fill the viewport and hide the text, so phone visitors (and the
installed PWA) get the whole screen. This keeps a single build, a single PWA scope, and the
existing share links, which point at the Pages root.

While there: `share_string` in `gui/toolbar.rs` builds links with `http://`. They should be
`https://`.

**Verify:** Chrome's Application panel shows the manifest as installable. The app installs on an
Android phone, opens full-screen in portrait, and loads offline.

---

## Phase 2: nothing is lost when the phone kills the app

### [x] 2.1 Save and restore the session

**Done**, in `gui/session.rs`. It mostly follows the notes below. Where it differs, and what
turned up along the way:

- Documents are saved as WOVEN's serializable form (`WovenDocument`, plus the filename, which
  WOVEN leaves out), not as share strings. The whole session is compressed once at the end, so
  compressing each document separately as well would only add base-64 bloat.
- `CellIdx` and `LaneIdx` didn't derive serde after all (the notes below said they did). Now they
  do. serde_json handles them as map keys, so `HashMap<CellIdx, Color>` needed nothing more.
- A lifted lasso selection *is* saved (`mask`, `offset`, `floating`). Lifting takes the cells out
  of the picture, so skipping the selection would have lost them.
- eframe's `Storage` is no use for this: its web `set_string` discards the error when a write
  fails, which is exactly the failure this needs to see. So the session goes straight to
  `localStorage` through `gloo_storage`, under `number-loom.session`. Every GitHub Pages site of
  the same user shares that origin's `localStorage` (and its quota), hence the prefix.
- A save that fails is retried without either canvas's undo and redo history. The first time
  that happens, there's a warning; the history in memory is untouched. If even that fails,
  there's an error message. A fingerprint of the last session written means an unchanged
  session isn't written again, and a failure isn't repeated every 30 seconds.
- eframe's "save before the page unloads" never runs: it listens for an event called
  `onbeforeunload`, which doesn't exist (the event is `beforeunload`). Saving still happens when
  the page loses focus (which includes being hidden, and navigating away) and every 30 seconds.
- After opening a link, the `#WOVEN-...` is taken off the address bar
  (`history.replaceState`). Otherwise reloading the tab, which a phone does to a tab it put to
  sleep, would open the link again over the saved session and lose the user's progress.

Tested natively: round trips of the editor with its history (including a `ReplaceDocument`), of
a solve in progress (progress, checked-off clues, clues, answer, the shared status bar), of a
floating selection, and of a session without history; and damaged sessions. Tested in headless
Chrome: work and undo history survive a reload, in the editor and in solve mode. A link opens in
solve mode with a message, and leaves the address bar clean. Simulating full storage (by making
`setItem` throw) saves without history and warns. A damaged session gives a message and a fresh
start.

Still to check on a real phone: killing the app from the recent-apps screen.

Fixed after a review:

- A session that *panics* when restored (rather than failing with an error) would have been
  left in storage to panic again on every load. `session::load` now takes it out of storage
  before decoding it, and `update` puts it back with a save after the first frame. Tested in
  headless Chrome with a session whose picture uses a character the palette doesn't have: that
  load panics, and the next one starts afresh.
- Replacing the document (`perform` with `ReplaceDocument`) stamps a lifted selection back into
  the picture first, so undo doesn't bring back a picture with a hole in it. This applies to
  opening a link over a restored session, and also to the Library, Open, resizing and palette
  edits.
- Saving skips serialization when no frame has run since the last save, and WOVEN conversion no
  longer clones each document.

**Known problem: more than one tab.** Every tab saves its whole session under the same key, so
with two tabs open (as happens on a desktop when a second share link opens a new tab), whichever
saved last is the only one a reload will find, and the other tab's work is lost without a
word. Possible fixes: a key per tab (in `sessionStorage`, which is per tab), with some rule for
which one a fresh start picks up; or listening for the `storage` event and warning, or deferring
to the newest tab. Phones mostly have one tab per app, so this matters most on desktop.

Phones kill background apps freely. Right now only settings survive (`UserSettings`), so leaving
the app to answer a message can lose a half-solved puzzle.

Save: the mode; the editor's document; in solve mode, the puzzle and the progress; and both
canvases' undo and redo stacks. Probably the current tool and color too.

- Implement `eframe::App::save`. On the web, eframe 0.31 calls it when the page loses focus
  (`visibilitychange`), on `beforeunload`, and every `auto_save_interval`. See `AppRunner::save`
  in eframe's `web/app_runner.rs`. Check whether eframe's `persistence` feature is needed for
  `Frame::storage` on the web. Saving natively too would be nice but isn't required.
- **Serialize the GUI state structs directly** (`NonogramGui`, `CanvasGui`, `SolveGui`, `Action`)
  by deriving serde on them, rather than keeping a separate session struct in sync with them. A
  field that is purely transient gets `#[serde(skip)]` and comes back as its default. A new field
  then has to be either serializable or explicitly skipped, or the build fails, so nothing gets
  forgotten.
- What that takes, from a look at the current code:
  - `Document` isn't `Serialize`, and neither are `DynSolution` and `DynPuzzle`. Rather than
    deriving serde on the puzzle types (they have frozen mirrors in `formats/woven.rs` for good
    reason), serialize these fields as WOVEN strings with `#[serde(with = ...)]`. WOVEN already
    covers solve progress (`set_in_progress`). This includes `Action::ReplaceDocument`.
  - `SolveGui::clues` and `intended_solution` can be skipped and rebuilt after loading, from
    `editor_gui.document`, the way `enter_solve_mode` / `SolveGui::new` build them. (Not from
    the solver canvas's own document: `SolveGui::new` overwrites that copy's cells with the
    progress, so the answer isn't in it.)
  - The `mpsc::Receiver` fields are skipped. Each dialog makes a fresh channel when it starts
    its work, and `NonogramGui::new` already fills them with `mpsc::channel().1`, so a
    `#[serde(skip, default = ...)]` function that does the same is enough.
  - `Staleable` has no `Default`; give it one (empty, never fresh), and skip the cached results
    (`solved_mask`, `disambiguator`, `backtrack_solver`, `line_analysis`, `fixed_clues`, `id`).
  - `status` and `progress` are `Rc`s *shared* between the editor's canvas and the solver's (see
    `main_ui`'s status bar). Skip them, and after loading, point the solver's canvas back at the
    editor's, the way `SolveGui` gets them when it's created. A deserialize that leaves them
    separate compiles fine and quietly loses messages, so test it.
  - `Replay` can be skipped: it's recaptured from the undo stack when the puzzle is next
    seen to be solved.
  - In-flight state (`line_tool_state`, `annotate_drag`, the lasso's `selection`, `panning`)
    can be skipped. `annotations` and `checked_clues` are worth keeping.
  - JSON map keys have to be strings, which `HashMap<CellIdx, Color>` keys aren't. Check that
    serde_json accepts them (it converts integer keys, but the newtype may need help), or
    serialize the map as a list of pairs.
- Brotli-compress the session unconditionally, as WOVEN does.
- Versioning: none for now. If a saved session won't decode (because the structs changed in an
  update, say), drop it and say so in a toast; never panic. A session format that survives
  updates can come later, if it turns out to matter. Saved sessions are not covered by WOVEN's
  golden tests.
- Size: `localStorage` holds about 5MB per origin. If a save fails, show a warning toast and drop the undo history.
- On startup, a WOVEN link in the URL hash takes precedence over the saved session, since opening
  a link is deliberate. Show a toast saying so.

Restoring the solve-mode undo stack also brings back the solve replay, which is built from it.

**Verify:** solve part of a puzzle on a phone, switch to another app, kill Number Loom from the
recent-apps screen, and reopen it. The progress is there and undo still works. Add a unit test
that round-trips a session, undo stack included.

---

## Phase 3: input that isn't a mouse

### [x] 3.1 Put pencil, line and flood fill behind an input struct

**Done**: `PaintPointer` in `gui/tools.rs`. The buttons themselves don't go into it, only what
they mean: a `PaintIntent` of `Current` (primary), `Background` (secondary) or `Erase` (middle).
`pointer_tool_input` works out the color from that, just as it did from the buttons, so touch
input in Phase 4 can say "rule this out" without pretending to be a right-click. New tests drive
the tools through it directly. Checked with a mouse (and a touch) in headless Chrome as well.

One oddity, kept as it was: flood fill acts on the click, which arrives once the button is up,
when no intent is left to report. So a right-click flood fills with the current color, not with
background.

The lasso and annotate tools already take their input as small structs (`LassoPointer`,
`AnnotatePointer`) rather than `egui::PointerState`. `pointer_tool_input` in `gui/tools.rs` still
reads `PointerState` directly, including `middle_down()` and `secondary_down()`. Give it the same
treatment: a struct with pressed/down/released/clicked, plus which color the press means.

This is a refactor that changes no behavior. It's what lets Phase 4 drive every tool without the
tools knowing about touch.

**Verify:** the existing tests pass, and the tools behave the same with a mouse.

### [x] 3.2 Track the input mode

**Done**: `InputMode` in `gui/input_mode.rs`, updated at the start of `main_ui` and read through
`NonogramGui::input_mode()`. It isn't saved with the session. A test in `tests/gui.rs` feeds it
a finger going down, dragging and lifting (as eframe's web backend reports them), then a hover.

Implement the touch/mouse switch described under "Decisions already made". Store it on
`NonogramGui`. Nothing reads it yet.

---

## Phase 4: the touch cursor

### [ ] 4.1 The cursor and its gestures

In touch mode, the canvas ignores where the finger is and only uses how far it moves:

- **Drag:** moves the cursor by the finger's movement, from anywhere on the canvas. The cursor
  stays on the grid, and when the finger lifts it settles on the nearest cell.
- **Tap:** a press and release of the current tool at the cursor's cell.
- **Hold, then drag:** after the finger stays still for the hold delay, a press at the cursor.
  Further movement drags the tool, moving the cursor as it goes, and lifting the finger releases.
  egui won't notice a hold on its own, so call `request_repaint_after` for the moment the hold
  delay ends. A short `navigator.vibrate` when the hold starts would help on Android (iOS ignores
  it).
- **Two fingers:** pinch to zoom (Phase 6). Any stroke the first finger started gets rolled back
  when the second finger arrives.

The cursor's cell becomes `hovered_cell`, so the clue gutters' hover hints work in touch mode.

The hover hints are computed from `hovered_cell` in `SolveGui::hover_blocks`, and the canvas sets
`hovered_cell` from `response.hover_pos()` in `canvas_with_clues` (`gui/canvas.rs`). In touch mode,
that has to come from the cursor instead.

Write the gesture logic as a state machine that's separate from egui (inputs: touch
down/move/up with positions and times; outputs: cursor moves and tool events), so that it can be
unit-tested the way the lasso and annotate inputs are.

**Tunable numbers**, to settle on a real phone: how far the cursor moves per point of finger
movement (and whether it speeds up with faster movement), the hold delay, and how far a finger may
wander before it stops counting as a tap or a hold.

**Verify:** unit tests for the state machine, covering tap, drag, hold-then-drag, and hold
without moving. Then play a whole puzzle on a phone.

---

## Phase 5: the compact layout

### [ ] 5.1 Split the sidebar into pieces

Before building the compact layout, split the sidebars into functions that each produce one
piece: the aids, the tool row, the palette, the line-solve report, the backtracking and
disambiguation controls, and the solved state. `common_sidebar_items` (in `gui.rs`) already does
some of this. The desktop layout should then be a rearrangement of the same calls, unchanged on
screen.

This keeps the two layouts from drifting apart: a fix to the palette lands in both.

### [ ] 5.2 Build the compact layout

Use the compact layout when `ctx.screen_rect().width() < 600.0`, with 48-point targets. Build the
screens as described under "The compact screens" above: the top bar, the bottom bar (one row in
solve mode, two in edit mode), and the drawer.

`egui_kittest` (already a dev-dependency) can render the app at 390×844. Use it to check that
nothing overflows and that both modes' bars contain what they should.

### [ ] 5.3 Dialogs

Full-screen Library, New, Save/share and Metadata in the compact layout. A non-modal Resize. The
long-press color dialog, and "+" to add a color.

### [ ] 5.4 Feedback

The progress bar under the top bar, toasts, the unsolved-count pill, the "Error detected" label,
and the solved modal with its replay and description.

---

## Phase 6: zoom and panning

### [ ] 6.1 Fit the puzzle when it opens

Without zoom buttons, the starting zoom has to work: at the default 16 points per cell, a
30-wide puzzle plus its clues is already wider than a phone. In the compact layout, pick the scale
that fits the puzzle and its clues into the canvas area whenever a puzzle is loaded.

### [ ] 6.2 Zoom around the pinch

`main_ui` applies `zoom_delta` to `self.scale` without adjusting the scroll offset, so the picture
drifts as you pinch. Keep the point between the fingers fixed on screen. With a mouse,
ctrl+scroll should keep the point under the pointer fixed. (This bug affects desktop too.)

### [ ] 6.3 Keep the cursor visible

One finger moves the cursor and no longer pans, so when the cursor nears the edge of the visible
area, scroll to keep it in view (`scroll_to_rect` on its cell).

---

## Phase 7: fitting in on Android

### [ ] 7.1 The back button

In an installed PWA, Android's back button leaves the app. Push a history entry when the drawer
or a full-screen dialog opens, and close it on `popstate`, so that back closes it instead.

### [ ] 7.2 The share sheet

In the compact layout, Save/share's link should open the system share sheet (`navigator.share`)
where it exists, falling back to copying to the clipboard. Pasting a WOVEN string into an egui
text field is unreliable on phones, so links matter more there.

### [ ] 7.3 Documentation

Add a section to `DEVELOPING.md`: how to test on a phone (`trunk serve --address 0.0.0.0`, then
open it from the phone over the LAN; it runs over plain HTTP, but installing needs HTTPS, so test
installation against Pages), remote debugging through `chrome://inspect`, the 600-point
breakpoint, and the touch-mode gestures. Add a `CHANGELOG.md` entry.
