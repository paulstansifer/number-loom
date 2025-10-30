#[cfg(test)]
mod tests {

    use egui::CentralPanel;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use number_loom::{gui::NonogramGui, import};

    #[test]
    fn basics() {
        let doc = import::load_path(&"examples/png/apron.png".into(), None);

        let state = (); // Maybe make mutable and use to sneak information out?
        let mut harness = Harness::new_state(
            |ctx, _state| {
                let mut nonogram_gui = NonogramGui::new(doc.clone());

                CentralPanel::default().show(ctx, |ui| {
                    nonogram_gui.main_ui(ctx, ui);
                });
            },
            state,
        );

        harness.get_by_label("Solve").click();
        harness.run();
        // report appears to be an empty string afterwards, though. Perhaps this is an edge case of
        // staleness?
    }
}
