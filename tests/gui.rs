#[cfg(test)]
mod tests {
    use egui::{Event, Modifiers, PointerButton, Pos2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use number_loom::{gui::NonogramGui, import};

    /// A point that's actually on the canvas, taken from where the last frame drew the picture.
    /// Hardcoding one goes stale every time the layout around the canvas shifts.
    fn canvas_point(nonogram_gui: &NonogramGui) -> Pos2 {
        nonogram_gui
            .editor_gui
            .picture_rect
            .expect("the canvas hasn't been drawn yet")
            .center()
    }

    /// Tap a bare key and let the GUI react to it.
    fn press_key(harness: &mut Harness<NonogramGui>, key: egui::Key) {
        harness.input_mut().events.push(Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
        harness.run();
    }

    #[test]
    fn test_solve_button() {
        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();

        let nonogram_gui = NonogramGui::new(doc.clone());
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui| {
                nonogram_gui.main_ui(ctx);
            },
            nonogram_gui,
        );

        harness.get_by_label("Puzzle").click();
        harness.run();

        let nonogram_gui = harness.state();
        assert!(nonogram_gui.solve_gui.is_some());
    }

    /// The backtracking-solve button spawns the search on a background thread and reports back
    /// over a channel, so the result doesn't land on the very frame the click does; this polls a
    /// few frames to give it a chance to. It also shades the canvas with the solve mask, sharing
    /// `editor_gui.solved_mask` with the plain `Solve` button.
    #[test]
    fn test_backtrack_solve_button() {
        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui: &mut NonogramGui| {
                nonogram_gui.main_ui(ctx);
            },
            NonogramGui::new(doc),
        );

        harness.get_by_label("Solve (backtracking)").click();
        harness.run();

        // `apron.png` solves by line logic alone, so the search reports back almost immediately.
        let mut found = false;
        for _ in 0..200 {
            if harness
                .query_by_label_contains("unsolved cells (upper bound): 0")
                .is_some()
            {
                found = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            harness.run();
        }
        assert!(found, "backtracking solve never reported back");

        let editor_gui = &harness.state().editor_gui;
        let solved_mask = &editor_gui.solved_mask;
        assert!(
            solved_mask.fresh(editor_gui.version),
            "the backtracking solve should have refreshed the solve mask"
        );
        assert!(
            solved_mask.val.1.iter().all(|&solved| solved),
            "apron.png is fully solved, so every cell should show as solved"
        );
    }

    /// Both solve buttons write into `editor_gui.solved_mask` (see `test_backtrack_solve_button`),
    /// but each keeps its own report label. `Solve` used to read its own report text back out of
    /// `solved_mask` via `get_or_refresh`, treating it as fresh (and so skipping a real re-solve)
    /// whenever *anything* had marked it fresh for the current version — including a backtracking
    /// run finishing after it. Clicking `Solve` again (or ticking `auto-solve`, which re-runs the
    /// same check every frame) would then silently show backtracking's report instead of its own.
    #[test]
    fn test_backtrack_solve_does_not_clobber_the_plain_solve_report() {
        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui: &mut NonogramGui| {
                nonogram_gui.main_ui(ctx);
            },
            NonogramGui::new(doc),
        );

        harness.get_by_label("Solve").click();
        harness.run();
        assert!(
            harness
                .query_by_label_contains("unsolved cells: 0")
                .is_some(),
            "the plain solve should have reported back on the same frame"
        );

        harness.get_by_label("Solve (backtracking)").click();
        harness.run();
        for _ in 0..200 {
            if harness
                .query_by_label_contains("unsolved cells (upper bound): 0")
                .is_some()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            harness.run();
        }
        assert!(
            harness
                .query_by_label_contains("unsolved cells: 0")
                .is_some(),
            "the plain solve's report should still be showing once backtracking lands"
        );

        // The actual regression: re-clicking `Solve` (what `auto-solve` would do on the very next
        // frame) used to pick up backtracking's cached report instead of running line logic again.
        harness.get_by_label("Solve").click();
        harness.run();
        assert!(
            harness
                .query_by_label_contains("unsolved cells: 0")
                .is_some(),
            "re-clicking Solve should still show its own report, not backtracking's"
        );
        assert!(
            harness
                .query_by_label_contains("unsolved cells (upper bound): 0")
                .is_some(),
            "the backtracking solve's own report should still be showing too"
        );
    }

    #[test]
    fn test_palette_editor() {
        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();

        let nonogram_gui = NonogramGui::new(doc.clone());
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui| {
                nonogram_gui.main_ui(ctx);
            },
            nonogram_gui,
        );

        assert_eq!(
            harness.state().editor_gui.current_color,
            number_loom::puzzle::Color(1)
        );

        harness
            .get_all_by_label("■")
            .into_iter()
            .find(|node| format!("{:?}", node).contains("disabled: false"))
            .expect("No enabled palette button found")
            .click();
        harness.run();

        let nonogram_gui = harness.state();
        assert_eq!(
            nonogram_gui.editor_gui.current_color,
            number_loom::puzzle::BACKGROUND
        );
    }

    /// Each palette row carries a text field for the color's name, and typing in it renames the
    /// color. Editor-only: the solver shows the puzzle's palette but doesn't rewrite it.
    #[test]
    fn test_renaming_a_palette_color() {
        use egui::accesskit::Role;

        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui: &mut NonogramGui| {
                nonogram_gui.main_ui(ctx);
            },
            NonogramGui::new(doc),
        );
        harness.run();

        let name_of = |harness: &Harness<NonogramGui>| {
            harness
                .state()
                .editor_gui
                .document
                .try_solution()
                .unwrap()
                .palette()[&number_loom::puzzle::BACKGROUND]
                .name
                .clone()
        };

        // The background's own name field, found by what it currently holds. A `TextEdit` shows
        // up in the tree twice — the input and the run of text inside it — so the role picks out
        // the one that can be typed into.
        assert_eq!(name_of(&harness), "white");
        harness
            .get_all_by_value("white")
            .find(|node| node.role() == Role::TextInput)
            .expect("the background should have a name field")
            .type_text("ish");
        harness.run();

        let renamed = name_of(&harness);
        assert!(
            renamed.contains("ish"),
            "typing in the name field should rename the color, got {renamed:?}"
        );

        // The solver shows the same palette, but with no name fields to type into.
        harness.get_by_label("Puzzle").click();
        harness.run();
        assert_eq!(
            harness
                .query_all_by_value(&renamed)
                .filter(|node| node.role() == Role::TextInput)
                .count(),
            0,
            "the solver should not offer a name field"
        );
    }

    /// The sidebar's shortcuts are bare keys, so a name field with the keyboard has to shut them
    /// all up — otherwise naming a color "sea green" would reach for the lasso halfway through.
    #[test]
    fn test_typing_a_name_doesnt_fire_shortcuts() {
        use egui::accesskit::Role;
        use number_loom::gui::Tool;

        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui: &mut NonogramGui| {
                nonogram_gui.main_ui(ctx);
            },
            NonogramGui::new(doc),
        );
        harness.run();

        harness
            .get_all_by_value("white")
            .find(|node| node.role() == Role::TextInput)
            .expect("the background should have a name field")
            .focus();
        harness.run();

        // `S` is the lasso and `1` is the background swatch, both of them bare keys.
        press_key(&mut harness, egui::Key::S);
        press_key(&mut harness, egui::Key::Num1);

        assert_eq!(
            harness.state().editor_gui.current_tool,
            Tool::Pencil,
            "typing a name shouldn't switch tools"
        );
        assert_eq!(
            harness.state().editor_gui.current_color,
            number_loom::puzzle::Color(1),
            "typing a name shouldn't repaint the palette choice"
        );
    }

    /// The name field trails the rest of its row and takes whatever width is left, so it's the
    /// one that has to stay inside the sidebar rather than spilling out under the canvas. The
    /// background's row has no delete button, but pads the gap, so every field starts level.
    #[test]
    fn test_palette_row_fits_the_sidebar() {
        use egui::accesskit::Role;

        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui: &mut NonogramGui| {
                nonogram_gui.main_ui(ctx);
            },
            NonogramGui::new(doc),
        );
        harness.run();

        // Everything in the sidebar has to stay left of the canvas beside it.
        let canvas_left = harness
            .state()
            .editor_gui
            .picture_rect
            .expect("the canvas hasn't been drawn yet")
            .min
            .x as f64;

        // By what they hold: the sidebar has other text fields (the resizer's, the metadata
        // section's), and only these two are palette names.
        let fields: Vec<_> = ["white", "black"]
            .iter()
            .map(|name| {
                harness
                    .get_all_by_value(name)
                    .find(|node| node.role() == Role::TextInput)
                    .unwrap_or_else(|| panic!("no name field holding {name:?}"))
                    .raw_bounds()
                    .expect("the name field has no bounds")
            })
            .collect();

        for field in &fields {
            assert!(
                field.x1 < canvas_left,
                "a name field (to {}) spilled out of the sidebar, which ends at {canvas_left}",
                field.x1
            );
            assert!(
                field.x1 - field.x0 >= 24.0,
                "a name field came out only {} wide",
                field.x1 - field.x0
            );
        }
        assert_eq!(
            fields[0].x0, fields[1].x0,
            "the background's row pads the missing delete button, so the fields start level"
        );
    }

    /// The number keys pick palette entries by their position in the palette editor, so `1` is
    /// the background and `2` is the first drawing color — not `Color(1)` and `Color(2)`.
    #[test]
    fn test_palette_number_keys() {
        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();

        let nonogram_gui = NonogramGui::new(doc.clone());
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui| {
                nonogram_gui.main_ui(ctx);
            },
            nonogram_gui,
        );
        harness.run();

        press_key(&mut harness, egui::Key::Num1);
        assert_eq!(
            harness.state().editor_gui.current_color,
            number_loom::puzzle::BACKGROUND
        );

        press_key(&mut harness, egui::Key::Num2);
        assert_eq!(
            harness.state().editor_gui.current_color,
            number_loom::puzzle::Color(1)
        );

        // A key past the end of the palette leaves the choice alone.
        press_key(&mut harness, egui::Key::Num9);
        assert_eq!(
            harness.state().editor_gui.current_color,
            number_loom::puzzle::Color(1)
        );
    }

    /// Each tool has a bare-key shortcut, and the two editor-only tools' keys do nothing in the
    /// solver, where those buttons aren't offered.
    #[test]
    fn test_tool_shortcuts() {
        use number_loom::gui::Tool;

        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();

        let nonogram_gui = NonogramGui::new(doc.clone());
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui| {
                nonogram_gui.main_ui(ctx);
            },
            nonogram_gui,
        );
        harness.run();

        assert_eq!(harness.state().editor_gui.current_tool, Tool::Pencil);

        for (key, tool) in [
            (egui::Key::L, Tool::LineAlongLane),
            (egui::Key::F, Tool::FloodFill),
            (egui::Key::S, Tool::Lasso),
            (egui::Key::P, Tool::Pencil),
        ] {
            press_key(&mut harness, key);
            assert_eq!(harness.state().editor_gui.current_tool, tool);
        }

        harness.get_by_label("Puzzle").click();
        harness.run();
        assert!(harness.state().solve_gui.is_some());

        // The solver offers the pencil and the line tool, but not flood fill or the lasso.
        press_key(&mut harness, egui::Key::L);
        let solve_tool = |harness: &Harness<NonogramGui>| {
            harness
                .state()
                .solve_gui
                .as_ref()
                .unwrap()
                .canvas
                .current_tool
        };
        assert_eq!(solve_tool(&harness), Tool::LineAlongLane);

        press_key(&mut harness, egui::Key::S);
        assert_eq!(solve_tool(&harness), Tool::LineAlongLane);

        press_key(&mut harness, egui::Key::F);
        assert_eq!(solve_tool(&harness), Tool::LineAlongLane);

        // ...and annotate is the mirror image: live here, dead back in the editor.
        press_key(&mut harness, egui::Key::A);
        assert_eq!(solve_tool(&harness), Tool::Annotate);
    }

    /// A tool's own key, pressed while that tool is already up, goes back to whatever was in use
    /// before it — so one key both reaches a tool and leaves it again.
    #[test]
    fn test_tool_key_toggles_back() {
        use number_loom::gui::Tool;

        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui: &mut NonogramGui| {
                nonogram_gui.main_ui(ctx);
            },
            NonogramGui::new(doc),
        );
        harness.run();

        let tool = |harness: &Harness<NonogramGui>| harness.state().editor_gui.current_tool;

        assert_eq!(tool(&harness), Tool::Pencil);

        press_key(&mut harness, egui::Key::L);
        assert_eq!(tool(&harness), Tool::LineAlongLane);
        press_key(&mut harness, egui::Key::L);
        assert_eq!(tool(&harness), Tool::Pencil, "L again should go back");

        // "Before" means whatever was up last, not whatever the tool started as.
        press_key(&mut harness, egui::Key::F);
        press_key(&mut harness, egui::Key::L);
        assert_eq!(tool(&harness), Tool::LineAlongLane);
        press_key(&mut harness, egui::Key::L);
        assert_eq!(tool(&harness), Tool::FloodFill);
    }

    /// Over the canvas the wheel steps through the palette rather than scrolling — panning is
    /// the middle button's job — and it comes back around at the end of the list.
    #[test]
    fn test_wheel_cycles_the_palette() {
        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui: &mut NonogramGui| {
                nonogram_gui.main_ui(ctx);
            },
            NonogramGui::new(doc),
        );
        harness.run();

        // The wheel only steps the palette while the pointer is over the canvas, and where the
        // pointer is only settles once a frame has hit-tested it.
        let center = canvas_point(harness.state());
        harness.input_mut().events.push(Event::PointerMoved(center));
        harness.run();

        let notch = |harness: &mut Harness<NonogramGui>| {
            harness.input_mut().events.push(Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, -1.0),
                modifiers: Modifiers::NONE,
            });
            // A single frame: egui spreads a wheel notch over the next few, and `run` would
            // treat that as a UI that never settles.
            harness.step();
        };

        assert_eq!(
            harness.state().editor_gui.current_color,
            number_loom::puzzle::Color(1)
        );

        // A black-and-white palette holds just the background and the one drawing color.
        notch(&mut harness);
        assert_eq!(
            harness.state().editor_gui.current_color,
            number_loom::puzzle::BACKGROUND
        );
        notch(&mut harness);
        assert_eq!(
            harness.state().editor_gui.current_color,
            number_loom::puzzle::Color(1)
        );
    }

    /// Middle-drag pans a picture that's too big for the window — and while it does, the middle
    /// button belongs to the pan, so it must not also paint.
    #[test]
    fn test_middle_drag_pans_instead_of_painting() {
        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui: &mut NonogramGui| {
                nonogram_gui.main_ui(ctx);
            },
            NonogramGui::new(doc),
        );
        harness.run();

        // Zoom right in, so that this 10x15 picture is taller than the window.
        for _ in 0..20 {
            press_key(&mut harness, egui::Key::Equals);
        }
        harness.run();

        let picture_rect = |harness: &Harness<NonogramGui>| {
            harness
                .state()
                .editor_gui
                .picture_rect
                .expect("the canvas hasn't been drawn yet")
        };
        let cells = |harness: &Harness<NonogramGui>| {
            harness
                .state()
                .editor_gui
                .document
                .try_solution()
                .unwrap()
                .cells()
                .to_vec()
        };

        let before_rect = picture_rect(&harness);
        let before_cells = cells(&harness);
        let start = before_rect.center();

        // A frame with the pointer merely resting on the canvas: where it is only settles once
        // a frame has hit-tested it, and a pan won't start anywhere else.
        harness.input_mut().events.push(Event::PointerMoved(start));
        harness.run();

        harness.input_mut().events.push(Event::PointerButton {
            pos: start,
            button: PointerButton::Middle,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        harness.step();

        harness
            .input_mut()
            .events
            .push(Event::PointerMoved(start - egui::vec2(0.0, 50.0)));
        harness.step();
        // The scroll area applies the pan after its contents have been laid out, so the picture
        // lands in its new place on the following frame.
        harness.step();

        assert!(
            picture_rect(&harness).min.y < before_rect.min.y - 40.0,
            "dragging up should have pulled the picture up with it"
        );
        assert_eq!(
            cells(&harness),
            before_cells,
            "a middle-drag that pans must not paint as well"
        );
    }

    /// `A` belongs to the solver, so it does nothing in the editor — where there's no annotate
    /// button to go with it.
    #[test]
    fn test_annotate_key_is_dead_in_the_editor() {
        use number_loom::gui::Tool;

        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui: &mut NonogramGui| {
                nonogram_gui.main_ui(ctx);
            },
            NonogramGui::new(doc),
        );
        harness.run();

        press_key(&mut harness, egui::Key::A);
        assert_eq!(harness.state().editor_gui.current_tool, Tool::Pencil);
    }

    #[test]
    fn test_pencil_tool() {
        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();
        let original_grid = doc.try_solution().unwrap().cells().to_vec();

        let nonogram_gui = NonogramGui::new(doc);
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui| {
                nonogram_gui.main_ui(ctx);
            },
            nonogram_gui,
        );

        // Pencil is the default tool, so no need to select it.

        let center = canvas_point(harness.state());
        harness.input_mut().events.push(Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        harness.input_mut().events.push(Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        harness.run();

        let nonogram_gui = harness.state();
        assert_ne!(
            nonogram_gui
                .editor_gui
                .document
                .try_solution()
                .unwrap()
                .cells(),
            original_grid
        );
    }

    #[test]
    fn test_undo_redo() {
        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();
        let original_grid = doc.try_solution().unwrap().cells().to_vec();

        let nonogram_gui = NonogramGui::new(doc);
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui| {
                nonogram_gui.main_ui(ctx);
            },
            nonogram_gui,
        );

        // Pencil is the default tool, so no need to select it.

        let center = canvas_point(harness.state());
        harness.input_mut().events.push(Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        harness.input_mut().events.push(Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        harness.run();

        let modified_grid = harness
            .state()
            .editor_gui
            .document
            .try_solution()
            .unwrap()
            .cells()
            .to_vec();
        assert_ne!(modified_grid, original_grid);

        harness.get_by_label("\u{e166}").click();
        harness.run();

        let undone_grid = harness
            .state()
            .editor_gui
            .document
            .try_solution()
            .unwrap()
            .cells()
            .to_vec();
        assert_eq!(undone_grid, original_grid);

        harness.get_by_label("\u{e15a}").click();
        harness.run();

        let redone_grid = harness
            .state()
            .editor_gui
            .document
            .try_solution()
            .unwrap()
            .cells()
            .to_vec();
        assert_eq!(redone_grid, modified_grid);
    }

    /// A triddler must open in the editor and be paintable, just like a square puzzle. This is
    /// the end-to-end check that the canvas sizing, the hit test and the render loop all agree
    /// about a shape with three clue directions and triangular cells.
    #[test]
    fn test_editing_a_triddler() {
        let doc = import::load_path(&"examples/triddler/blob.g".into(), None).unwrap();
        let original = doc.try_solution().is_some();
        assert!(!original, "an olsak file gives a puzzle, not a picture");

        let nonogram_gui = NonogramGui::new(doc);
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui| {
                nonogram_gui.main_ui(ctx);
            },
            nonogram_gui,
        );
        harness.run();

        let before = harness
            .state()
            .editor_gui
            .document
            .try_solution()
            .unwrap()
            .cells()
            .to_vec();
        assert_eq!(before.len(), 96, "a hexagon of side 4 has 6 * 4^2 cells");

        // Somewhere inside this puzzle's (smaller) canvas.
        let center = canvas_point(harness.state());
        harness.input_mut().events.push(Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        harness.input_mut().events.push(Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        harness.run();

        let after = harness
            .state()
            .editor_gui
            .document
            .try_solution()
            .unwrap()
            .cells()
            .to_vec();
        assert_ne!(before, after, "clicking the canvas should paint a triangle");
        assert_eq!(before.len(), after.len(), "painting must not resize");
        assert_eq!(
            before.iter().zip(&after).filter(|(a, b)| a != b).count(),
            1,
            "exactly one triangle should change"
        );
    }

    /// Solve mode must render a triddler's clues, which share the picture's painter rather than
    /// living in panels beside it.
    #[test]
    fn test_solving_a_triddler() {
        let doc = import::load_path(&"examples/triddler/blob.g".into(), None).unwrap();

        let nonogram_gui = NonogramGui::new(doc);
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui| {
                nonogram_gui.main_ui(ctx);
            },
            nonogram_gui,
        );

        harness.get_by_label("Puzzle").click();
        harness.run();

        let gui = harness.state();
        let solve_gui = gui.solve_gui.as_ref().expect("solve mode is on");
        // Solve mode blanks the picture (bar whatever background it can infer immediately), so
        // most of it should still be undecided.
        let cells = solve_gui.canvas.document.try_solution().unwrap().cells();
        let unsolved = cells
            .iter()
            .filter(|c| **c == number_loom::puzzle::UNSOLVED)
            .count();
        assert!(
            unsolved > cells.len() / 2,
            "{unsolved} of {} cells undecided",
            cells.len()
        );

        // Draw a few more frames; this is what would panic if the clue layout were malformed.
        harness.run();
        harness.run();
    }

    /// The "New" dialog's Triddler option must actually produce an editable triangular puzzle,
    /// not just compile. This is the path that constructs a `Geometry::<Tri>` outline and a
    /// `DynSolution::Tri` from scratch, rather than loading one from a file.
    #[test]
    fn test_new_triddler_dialog() {
        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();
        let nonogram_gui = NonogramGui::new(doc);
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui| {
                nonogram_gui.main_ui(ctx);
            },
            nonogram_gui,
        );
        harness.run();

        harness.get_by_label("New").click();
        harness.run();
        harness.get_by_label("Triddler").click();
        harness.run();
        harness.get_by_label("Ok").click();
        harness.run();

        let solution = harness
            .state()
            .editor_gui
            .document
            .try_solution()
            .unwrap()
            .clone();
        assert!(
            matches!(
                solution.shape(),
                number_loom::geometry::Shape::Triangular(_)
            ),
            "expected a triangular puzzle after choosing Triddler"
        );
        assert!(solution.cells().len() > 6, "a side-3 hexagon has 54 cells");
        assert!(
            solution
                .cells()
                .iter()
                .all(|c| *c == number_loom::puzzle::BACKGROUND),
            "a fresh puzzle should start blank"
        );

        // And it must be paintable, same as any other triddler.
        let before = solution.cells().to_vec();
        let center = canvas_point(harness.state());
        harness.input_mut().events.push(Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        harness.input_mut().events.push(Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        harness.run();
        let after = harness
            .state()
            .editor_gui
            .document
            .try_solution()
            .unwrap()
            .cells()
            .to_vec();
        assert_ne!(
            before, after,
            "clicking a fresh triddler should paint a triangle"
        );
    }

    /// The lasso is an editing tool: the editor's sidebar offers it, the solver's must not, since
    /// rearranging the picture is exactly what solving isn't.
    #[test]
    fn test_lasso_tool_is_editor_only() {
        // The material icon the lasso button is labelled with.
        const LASSO: &str = "\u{eb03}";

        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();
        let nonogram_gui = NonogramGui::new(doc);
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui| {
                nonogram_gui.main_ui(ctx);
            },
            nonogram_gui,
        );
        harness.run();

        assert_eq!(
            harness.query_all_by_label(LASSO).count(),
            1,
            "the editor should offer the lasso"
        );

        harness.get_by_label("Puzzle").click();
        harness.run();
        assert!(harness.state().solve_gui.is_some());
        assert_eq!(
            harness.query_all_by_label(LASSO).count(),
            0,
            "the solver should not offer the lasso"
        );
    }

    /// While solving, clicking a cell that already holds the color you're painting with takes it
    /// back to undecided, rather than to background: the same click both makes and unmakes a
    /// guess, and "ruled out" is a different claim from "no idea yet".
    #[test]
    fn test_solve_click_goes_back_to_unknown() {
        use number_loom::puzzle::{Color, UNSOLVED};

        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui: &mut NonogramGui| {
                nonogram_gui.main_ui(ctx);
            },
            NonogramGui::new(doc),
        );
        harness.get_by_label("Puzzle").click();
        harness.run();

        let solve_cells = |harness: &Harness<NonogramGui>| {
            harness
                .state()
                .solve_gui
                .as_ref()
                .unwrap()
                .canvas
                .document
                .try_solution()
                .unwrap()
                .cells()
                .to_vec()
        };
        let click_at = |harness: &mut Harness<NonogramGui>, pos: Pos2| {
            for pressed in [true, false] {
                harness.input_mut().events.push(Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                });
            }
            harness.run();
        };

        let center = harness
            .state()
            .solve_gui
            .as_ref()
            .unwrap()
            .canvas
            .picture_rect
            .expect("the solver's canvas hasn't been drawn yet")
            .center();

        let before = solve_cells(&harness);
        click_at(&mut harness, center);
        let after = solve_cells(&harness);

        // Whichever cell the click landed on. (Background inference may have filled in others,
        // so this looks for the one that took the drawing color.)
        let painted: Vec<usize> = (0..after.len())
            .filter(|i| after[*i] == Color(1) && before[*i] != Color(1))
            .collect();
        assert_eq!(painted.len(), 1, "one click should paint one cell");

        click_at(&mut harness, center);
        assert_eq!(
            solve_cells(&harness)[painted[0]],
            UNSOLVED,
            "clicking the same cell again should take it back to undecided"
        );
    }

    /// Finishing a puzzle puts a replay of the solve in the sidebar, and drawing it doesn't
    /// panic on a real puzzle's geometry and palette.
    /// Clicking a clue in the gutter checks it off by hand, and clicking it again un-checks it.
    /// A clue the solver has already checked off ignores clicks.
    #[test]
    fn test_click_to_check_off_clues() {
        use number_loom::gui::{Action, ActionMood};
        use number_loom::with_puzzle;

        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui: &mut NonogramGui| {
                nonogram_gui.main_ui(ctx);
            },
            NonogramGui::new(doc),
        );

        harness.get_by_label("Puzzle").click();
        harness.run();

        // The row gutter's clue nearest the grid, on the first row that has any clues. Derived
        // from where the picture was drawn, since hardcoding a point goes stale with the layout.
        let solve_gui = harness.state().solve_gui.as_ref().unwrap();
        let row_clues: Vec<usize> = with_puzzle!(&solve_gui.clues, |p| {
            let rows = p.geometry.lane_map().family(0);
            p.lines[rows].iter().map(|l| l.len()).collect()
        });
        let row = row_clues.iter().position(|n| *n > 0).unwrap();
        let clue = (row, row_clues[row] - 1);

        // `draw_clues`' own layout, in reverse: one cell per row, a `CLUE_PAD` gap against the
        // grid, and then the boxes marching outward. The picture's rect is inset by the canvas's
        // one-pixel border; the gutter beside it is not.
        const SCALE: f32 = 16.0; // `NonogramGui`'s starting zoom
        let box_side = SCALE * 0.9;
        let picture = solve_gui.canvas.picture_rect.unwrap();
        let at = Pos2::new(
            picture.min.x - 1.0 - SCALE * number_loom::layout::CLUE_PAD - box_side / 2.0,
            picture.min.y - 1.0 + (row as f32 + 0.5) * SCALE,
        );

        let click = |harness: &mut Harness<NonogramGui>| {
            for pressed in [true, false] {
                harness.input_mut().events.push(Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                });
            }
            harness.run();
        };
        let checked = |harness: &Harness<NonogramGui>| {
            harness
                .state()
                .solve_gui
                .as_ref()
                .unwrap()
                .canvas
                .checked_clues
                .clone()
        };

        click(&mut harness);
        assert_eq!(
            checked(&harness).iter().copied().collect::<Vec<_>>(),
            vec![clue]
        );

        // Clicking it again puts it back.
        click(&mut harness);
        assert!(checked(&harness).is_empty());

        // Check-offs are undoable like anything else, and a redo puts them back.
        click(&mut harness);
        harness
            .state_mut()
            .solve_gui
            .as_mut()
            .unwrap()
            .canvas
            .un_or_re_do(true);
        harness.run();
        assert!(
            checked(&harness).is_empty(),
            "undo should un-check the clue"
        );
        harness
            .state_mut()
            .solve_gui
            .as_mut()
            .unwrap()
            .canvas
            .un_or_re_do(false);
        harness.run();
        assert_eq!(
            checked(&harness).iter().copied().collect::<Vec<_>>(),
            vec![clue],
            "redo should check it off again"
        );
        harness
            .state_mut()
            .solve_gui
            .as_mut()
            .unwrap()
            .canvas
            .un_or_re_do(true);
        harness.run();

        // Once the solver has checked a clue off itself, clicks on it do nothing.
        harness
            .state_mut()
            .solve_gui
            .as_mut()
            .unwrap()
            .mark_fixed_clues = true;
        let solution = harness
            .state()
            .solve_gui
            .as_ref()
            .unwrap()
            .intended_solution
            .cells()
            .to_vec();
        let changes = solution
            .iter()
            .enumerate()
            .map(|(i, c)| (i as u32, *c))
            .collect();
        harness
            .state_mut()
            .solve_gui
            .as_mut()
            .unwrap()
            .canvas
            .perform(Action::ChangeColor { changes }, ActionMood::Normal);
        // `step`, not `run`: the finished solve's replay asks to be repainted forever.
        harness.step();
        // The clue under the pointer really is one the solver has checked off, so the click below
        // is genuinely being ignored rather than missing.
        assert!(
            harness
                .state()
                .solve_gui
                .as_ref()
                .unwrap()
                .fixed_clues
                .val
                .as_ref()
                .unwrap()[0][row]
                .contains(&clue.1),
            "the solver should have resolved the clue being clicked"
        );
        harness.input_mut().events.push(Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        harness.step();
        harness.input_mut().events.push(Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        harness.step();
        assert!(
            checked(&harness).is_empty(),
            "an auto-resolved clue should ignore clicks"
        );
    }

    /// The solver's "Mark resolved clues" aid reports a clue as resolved once it is pinned down
    /// *and* painted: nothing is resolved on an empty grid, and everything is on a finished one.
    #[test]
    fn test_mark_fixed_clues() {
        use number_loom::gui::{Action, ActionMood};
        use number_loom::with_puzzle;

        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui: &mut NonogramGui| {
                nonogram_gui.main_ui(ctx);
            },
            NonogramGui::new(doc),
        );

        harness.get_by_label("Puzzle").click();
        harness.run();
        // Set directly rather than clicking the checkbox, which would persist the setting into
        // the real user preferences.
        harness
            .state_mut()
            .solve_gui
            .as_mut()
            .unwrap()
            .mark_fixed_clues = true;
        harness.run();

        let fixed = |harness: &Harness<NonogramGui>| -> Vec<Vec<Vec<usize>>> {
            harness
                .state()
                .solve_gui
                .as_ref()
                .unwrap()
                .fixed_clues
                .val
                .clone()
                .expect("the aid is on, so it should have run")
        };

        // Nothing painted, so nothing is resolved yet.
        assert!(fixed(&harness).iter().flatten().all(|line| line.is_empty()));

        let solve_gui = harness.state().solve_gui.as_ref().unwrap();
        let solution = solve_gui.intended_solution.cells().to_vec();
        // One clue count per lane, in `lane_map` order: families, then lines within each.
        let clue_counts: Vec<usize> = with_puzzle!(&solve_gui.clues, |p| p
            .lines
            .iter()
            .map(|l| l.len())
            .collect());

        let changes = solution
            .iter()
            .enumerate()
            .map(|(i, c)| (i as u32, *c))
            .collect();
        harness
            .state_mut()
            .solve_gui
            .as_mut()
            .unwrap()
            .canvas
            .perform(Action::ChangeColor { changes }, ActionMood::Normal);
        // `step`, not `run`: the finished solve's replay asks to be repainted, which `run` treats
        // as a UI that never settles.
        harness.step();

        // A solved picture resolves every clue in it.
        let counts: Vec<usize> = fixed(&harness)
            .iter()
            .flatten()
            .map(|line| line.len())
            .collect();
        assert_eq!(counts, clue_counts);
        assert!(clue_counts.iter().sum::<usize>() > 0);
    }

    #[test]
    fn test_solve_replay() {
        use number_loom::gui::{Action, ActionMood};

        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();

        let nonogram_gui = NonogramGui::new(doc);
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui| {
                nonogram_gui.main_ui(ctx);
            },
            nonogram_gui,
        );

        harness.get_by_label("Puzzle").click();
        harness.run();
        assert!(harness.state().solve_gui.as_ref().unwrap().replay.is_none());

        // Fill in the answer in three goes, so the replay has more than one step to play.
        let solution = harness
            .state()
            .solve_gui
            .as_ref()
            .unwrap()
            .intended_solution
            .cells()
            .to_vec();
        let third = solution.len().div_ceil(3);
        for chunk in 0..3 {
            let changes = solution
                .iter()
                .enumerate()
                .skip(chunk * third)
                .take(third)
                .map(|(i, c)| (i as u32, *c))
                .collect();
            harness
                .state_mut()
                .solve_gui
                .as_mut()
                .unwrap()
                .canvas
                .perform(Action::ChangeColor { changes }, ActionMood::Normal);
        }

        // `step`, not `run`: a replay in progress asks to be repainted, which `run` treats as a
        // UI that never settles.
        harness.step();
        assert!(harness.state().solve_gui.as_ref().unwrap().replay.is_some());

        // Three steps at 15 a second is over in a fifth of a second; wait it out and the replay
        // is showing the finished picture.
        std::thread::sleep(std::time::Duration::from_millis(400));
        harness.step();
        let replay = harness
            .state()
            .solve_gui
            .as_ref()
            .unwrap()
            .replay
            .as_ref()
            .unwrap();
        assert!(replay.step() >= 3, "the replay should have run to the end");

        // Clicking it starts the animation over, and leaves the picture itself alone.
        let at = replay
            .rect
            .expect("the replay hasn't been drawn yet")
            .center();
        for pressed in [true, false] {
            harness.input_mut().events.push(Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
        harness.step();
        let solve_gui = harness.state().solve_gui.as_ref().unwrap();
        assert_eq!(solve_gui.replay.as_ref().unwrap().step(), 0);
        assert_eq!(
            solve_gui.canvas.document.try_solution().unwrap().cells(),
            solution
        );
    }

    /// Drives the annotate tool through the real solve-mode canvas: the pointer positions, the
    /// coordinate transform and the tool dispatch, none of which the unit tests in `annotate` see.
    ///
    /// Returns the harness sitting in solve mode, plus the picture's on-screen rect and the size
    /// of one cell in it, so a test can aim at a particular border.
    fn solving_harness() -> (Harness<'static, NonogramGui>, egui::Rect, egui::Vec2) {
        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();
        let (width, height) = match doc.try_solution().unwrap().shape() {
            number_loom::geometry::Shape::Square { width, height } => (width, height),
            _ => panic!("apron.png should be a square puzzle"),
        };

        let mut harness = Harness::new_state(
            |ctx, nonogram_gui: &mut NonogramGui| {
                nonogram_gui.main_ui(ctx);
            },
            NonogramGui::new(doc),
        );
        harness.get_by_label("Puzzle").click();
        harness.run();

        let rect = harness
            .state()
            .solve_gui
            .as_ref()
            .unwrap()
            .canvas
            .picture_rect
            .expect("the solve canvas hasn't been drawn yet");
        let cell = egui::Vec2::new(rect.width() / width as f32, rect.height() / height as f32);
        (harness, rect, cell)
    }

    fn annotations<'a>(
        harness: &'a Harness<'a, NonogramGui>,
    ) -> &'a [number_loom::gui::Annotation] {
        &harness
            .state()
            .solve_gui
            .as_ref()
            .unwrap()
            .canvas
            .annotations
    }

    /// Press, move and release, one frame each, so the drag actually reads as a drag.
    ///
    /// `modifiers` goes on the `RawInput` as well as on the events: `InputState::modifiers` —
    /// which is what a held-shift check reads — comes from there, not from the events.
    fn drag(harness: &mut Harness<NonogramGui>, from: Pos2, to: Pos2, modifiers: Modifiers) {
        let mut frame = |events: Vec<Event>| {
            harness.input_mut().modifiers = modifiers;
            harness.input_mut().events.extend(events);
            harness.run();
        };

        frame(vec![
            Event::PointerMoved(from),
            Event::PointerButton {
                pos: from,
                button: PointerButton::Primary,
                pressed: true,
                modifiers,
            },
        ]);
        frame(vec![Event::PointerMoved(to)]);
        frame(vec![Event::PointerButton {
            pos: to,
            button: PointerButton::Primary,
            pressed: false,
            modifiers,
        }]);

        harness.input_mut().modifiers = Modifiers::NONE;
    }

    /// A drag along a row measures the cells it covered, and leaves the picture, the undo stack
    /// and `version` completely alone — annotations are scratch.
    #[test]
    fn test_annotate_drag_measures_a_span() {
        use number_loom::gui::Tool;

        let (mut harness, rect, cell) = solving_harness();
        press_key(&mut harness, egui::Key::A);
        assert_eq!(
            harness
                .state()
                .solve_gui
                .as_ref()
                .unwrap()
                .canvas
                .current_tool,
            Tool::Annotate
        );

        let before = harness.state().solve_gui.as_ref().unwrap().canvas.version;
        let grid = harness
            .state()
            .solve_gui
            .as_ref()
            .unwrap()
            .canvas
            .document
            .try_solution()
            .unwrap()
            .cells()
            .to_vec();

        // From inside column 0, along the middle of row 2, to inside column 3. A mark runs cell
        // to cell, so that covers columns 0 through 3 inclusive: four cells.
        let y = rect.min.y + 2.5 * cell.y;
        let at = |column: f32| Pos2::new(rect.min.x + (column + 0.5) * cell.x, y);
        drag(&mut harness, at(0.0), at(3.0), Modifiers::NONE);

        assert_eq!(annotations(&harness).len(), 1);
        assert_eq!(annotations(&harness)[0].cells_covered(), 4);

        let canvas = &harness.state().solve_gui.as_ref().unwrap().canvas;
        assert_eq!(canvas.version, before);
        assert!(canvas.undo_stack.is_empty());
        assert_eq!(canvas.document.try_solution().unwrap().cells(), grid);
    }

    /// Holding shift borrows the annotate tool for the length of one drag without giving up the
    /// tool that's actually selected, and Escape clears whatever the marks were.
    #[test]
    fn test_shift_annotates_without_switching_tools() {
        use number_loom::gui::Tool;

        let (mut harness, rect, cell) = solving_harness();
        // The line tool is the solver's default, and the one shift is meant to be borrowed from.
        assert_eq!(
            harness
                .state()
                .solve_gui
                .as_ref()
                .unwrap()
                .canvas
                .current_tool,
            Tool::LineAlongLane
        );

        let y = rect.min.y + 2.5 * cell.y;
        let at = |column: f32| Pos2::new(rect.min.x + (column + 0.5) * cell.x, y);
        drag(&mut harness, at(0.0), at(2.0), Modifiers::SHIFT);

        assert_eq!(annotations(&harness).len(), 1);
        assert_eq!(annotations(&harness)[0].cells_covered(), 3);
        // Shift is momentary: the line tool is still the one that's selected.
        assert_eq!(
            harness
                .state()
                .solve_gui
                .as_ref()
                .unwrap()
                .canvas
                .current_tool,
            Tool::LineAlongLane
        );
        // ...and shift-dragging painted nothing, since annotating never touches the picture.
        assert!(
            harness
                .state()
                .solve_gui
                .as_ref()
                .unwrap()
                .canvas
                .undo_stack
                .is_empty()
        );

        // The clear button only exists while there's something to clear, and Escape does the
        // same job.
        harness.get_by_label("Clear annotations");
        press_key(&mut harness, egui::Key::Escape);
        assert!(annotations(&harness).is_empty());
    }
}
