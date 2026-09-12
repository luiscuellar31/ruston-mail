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

/// Puts a group of widgets in the middle of the space left in `ui`, on both
/// axes, and answers with the group's response.
///
/// egui centres a single widget with `centered_and_justified`, but not a
/// group: a top-down layout starts its cursor at the top of its rect whatever
/// its `main_align`, so nesting a vertical layout inside a justified one only
/// stretches the group and leaves it at the top. The height has to be known
/// first, so the group is laid out once in an invisible sizing pass. That pass
/// gets a child of its own, which — unlike `Ui::scope` — leaves the parent's
/// cursor where it was, and an id of its own, so the two passes cannot clash.
/// Being invisible it is also disabled, so a button in the group cannot report
/// a click twice.
///
/// Both axes are placed from the measurement, not just the height. A layout
/// centres the widgets it places itself, but it does not move a nested one:
/// left to `vertical_centered`, a group built around a `ui.horizontal` row
/// would stay against the left edge.
pub fn centered_group(ui: &mut egui::Ui, mut content: impl FnMut(&mut egui::Ui)) -> egui::Response {
    let mut measure = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("centered-group-measure")
            .sizing_pass()
            .invisible(),
    );
    measure.vertical_centered(|ui| content(ui));
    let size = measure.min_rect().size();

    let outer = ui.available_rect_before_wrap();
    let rect = egui::Align2::CENTER_CENTER.align_size_within_rect(size.min(outer.size()), outer);
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.vertical_centered(content);
    })
    .response
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

    /// Lays `content` out through [`centered_group`] in a 400x600 panel and
    /// answers with the group's rectangle and the panel's.
    fn centered(content: impl FnMut(&mut egui::Ui) + Copy) -> (egui::Rect, egui::Rect) {
        let context = egui::Context::default();
        let panel = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 600.0));
        let mut result = (egui::Rect::NOTHING, egui::Rect::NOTHING);
        context
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(panel),
                    ..Default::default()
                },
                |ui| {
                    let available = ui.available_rect_before_wrap();
                    result = (centered_group(ui, content).rect, available);
                },
            )
            .drop_without_applying_deltas();
        result
    }

    fn assert_centered(group: egui::Rect, available: egui::Rect) {
        assert!(group.height() > 0.0, "the group was never laid out");
        assert!(group.height() < available.height(), "it filled the height");
        assert!(group.width() < available.width(), "it filled the width");
        for (axis, group, available) in [
            ("vertically", group.center().y, available.center().y),
            ("horizontally", group.center().x, available.center().x),
        ] {
            assert!(
                (group - available).abs() <= 1.0,
                "the group is not centered {axis}: {group} against {available}"
            );
        }
    }

    #[test]
    fn a_centered_group_of_widgets_sits_in_the_middle_of_the_space_it_is_given() {
        let (group, available) = centered(|ui| {
            ui.heading("Ruston Mail");
            ui.label("Opening Ruston Mail…");
            ui.spinner();
        });

        assert_centered(group, available);
    }

    #[test]
    fn a_centered_group_built_from_a_row_is_centered_too() {
        // A nested layout is not moved by the one around it, so this is the
        // case that stays against the left edge if only the height is placed.
        let (group, available) = centered(|ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Loading conversation…");
            });
        });

        assert_centered(group, available);
    }

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
