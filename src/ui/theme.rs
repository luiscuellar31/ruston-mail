use eframe::egui::{
    self, Atom, AtomLayoutResponse, Color32, CornerRadius, Id, Response, Stroke, Widget,
    WidgetInfo, WidgetType,
};

pub const ACCENT: Color32 = Color32::from_rgb(109, 92, 245);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(122, 107, 247);
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(43, 38, 79);
pub const PANEL: Color32 = Color32::from_rgb(24, 25, 31);
pub const PANEL_RAISED: Color32 = Color32::from_rgb(31, 32, 40);
pub const SIDEBAR: Color32 = Color32::from_rgb(20, 21, 27);
pub const BORDER: Color32 = Color32::from_rgb(51, 53, 64);
pub const MUTED: Color32 = Color32::from_rgb(159, 162, 178);
pub const DANGER: Color32 = Color32::from_rgb(245, 112, 112);
pub const SUCCESS: Color32 = Color32::from_rgb(105, 210, 160);
pub const PANEL_PADDING: i8 = 16;
const COMPACT_BUTTON_HEIGHT: f32 = 24.0;
const ICON_SIZE: f32 = 16.0;
const ICON_ATOM_ID: &str = "ruston-vector-icon";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    Inbox,
    Drafts,
    Sent,
    Star,
    Archive,
    Spam,
    Trash,
    Folder,
    Label,
    Mail,
}

pub fn icon_atom() -> Atom<'static> {
    Atom::custom(Id::new(ICON_ATOM_ID), egui::Vec2::splat(ICON_SIZE))
}

pub fn paint_atom_icon(ui: &egui::Ui, response: &AtomLayoutResponse, icon: Icon, color: Color32) {
    if let Some(rect) = response.rect(Id::new(ICON_ATOM_ID)) {
        paint_icon(ui.painter(), rect.center(), icon, color);
    }
}

pub struct IconButton<'a> {
    icon: Icon,
    label: &'a str,
    selected: Option<bool>,
    danger: bool,
}

impl<'a> IconButton<'a> {
    pub fn new(icon: Icon, label: &'a str) -> Self {
        Self {
            icon,
            label,
            selected: None,
            danger: false,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = Some(selected);
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

impl Widget for IconButton<'_> {
    fn ui(self, ui: &mut egui::Ui) -> Response {
        let mut button = egui::Button::new(icon_atom())
            .small()
            .min_size(egui::vec2(32.0, 28.0));
        if let Some(selected) = self.selected {
            button = button.selected(selected);
        }
        let layout = button.atom_ui(ui);
        let color = if !ui.is_enabled() {
            ui.visuals().widgets.noninteractive.fg_stroke.color
        } else if self.danger {
            DANGER
        } else if self.selected == Some(true) {
            Color32::WHITE
        } else {
            ui.style().interact(&layout.response).fg_stroke.color
        };
        paint_atom_icon(ui, &layout, self.icon, color);

        let response = layout.response;
        response.widget_info(|| match self.selected {
            Some(selected) => {
                WidgetInfo::selected(WidgetType::Button, ui.is_enabled(), selected, self.label)
            }
            None => WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), self.label),
        });
        response.on_hover_text(self.label)
    }
}

pub fn paint_icon(painter: &egui::Painter, center: egui::Pos2, icon: Icon, color: Color32) {
    let stroke = Stroke::new(1.4, color);
    let point = |x, y| center + egui::vec2(x, y);
    let line = |points: &[(f32, f32)]| {
        painter.add(egui::Shape::line(
            points.iter().map(|(x, y)| point(*x, *y)).collect(),
            stroke,
        ));
    };

    match icon {
        Icon::Inbox => {
            painter.rect_stroke(
                egui::Rect::from_min_max(point(-6.5, -5.0), point(6.5, 5.5)),
                CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
            line(&[
                (-6.0, 1.0),
                (-2.5, 1.0),
                (-1.0, 3.0),
                (1.0, 3.0),
                (2.5, 1.0),
                (6.0, 1.0),
            ]);
        }
        Icon::Drafts => {
            line(&[
                (-5.0, -7.0),
                (2.0, -7.0),
                (6.0, -3.0),
                (6.0, 7.0),
                (-5.0, 7.0),
                (-5.0, -7.0),
            ]);
            line(&[(2.0, -7.0), (2.0, -3.0), (6.0, -3.0)]);
            painter.line_segment([point(-2.5, 0.0), point(3.0, 0.0)], stroke);
            painter.line_segment([point(-2.5, 3.0), point(2.0, 3.0)], stroke);
        }
        Icon::Sent => {
            line(&[
                (-7.0, -5.5),
                (7.0, 0.0),
                (-7.0, 5.5),
                (-3.0, 0.0),
                (-7.0, -5.5),
            ]);
            painter.line_segment([point(-3.0, 0.0), point(7.0, 0.0)], stroke);
        }
        Icon::Star => {
            let mut points = Vec::with_capacity(11);
            for index in 0..=10 {
                let angle =
                    -std::f32::consts::FRAC_PI_2 + index as f32 * std::f32::consts::PI / 5.0;
                let radius = if index % 2 == 0 { 7.0 } else { 3.1 };
                points.push(center + egui::vec2(angle.cos(), angle.sin()) * radius);
            }
            painter.add(egui::Shape::line(points, stroke));
        }
        Icon::Archive => {
            painter.rect_stroke(
                egui::Rect::from_min_max(point(-5.5, -2.5), point(5.5, 6.0)),
                CornerRadius::same(1),
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.rect_stroke(
                egui::Rect::from_min_max(point(-7.0, -6.0), point(7.0, -2.5)),
                CornerRadius::same(1),
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment([point(-2.0, 1.0), point(2.0, 1.0)], stroke);
        }
        Icon::Spam => {
            painter.circle_stroke(center, 6.5, stroke);
            painter.line_segment([point(0.0, -3.5), point(0.0, 1.5)], stroke);
            painter.circle_filled(point(0.0, 4.0), 1.0, color);
        }
        Icon::Trash => {
            line(&[(-5.0, -3.5), (-4.0, 6.5), (4.0, 6.5), (5.0, -3.5)]);
            painter.line_segment([point(-6.5, -3.5), point(6.5, -3.5)], stroke);
            line(&[(-2.5, -3.5), (-1.5, -6.0), (1.5, -6.0), (2.5, -3.5)]);
            painter.line_segment([point(-1.5, -0.5), point(-1.0, 4.0)], stroke);
            painter.line_segment([point(1.5, -0.5), point(1.0, 4.0)], stroke);
        }
        Icon::Folder => {
            line(&[
                (-7.0, -4.5),
                (-1.5, -4.5),
                (0.5, -2.5),
                (7.0, -2.5),
                (6.0, 5.5),
                (-7.0, 5.5),
                (-7.0, -4.5),
            ]);
        }
        Icon::Label => {
            painter.circle_filled(center, 4.0, color);
        }
        Icon::Mail => {
            let rect = egui::Rect::from_min_max(point(-7.0, -5.0), point(7.0, 5.0));
            painter.rect_stroke(
                rect,
                CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
            line(&[(-6.5, -4.0), (0.0, 1.0), (6.5, -4.0)]);
        }
    }
}

/// A single-line field with enough vertical room for comfortable reading and clicking.
pub fn text_field(text: &mut String) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text).margin(egui::Margin::symmetric(4, 6))
}

/// A compact button that remains easy to click.
pub fn compact_button<'a>(atoms: impl egui::IntoAtoms<'a>) -> egui::Button<'a> {
    egui::Button::new(atoms)
        .small()
        .min_size(egui::vec2(0.0, COMPACT_BUTTON_HEIGHT))
}

/// Text marks guaranteed by egui's bundled fonts.
/// Missing marks such as the paperclip and chevron are painted instead.
pub const STAR: &str = "★";
/// Separates the facts at the end of a conversation row.
pub const DOT: &str = "·";

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
    style.visuals.widgets.hovered.bg_stroke = style.visuals.widgets.inactive.bg_stroke;
    style.visuals.widgets.active.bg_stroke = style.visuals.widgets.inactive.bg_stroke;
    style.visuals.widgets.inactive.corner_radius = CornerRadius::same(7);
    style.visuals.widgets.hovered.corner_radius = CornerRadius::same(7);
    style.visuals.widgets.active.corner_radius = CornerRadius::same(7);
    style.visuals.widgets.open.corner_radius = CornerRadius::same(7);
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    // Solid scrollbars reserve space instead of covering content.
    style.spacing.scroll = egui::style::ScrollStyle::solid();
    style.scroll_animation =
        egui::style::ScrollAnimation::new(1_200.0, egui::Rangef::new(0.08, 0.24));
    // Only message bodies opt into selectable labels.
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

/// Centers a widget group after measuring it in an inert sizing pass.
/// A separate child and id avoid advancing the parent or duplicating input.
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
    paint_truncated_text_aligned(
        painter,
        position,
        egui::Align2::LEFT_TOP,
        text,
        font_id,
        color,
        max_width,
    );
}

pub fn paint_truncated_text_aligned(
    painter: &egui::Painter,
    anchor: egui::Pos2,
    align: egui::Align2,
    text: &str,
    font_id: egui::FontId,
    color: Color32,
    max_width: f32,
) {
    let width = max_width.max(0.0);
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font_id, color, width);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    let galley = painter.layout_job(job);
    let position = align.anchor_size(anchor, galley.size()).min;
    painter.galley(position, galley, color);
}

/// Paints the paperclip missing from egui's bundled fonts.
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
    egui::Frame::new().fill(fill).inner_margin(PANEL_PADDING)
}

pub fn panel_scroll_style() -> egui::style::ScrollStyle {
    let mut style = egui::style::ScrollStyle::floating();
    style.bar_outer_margin = -(PANEL_PADDING as f32 + style.floating_width) * 0.5;
    style
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
    fn every_mark_the_interface_draws_has_a_glyph() {
        // Missing glyphs have the replacement box's width.
        let context = egui::Context::default();
        install(&context);
        // Fonts are built on the first pass, not before it.
        context
            .run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        let font = egui::FontId::proportional(14.0);
        let width = |text: &str| {
            context.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE)
                    .size()
                    .x
            })
        };
        let missing = width("\u{fffd}");

        // The parser's list bullet must be available too.
        for mark in [STAR, DOT, "•"] {
            let drawn = width(mark);
            assert!(
                (drawn - missing).abs() > f32::EPSILON,
                "{mark:?} has no glyph and would be drawn as the replacement box"
            );
        }
        // The comparison itself has to be able to fail, or it proves nothing.
        assert_eq!(width("\u{1f4ce}"), missing, "the paperclip is still absent");
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

    #[test]
    fn compact_buttons_keep_a_comfortable_height() {
        egui::__run_test_ui(|ui| {
            let button = ui.add(compact_button("More options"));
            assert!(button.rect.height() >= COMPACT_BUTTON_HEIGHT);
        });
    }

    #[test]
    fn a_compact_button_keeps_its_size_on_hover() {
        let context = egui::Context::default();
        install(&context);
        let render = |input| {
            let mut rect = egui::Rect::NOTHING;
            context
                .run_ui(input, |ui| {
                    rect = ui
                        .add(
                            compact_button("Reply")
                                .fill(ACCENT_SOFT)
                                .stroke(Stroke::new(1.0, ACCENT)),
                        )
                        .rect;
                })
                .drop_without_applying_deltas();
            rect
        };

        let idle = render(egui::RawInput::default());
        let hovered = render(egui::RawInput {
            events: vec![egui::Event::PointerMoved(idle.center())],
            ..Default::default()
        });

        assert_eq!(hovered.size(), idle.size());
    }

    #[test]
    fn vector_icon_buttons_are_consistent_click_targets() {
        egui::__run_test_ui(|ui| {
            for icon in [
                Icon::Inbox,
                Icon::Drafts,
                Icon::Sent,
                Icon::Star,
                Icon::Archive,
                Icon::Spam,
                Icon::Trash,
                Icon::Folder,
                Icon::Label,
                Icon::Mail,
            ] {
                let response = ui.add(IconButton::new(icon, "Action"));
                assert!(response.rect.width() >= 32.0);
                assert!(response.rect.height() >= 28.0);
            }
        });
    }

    #[test]
    fn scrollbars_reserve_space_and_share_a_short_animation() {
        let context = egui::Context::default();
        install(&context);
        let style = context.style_of(egui::Theme::Dark);

        assert!(!style.spacing.scroll.floating);
        assert!(style.spacing.scroll.allocated_width() > 0.0);
        assert_eq!(style.scroll_animation.points_per_second, 1_200.0);
        assert_eq!(
            style.scroll_animation.duration,
            egui::Rangef::new(0.08, 0.24)
        );
    }
}
