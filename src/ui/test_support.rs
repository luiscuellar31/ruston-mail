//! Helpers the interface's tests share to find what a frame drew.

use eframe::egui::{self, epaint::ClippedShape, epaint::Shape};

/// Where `label` is drawn in `shape`, looking inside grouped shapes.
pub(super) fn shape_text_rect(shape: &Shape, label: &str) -> Option<egui::Rect> {
    match shape {
        Shape::Text(text) if text.galley.text() == label => {
            Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
        }
        Shape::Vec(shapes) => shapes
            .iter()
            .find_map(|shape| shape_text_rect(shape, label)),
        _ => None,
    }
}

/// Where `label` is drawn among `shapes`; the test fails if it is not.
pub(super) fn text_rect(shapes: &[ClippedShape], label: &str) -> egui::Rect {
    shapes
        .iter()
        .find_map(|clipped| shape_text_rect(&clipped.shape, label))
        .unwrap_or_else(|| panic!("missing text: {label}"))
}

/// Where `label` is drawn inside the area its shape is clipped to, if it is.
pub(super) fn visible_text_rect(shapes: &[ClippedShape], label: &str) -> Option<egui::Rect> {
    shapes.iter().find_map(|clipped| {
        shape_text_rect(&clipped.shape, label).filter(|rect| clipped.clip_rect.contains_rect(*rect))
    })
}
