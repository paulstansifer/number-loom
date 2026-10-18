//! The UI for a gallery of puzzles.

use crate::puzzle::{Document, DynSolution};
use eframe::egui;
use egui::{CornerRadius, Vec2};
use itertools::Itertools;
use std::collections::HashMap;

fn palette_bar(ui: &mut egui::Ui, rect: egui::Rect, doc: &Document) {
    let color_counts = count_colors(doc.solution());
    let total_pixels = color_counts.values().sum::<usize>();

    if total_pixels > 0 {
        let mut color_bar_rect = rect;
        color_bar_rect.min.y = rect.max.y - 10.0;

        let mut current_x = color_bar_rect.min.x;
        // White last, and otherwise the commonest colors first. (The colors themselves are a
        // tiebreak, to avoid flickering.)
        for ((r, g, b), &count) in color_counts
            .iter()
            .sorted_by_key(|&(&rgb, &count)| (rgb == (255, 255, 255), count, rgb))
            .rev()
        {
            let color = egui::Color32::from_rgb(*r, *g, *b);
            let width = (count as f32 / total_pixels as f32) * color_bar_rect.width();
            let mut segment_rect = color_bar_rect;
            segment_rect.min.x = current_x;
            segment_rect.max.x = current_x + width;
            ui.painter()
                .rect_filled(segment_rect, egui::CornerRadius::ZERO, color);
            current_x += width;
        }
    }
}

/// The width of the contents of a preview, for desktop.
pub const DESKTOP_PREVIEW_WIDTH: f32 = 250.0;

/// How wide previews' contents should be for `columns` of them to fill `available` width.
pub fn preview_width_for(ui: &egui::Ui, available: f32, columns: usize) -> f32 {
    // Each preview's frame adds a margin and a stroke on both sides.
    let frame_extra = 2.0 * (5.0 + 1.0);
    let gaps = (columns - 1) as f32 * ui.spacing().item_spacing.x;
    ((available - gaps) / columns as f32 - frame_extra).clamp(100.0, DESKTOP_PREVIEW_WIDTH)
}

/// Draws a gallery item for a document, with contents `width` wide.
pub fn gallery_puzzle_preview(ui: &mut egui::Ui, doc: &Document, width: f32) -> egui::Response {
    let title = doc.get_or_make_up_title();

    let dims_label = doc.dims_label();

    let solution = doc.solution();
    let puzzle_type = match (solution.shape(), solution.clue_style()) {
        (crate::geometry::Shape::Triangular(_), _) => "triddler",
        (_, crate::puzzle::ClueStyle::Nono) => "nonogram",
        (_, crate::puzzle::ClueStyle::Triano) => "trianogram",
    };

    let inner_response = egui::Frame::new()
        .corner_radius(CornerRadius::same(5))
        .stroke(egui::Stroke::new(1.0, egui::Color32::GRAY))
        .inner_margin(egui::Margin::same(5))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_max_width(width);
                ui.add(egui::Label::new(egui::RichText::new(title).strong()).truncate());
                let (mut rect, _response) =
                    ui.allocate_exact_size(egui::vec2(width, 10.0), egui::Sense::hover());

                rect = rect.expand2(Vec2::new(5.0, 0.0));

                palette_bar(ui, rect, doc);

                ui.horizontal(|ui| {
                    ui.small(dims_label);
                    ui.small(puzzle_type);
                });
            });
        });

    let response = ui.interact(
        inner_response.response.rect,
        ui.next_auto_id(),
        egui::Sense::click(),
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

fn count_colors(solution: &DynSolution) -> HashMap<(u8, u8, u8), usize> {
    let mut counts = HashMap::new();
    for color in solution.cells() {
        if let Some(color_info) = solution.palette().get(color)
            && color_info.corner.is_none()
        {
            *counts.entry(color_info.rgb).or_insert(0) += 1;
        }
    }
    counts
}
