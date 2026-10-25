use std::time::Duration;

use crate::settings::Appearance;
use eframe::egui::{
    self, Atom, AtomLayoutResponse, Color32, CornerRadius, Id, Response, Stroke, Widget,
    WidgetInfo, WidgetType,
};

pub const ACCENT: Color32 = Color32::from_rgb(109, 92, 245);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(100, 83, 226);
pub const ACCENT_PRESSED: Color32 = Color32::from_rgb(86, 69, 198);
/// Text and marks drawn on the accent color.
pub const ON_ACCENT: Color32 = Color32::WHITE;
pub const PANEL_PADDING: i8 = 16;
pub const FOOTNOTE_SIZE: f32 = 11.0;
/// Text in a conversation row and a message header: names, subjects and
/// previews.
pub const ROW_TEXT_SIZE: f32 = 14.0;
/// A message's body, in the reader and the composer.
pub const BODY_SIZE: f32 = 15.0;
/// The app's name heading the sign-in screen and the mailbox's sidebar.
pub const APP_NAME_SIZE: f32 = 23.0;
pub const ICON_BUTTON_MIN_SIZE: egui::Vec2 = egui::Vec2::new(32.0, 28.0);
const SKELETON_PERIOD_SECONDS: f64 = 1.6;
/// How long a looping animation waits before asking for its next frame.
const ANIMATION_FRAME: Duration = Duration::from_millis(50);
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
            ON_ACCENT
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

/// An icon button that opens a menu: drawn, sized and announced like the
/// other icon buttons, with the menu egui's `MenuButton` would open.
pub fn icon_menu<R>(
    ui: &mut egui::Ui,
    enabled: bool,
    button: IconButton<'_>,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<egui::InnerResponse<R>> {
    use egui::containers::menu::MenuConfig;
    let response = ui.add_enabled(enabled, button);
    let config = MenuConfig::find(ui);
    egui::Popup::menu(&response)
        .close_behavior(config.close_behavior)
        .style(config.style.clone())
        .info(
            egui::UiStackInfo::new(egui::UiKind::Menu)
                .with_tag_value(MenuConfig::MENU_CONFIG_TAG, config),
        )
        .show(content)
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
        Stroke::new(CHECK_MARK_WIDTH, ON_ACCENT),
    ));
}

/// A single-line field with enough vertical room for comfortable reading and clicking.
pub fn text_field<'t>(ui: &egui::Ui, text: &'t mut String) -> egui::TextEdit<'t> {
    egui::TextEdit::singleline(text).frame(field_frame(ui))
}

/// The names screen readers hear for the text fields in a frame: a field's
/// own label, or the text of the labels that point at it.
#[cfg(test)]
pub(super) fn text_field_names(output: &egui::FullOutput) -> Vec<String> {
    use egui::accesskit::Role;

    let nodes: std::collections::HashMap<_, _> = output
        .platform_output
        .accesskit_update
        .iter()
        .flat_map(|update| update.nodes.iter().cloned())
        .collect();
    nodes
        .values()
        .filter(|node| {
            matches!(
                node.role(),
                Role::TextInput | Role::MultilineTextInput | Role::PasswordInput
            )
        })
        .map(|node| match node.label() {
            Some(label) => label.to_owned(),
            None => node
                .labelled_by()
                .iter()
                .filter_map(|id| nodes.get(id))
                .filter_map(|label| label.label().or(label.value()))
                .collect::<Vec<_>>()
                .join(" "),
        })
        .collect()
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
        state.fg_stroke.color = ON_ACCENT;
    }
}

/// Gives a button in `ui` that senses no clicks, because it only shows that
/// something is under way, the look the same button has at rest. egui would
/// otherwise draw it in the plain colors of text, and a disabled one at half
/// strength, which white on the accent does not survive.
pub fn busy_visuals(ui: &mut egui::Ui) {
    let widgets = &mut ui.visuals_mut().widgets;
    widgets.noninteractive = widgets.inactive;
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
    confirmation_with(
        context,
        id,
        question,
        |ui| {
            ui.label(detail);
        },
        choices,
        dismissed,
    )
}

/// [`confirmation`] with a detail drawn by `detail` instead of one line.
pub fn confirmation_with<T: Clone>(
    context: &egui::Context,
    id: &str,
    question: &str,
    detail: impl FnOnce(&mut egui::Ui),
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
            detail(ui);
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
        style.scroll_animation = gliding_scroll();
        // Only message bodies opt into selectable labels.
        style.interaction.selectable_labels = false;
        context.set_style_of(mode, style);
    }
    context.set_theme(egui::Theme::Dark);
    // The zoom is a setting, which the app reapplies every pass; egui's own
    // keys would draw one frame at another scale and snap back.
    context.options_mut(|options| options.zoom_with_keyboard = false);
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
    painter.rect_filled(rect, CornerRadius::same(4), fill);
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
mod tests;
