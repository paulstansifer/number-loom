#[cfg(test)]
mod tests {
    use egui::{CentralPanel, Event, Modifiers, PointerButton, Pos2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{Queryable};
    use number_loom::{gui::NonogramGui, import};

    #[test]
    fn test_solve_button() {
        let doc = import::load_path(&"examples/png/apron.png".into(), None);

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
        let doc = import::load_path(&"examples/png/apron.png".into(), None);

        let nonogram_gui = NonogramGui::new(doc.clone());
        let mut harness = Harness::new_state(
            |ctx, nonogram_gui| {
                CentralPanel::default().show(ctx, |ui| {
                    nonogram_gui.main_ui(ctx, ui);
                });
            },
            nonogram_gui,
        );

        assert_eq!(harness.state().editor_gui.current_color, number_loom::puzzle::Color(1));

        harness
            .get_all_by_label("■")
            .into_iter()
            .find(|node| format!("{:?}", node).contains("disabled: false"))
            .expect("No enabled palette button found")
            .click();
        harness.run();

        let nonogram_gui = harness.state();
        assert_eq!(nonogram_gui.editor_gui.current_color, number_loom::puzzle::BACKGROUND);
    }

    #[test]
    fn test_pencil_tool() {
        let doc = import::load_path(&"examples/png/apron.png".into(), None);
        let original_grid = doc.try_solution().unwrap().grid.clone();

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
        assert_ne!(nonogram_gui.editor_gui.document.try_solution().unwrap().grid, original_grid);
    }

    #[test]
    fn test_undo_redo() {
        let doc = import::load_path(&"examples/png/apron.png".into(), None);
        let original_grid = doc.try_solution().unwrap().grid.clone();

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

        let modified_grid = harness.state().editor_gui.document.try_solution().unwrap().grid.clone();
        assert_ne!(modified_grid, original_grid);

        harness.get_by_label("\u{e166}").click();
        harness.run();

        let undone_grid = harness.state().editor_gui.document.try_solution().unwrap().grid.clone();
        assert_eq!(undone_grid, original_grid);

        harness.get_by_label("\u{e15a}").click();
        harness.run();

        let redone_grid = harness.state().editor_gui.document.try_solution().unwrap().grid.clone();
        assert_eq!(redone_grid, modified_grid);
    }
}
