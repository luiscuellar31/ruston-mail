use eframe::egui::{self, Color32, CornerRadius, Stroke};

pub const ACCENT: Color32 = Color32::from_rgb(109, 92, 245);
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(43, 38, 79);
pub const PANEL: Color32 = Color32::from_rgb(24, 25, 31);
pub const PANEL_RAISED: Color32 = Color32::from_rgb(31, 32, 40);
pub const SIDEBAR: Color32 = Color32::from_rgb(20, 21, 27);
pub const BORDER: Color32 = Color32::from_rgb(51, 53, 64);
pub const MUTED: Color32 = Color32::from_rgb(159, 162, 178);
pub const DANGER: Color32 = Color32::from_rgb(245, 112, 112);
pub const SUCCESS: Color32 = Color32::from_rgb(105, 210, 160);

pub fn install(context: &egui::Context) {
    context.set_theme(egui::Theme::Dark);
    let mut style = (*context.style_of(egui::Theme::Dark)).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = PANEL;
    style.visuals.window_fill = PANEL_RAISED;
    style.visuals.extreme_bg_color = Color32::from_rgb(17, 18, 23);
    style.visuals.faint_bg_color = Color32::from_rgb(37, 38, 47);
    style.visuals.selection.bg_fill = ACCENT;
    style.visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    style.visuals.hyperlink_color = Color32::from_rgb(150, 139, 255);
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    style.visuals.widgets.inactive.corner_radius = CornerRadius::same(7);
    style.visuals.widgets.hovered.corner_radius = CornerRadius::same(7);
    style.visuals.widgets.active.corner_radius = CornerRadius::same(7);
    style.visuals.widgets.open.corner_radius = CornerRadius::same(7);
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    // Navigation remains clickable. Message bodies opt into selection in a
    // local scope, where labels cannot steal clicks from row headers.
    style.interaction.selectable_labels = false;
    context.set_style_of(egui::Theme::Dark, style);
}

pub fn selectable_text<R>(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        ui.style_mut().interaction.selectable_labels = true;
        content(ui)
    })
    .inner
}

/// Paints one left-aligned line without creating a child widget that could
/// intercept clicks from its containing row.
pub fn paint_truncated_text(
    painter: &egui::Painter,
    position: egui::Pos2,
    text: &str,
    font_id: egui::FontId,
    color: Color32,
    max_width: f32,
) {
    let width = max_width.max(0.0);
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font_id, color, width);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    let galley = painter.layout_job(job);
    painter.galley(position, galley, color);
}

/// A small paperclip, drawn because egui's bundled fonts have no glyph for
/// one: the emoji renders as the replacement box.
///
/// One stroke, following the wire: down the outer arm, around the bottom, up
/// the far side, over the top and back down the short inner tongue. The
/// corners are cut rather than square, which at this size reads as bent wire
/// instead of a rectangle.
pub fn paint_clip(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    const WIRE: [(f32, f32); 11] = [
        (2.0, -4.5),
        (2.0, 3.5),
        (1.2, 4.7),
        (0.0, 5.1),
        (-1.2, 4.7),
        (-2.0, 3.5),
        (-2.0, -3.5),
        (-1.4, -4.7),
        (-0.4, -5.1),
        (0.4, -4.5),
        (0.4, 2.3),
    ];
    painter.add(egui::Shape::line(
        WIRE.iter()
            .map(|(dx, dy)| center + egui::vec2(*dx, *dy))
            .collect(),
        Stroke::new(1.0, color),
    ));
}

pub fn panel_frame(fill: Color32) -> egui::Frame {
    egui::Frame::new().fill(fill).inner_margin(16)
}

pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL_RAISED)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(14)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_is_limited_to_mail_content() {
        let context = egui::Context::default();
        install(&context);
        assert!(
            !context
                .style_of(egui::Theme::Dark)
                .interaction
                .selectable_labels
        );

        egui::__run_test_ui(|ui| {
            ui.style_mut().interaction.selectable_labels = false;
            assert!(!ui.style().interaction.selectable_labels);
            selectable_text(ui, |ui| {
                assert!(ui.style().interaction.selectable_labels);
            });
            assert!(!ui.style().interaction.selectable_labels);
        });
    }
}
