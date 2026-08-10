//! The UI for a gallery of puzzles.

use crate::puzzle::{BACKGROUND, Clue, Document, DynSolution};
use eframe::egui;
use egui::{CornerRadius, Vec2};
use itertools::Itertools;
use std::collections::HashMap;

fn palette_bar(ui: &mut egui::Ui, rect: egui::Rect, doc: &Document) {
    let color_counts = count_colors(doc);
    let total_pixels = color_counts.values().sum::<usize>();

    if total_pixels > 0 {
        let mut color_bar_rect = rect;
        color_bar_rect.min.y = rect.max.y - 10.0;

        let mut current_x = color_bar_rect.min.x;
        for ((r, g, b), &count) in color_counts
            .iter()
            // Tiebreak to avoid flickering:
            .sorted_by_key(|((r, g, b), _)| (*r as u32) * 256 * 256 + (*g as u32) * 256 + *b as u32)
            .sorted_by_key(|(_, count)| *count)
            .sorted_by_key(|((r, g, b), _)| (*r == 255 && *g == 255 && *b == 255) as u32)
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

/// Draws a gallery item for a document.
pub fn gallery_puzzle_preview(ui: &mut egui::Ui, doc: &Document) -> egui::Response {
    let title = doc
        .get_or_make_up_title()
        .unwrap_or_else(|_| "Untitled".to_string());

    let dims_label = doc.dims_label();

    let puzzle_type = match (doc.try_solution(), doc.try_puzzle()) {
        (Some(s), _) => match (s.shape(), s.clue_style()) {
            (crate::geometry::Shape::Triangular(_), _) => "triddler",
            (_, crate::puzzle::ClueStyle::Nono) => "nonogram",
            (_, crate::puzzle::ClueStyle::Triano) => "triangogram",
        },
        (_, Some(crate::puzzle::DynPuzzle::SquareTriano(_))) => "triangogram",
        (_, Some(crate::puzzle::DynPuzzle::TriNono(_))) => "triddler",
        _ => "nonogram",
    };

    let inner_response = egui::Frame::new()
        .corner_radius(CornerRadius::same(5))
        .stroke(egui::Stroke::new(1.0, egui::Color32::GRAY))
        .inner_margin(egui::Margin::same(5))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(title).strong());
                let (mut rect, _response) =
                    ui.allocate_exact_size(egui::vec2(250.0, 10.0), egui::Sense::hover());

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

fn count_colors(doc: &Document) -> HashMap<(u8, u8, u8), usize> {
    if let Some(solution) = doc.try_solution() {
        count_colors_from_solution(solution)
    } else {
        // Every family covers the whole picture, so counting one is enough. `Clue::express`
        // flattens both clue styles into (colour, count) pairs, which is what makes this one loop
        // rather than one per clue type.
        let mut counts = HashMap::new();
        let puzzle = doc.try_puzzle().unwrap();
        let mut filled = 0usize;
        crate::with_puzzle!(puzzle, |p| {
            for lane in p.lane_map().family(0) {
                for clue in &p.lines[lane] {
                    for (color_info, count) in clue.express(&p.palette) {
                        let n = count.unwrap_or(1) as usize;
                        filled += n;
                        if color_info.corner.is_none() {
                            *counts.entry(color_info.rgb).or_insert(0) += n;
                        }
                    }
                }
            }
            counts.insert(
                p.palette[&BACKGROUND].rgb,
                p.geometry.cell_count().saturating_sub(filled),
            );
        });

        counts
    }
}

fn count_colors_from_solution(solution: &DynSolution) -> HashMap<(u8, u8, u8), usize> {
    let mut counts = HashMap::new();
    for color in solution.cells() {
        if let Some(color_info) = solution.palette().get(color) {
            if color_info.corner.is_none() {
                *counts.entry(color_info.rgb).or_insert(0) += 1;
            }
        }
    }
    counts
}
