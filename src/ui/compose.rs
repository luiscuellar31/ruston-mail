use std::path::PathBuf;

use eframe::egui::{self, Align, Layout};

use super::settings::ZoomStep;
use super::theme::{self, Emphasis};
use super::{identity, mailbox::detail, reader};
use crate::app::{AnswerKind, Answering, Compose, ComposeField, ComposeId, Message, Sending};
use crate::mail::SendError;
use crate::settings::MIN_WINDOW_SIZE;

const WINDOW_SIZE: [f32; 2] = [640.0, 600.0];

pub(super) fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("compose-window")
}

pub(super) fn show_in_pane(ui: &mut egui::Ui, compose: &Compose, messages: &mut Vec<Message>) {
    ui.scope(|ui| {
        ui.spacing_mut().scroll = theme::panel_scroll_style();
        egui::ScrollArea::vertical()
            .id_salt("compose-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| page(ui, compose, messages));
    });
    drop_hint(ui, ui.max_rect());
}

/// While files are dragged over the window, says that the draft takes them.
fn drop_hint(ui: &egui::Ui, rect: egui::Rect) {
    if ui.ctx().input(|input| input.raw.hovered_files.is_empty()) {
        return;
    }
    let colors = theme::colors(ui);
    ui.painter().rect(
        rect.shrink(4.0),
        8.0,
        colors.accent_soft,
        ui.visuals().selection.stroke,
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "Drop files to attach",
        egui::FontId::proportional(18.0),
        ui.visuals().strong_text_color(),
    );
}

pub(super) fn window_viewport(compose: &Compose) -> egui::ViewportBuilder {
    identity::viewport()
        .with_title(heading(compose))
        .with_inner_size(WINDOW_SIZE)
        .with_min_inner_size(MIN_WINDOW_SIZE)
}

/// What the composer's own window hands the shell after one pass.
pub(super) struct WindowPass {
    pub(super) messages: Vec<Message>,
    /// The step its zoom keys ask for: zoom applies to both windows, as from
    /// the mailbox window.
    pub(super) zoom: Option<ZoomStep>,
    /// Files dropped on the window, which the shell attaches when it may.
    pub(super) dropped: Vec<PathBuf>,
}

/// Shows the composer in its own window. `edits` are the window's Edit menu
/// commands, which reach its text as the keys would have; `title_bar` keeps
/// the window's title bar in the app's theme on Windows.
pub(super) fn show_window(
    context: &egui::Context,
    compose: &Compose,
    blocked: bool,
    mut edits: Vec<egui::Event>,
    title_bar: &mut theme::TitleBar,
) -> WindowPass {
    context.show_viewport_immediate(viewport_id(), window_viewport(compose), |root, _class| {
        title_bar.keep(root.ctx());
        root.ctx()
            .input_mut(|input| input.events.append(&mut edits));
        let zoom = super::zoom_keys(root.ctx());
        let dropped = super::dropped_paths(root.ctx());
        let mut messages = Vec::new();
        root.add_enabled_ui(!blocked, |root| show(root, compose, &mut messages));
        theme::paint_focus_ring(root.ctx());

        if blocked {
            if root.input(|input| input.viewport().close_requested()) {
                root.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
            return WindowPass {
                messages: Vec::new(),
                zoom,
                dropped: Vec::new(),
            };
        }

        if root.input(|input| input.viewport().close_requested()) {
            if compose.in_flight() {
                root.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            } else if compose.is_untouched() || compose.confirming_discard() {
                messages.push(Message::DiscardCompose);
            } else {
                root.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::CancelClose);
                messages.push(Message::CloseCompose);
            }
        } else if !egui::Popup::is_any_open(root.ctx())
            && root.input(|input| input.key_pressed(egui::Key::Escape))
        {
            // An open menu takes Escape first, as in the mailbox window.
            if compose.confirming_discard() {
                messages.push(Message::CancelDiscard);
            } else {
                messages.push(Message::CloseCompose);
            }
        } else if root.input(|input| input.modifiers.command && input.key_pressed(egui::Key::Enter))
            && compose.not_ready().is_none()
            && !compose.in_flight()
        {
            messages.push(Message::Send);
        }

        WindowPass {
            messages,
            zoom,
            dropped,
        }
    })
}

fn show(root: &mut egui::Ui, compose: &Compose, messages: &mut Vec<Message>) {
    egui::CentralPanel::default()
        .frame(theme::panel_frame(theme::colors(root).panel))
        .show(root, |ui| show_in_pane(ui, compose, messages));
}

fn page(ui: &mut egui::Ui, compose: &Compose, messages: &mut Vec<Message>) {
    let sending = compose.sending();
    let leaving = sending == Sending::InFlight;
    focus_new_draft(ui, compose);

    title_and_actions(ui, compose, leaving, send_label(&sending), messages);
    // Why the last send failed, right under Send, where the person looks once
    // they have pressed it.
    if let Sending::Failed(error) = &sending {
        ui.label(egui::RichText::new(error.message()).color(theme::colors(ui).danger));
    }
    ui.add_space(10.0);
    if compose.asked_again() {
        ui.label("You are already writing a message. Send or discard it to start another.");
        ui.add_space(10.0);
    }
    if compose.confirming_discard()
        && let Some(answer) = discard_prompt(ui.ctx())
    {
        messages.push(answer);
    }
    if let Some(answer) = compose.answering() {
        answered(ui, answer, compose.asks_for_recipients());
    }
    if compose.asks_for_recipients() {
        recipient_fields(ui, compose, leaving, messages);
        ui.add_space(4.0);
    }
    if compose.asks_for_subject() {
        line(
            ui,
            "Subject",
            ComposeField::Subject,
            compose,
            leaving,
            messages,
        );
    }
    ui.add_space(6.0);
    if !compose.attachments().is_empty() {
        attachment_list(ui, compose, leaving, messages);
        ui.add_space(4.0);
    }
    ui.add_space(6.0);
    if !matches!(sending, Sending::Failed(_)) {
        not_ready(ui, compose);
    }
    ui.add_space(6.0);
    body_editor(ui, compose, leaving, messages);
}

/// The title, with Send, Discard and Attach… beside it.
fn title_and_actions(
    ui: &mut egui::Ui,
    compose: &Compose,
    leaving: bool,
    send_label: &str,
    messages: &mut Vec<Message>,
) {
    ui.horizontal_wrapped(|ui| {
        ui.heading(egui::RichText::new(heading(compose)).size(24.0).strong());
        ui.with_layout(
            Layout::right_to_left(Align::Center).with_main_wrap(true),
            |ui| {
                // Nothing is offered while the message is on its way: it is out
                // of the sender's hands, and a second press must not send twice.
                let ready = compose.not_ready().is_none();
                if ui
                    .add_enabled(
                        ready && !leaving,
                        theme::primary(egui::Button::new(send_label)),
                    )
                    .clicked()
                {
                    messages.push(Message::Send);
                }
                if ui
                    .add_enabled(!leaving, egui::Button::new("Discard"))
                    .clicked()
                {
                    messages.push(Message::CloseCompose);
                }
                if ui
                    .add_enabled(!leaving, egui::Button::new("Attach…"))
                    .clicked()
                {
                    messages.push(Message::PickComposeAttachments);
                }
            },
        );
    });
}

/// What the Send button says for where the send stands.
fn send_label(sending: &Sending) -> &'static str {
    match sending {
        Sending::InFlight => "Sending…",
        Sending::Failed(SendError::Unconfirmed) => "Send again",
        _ => "Send",
    }
}

/// Asks whether to throw the written message away, over the window the
/// draft is in, so the form under the question stays where it is.
fn discard_prompt(context: &egui::Context) -> Option<Message> {
    theme::confirmation(
        context,
        "discard-draft",
        "Discard unsaved message?",
        "What you wrote will be lost.",
        &[
            ("Keep writing", Emphasis::Secondary, Message::CancelDiscard),
            ("Discard", Emphasis::Danger, Message::DiscardCompose),
        ],
        Message::CancelDiscard,
    )
}

/// To, and Cc and Bcc once asked for; what the typed addresses mean for how
/// the message is protected; and the button that shows or hides the copies.
fn recipient_fields(
    ui: &mut egui::Ui,
    compose: &Compose,
    leaving: bool,
    messages: &mut Vec<Message>,
) {
    let mut left = line(ui, "To", ComposeField::To, compose, leaving, messages).lost_focus();
    if compose.showing_copies() {
        left |= line(ui, "Cc", ComposeField::Cc, compose, leaving, messages).lost_focus();
        left |= line(ui, "Bcc", ComposeField::Bcc, compose, leaving, messages).lost_focus();
    }
    // Addresses are looked up once typing has moved on, never per keystroke.
    if left {
        messages.push(Message::ComposeRecipientsLeft);
    }
    if let Some(notice) = compose.international_notice() {
        ui.label(egui::RichText::new(notice).color(theme::colors(ui).danger));
    }
    if let Some(notice) = compose.protection_notice() {
        if notice.is_warning() {
            // A copy that may leave without end-to-end encryption must
            // stand out from the field labels.
            ui.label(egui::RichText::new(notice.message()).color(theme::colors(ui).danger));
        } else {
            detail(ui, &notice.message());
        }
    }
    let more = if compose.showing_copies() {
        "Fewer options"
    } else {
        "More options"
    };
    if ui
        .add_enabled(!leaving, theme::compact_button(more))
        .clicked()
    {
        messages.push(Message::ToggleComposeCopies);
    }
}

/// The attached files, each with its size and a Remove button.
fn attachment_list(
    ui: &mut egui::Ui,
    compose: &Compose,
    leaving: bool,
    messages: &mut Vec<Message>,
) {
    detail(ui, "Attachments");
    for (index, path) in compose.attachments().iter().enumerate() {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Attachment");
        let size = compose
            .attachment_size(path)
            .map(|bytes| format!(" ({})", reader::size_label(bytes)))
            .unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("📎 {name}{size}")).color(ui.visuals().text_color()),
            );
            if ui
                .add_enabled(!leaving, theme::compact_button("Remove"))
                .clicked()
            {
                messages.push(Message::RemoveComposeAttachment(index));
            }
        });
    }
}

/// What still stops this message from going, under the fields it is about.
/// A failed send says why under Send instead, and takes priority.
fn not_ready(ui: &mut egui::Ui, compose: &Compose) {
    if let Some(problem) = compose.not_ready().filter(|_| !compose.is_untouched()) {
        detail(ui, &problem.message());
    }
}

/// The message body.
fn body_editor(ui: &mut egui::Ui, compose: &Compose, leaving: bool, messages: &mut Vec<Message>) {
    let mut body = compose.field(ComposeField::Body).to_owned();
    let response = ui.add_enabled(
        !leaving,
        egui::TextEdit::multiline(&mut body)
            .id(body_id(compose.id()))
            .hint_text("Write your message…")
            // What is written reads at the size it will be read at.
            .font(egui::FontId::proportional(theme::BODY_SIZE))
            .frame(theme::field_frame(ui))
            .desired_width(f32::INFINITY)
            .desired_rows(16),
    );
    // No label is drawn above the body, and its hint is only a placeholder.
    ui.ctx()
        .accesskit_node_builder(response.id, |node| node.set_label("Message"));
    if response.changed() {
        messages.push(Message::ComposeChanged(ComposeField::Body, body));
    }
}

/// Puts the cursor where writing starts the first time a draft shows: in To
/// for a new message or a forward, in the body for a reply. Asked before the
/// fields are made, so they take it on this pass.
fn focus_new_draft(ui: &egui::Ui, compose: &Compose) {
    let shown = egui::Id::new("compose-focused-draft");
    let draft = compose.id();
    if ui.data(|data| data.get_temp::<ComposeId>(shown)) == Some(draft) {
        return;
    }
    ui.data_mut(|data| data.insert_temp(shown, draft));
    let field = if compose.asks_for_recipients() {
        field_id(ComposeField::To, draft)
    } else {
        body_id(draft)
    };
    ui.memory_mut(|memory| memory.request_focus(field));
}

/// A draft folded above the reader: what it is, and the way back to it. A
/// long subject is shortened, whole on hover, so the button stays in view.
pub(super) fn folded_strip(ui: &mut egui::Ui, compose: &Compose, messages: &mut Vec<Message>) {
    let subject = compose.field(ComposeField::Subject).trim();
    let about = if subject.is_empty() {
        heading(compose)
    } else {
        subject
    };
    theme::card(ui).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(theme::compact_button("Continue writing")).clicked() {
                    messages.push(Message::ContinueWriting);
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.add(egui::Label::new(format!("Draft: {about}")).truncate());
                });
            });
        });
    });
    ui.add_space(8.0);
}

fn heading(compose: &Compose) -> &'static str {
    match compose.answering() {
        None => "New message",
        Some(answer) => match answer.kind {
            AnswerKind::Reply => "Reply",
            AnswerKind::ReplyAll => "Reply to everyone",
            AnswerKind::Forward => "Forward",
        },
    }
}

/// What is being answered and, for a reply, exactly where it goes.
fn answered(ui: &mut egui::Ui, answer: &Answering, addressed_here: bool) {
    detail(
        ui,
        &format!("Answering “{}” from {}.", answer.subject, answer.sender),
    );
    // What goes out with the text, though the composer does not show it.
    if answer.quotes_original {
        detail(ui, "The original is quoted under your text.");
    }
    if !answer.forwarded.is_empty() {
        detail(
            ui,
            &format!(
                "Its attachments go with it: {}.",
                answer.forwarded.join(", ")
            ),
        );
    }
    if !addressed_here {
        detail(ui, &format!("To: {}", answer.to.join(", ")));
        if !answer.cc.is_empty() {
            detail(ui, &format!("Cc: {}", answer.cc.join(", ")));
        }
        // A Reply-To header can send the answer somewhere the sender is not.
        if let Some(sender) = &answer.redirected_from {
            ui.label(
                egui::RichText::new(format!(
                    "The message asks for replies to go to {}, not to its sender, {sender}.",
                    answer.to.join(", ")
                ))
                .color(theme::colors(ui).danger),
            );
        }
    }
    ui.add_space(8.0);
}

/// One labelled single-line field.
fn line(
    ui: &mut egui::Ui,
    label: &str,
    field: ComposeField,
    compose: &Compose,
    leaving: bool,
    messages: &mut Vec<Message>,
) -> egui::Response {
    let name = ui.label(egui::RichText::new(label).color(theme::colors(ui).muted));
    let mut value = compose.field(field).to_owned();
    let response = ui
        .add_enabled(
            !leaving,
            theme::text_field(ui, &mut value)
                .id(field_id(field, compose.id()))
                .desired_width(f32::INFINITY),
        )
        .labelled_by(name.id);
    if response.changed() {
        messages.push(Message::ComposeChanged(field, value));
    }
    response
}

/// Every field a draft has an editor for, so forgetting a draft reaches them
/// all.
const FIELDS: [ComposeField; 5] = [
    ComposeField::To,
    ComposeField::Cc,
    ComposeField::Bcc,
    ComposeField::Subject,
    ComposeField::Body,
];

/// Every draft gets its own editors: their undo history holds what was typed,
/// and must never reach into another draft or account. An editor is known by
/// its field, never by the label it shows.
fn field_id(field: ComposeField, draft: ComposeId) -> egui::Id {
    egui::Id::new(("compose", field, draft))
}

fn body_id(draft: ComposeId) -> egui::Id {
    field_id(ComposeField::Body, draft)
}

/// Drops the editor memory of a draft that is gone, with the text its undo
/// history kept.
pub(super) fn forget_draft(context: &egui::Context, draft: ComposeId) {
    context.data_mut(|data| {
        for id in draft_editors(draft) {
            data.remove::<egui::text_edit::TextEditState>(id);
        }
    });
}

fn draft_editors(draft: ComposeId) -> impl Iterator<Item = egui::Id> {
    FIELDS.into_iter().map(move |field| field_id(field, draft))
}

/// Gives every editor of `draft` some memory, as typing would.
#[cfg(test)]
pub(super) fn remember_draft(context: &egui::Context, draft: ComposeId) {
    for id in draft_editors(draft) {
        egui::text_edit::TextEditState::default().store(context, id);
    }
}

/// Whether any editor of `draft` still has memory.
#[cfg(test)]
pub(super) fn remembers_draft(context: &egui::Context, draft: ComposeId) -> bool {
    draft_editors(draft).any(|id| egui::text_edit::TextEditState::load(context, id).is_some())
}

#[cfg(test)]
mod tests;
