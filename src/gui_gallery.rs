//! The UI for a gallery of puzzles.

use crate::puzzle::{BACKGROUND, Document, Solution};
use eframe::egui;
use egui::{CornerRadius, Vec2};
use itertools::Itertools;
use std::collections::HashMap;

fn palette_bar(ui: &mut egui::Ui, rect: egui::Rect, doc: &mut Document) {
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
pub fn draw_gallery_item(ui: &mut egui::Ui, doc: &mut Document) {
    let title = doc
        .get_or_make_up_title()
        .unwrap_or_else(|_| "Untitled".to_string());

    let (width, height) = if let Some(solution) = doc.try_solution() {
        (solution.x_size(), solution.y_size())
    } else {
        let p = doc.try_puzzle().unwrap();
        p.specialize(
            |n| (n.cols.len(), n.rows.len()),
            |t| (t.cols.len(), t.rows.len()),
        )
    };

    let puzzle_type = if let Some(solution) = doc.try_solution() {
        match solution.clue_style {
            crate::puzzle::ClueStyle::Nono => "nonogram",
            crate::puzzle::ClueStyle::Triano => "triangogram",
        }
    } else {
        doc.try_puzzle()
            .unwrap()
            .specialize(|_| "nonogram", |_| "triangogram")
    };

    egui::Frame::new()
        .corner_radius(CornerRadius::same(5))
        .stroke(egui::Stroke::new(1.0, egui::Color32::GRAY))
        .inner_margin(egui::Margin::same(5))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(title).strong());
                let (mut rect, _response) =
                    ui.allocate_exact_size(egui::vec2(250.0, 10.0), egui::Sense::click());

                rect = rect.expand2(Vec2::new(5.0, 0.0));

                palette_bar(ui, rect, doc);

                ui.horizontal(|ui| {
                    ui.small(format!("{}x{}", width, height));
                    ui.small(puzzle_type);
                });
            });
        });
}

fn count_colors(doc: &Document) -> HashMap<(u8, u8, u8), usize> {
    if let Some(solution) = doc.try_solution() {
        count_colors_from_solution(solution)
    } else {
        let mut counts = HashMap::new();

        let puzzle = doc.try_puzzle().unwrap();
        puzzle.specialize(
            |p| {
                for row in &p.rows {
                    for clue in row {
                        let color_info = &p.palette[&clue.color];
                        *counts.entry(color_info.rgb).or_insert(0) += clue.count as usize;
                    }
                }

                counts.insert(
                    p.palette[&BACKGROUND].rgb,
                    p.rows.len() * p.cols.len() - counts.values().sum::<usize>(),
                );
            },
            |p| {
                let mut counts = HashMap::new();
                for row in &p.rows {
                    for clue in row {
                        if let Some(color_info) = p.palette.get(&clue.body_color) {
                            if color_info.corner.is_none() {
                                *counts.entry(color_info.rgb).or_insert(0) +=
                                    clue.body_len as usize;
                            }
                        }
                        if let Some(front_cap) = clue.front_cap {
                            if let Some(color_info) = p.palette.get(&front_cap) {
                                if color_info.corner.is_none() {
                                    *counts.entry(color_info.rgb).or_insert(0) += 1;
                                }
                            }
                        }
                        if let Some(back_cap) = clue.back_cap {
                            if let Some(color_info) = p.palette.get(&back_cap) {
                                if color_info.corner.is_none() {
                                    *counts.entry(color_info.rgb).or_insert(0) += 1;
                                }
                            }
                        }
                    }
                }
                counts.insert(
                    p.palette[&BACKGROUND].rgb,
                    p.rows.len() * p.cols.len() - counts.values().sum::<usize>(),
                );
            },
        );

        counts
    }
}

fn count_colors_from_solution(solution: &Solution) -> HashMap<(u8, u8, u8), usize> {
    let mut counts = HashMap::new();
    for col in &solution.grid {
        for &color in col {
            if let Some(color_info) = solution.palette.get(&color) {
                if color_info.corner.is_none() {
                    *counts.entry(color_info.rgb).or_insert(0) += 1;
                }
            }
        }
    }
    counts
}
