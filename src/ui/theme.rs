use std::time::Duration;

use crate::settings::Appearance;
use eframe::egui::{
    self, Atom, AtomLayoutResponse, Color32, CornerRadius, Id, Response, Stroke, Widget,
    WidgetInfo, WidgetType,
};

pub const ACCENT: Color32 = Color32::from_rgb(109, 92, 245);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(100, 83, 226);
pub const ACCENT_PRESSED: Color32 = Color32::from_rgb(86, 69, 198);
pub const PANEL_PADDING: i8 = 16;
pub const FOOTNOTE_SIZE: f32 = 11.0;
pub const ICON_BUTTON_MIN_SIZE: egui::Vec2 = egui::Vec2::new(32.0, 28.0);
const SKELETON_PERIOD_SECONDS: f64 = 1.6;
/// How long a looping animation waits before asking for its next frame.
const ANIMATION_FRAME: Duration = Duration::from_millis(50);
/// How much of its circle the loading spinner's arc covers.
const SPINNER_SWEEP_DEGREES: f64 = 240.0;
const COMPACT_BUTTON_HEIGHT: f32 = 24.0;
const DIALOG_BUTTON_HEIGHT: f32 = 32.0;
const ICON_SIZE: f32 = 16.0;
const ICON_ATOM_ID: &str = "ruston-vector-icon";
const FOCUS_RING_WIDTH: f32 = 2.0;
const CHECKBOX_ATOM_ID: &str = "ruston-check-box";
const CHECKBOX_RADIUS: u8 = 4;
const CHECK_MARK_WIDTH: f32 = 1.6;
const FIELD_MARGIN: egui::Margin = egui::Margin::symmetric(10, 7);

/// Colors used by the custom-painted parts of the interface. egui's native
/// widgets use the matching `Visuals` installed below.
#[derive(Clone, Copy)]
pub struct Colors {
    pub login_background: Color32,
    pub accent_soft: Color32,
    pub panel: Color32,
    pub panel_raised: Color32,
    pub sidebar: Color32,
    pub border: Color32,
    pub muted: Color32,
    pub danger: Color32,
    pub success: Color32,
    pub selected_folder_text: Color32,
    pub row_hover: Color32,
    /// A sidebar place under the pointer: a tint of the selection, quieter
    /// than it, so the open folder still reads as the chosen one.
    pub sidebar_hover: Color32,
    pub quote_fill: Color32,
    pub code_fill: Color32,
    /// Normal text and controls at rest; egui's own grey is dimmer than
    /// `muted`, which would make secondary text the more prominent.
    text: Color32,
    /// The ring around whatever has keyboard focus.
    focus: Color32,
    /// The outline of a control at rest, such as an unchecked box.
    control_border: Color32,
    /// A text field's hint, readable but clearly not typed text.
    placeholder: Color32,
    skeleton_low: Color32,
    skeleton_high: Color32,
    extreme_bg: Color32,
    hyperlink: Color32,
}

const DARK: Colors = Colors {
    login_background: Color32::from_rgb(21, 19, 34),
    accent_soft: Color32::from_rgb(43, 38, 79),
    panel: Color32::from_rgb(24, 25, 31),
    panel_raised: Color32::from_rgb(31, 32, 40),
    sidebar: Color32::from_rgb(20, 21, 27),
    border: Color32::from_rgb(51, 53, 64),
    muted: Color32::from_rgb(159, 162, 178),
    danger: Color32::from_rgb(245, 112, 112),
    success: Color32::from_rgb(105, 210, 160),
    selected_folder_text: Color32::WHITE,
    row_hover: Color32::from_rgb(28, 29, 36),
    sidebar_hover: Color32::from_rgb(30, 29, 45),
    quote_fill: Color32::from_rgb(27, 28, 35),
    code_fill: Color32::from_rgb(18, 19, 24),
    text: Color32::from_rgb(220, 221, 230),
    focus: Color32::from_rgb(133, 102, 255),
    control_border: Color32::from_rgb(110, 113, 133),
    placeholder: Color32::from_rgb(138, 141, 156),
    skeleton_low: Color32::from_rgb(37, 38, 47),
    skeleton_high: Color32::from_rgb(54, 56, 68),
    extreme_bg: Color32::from_rgb(17, 18, 23),
    hyperlink: Color32::from_rgb(150, 139, 255),
};

const LIGHT: Colors = Colors {
    login_background: Color32::from_rgb(245, 243, 255),
    accent_soft: Color32::from_rgb(235, 231, 255),
    panel: Color32::from_rgb(250, 250, 253),
    panel_raised: Color32::WHITE,
    sidebar: Color32::from_rgb(242, 241, 250),
    border: Color32::from_rgb(211, 213, 226),
    muted: Color32::from_rgb(91, 96, 113),
    danger: Color32::from_rgb(174, 43, 57),
    success: Color32::from_rgb(23, 117, 76),
    selected_folder_text: Color32::from_rgb(64, 49, 150),
    row_hover: Color32::from_rgb(241, 240, 249),
    sidebar_hover: Color32::from_rgb(237, 235, 248),
    quote_fill: Color32::from_rgb(246, 246, 250),
    code_fill: Color32::from_rgb(241, 242, 247),
    text: Color32::from_rgb(43, 45, 56),
    focus: Color32::from_rgb(109, 74, 255),
    control_border: Color32::from_rgb(134, 138, 158),
    placeholder: Color32::from_rgb(102, 107, 125),
    skeleton_low: Color32::from_rgb(230, 231, 239),
    skeleton_high: Color32::from_rgb(242, 243, 248),
    extreme_bg: Color32::from_rgb(237, 238, 246),
    hyperlink: Color32::from_rgb(80, 62, 192),
};

fn colors_for(theme: egui::Theme) -> Colors {
    match theme {
        egui::Theme::Dark => DARK,
        egui::Theme::Light => LIGHT,
    }
}

pub fn colors(ui: &egui::Ui) -> Colors {
    colors_for(ui.ctx().theme())
}

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
    Lock,
    Unlock,
}

impl Icon {
    pub(super) fn source(self) -> egui::ImageSource<'static> {
        match self {
            Self::Inbox => egui::include_image!("../../assets/icons/bootstrap/inbox.svg"),
            Self::Drafts => {
                egui::include_image!("../../assets/icons/bootstrap/file-earmark-text.svg")
            }
            Self::Sent => egui::include_image!("../../assets/icons/bootstrap/send.svg"),
            Self::Star => egui::include_image!("../../assets/icons/bootstrap/star.svg"),
            Self::Archive => egui::include_image!("../../assets/icons/bootstrap/archive.svg"),
            Self::Spam => {
                egui::include_image!("../../assets/icons/bootstrap/exclamation-circle.svg")
            }
            Self::Trash => egui::include_image!("../../assets/icons/bootstrap/trash.svg"),
            Self::Folder => egui::include_image!("../../assets/icons/bootstrap/folder.svg"),
            Self::Label => egui::include_image!("../../assets/icons/bootstrap/tag.svg"),
            Self::Mail => egui::include_image!("../../assets/icons/bootstrap/envelope.svg"),
            Self::Lock => egui::include_image!("../../assets/icons/bootstrap/lock-fill.svg"),
            Self::Unlock => egui::include_image!("../../assets/icons/bootstrap/unlock.svg"),
        }
    }
}

pub fn icon_atom() -> Atom<'static> {
    Atom::custom(Id::new(ICON_ATOM_ID), egui::Vec2::splat(ICON_SIZE))
}

pub fn paint_atom_icon(ui: &egui::Ui, response: &AtomLayoutResponse, icon: Icon, color: Color32) {
    if let Some(rect) = response.rect(Id::new(ICON_ATOM_ID))
        && ui.is_rect_visible(rect)
    {
        egui::Image::new(icon.source())
            .tint(color)
            .paint_at(ui, rect);
    }
}

/// A bundled icon on its own, at the size the sidebar and the buttons use.
pub fn icon_image(icon: Icon, color: Color32) -> egui::Image<'static> {
    egui::Image::new(icon.source())
        .tint(color)
        .fit_to_exact_size(egui::Vec2::splat(ICON_SIZE))
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
            .min_size(ICON_BUTTON_MIN_SIZE);
        if let Some(selected) = self.selected {
            button = button.selected(selected);
        }
        let layout = button.atom_ui(ui);
        let color = if !ui.is_enabled() {
            ui.visuals().widgets.noninteractive.fg_stroke.color
        } else if self.danger {
            colors(ui).danger
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

/// An on/off box with a label. egui draws its box with the theme's widget
/// corners, which on a box this small make a circle, the shape of a radio
/// button; this one keeps square corners, an outline at rest, and the
/// accent with a white mark when checked.
pub struct Checkbox<'a> {
    checked: &'a mut bool,
    atoms: egui::Atoms<'a>,
}

impl<'a> Checkbox<'a> {
    pub fn new(checked: &'a mut bool, atoms: impl egui::IntoAtoms<'a>) -> Self {
        Self {
            checked,
            atoms: atoms.into_atoms(),
        }
    }
}

impl Widget for Checkbox<'_> {
    fn ui(self, ui: &mut egui::Ui) -> Response {
        let Self { checked, mut atoms } = self;
        // Laid out as egui lays out its own check box, so a row keeps its size.
        let size = ui.spacing().icon_width;
        let height = ui.spacing().interact_size.y.max(size);
        atoms.push_left(Atom::custom(
            Id::new(CHECKBOX_ATOM_ID),
            egui::vec2(size, height),
        ));
        let text = atoms.text().map(String::from);
        let mut prepared = egui::AtomLayout::new(atoms)
            .sense(egui::Sense::click())
            .min_size(egui::Vec2::splat(height))
            .allocate(ui);
        if prepared.response.clicked() {
            *checked = !*checked;
            prepared.response.mark_changed();
        }
        prepared.response.widget_info(|| {
            WidgetInfo::selected(
                WidgetType::Checkbox,
                ui.is_enabled(),
                *checked,
                text.as_deref().unwrap_or(""),
            )
        });
        if !ui.is_rect_visible(prepared.response.rect) {
            return prepared.response;
        }

        prepared.fallback_text_color = ui.style().interact(&prepared.response).text_color();
        let laid_out = prepared.paint(ui);
        if let Some(rect) = laid_out.rect(Id::new(CHECKBOX_ATOM_ID)) {
            let square = egui::Rect::from_center_size(
                egui::pos2(rect.left() + size / 2.0, rect.center().y),
                egui::Vec2::splat(size),
            );
            paint_check_box(ui, square, *checked, &laid_out.response);
        }
        laid_out.response
    }
}

fn paint_check_box(ui: &egui::Ui, square: egui::Rect, checked: bool, response: &Response) {
    let corners = CornerRadius::same(CHECKBOX_RADIUS);
    if !checked {
        let border = if response.hovered() {
            ui.visuals().strong_text_color()
        } else {
            colors(ui).control_border
        };
        ui.painter().rect_stroke(
            square,
            corners,
            Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );
        return;
    }

    let fill = if response.is_pointer_button_down_on() {
        ACCENT_PRESSED
    } else if response.hovered() {
        ACCENT_HOVER
    } else {
        ACCENT
    };
    ui.painter().rect_filled(square, corners, fill);
    let mark = egui::Rect::from_center_size(
        square.center(),
        egui::Vec2::splat(ui.spacing().icon_width_inner),
    );
    ui.painter().add(egui::Shape::line(
        vec![
            egui::pos2(mark.left(), mark.center().y),
            egui::pos2(mark.center().x, mark.bottom()),
            egui::pos2(mark.right(), mark.top()),
        ],
        Stroke::new(CHECK_MARK_WIDTH, Color32::WHITE),
    ));
}

/// A single-line field with enough vertical room for comfortable reading and clicking.
pub fn text_field<'t>(ui: &egui::Ui, text: &'t mut String) -> egui::TextEdit<'t> {
    egui::TextEdit::singleline(text).frame(field_frame(ui))
}

/// Every text field's frame: its fill, an outline that marks it as a field
/// at rest, and the same padding. The focus ring covers it while typing.
pub fn field_frame(ui: &egui::Ui) -> egui::Frame {
    egui::Frame::new()
        .fill(ui.visuals().text_edit_bg_color())
        .stroke(Stroke::new(1.0, colors(ui).control_border))
        .corner_radius(ui.visuals().widgets.inactive.corner_radius)
        .inner_margin(FIELD_MARGIN)
}

/// A compact button that remains easy to click.
pub fn compact_button<'a>(atoms: impl egui::IntoAtoms<'a>) -> egui::Button<'a> {
    egui::Button::new(atoms)
        .small()
        .min_size(egui::vec2(0.0, COMPACT_BUTTON_HEIGHT))
}

/// How much a button asks to be pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emphasis {
    /// The one action a view is for, such as Send or Apply: the solid accent.
    Primary,
    /// Everything else: egui's neutral button.
    Secondary,
    /// What throws something away: a red outline and red text.
    Danger,
}

/// `button` with the look of its `emphasis`.
pub fn emphasized(button: egui::Button<'_>, emphasis: Emphasis) -> impl Widget + '_ {
    move |ui: &mut egui::Ui| {
        ui.scope(|ui| {
            match emphasis {
                Emphasis::Primary => primary_visuals(ui),
                Emphasis::Secondary => {}
                Emphasis::Danger => danger_visuals(ui),
            }
            ui.add(button)
        })
        .inner
    }
}

pub fn primary(button: egui::Button<'_>) -> impl Widget + '_ {
    emphasized(button, Emphasis::Primary)
}

pub fn danger(button: egui::Button<'_>) -> impl Widget + '_ {
    emphasized(button, Emphasis::Danger)
}

/// Gives the buttons in `ui` the solid accent and white text. A disabled one
/// keeps the look at egui's faded opacity.
pub fn primary_visuals(ui: &mut egui::Ui) {
    let widgets = &mut ui.visuals_mut().widgets;
    for (state, fill) in [
        (&mut widgets.inactive, ACCENT),
        (&mut widgets.hovered, ACCENT_HOVER),
        (&mut widgets.active, ACCENT_PRESSED),
    ] {
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::NONE;
        state.fg_stroke.color = Color32::WHITE;
    }
}

fn danger_visuals(ui: &mut egui::Ui) {
    let danger = colors(ui).danger;
    let widgets = &mut ui.visuals_mut().widgets;
    // Clear at rest, so the red text keeps its contrast; tinted under the
    // pointer.
    for (state, tint) in [
        (&mut widgets.inactive, 0.0),
        (&mut widgets.hovered, 0.12),
        (&mut widgets.active, 0.24),
    ] {
        state.weak_bg_fill = danger.gamma_multiply(tint);
        state.bg_stroke = Stroke::new(1.0, danger);
        state.fg_stroke.color = danger;
    }
}

/// A confirmation over the window: a question, what answering it does, and
/// the choices, from the safe one to the one the question is about. Escape
/// or a click outside answers `dismissed`.
pub fn confirmation<T: Clone>(
    context: &egui::Context,
    id: &str,
    question: &str,
    detail: &str,
    choices: &[(&str, Emphasis, T)],
    dismissed: T,
) -> Option<T> {
    let mut decision = None;
    let width = (context.content_rect().width() - 48.0).clamp(0.0, 440.0);
    let frame = egui::Frame::popup(&context.style_of(context.theme())).inner_margin(20);
    let modal = egui::Modal::new(Id::new(id))
        .frame(frame)
        .show(context, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
            ui.heading(egui::RichText::new(question).strong());
            ui.label(detail);
            ui.horizontal_wrapped(|ui| {
                for (label, emphasis, choice) in choices {
                    let button =
                        egui::Button::new(*label).min_size(egui::vec2(0.0, DIALOG_BUTTON_HEIGHT));
                    if ui.add(emphasized(button, *emphasis)).clicked() {
                        decision = Some(choice.clone());
                    }
                }
            });
        });
    if decision.is_none() && modal.should_close() {
        decision = Some(dismissed);
    }
    decision
}

/// Text marks guaranteed by egui's bundled fonts.
/// Missing marks such as the paperclip and chevron are painted instead.
pub const STAR: &str = "★";
/// Separates the facts at the end of a conversation row.
pub const DOT: &str = "·";

pub fn install(context: &egui::Context) {
    super::fonts::apply(context, false);
    egui_extras::install_image_loaders(context);
    for mode in [egui::Theme::Dark, egui::Theme::Light] {
        let colors = colors_for(mode);
        let mut style = (*context.style_of(mode)).clone();
        style.visuals = mode.default_visuals();
        style.visuals.panel_fill = colors.panel;
        style.visuals.window_fill = colors.panel_raised;
        style.visuals.extreme_bg_color = colors.extreme_bg;
        style.visuals.faint_bg_color = colors.skeleton_low;
        style.visuals.selection.bg_fill = ACCENT;
        // Only its color shows, on selected text and selected buttons. egui
        // would also draw it as a focused field's border, which
        // `paint_focus_ring` replaces.
        style.visuals.selection.stroke = Stroke::new(0.0, Color32::WHITE);
        style.visuals.hyperlink_color = colors.hyperlink;
        // egui draws only hint text with it in this app.
        style.visuals.weak_text_color = Some(colors.placeholder);
        style.visuals.widgets.noninteractive.fg_stroke.color = colors.text;
        style.visuals.widgets.inactive.fg_stroke.color = colors.text;
        style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, colors.border);
        style.visuals.widgets.hovered.bg_stroke = style.visuals.widgets.inactive.bg_stroke;
        style.visuals.widgets.active.bg_stroke = style.visuals.widgets.inactive.bg_stroke;
        style.visuals.widgets.inactive.corner_radius = CornerRadius::same(7);
        style.visuals.widgets.hovered.corner_radius = CornerRadius::same(7);
        style.visuals.widgets.active.corner_radius = CornerRadius::same(7);
        style.visuals.widgets.open.corner_radius = CornerRadius::same(7);
        // egui's 9 pt is too small for the notices and details drawn with it.
        style.text_styles.insert(
            egui::TextStyle::Small,
            egui::FontId::proportional(FOOTNOTE_SIZE),
        );
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        // Solid scrollbars reserve space instead of covering content.
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.scroll_animation =
            egui::style::ScrollAnimation::new(1_200.0, egui::Rangef::new(0.08, 0.24));
        // Only message bodies opt into selectable labels.
        style.interaction.selectable_labels = false;
        context.set_style_of(mode, style);
    }
    context.set_theme(egui::Theme::Dark);
}

pub fn apply(context: &egui::Context, appearance: Appearance) {
    let mode = match appearance {
        Appearance::Dark => egui::Theme::Dark,
        Appearance::Light => egui::Theme::Light,
    };
    if context.theme() != mode {
        context.set_theme(mode);
    }
}

/// Rings the widget that has keyboard focus. egui marks a focused button only
/// with a slightly darker fill, and a focused row or message header not at
/// all, so one ring serves every kind. Called once a window's widgets are all
/// laid out, so the ring sits on top of the widget it marks.
pub fn paint_focus_ring(ctx: &egui::Context) {
    // A window in the background shows no focus, as native ones do.
    if !ctx.input(|input| input.focused) {
        return;
    }
    // Only a widget drawn in this pass: one that just went away keeps focus
    // until the pass ends, and a ring where it used to be could linger.
    let Some(widget) = ctx
        .memory(|memory| memory.focused())
        .and_then(|focused| {
            ctx.viewport(|viewport| viewport.this_pass.widgets.get(focused).copied())
        })
        .filter(|widget| widget.rect.is_positive() && widget.rect.is_finite())
    else {
        return;
    };
    // Kept to the part of the widget that is in view, as the widget is.
    let painter = ctx
        .layer_painter(widget.layer_id)
        .with_clip_rect(widget.interact_rect.expand(FOCUS_RING_WIDTH));
    painter.rect_stroke(
        widget.rect,
        CornerRadius::same(8),
        Stroke::new(FOCUS_RING_WIDTH, colors_for(ctx.theme()).focus),
        egui::StrokeKind::Outside,
    );
}

pub fn selectable_text<R>(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        ui.style_mut().interaction.selectable_labels = true;
        content(ui)
    })
    .inner
}

/// The clock for a looping animation. The next frame is asked for after a
/// short delay, never at once; an unfocused window, or a style without
/// animations, gets a still picture at time zero and asks for nothing.
fn animation_time(ui: &egui::Ui) -> f64 {
    let animated = ui.style().animation_time > 0.0 && ui.input(|input| input.focused);
    if !animated {
        return 0.0;
    }
    ui.ctx().request_repaint_after(ANIMATION_FRAME);
    ui.input(|input| input.time)
}

/// A quiet loading pulse shared by skeleton layouts. Delayed repaints keep it
/// inexpensive, and an unfocused window falls back to a static placeholder.
pub fn skeleton_fill(ui: &egui::Ui) -> Color32 {
    skeleton_fill_at(animation_time(ui), colors(ui))
}

/// A loading spinner drawn like egui's, which asks for a new frame on every
/// frame. This one keeps to the skeletons' clock, so a screen that only waits
/// stays cheap and holds still in the background.
pub fn spinner(ui: &mut egui::Ui) -> Response {
    let size = ui.style().spacing.interact_size.y;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    response.widget_info(|| WidgetInfo::new(WidgetType::ProgressIndicator));
    if ui.is_rect_visible(rect) {
        let start = animation_time(ui) * std::f64::consts::TAU;
        let sweep = SPINNER_SWEEP_DEGREES.to_radians();
        let radius = rect.height().min(rect.width()) / 2.0 - 2.0;
        let segments = (radius.round() as u32).clamp(8, 128);
        let points: Vec<egui::Pos2> = (0..=segments)
            .map(|index| {
                let angle = start + sweep * f64::from(index) / f64::from(segments);
                let (sin, cos) = angle.sin_cos();
                rect.center() + radius * egui::vec2(cos as f32, sin as f32)
            })
            .collect();
        let stroke = Stroke::new(3.0, ui.visuals().strong_text_color());
        ui.painter().add(egui::Shape::line(points, stroke));
    }
    response
}

pub fn paint_skeleton(painter: &egui::Painter, rect: egui::Rect, fill: Color32) {
    painter.rect_filled(rect, CornerRadius::same(4), fill);
}

fn skeleton_fill_at(time: f64, colors: Colors) -> Color32 {
    let wave = ((time * std::f64::consts::TAU / SKELETON_PERIOD_SECONDS).sin() as f32 + 1.0) * 0.5;
    let channel =
        |low: u8, high: u8| (low as f32 + (high as f32 - low as f32) * wave).round() as u8;
    Color32::from_rgb(
        channel(colors.skeleton_low.r(), colors.skeleton_high.r()),
        channel(colors.skeleton_low.g(), colors.skeleton_high.g()),
        channel(colors.skeleton_low.b(), colors.skeleton_high.b()),
    )
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
    let galley = truncated_galley(painter, text, font_id, color, max_width);
    let position = align.anchor_size(anchor, galley.size()).min;
    painter.galley(position, galley, color);
}

/// Lays out one line cut to `max_width`, for callers that need its size
/// before placing it.
pub fn truncated_galley(
    painter: &egui::Painter,
    text: &str,
    font_id: egui::FontId,
    color: Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let width = max_width.max(0.0);
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font_id, color, width);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    painter.layout_job(job)
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

/// macOS hides the titlebar and draws the window content all the way to the
/// top edge, so whatever sits at the top of the window has to leave room for
/// the traffic lights. Zero everywhere else, and in fullscreen, where the
/// buttons are gone.
pub fn titlebar_inset(ctx: &egui::Context) -> f32 {
    if cfg!(target_os = "macos") && !ctx.input(|input| input.viewport().fullscreen.unwrap_or(false))
    {
        // Native metrics are in macOS points, before egui's application zoom.
        ctx.data(|data| data.get_temp::<f32>(egui::Id::new("native-titlebar-height")))
            .unwrap_or(28.0)
            / ctx.zoom_factor()
    } else {
        0.0
    }
}

pub fn panel_frame(fill: Color32) -> egui::Frame {
    egui::Frame::new().fill(fill).inner_margin(PANEL_PADDING)
}

pub fn top_panel_frame(fill: Color32, top_inset: f32) -> egui::Frame {
    let top = (PANEL_PADDING as f32 + top_inset.max(0.0))
        .ceil()
        .min(i8::MAX as f32) as i8;
    egui::Frame::new().fill(fill).inner_margin(egui::Margin {
        left: PANEL_PADDING,
        right: PANEL_PADDING,
        top,
        bottom: PANEL_PADDING,
    })
}

pub fn panel_scroll_style() -> egui::style::ScrollStyle {
    let mut style = egui::style::ScrollStyle::floating();
    style.bar_outer_margin = -(PANEL_PADDING as f32 + style.floating_width) * 0.5;
    style
}

pub fn card(ui: &egui::Ui) -> egui::Frame {
    let colors = colors(ui);
    egui::Frame::new()
        .fill(colors.panel_raised)
        .stroke(Stroke::new(1.0, colors.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(14)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_ui_icons_render_and_can_be_tinted() {
        let context = egui::Context::default();
        install(&context);

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
            Icon::Lock,
            Icon::Unlock,
        ] {
            let egui::ImageSource::Bytes { bytes, .. } = icon.source() else {
                panic!("{icon:?} must be bundled");
            };
            let image = egui_extras::image::load_svg_bytes(bytes.as_ref(), &Default::default())
                .expect("bundled icon must render");
            assert_eq!(image.size, [16, 16], "{icon:?}");
            assert!(
                image.pixels.iter().any(|pixel| {
                    pixel.a() > 0 && pixel.r() == 255 && pixel.g() == 255 && pixel.b() == 255
                }),
                "{icon:?} must have white pixels for tinting"
            );
            assert!(
                egui::Image::new(icon.source())
                    .load_for_size(&context, egui::Vec2::splat(ICON_SIZE))
                    .expect("bundled icon must load through egui")
                    .is_ready(),
                "{icon:?} must be ready to paint"
            );
        }
    }

    /// WCAG contrast ratio between two opaque colors.
    fn contrast(a: Color32, b: Color32) -> f64 {
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

    /// The rectangles the focus ring marks in one frame of `content`.
    fn focus_rings(
        context: &egui::Context,
        input: egui::RawInput,
        mut content: impl FnMut(&mut egui::Ui),
    ) -> Vec<egui::Rect> {
        let output = context.run_ui(input, |ui| {
            content(ui);
            paint_focus_ring(ui.ctx());
        });
        let rings = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect) if rect.stroke.color == DARK.focus => Some(rect.rect),
                _ => None,
            })
            .collect();
        output.drop_without_applying_deltas();
        rings
    }

    #[test]
    fn the_widget_with_keyboard_focus_gets_a_ring() {
        let context = egui::Context::default();
        install(&context);
        let mut text = String::new();
        for field in [false, true] {
            let mut target = egui::Rect::NOTHING;
            let rings = focus_rings(&context, egui::RawInput::default(), |ui| {
                let button = ui.button("Archive");
                let edit = ui.add(text_field(ui, &mut text));
                let focused = if field { edit } else { button };
                focused.request_focus();
                target = focused.rect;
            });
            assert_eq!(rings, [target], "field: {field}");
        }
        // The ring takes the place of the border egui gives a focused field.
        assert_eq!(
            context
                .style_of(egui::Theme::Dark)
                .visuals
                .selection
                .stroke
                .width,
            0.0
        );

        // Focus outlasts a widget for the rest of the pass it disappears in,
        // but the ring does not.
        assert!(focus_rings(&context, egui::RawInput::default(), |_| {}).is_empty());
        // Nor does a window in the background show one.
        let background = egui::RawInput {
            focused: false,
            ..Default::default()
        };
        let rings = focus_rings(&context, background, |ui| {
            ui.button("Archive").request_focus()
        });
        assert!(rings.is_empty());
    }

    #[test]
    fn a_check_box_is_square_with_an_outline_or_the_accent() {
        let context = egui::Context::default();
        install(&context);
        for checked in [false, true] {
            let mut value = checked;
            let output = context.run_ui(egui::RawInput::default(), |ui| {
                ui.add(Checkbox::new(&mut value, "Ask before opening a link"));
            });
            let boxes: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Rect(rect)
                        if rect.corner_radius == CornerRadius::same(CHECKBOX_RADIUS) =>
                    {
                        Some(rect.clone())
                    }
                    _ => None,
                })
                .collect();
            output.drop_without_applying_deltas();

            let [check_box] = boxes.as_slice() else {
                panic!("expected one box, checked: {checked}");
            };
            assert_eq!(check_box.rect.width(), check_box.rect.height());
            if checked {
                assert_eq!(check_box.fill, ACCENT);
            } else {
                assert_eq!(check_box.stroke, Stroke::new(1.0, DARK.control_border));
            }
        }
    }

    #[test]
    fn clicking_a_check_box_toggles_it() {
        let context = egui::Context::default();
        install(&context);
        let mut value = false;
        let mut rect = egui::Rect::NOTHING;
        context
            .run_ui(egui::RawInput::default(), |ui| {
                rect = ui.add(Checkbox::new(&mut value, "Ask")).rect;
            })
            .drop_without_applying_deltas();
        let click = egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ],
            ..Default::default()
        };
        let mut changed = false;
        context
            .run_ui(click, |ui| {
                changed = ui.add(Checkbox::new(&mut value, "Ask")).changed();
            })
            .drop_without_applying_deltas();

        assert!(value);
        assert!(changed);
    }

    #[test]
    fn buttons_come_in_three_weights() {
        let context = egui::Context::default();
        install(&context);
        for emphasis in [Emphasis::Primary, Emphasis::Secondary, Emphasis::Danger] {
            let mut neutral = Color32::PLACEHOLDER;
            let output = context.run_ui(egui::RawInput::default(), |ui| {
                neutral = ui.visuals().widgets.inactive.weak_bg_fill;
                ui.add(emphasized(egui::Button::new("Send"), emphasis));
            });
            let (frame, text) =
                output
                    .shapes
                    .iter()
                    .fold((None, None), |(frame, text), clipped| {
                        match &clipped.shape {
                            egui::Shape::Rect(rect) => (Some((rect.fill, rect.stroke)), text),
                            egui::Shape::Text(shape) => (frame, Some(shape.fallback_color)),
                            _ => (frame, text),
                        }
                    });
            output.drop_without_applying_deltas();

            let (fill, stroke) = frame.expect("a button frame");
            let danger = DARK.danger;
            let expected = match emphasis {
                Emphasis::Primary => (ACCENT, Stroke::NONE, Color32::WHITE),
                Emphasis::Secondary => (neutral, Stroke::NONE, DARK.text),
                Emphasis::Danger => (Color32::TRANSPARENT, Stroke::new(1.0, danger), danger),
            };
            assert_eq!(
                (fill, stroke, text),
                (expected.0, expected.1, Some(expected.2))
            );
        }
    }

    #[test]
    fn small_text_is_drawn_at_footnote_size() {
        let context = egui::Context::default();
        install(&context);
        for mode in [egui::Theme::Dark, egui::Theme::Light] {
            let style = context.style_of(mode);
            assert_eq!(
                egui::TextStyle::Small.resolve(&style).size,
                FOOTNOTE_SIZE,
                "{mode:?}"
            );
        }
    }

    #[test]
    fn white_text_on_accent_buttons_keeps_adequate_contrast() {
        for fill in [ACCENT, ACCENT_HOVER, ACCENT_PRESSED] {
            let ratio = contrast(Color32::WHITE, fill);
            assert!(ratio >= 4.5, "white on {fill:?} has {ratio:.2}:1 contrast");
        }
    }

    #[test]
    fn normal_text_stands_above_secondary_text_in_both_themes() {
        let context = egui::Context::default();
        install(&context);
        for (mode, colors) in [(egui::Theme::Dark, DARK), (egui::Theme::Light, LIGHT)] {
            let visuals = &context.style_of(mode).visuals;
            let text = visuals.text_color();
            // A button at rest never reads dimmer than the text beside it.
            assert_eq!(visuals.widgets.inactive.text_color(), text, "{mode:?}");
            for surface in [
                colors.panel,
                colors.panel_raised,
                colors.sidebar,
                colors.accent_soft,
            ] {
                let normal = contrast(text, surface);
                let secondary = contrast(colors.muted, surface);
                assert!(
                    normal > secondary && normal >= 7.0,
                    "{mode:?} on {surface:?}: text {normal:.2}:1, secondary {secondary:.2}:1"
                );
            }
        }
    }

    #[test]
    fn text_fields_are_outlined_at_rest_with_a_readable_hint() {
        for (mode, colors) in [(egui::Theme::Dark, DARK), (egui::Theme::Light, LIGHT)] {
            let context = egui::Context::default();
            install(&context);
            context.set_theme(mode);
            let mut text = String::new();
            let output = context.run_ui(egui::RawInput::default(), |ui| {
                ui.add(text_field(ui, &mut text).hint_text("Search mail…"));
            });
            let fill = context.style_of(mode).visuals.text_edit_bg_color();
            let outlined = output.shapes.iter().any(|clipped| {
                matches!(&clipped.shape, egui::epaint::Shape::Rect(rect)
                    if rect.fill == fill && rect.stroke.color == colors.control_border)
            });
            let hint = output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::epaint::Shape::Text(text) if text.galley.text() == "Search mail…" => {
                        Some(text.galley.job.sections[0].format.color)
                    }
                    _ => None,
                })
                .expect("the hint should be drawn");
            output.drop_without_applying_deltas();

            assert!(outlined, "{mode:?}: a field at rest needs its outline");
            assert_eq!(hint, colors.placeholder, "{mode:?}");
            let ratio = contrast(hint, fill);
            assert!(ratio >= 4.5, "{mode:?}: hint at {ratio:.2}:1");
            assert!(
                contrast(colors.control_border, colors.panel) >= 3.0,
                "{mode:?}"
            );
        }
    }

    #[test]
    fn a_hovered_sidebar_place_stays_quieter_than_the_open_one() {
        for (mode, colors) in [(egui::Theme::Dark, DARK), (egui::Theme::Light, LIGHT)] {
            let hover = contrast(colors.sidebar_hover, colors.sidebar);
            let selection = contrast(colors.accent_soft, colors.sidebar);
            assert!(
                1.03 < hover && hover < selection,
                "{mode:?}: hover {hover:.2}:1, selection {selection:.2}:1"
            );
        }
    }

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
        let context = egui::Context::default();
        install(&context);
        // Fonts are built on the first pass, not before it.
        context
            .run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        let font = egui::FontId::proportional(14.0);
        // The parser's list bullet must be available too.
        for mark in [STAR, DOT, "•"] {
            assert!(
                context.fonts_mut(|fonts| fonts.has_glyphs(&font, mark)),
                "{mark:?} has no glyph and would be drawn as the replacement box"
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
                                .fill(DARK.accent_soft)
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

    #[test]
    fn appearance_switches_visuals_and_custom_colors() {
        let context = egui::Context::default();
        install(&context);
        assert_eq!(context.theme(), egui::Theme::Dark);
        assert_eq!(
            context.style_of(egui::Theme::Dark).visuals.panel_fill,
            DARK.panel
        );

        apply(&context, Appearance::Light);
        assert_eq!(context.theme(), egui::Theme::Light);
        assert_eq!(
            context.style_of(egui::Theme::Light).visuals.panel_fill,
            LIGHT.panel
        );
        context
            .run_ui(egui::RawInput::default(), |ui| {
                assert!(!ui.visuals().dark_mode);
                assert_eq!(colors(ui).muted, LIGHT.muted);
                assert_eq!(card(ui).fill, LIGHT.panel_raised);
            })
            .drop_without_applying_deltas();

        apply(&context, Appearance::Dark);
        assert_eq!(context.theme(), egui::Theme::Dark);
        assert_eq!(
            context.style_of(egui::Theme::Dark).visuals.panel_fill,
            DARK.panel
        );
    }

    #[test]
    fn skeleton_pulse_stays_between_its_theme_colors() {
        for colors in [DARK, LIGHT] {
            assert_eq!(
                skeleton_fill_at(SKELETON_PERIOD_SECONDS * 0.25, colors),
                colors.skeleton_high
            );
            assert_eq!(
                skeleton_fill_at(SKELETON_PERIOD_SECONDS * 0.75, colors),
                colors.skeleton_low
            );
        }
    }

    #[test]
    fn titlebar_inset_tracks_fullscreen_and_platform() {
        let context = egui::Context::default();
        if cfg!(target_os = "macos") {
            assert_eq!(titlebar_inset(&context), 28.0);
            context.data_mut(|data| {
                data.insert_temp(egui::Id::new("native-titlebar-height"), 35.0_f32);
            });
            assert_eq!(titlebar_inset(&context), 35.0);
            context.set_zoom_factor(1.75);
            context
                .run_ui(egui::RawInput::default(), |_| {})
                .drop_without_applying_deltas();
            assert_eq!(titlebar_inset(&context), 20.0);
            let mut raw = egui::RawInput::default();
            raw.viewports.insert(
                egui::ViewportId::ROOT,
                egui::ViewportInfo {
                    fullscreen: Some(true),
                    ..Default::default()
                },
            );
            context.run_ui(raw, |_| {}).drop_without_applying_deltas();
            assert_eq!(titlebar_inset(&context), 0.0);
        } else {
            assert_eq!(titlebar_inset(&context), 0.0);
        }
    }

    #[test]
    fn top_panel_frame_adds_inset_to_panel_padding() {
        let zero_inset = top_panel_frame(DARK.sidebar, 0.0);
        assert_eq!(zero_inset.inner_margin.top, PANEL_PADDING);
        assert_eq!(zero_inset.inner_margin.left, PANEL_PADDING);
        assert_eq!(zero_inset.inner_margin.right, PANEL_PADDING);
        assert_eq!(zero_inset.inner_margin.bottom, PANEL_PADDING);

        let inset_frame = top_panel_frame(DARK.sidebar, 28.0);
        assert_eq!(inset_frame.inner_margin.top, PANEL_PADDING + 28);
        assert_eq!(inset_frame.inner_margin.left, PANEL_PADDING);
        assert_eq!(inset_frame.inner_margin.right, PANEL_PADDING);
        assert_eq!(inset_frame.inner_margin.bottom, PANEL_PADDING);
        assert_eq!(
            top_panel_frame(DARK.sidebar, 20.1).inner_margin.top,
            PANEL_PADDING + 21
        );
    }
}
