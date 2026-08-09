#[cfg(test)]
mod tests {
    use egui::{CentralPanel, Event, Modifiers, PointerButton, Pos2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use number_loom::{gui::NonogramGui, import};

    #[test]
    fn test_solve_button() {
        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();

        let nonogram_gui = NonogramGui::new(doc.clone());
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui| {
                CentralPanel::default().show(ctx, |ui| {
                    nonogram_gui.main_ui(ctx, ui);
                });
            },
            nonogram_gui,
        );

        harness.get_by_label("Puzzle").click();
        harness.run();

        let nonogram_gui = harness.state();
        assert!(nonogram_gui.solve_mode);
        assert!(nonogram_gui.solve_gui.is_some());
    }

    #[test]
    fn test_palette_editor() {
        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();

        let nonogram_gui = NonogramGui::new(doc.clone());
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui| {
                CentralPanel::default().show(ctx, |ui| {
                    nonogram_gui.main_ui(ctx, ui);
                });
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

    #[test]
    fn test_pencil_tool() {
        let doc = import::load_path(&"examples/png/apron.png".into(), None).unwrap();
        let original_grid = doc.try_solution().unwrap().cells().to_vec();

        let nonogram_gui = NonogramGui::new(doc);
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui| {
                CentralPanel::default().show(ctx, |ui| {
                    nonogram_gui.main_ui(ctx, ui);
                });
            },
            nonogram_gui,
        );

        // Pencil is the default tool, so no need to select it.

        let center = Pos2::new(237.0, 159.4);
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
                CentralPanel::default().show(ctx, |ui| {
                    nonogram_gui.main_ui(ctx, ui);
                });
            },
            nonogram_gui,
        );

        // Pencil is the default tool, so no need to select it.

        let center = Pos2::new(237.0, 159.4);
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
                CentralPanel::default().show(ctx, |ui| {
                    nonogram_gui.main_ui(ctx, ui);
                });
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
        let center = Pos2::new(220.0, 120.0);
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
                CentralPanel::default().show(ctx, |ui| {
                    nonogram_gui.main_ui(ctx, ui);
                });
            },
            nonogram_gui,
        );

        harness.get_by_label("Puzzle").click();
        harness.run();

        let gui = harness.state();
        assert!(gui.solve_mode);
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
                CentralPanel::default().show(ctx, |ui| {
                    nonogram_gui.main_ui(ctx, ui);
                });
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
        let center = Pos2::new(220.0, 120.0);
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
                CentralPanel::default().show(ctx, |ui| {
                    nonogram_gui.main_ui(ctx, ui);
                });
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
        assert!(harness.state().solve_mode);
        assert_eq!(
            harness.query_all_by_label(LASSO).count(),
            0,
            "the solver should not offer the lasso"
        );
    }
}
