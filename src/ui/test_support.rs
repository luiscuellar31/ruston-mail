//! Helpers the interface's tests share: what a frame drew, and where to
//! click.

use eframe::egui::{self, Color32, epaint::ClippedShape, epaint::Shape};

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

/// A click of `button` at `pos`: the pointer moves there, presses and lets
/// go, in one pass.
pub(super) fn click_events(pos: egui::Pos2, button: egui::PointerButton) -> Vec<egui::Event> {
    let press = |pressed| egui::Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: egui::Modifiers::default(),
    };
    vec![egui::Event::PointerMoved(pos), press(true), press(false)]
}

/// WCAG contrast ratio between two opaque colors.
pub(super) fn contrast(a: Color32, b: Color32) -> f64 {
    let linear = |channel: u8| {
        let value = f64::from(channel) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = |color: Color32| {
        0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
    };
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
