#[cfg(test)]
mod tests {
    use egui::CentralPanel;
    use egui_kittest::Harness;
    use number_loom::{gui::NonogramGui, puzzle::Document};

    #[test]
    fn basics() {
        let mut checked = false;
        let mut harness = Harness::new_state(
            |ctx, checked| {
                eframe::run_native(
                    "Number Loom Testbed",
                    eframe::NativeOptions::default(),
                    Ok(Box::new(|cc| {
                        NonogramGui::new(Document::from_solution(solution, file))
                    })),
                );
            },
            checked,
        );
    }
}
