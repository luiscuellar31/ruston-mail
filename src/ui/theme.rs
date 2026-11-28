use std::collections::BTreeMap;
use std::time::Duration;

use crate::settings::Appearance;
use eframe::egui::{
    self, Atom, AtomLayoutResponse, Color32, CornerRadius, Id, Response, Stroke, WidgetInfo,
    WidgetType,
};

pub const ACCENT: Color32 = Color32::from_rgb(109, 92, 245);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(100, 83, 226);
pub const ACCENT_PRESSED: Color32 = Color32::from_rgb(86, 69, 198);
/// Text and marks drawn on the accent color.
pub const ON_ACCENT: Color32 = Color32::WHITE;
pub const PANEL_PADDING: i8 = 16;
// Text sizes come from one scale, 11, 13, 14, 15, 17, 21 and 28 points, and
// corner radii from another, 4, 8, 12 and 20. Text is drawn at these sizes
// times the text scale Settings chose: see `text_size`.
pub const FOOTNOTE_SIZE: f32 = 11.0;
/// Text in a conversation row and a message header: names, subjects and
/// previews.
pub const ROW_TEXT_SIZE: f32 = 14.0;
/// Body text: a message's, in the reader and the composer, and the label of
/// the New message button.
pub const BODY_SIZE: f32 = 15.0;
/// The app's name heading the sign-in screen and the mailbox's sidebar.
pub const APP_NAME_SIZE: f32 = 21.0;
/// Small print set apart from the text around it: the account under the
/// sidebar's name and the keys of a shortcut.
pub const CAPTION_SIZE: f32 = 13.0;
/// The title of a group of settings.
pub const SECTION_TITLE_SIZE: f32 = 17.0;
/// The app's name and version in Settings' About.
pub const ABOUT_TITLE_SIZE: f32 = 17.0;
/// The title over the conversation list: the open folder or the search.
pub const LIST_TITLE_SIZE: f32 = 21.0;
/// The title of a page of its own: Settings and the composer.
pub const PAGE_TITLE_SIZE: f32 = 21.0;
/// The app's name alone in the window, while it starts.
pub const DISPLAY_SIZE: f32 = 28.0;
/// The headings of a message's body, from its first level to its third and
/// on.
pub const BODY_HEADING_SIZES: [f32; 3] = [28.0, 21.0, 17.0];
pub const ICON_BUTTON_MIN_SIZE: egui::Vec2 = egui::Vec2::new(32.0, 28.0);
const SKELETON_PERIOD_SECONDS: f64 = 1.6;
/// How long a looping animation waits before asking for its next frame.
const ANIMATION_FRAME: Duration = Duration::from_millis(50);
/// The height of [`compact_button`](super::widgets::compact_button) at the
/// usual text size: see
/// [`compact_button_height`](super::widgets::compact_button_height).
pub const COMPACT_BUTTON_HEIGHT: f32 = 24.0;
pub(super) const DIALOG_BUTTON_HEIGHT: f32 = 32.0;
pub const ICON_SIZE: f32 = 16.0;
const ICON_ATOM_ID: &str = "ruston-vector-icon";
const FOCUS_RING_WIDTH: f32 = 2.0;
pub(super) const CHECKBOX_RADIUS: u8 = 4;
/// The corner radius of buttons, fields and the other widgets.
pub(crate) const WIDGET_CORNER_RADIUS: u8 = 8;
/// The corner radius of a card, such as a message's in the reader.
pub(crate) const CARD_CORNER_RADIUS: u8 = 12;
/// The corner radius of the bars a loading skeleton draws; a checkbox's is
/// the same.
pub(crate) const SKELETON_CORNER_RADIUS: u8 = 4;
/// The corner radius of the ring around the widget the keys reach.
const FOCUS_RING_RADIUS: u8 = 8;
/// The corner radius of a block set apart in a message's body: preformatted
/// text, or the description of an image not loaded.
pub(crate) const BLOCK_CORNER_RADIUS: u8 = 8;
/// The corner radius of a picture shown in a message's body.
pub(crate) const PICTURE_CORNER_RADIUS: u8 = 4;
/// The corner radius of the sign-in card.
pub(crate) const SIGN_IN_CORNER_RADIUS: u8 = 20;
pub(super) const CHECK_MARK_WIDTH: f32 = 1.6;
pub(super) const FIELD_MARGIN: egui::Margin = egui::Margin::symmetric(10, 7);

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
    pub(super) text: Color32,
    /// The ring around whatever has keyboard focus.
    focus: Color32,
    /// The outline of a control at rest, such as an unchecked box.
    pub(super) control_border: Color32,
    /// A text field's hint, readable but clearly not typed text.
    pub(super) placeholder: Color32,
    skeleton_low: Color32,
    skeleton_high: Color32,
    extreme_bg: Color32,
    hyperlink: Color32,
}

pub(super) const DARK: Colors = Colors {
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

pub(super) const LIGHT: Colors = Colors {
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
    /// Writing a new message.
    Compose,
    Settings,
    /// Folding the sidebar, or unfolding it.
    Sidebar,
    /// Leaving Settings for the mailbox.
    Back,
    /// Loading the list again.
    Refresh,
    // Settings' sections, the ones Writing's Compose does not stand for.
    Reading,
    Palette,
    Toggles,
    Sliders,
    Keyboard,
    About,
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
            Self::Compose => {
                egui::include_image!("../../assets/icons/bootstrap/pencil-square.svg")
            }
            Self::Settings => egui::include_image!("../../assets/icons/bootstrap/gear.svg"),
            Self::Sidebar => {
                egui::include_image!("../../assets/icons/bootstrap/layout-sidebar.svg")
            }
            Self::Back => egui::include_image!("../../assets/icons/bootstrap/arrow-left.svg"),
            Self::Refresh => {
                egui::include_image!("../../assets/icons/bootstrap/arrow-clockwise.svg")
            }
            Self::Reading => {
                egui::include_image!("../../assets/icons/bootstrap/envelope-open.svg")
            }
            Self::Palette => egui::include_image!("../../assets/icons/bootstrap/palette.svg"),
            Self::Toggles => egui::include_image!("../../assets/icons/bootstrap/toggles.svg"),
            Self::Sliders => egui::include_image!("../../assets/icons/bootstrap/sliders.svg"),
            Self::Keyboard => egui::include_image!("../../assets/icons/bootstrap/keyboard.svg"),
            Self::About => egui::include_image!("../../assets/icons/bootstrap/info-circle.svg"),
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

/// Text marks guaranteed by egui's bundled fonts.
/// Missing marks such as the paperclip and chevron are painted instead.
pub const STAR: &str = "★";
/// Separates the facts at the end of a conversation row.
pub const DOT: &str = "·";

pub fn install(context: &egui::Context) {
    super::fonts::apply(context, super::fonts::Typeface::Bundled);
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
        style.visuals.selection.stroke = Stroke::new(0.0, ON_ACCENT);
        style.visuals.hyperlink_color = colors.hyperlink;
        // egui draws only hint text with it in this app.
        style.visuals.weak_text_color = Some(colors.placeholder);
        style.visuals.widgets.noninteractive.fg_stroke.color = colors.text;
        style.visuals.widgets.inactive.fg_stroke.color = colors.text;
        style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, colors.border);
        style.visuals.widgets.hovered.bg_stroke = style.visuals.widgets.inactive.bg_stroke;
        style.visuals.widgets.active.bg_stroke = style.visuals.widgets.inactive.bg_stroke;
        let widget_corners = CornerRadius::same(WIDGET_CORNER_RADIUS);
        style.visuals.widgets.inactive.corner_radius = widget_corners;
        style.visuals.widgets.hovered.corner_radius = widget_corners;
        style.visuals.widgets.active.corner_radius = widget_corners;
        style.visuals.widgets.open.corner_radius = widget_corners;
        style.text_styles = text_styles(1.0);
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        // Solid scrollbars reserve space instead of covering content.
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.scroll_animation = gliding_scroll();
        // Only message bodies opt into selectable labels.
        style.interaction.selectable_labels = false;
        context.set_style_of(mode, style);
    }
    context.data_mut(|data| data.remove::<f32>(Id::new(TEXT_SCALE)));
    context.set_theme(egui::Theme::Dark);
    // The zoom is a setting, which the app reapplies every pass; egui's own
    // keys would draw one frame at another scale and snap back.
    context.options_mut(|options| options.zoom_with_keyboard = false);
}

/// Where [`set_text_scale`] keeps the scale text is drawn at.
const TEXT_SCALE: &str = "ruston-text-scale";

/// The share of the sizes above that text is drawn at: 1 unless Settings
/// asks for larger or smaller text. A length made to hold text, such as a
/// row's height or the step from one line of a header to the next, is drawn
/// at it too; every other length keeps its size, which only the zoom changes.
pub fn text_scale(context: &egui::Context) -> f32 {
    context
        .data(|data| data.get_temp::<f32>(Id::new(TEXT_SCALE)))
        .unwrap_or(1.0)
}

/// `size`, one of the sizes above or a length made to hold text, at the
/// scale text is drawn at.
pub fn text_size(ui: &egui::Ui, size: f32) -> f32 {
    size * text_scale(ui.ctx())
}

/// Draws text at `scale` times the sizes above: egui's own text styles,
/// which its widgets read, and every size [`text_size`] gives.
pub fn set_text_scale(context: &egui::Context, scale: f32) {
    if (text_scale(context) - scale).abs() <= f32::EPSILON {
        return;
    }
    context.all_styles_mut(|style| style.text_styles = text_styles(scale));
    context.data_mut(|data| data.insert_temp(Id::new(TEXT_SCALE), scale));
}

/// egui's text styles at `scale`: its own sizes, but for small text, whose
/// 9 pt is too small for the notices and details drawn with it.
fn text_styles(scale: f32) -> BTreeMap<egui::TextStyle, egui::FontId> {
    let mut styles = egui::style::default_text_styles();
    styles.insert(
        egui::TextStyle::Small,
        egui::FontId::proportional(FOOTNOTE_SIZE),
    );
    for font in styles.values_mut() {
        font.size *= scale;
    }
    styles
}

/// Draws in `appearance`.
pub fn apply(context: &egui::Context, appearance: Appearance) {
    let mode = egui_theme(appearance);
    prefer(context, mode.into(), mode);
}

/// Draws in the system's light or dark mode, which egui follows as it
/// changes, and in `appearance` where the system tells none.
pub fn follow_system(context: &egui::Context, appearance: Appearance) {
    prefer(
        context,
        egui::ThemePreference::System,
        egui_theme(appearance),
    );
}

/// How a scroll to a place glides there while motion is not reduced.
fn gliding_scroll() -> egui::style::ScrollAnimation {
    egui::style::ScrollAnimation::new(1_200.0, egui::Rangef::new(0.08, 0.24))
}

/// Holds the interface still while the system asks for less motion: no
/// fades or gliding scrolls, and loading pulses that keep the still picture
/// an unfocused window gets.
pub fn reduce_motion(context: &egui::Context, reduced: bool) {
    let (animation_time, scroll) = if reduced {
        (0.0, egui::style::ScrollAnimation::none())
    } else {
        (egui::Style::default().animation_time, gliding_scroll())
    };
    let style = context.global_style();
    if style.animation_time != animation_time || style.scroll_animation != scroll {
        context.all_styles_mut(|style| {
            style.animation_time = animation_time;
            style.scroll_animation = scroll;
        });
    }
}

fn egui_theme(appearance: Appearance) -> egui::Theme {
    match appearance {
        Appearance::Dark => egui::Theme::Dark,
        Appearance::Light => egui::Theme::Light,
    }
}

fn prefer(context: &egui::Context, preference: egui::ThemePreference, fallback: egui::Theme) {
    let current = context.options(|options| (options.theme_preference, options.fallback_theme));
    if current != (preference, fallback) {
        context.options_mut(|options| {
            options.theme_preference = preference;
            options.fallback_theme = fallback;
        });
    }
}

/// Gives a window's title bar back the theme egui gave it, when Windows may
/// have taken it away. egui tells a window its title bar's theme when the
/// app's theme changes, but winit 0.30 gives the bar the Windows theme again
/// whenever Windows announces a changed setting, which can leave a dark bar
/// over a light window. That announcement does not reach the app, so the
/// request is repeated when the Windows theme changes and when the window
/// comes back to the front, which usually follows a change made elsewhere.
/// Never every pass, since each request repaints the window's frame.
#[derive(Default)]
pub struct TitleBar {
    /// What the window was like at the last pass.
    seen: Option<Seen>,
}

/// The Windows theme and whether the window had focus, at one pass.
#[derive(Clone, Copy)]
struct Seen {
    system_theme: Option<egui::Theme>,
    focused: bool,
}

impl TitleBar {
    /// Repeats egui's title bar theme for the window `context` is drawing,
    /// when it may need it. Other systems keep their title bars.
    pub fn keep(&mut self, context: &egui::Context) {
        if !cfg!(target_os = "windows") {
            return;
        }
        // `raw` holds the focus even when only the app's logic runs.
        let now = context.input(|input| Seen {
            system_theme: input.raw.system_theme,
            focused: input.raw.focused,
        });
        let may_need_theme = self.seen.is_some_and(|seen| {
            seen.system_theme != now.system_theme || (now.focused && !seen.focused)
        });
        self.seen = Some(now);
        if may_need_theme {
            // As egui itself sends it.
            let theme = match context.options(|options| options.theme_preference) {
                egui::ThemePreference::System => egui::SystemTheme::SystemDefault,
                egui::ThemePreference::Dark => egui::SystemTheme::Dark,
                egui::ThemePreference::Light => egui::SystemTheme::Light,
            };
            context.send_viewport_cmd(egui::ViewportCommand::SetTheme(theme));
        }
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
        CornerRadius::same(FOCUS_RING_RADIUS),
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

/// The loading mark of the screens that only wait: a ring whose ink pulses
/// on the skeletons' clock. An arc turning once a second jumped 18° on each
/// of that clock's frames; a pulse changes too little between them to jump.
pub fn spinner(ui: &mut egui::Ui) -> Response {
    let size = ui.style().spacing.interact_size.y;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    response.widget_info(|| WidgetInfo::new(WidgetType::ProgressIndicator));
    if ui.is_rect_visible(rect) {
        let colors = colors(ui);
        let ink = pulse(animation_time(ui), colors.skeleton_high, colors.muted);
        let radius = rect.height().min(rect.width()) / 2.0 - 2.0;
        ui.painter()
            .circle_stroke(rect.center(), radius, Stroke::new(3.0, ink));
    }
    response
}

pub fn paint_skeleton(painter: &egui::Painter, rect: egui::Rect, fill: Color32) {
    painter.rect_filled(rect, CornerRadius::same(SKELETON_CORNER_RADIUS), fill);
}

fn skeleton_fill_at(time: f64, colors: Colors) -> Color32 {
    pulse(time, colors.skeleton_low, colors.skeleton_high)
}

/// A colour that rises from `low` to `high` and back over the skeletons'
/// period.
fn pulse(time: f64, low: Color32, high: Color32) -> Color32 {
    let wave = ((time * std::f64::consts::TAU / SKELETON_PERIOD_SECONDS).sin() as f32 + 1.0) * 0.5;
    let channel =
        |low: u8, high: u8| (low as f32 + (high as f32 - low as f32) * wave).round() as u8;
    Color32::from_rgb(
        channel(low.r(), high.r()),
        channel(low.g(), high.g()),
        channel(low.b(), high.b()),
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
        ctx.data(|data| data.get_temp::<f32>(native_titlebar_height_id()))
            .unwrap_or(DEFAULT_TITLEBAR_HEIGHT)
            / ctx.zoom_factor()
    } else {
        0.0
    }
}

/// The room the traffic lights take until the window reports its own.
const DEFAULT_TITLEBAR_HEIGHT: f32 = 28.0;

fn native_titlebar_height_id() -> egui::Id {
    egui::Id::new("native-titlebar-height")
}

/// Keeps the height the window reports for its traffic lights, in macOS
/// points, for [`titlebar_inset`] to leave that much room.
#[cfg(any(target_os = "macos", test))]
pub fn set_native_titlebar_height(ctx: &egui::Context, height: f32) {
    ctx.data_mut(|data| data.insert_temp(native_titlebar_height_id(), height));
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
        .corner_radius(CornerRadius::same(CARD_CORNER_RADIUS))
        .inner_margin(14)
}

#[cfg(test)]
mod tests;
