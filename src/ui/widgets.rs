//! The widgets the screens share: icon buttons and their menus, the check
//! box, text fields, buttons and their emphasis, and confirmations. Their
//! colours, sizes and icons are the theme's.

use eframe::egui::{
    self, Atom, Color32, CornerRadius, Id, Response, Stroke, Widget, WidgetInfo, WidgetType,
};

use super::theme::{
    ACCENT, ACCENT_HOVER, ACCENT_PRESSED, CHECK_MARK_WIDTH, CHECKBOX_RADIUS, COMPACT_BUTTON_HEIGHT,
    DIALOG_BUTTON_HEIGHT, FIELD_MARGIN, ICON_BUTTON_MIN_SIZE, Icon, ON_ACCENT, colors, icon_atom,
    paint_atom_icon, text_size,
};

const CHECKBOX_ATOM_ID: &str = "ruston-check-box";

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
            .min_size(icon_button_size(ui));
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

/// An icon button's size: as tall as a button of words, which grows with the
/// text, so a row of both keeps one height, and never under its usual size.
pub fn icon_button_size(ui: &egui::Ui) -> egui::Vec2 {
    // Measured as a button lays its words out, rounded to the same pixels.
    let font = egui::TextStyle::Button.resolve(ui.style());
    let line = ui
        .fonts_mut(|fonts| fonts.layout_no_wrap("M".into(), font, Color32::PLACEHOLDER))
        .size()
        .y;
    let words = line + 2.0 * ui.spacing().button_padding.y;
    egui::vec2(ICON_BUTTON_MIN_SIZE.x, ICON_BUTTON_MIN_SIZE.y.max(words))
}

/// An icon button that opens a menu: drawn, sized and announced like the
/// other icon buttons, with the menu egui's `MenuButton` would open.
pub fn icon_menu<R>(
    ui: &mut egui::Ui,
    enabled: bool,
    button: IconButton<'_>,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<egui::InnerResponse<R>> {
    let response = ui.add_enabled(enabled, button);
    menu_below(ui, &response, content)
}

/// The menu a click on `button` opens, as egui's `MenuButton` opens one.
pub fn menu_below<R>(
    ui: &egui::Ui,
    button: &Response,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<egui::InnerResponse<R>> {
    use egui::containers::menu::MenuConfig;
    let config = MenuConfig::find(ui);
    egui::Popup::menu(button)
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
pub fn compact_button<'a>(ui: &egui::Ui, atoms: impl egui::IntoAtoms<'a>) -> egui::Button<'a> {
    egui::Button::new(atoms)
        .small()
        .min_size(egui::vec2(0.0, compact_button_height(ui)))
}

/// The height of [`compact_button`] at the scale text is drawn at, which a
/// line of words beside one takes.
pub fn compact_button_height(ui: &egui::Ui) -> f32 {
    text_size(ui, COMPACT_BUTTON_HEIGHT)
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

#[cfg(test)]
mod tests;
