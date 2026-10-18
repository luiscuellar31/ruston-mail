use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui::{self, Align, Layout};

use super::settings::ZoomStep;
use super::{identity, mailbox::detail, reader, theme};
use crate::app::{AnswerKind, Answering, Compose, ComposeField, ComposeId, Message, Sending};
use crate::mail::SendError;

const WINDOW_SIZE: [f32; 2] = [640.0, 600.0];
const MIN_WINDOW_SIZE: [f32; 2] = [480.0, 420.0];

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

    let heading = heading(compose);
    ui.horizontal_wrapped(|ui| {
        ui.heading(egui::RichText::new(heading).size(24.0).strong());
        ui.with_layout(
            Layout::right_to_left(Align::Center).with_main_wrap(true),
            |ui| {
                // Nothing is offered while the message is on its way: it is out
                // of the sender's hands, and a second press must not send twice.
                let ready = compose.not_ready().is_none();
                if ui
                    .add_enabled(
                        ready && !leaving,
                        theme::primary(egui::Button::new(if leaving {
                            "Sending…"
                        } else if sending == Sending::Failed(SendError::Unconfirmed) {
                            "Send again"
                        } else {
                            "Send"
                        })),
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
    ui.add_space(10.0);

    if compose.confirming_discard() {
        theme::card(ui)
            .stroke(egui::Stroke::new(1.0, theme::colors(ui).danger))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("Discard unsaved message?")
                            .strong()
                            .color(theme::colors(ui).danger),
                    );
                    ui.with_layout(
                        Layout::right_to_left(Align::Center).with_main_wrap(true),
                        |ui| {
                            if ui
                                .add(theme::danger(egui::Button::new("Discard")))
                                .clicked()
                            {
                                messages.push(Message::DiscardCompose);
                            }
                            if ui.button("Keep writing").clicked() {
                                messages.push(Message::CancelDiscard);
                            }
                        },
                    );
                });
            });
        ui.add_space(10.0);
    }

    if let Some(answer) = compose.answering() {
        answered(ui, answer, compose.asks_for_recipients());
    }

    if compose.asks_for_recipients() {
        let mut left = line(ui, "To", ComposeField::To, compose, leaving, messages).lost_focus();
        if compose.showing_more() {
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
        let more = if compose.showing_more() {
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
        detail(ui, "Attachments");
        let sizes = attachment_sizes(ui, compose);
        for (index, path) in compose.attachments().iter().enumerate() {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Attachment");
            let size = sizes
                .get(path)
                .copied()
                .flatten()
                .map(|bytes| format!(" ({})", reader::size_label(bytes)))
                .unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("📎 {name}{size}"))
                        .color(ui.visuals().text_color()),
                );
                if ui
                    .add_enabled(!leaving, theme::compact_button("Remove"))
                    .clicked()
                {
                    messages.push(Message::RemoveComposeAttachment(index));
                }
            });
        }
        ui.add_space(4.0);
    }

    ui.add_space(6.0);

    // A send result takes priority over validation hints.
    match sending {
        Sending::Failed(error) => {
            ui.label(egui::RichText::new(error.message()).color(theme::colors(ui).danger));
        }
        _ => {
            if let Some(problem) = compose.not_ready().filter(|_| !compose.is_untouched()) {
                detail(ui, &problem.message());
            }
        }
    }
    ui.add_space(6.0);

    let mut body = compose.field(ComposeField::Body).to_owned();
    let response = ui.add_enabled(
        !leaving,
        egui::TextEdit::multiline(&mut body)
            .id(body_id(compose.id()))
            .hint_text("Write your message…")
            // What is written reads at the size it will be read at.
            .font(egui::FontId::proportional(reader::BODY_SIZE))
            .frame(theme::field_frame(ui))
            .desired_width(f32::INFINITY)
            .desired_rows(16),
    );
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
        field_id("To", draft)
    } else {
        body_id(draft)
    };
    ui.memory_mut(|memory| memory.request_focus(field));
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
    ui.label(egui::RichText::new(label).color(theme::colors(ui).muted));
    let mut value = compose.field(field).to_owned();
    let response = ui.add_enabled(
        !leaving,
        theme::text_field(ui, &mut value)
            .id(field_id(label, compose.id()))
            .desired_width(f32::INFINITY),
    );
    if response.changed() {
        messages.push(Message::ComposeChanged(field, value));
    }
    response
}

/// The labelled fields a draft can show.
const FIELDS: [&str; 4] = ["To", "Cc", "Bcc", "Subject"];

/// Every draft gets its own editors: their undo history holds what was typed,
/// and must never reach into another draft or account.
fn body_id(draft: ComposeId) -> egui::Id {
    egui::Id::new(("compose-body", draft))
}

fn field_id(label: &str, draft: ComposeId) -> egui::Id {
    egui::Id::new(("compose", label, draft))
}

/// The size of each of a draft's attachments, as read when it first showed.
type AttachmentSizes = HashMap<PathBuf, Option<u64>>;

fn sizes_id(draft: ComposeId) -> egui::Id {
    egui::Id::new(("compose-attachment-sizes", draft))
}

/// Reads each attachment's size once. Asking the file system on every frame
/// held up the window for as long as a network share that went away took to
/// answer.
fn attachment_sizes(ui: &egui::Ui, compose: &Compose) -> AttachmentSizes {
    let id = sizes_id(compose.id());
    let mut sizes: AttachmentSizes = ui.data(|data| data.get_temp(id)).unwrap_or_default();
    let unread: Vec<_> = compose
        .attachments()
        .iter()
        .filter(|path| !sizes.contains_key(*path))
        .cloned()
        .collect();
    if !unread.is_empty() {
        for path in unread {
            let size = std::fs::metadata(&path).ok().map(|metadata| metadata.len());
            sizes.insert(path, size);
        }
        ui.data_mut(|data| data.insert_temp(id, sizes.clone()));
    }
    sizes
}

/// Drops the editor memory of a draft that is gone, with the text its undo
/// history kept, and the sizes of its attachments.
pub(super) fn forget_draft(context: &egui::Context, draft: ComposeId) {
    context.data_mut(|data| {
        for id in draft_editors(draft) {
            data.remove::<egui::text_edit::TextEditState>(id);
        }
        data.remove::<AttachmentSizes>(sizes_id(draft));
    });
}

fn draft_editors(draft: ComposeId) -> impl Iterator<Item = egui::Id> {
    std::iter::once(body_id(draft)).chain(FIELDS.map(|label| field_id(label, draft)))
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
mod tests {
    use super::*;

    #[test]
    fn a_small_composer_keeps_actions_visible_and_fields_scrollable() {
        let context = egui::Context::default();
        theme::install(&context);
        let (mut app, _) = crate::app::App::boot(true, crate::settings::Settings::default());
        app.update(Message::OpenCompose);
        app.update(Message::ToggleComposeCopies);
        let window = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(410.0, 240.0));
        let mut messages = Vec::new();
        for _ in 0..3 {
            let output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(window),
                    ..Default::default()
                },
                |ui| show(ui, app.compose().unwrap(), &mut messages),
            );
            for label in ["Send", "Discard", "Attach…"] {
                let (rect, clip) = output
                    .shapes
                    .iter()
                    .find_map(|clipped| {
                        if let egui::epaint::Shape::Text(text) = &clipped.shape
                            && text.galley.text() == label
                        {
                            return Some((
                                egui::Rect::from_min_size(text.pos, text.galley.size()),
                                clipped.clip_rect,
                            ));
                        }
                        None
                    })
                    .unwrap();
                assert!(
                    window.contains_rect(rect) && clip.contains_rect(rect),
                    "{label} is clipped: {rect:?} in {clip:?}"
                );
            }
            output.drop_without_applying_deltas();
        }
        assert!(!messages.iter().any(|message| matches!(
            message,
            Message::Send | Message::CloseCompose | Message::ComposeChanged(_, _)
        )));
    }

    #[test]
    fn the_composer_offers_one_attach_button_and_writes_at_reading_size() {
        let context = egui::Context::default();
        theme::install(&context);
        let (mut app, _) = crate::app::App::boot(true, crate::settings::Settings::default());
        app.update(Message::OpenCompose);
        let draft = app.compose().unwrap().id();
        app.update(Message::AddComposeAttachments(
            draft,
            vec![std::path::PathBuf::from("/nonexistent/notes.txt")],
        ));
        app.update(Message::ComposeChanged(ComposeField::Body, "Hello".into()));
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            show(ui, app.compose().unwrap(), &mut Vec::new());
        });
        let texts: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.clone()),
                _ => None,
            })
            .collect();
        output.drop_without_applying_deltas();

        let attach: Vec<_> = texts
            .iter()
            .map(|galley| galley.text())
            .filter(|text| text.starts_with("Attach") && *text != "Attachments")
            .collect();
        assert_eq!(attach, ["Attach…"]);
        let body = texts
            .iter()
            .find(|galley| galley.text() == "Hello")
            .expect("the body should be drawn");
        assert_eq!(body.job.sections[0].format.font_id.size, reader::BODY_SIZE);
    }

    #[test]
    fn an_attachment_size_is_read_once_and_forgotten_with_its_draft() {
        let file =
            std::env::temp_dir().join(format!("ruston-attachment-size-{}.txt", std::process::id()));
        std::fs::write(&file, [b'a'; 2048]).unwrap();
        let context = egui::Context::default();
        theme::install(&context);
        let (mut app, _) = crate::app::App::boot(true, crate::settings::Settings::default());
        app.update(Message::OpenCompose);
        let draft = app.compose().unwrap().id();
        app.update(Message::AddComposeAttachments(draft, vec![file.clone()]));
        let attachment_label = |app: &crate::app::App| {
            let output = context.run_ui(egui::RawInput::default(), |ui| {
                show(ui, app.compose().unwrap(), &mut Vec::new());
            });
            let label = output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::epaint::Shape::Text(text) if text.galley.text().starts_with('📎') => {
                        Some(text.galley.text().to_owned())
                    }
                    _ => None,
                })
                .expect("the attachment should be listed");
            output.drop_without_applying_deltas();
            label
        };

        let first = attachment_label(&app);
        assert!(first.ends_with(&format!("({})", reader::size_label(2048))));
        // Later frames do not ask again, so a file that went away with its
        // share keeps its label without holding up the window.
        std::fs::remove_file(&file).unwrap();
        assert_eq!(attachment_label(&app), first);

        forget_draft(&context, draft);
        assert!(
            context
                .data(|data| data.get_temp::<AttachmentSizes>(sizes_id(draft)))
                .is_none()
        );
    }

    #[test]
    fn undo_in_a_new_draft_never_brings_back_an_earlier_one() {
        let context = egui::Context::default();
        theme::install(&context);
        let (mut app, _) = crate::app::App::boot(true, crate::settings::Settings::default());
        let mut time = 0.0;
        let mut frame = |app: &crate::app::App, events: Vec<egui::Event>| {
            time += 1.5;
            let mut messages = Vec::new();
            context
                .run_ui(
                    egui::RawInput {
                        events,
                        time: Some(time),
                        ..Default::default()
                    },
                    |ui| show(ui, app.compose().unwrap(), &mut messages),
                )
                .drop_without_applying_deltas();
            messages
        };
        let body_id = |app: &crate::app::App| body_id(app.compose().unwrap().id());

        app.update(Message::OpenCompose);
        frame(&app, Vec::new());
        context.memory_mut(|memory| memory.request_focus(body_id(&app)));
        frame(&app, Vec::new());
        for message in frame(&app, vec![egui::Event::Text("private draft".into())]) {
            app.update(message);
        }
        // Pauses let the editor record what was typed as an undo step.
        frame(&app, Vec::new());
        frame(&app, Vec::new());
        app.update(Message::CloseCompose);
        app.update(Message::DiscardCompose);
        assert!(app.compose().is_none());

        app.update(Message::OpenCompose);
        frame(&app, Vec::new());
        context.memory_mut(|memory| memory.request_focus(body_id(&app)));
        frame(&app, Vec::new());
        let undo = egui::Event::Key {
            key: egui::Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        };
        let messages = frame(&app, vec![undo]);
        assert!(
            !messages
                .iter()
                .any(|message| matches!(message, Message::ComposeChanged(_, text) if text.contains("private draft"))),
            "undo brought back another draft"
        );
    }

    #[test]
    fn forgetting_a_draft_drops_only_its_editor_memory() {
        let context = egui::Context::default();
        remember_draft(&context, 1);
        remember_draft(&context, 2);

        forget_draft(&context, 1);

        assert!(!remembers_draft(&context, 1));
        assert!(remembers_draft(&context, 2));
    }

    #[test]
    fn a_reply_lists_its_recipients_and_warns_when_reply_to_redirects_it() {
        let context = egui::Context::default();
        theme::install(&context);
        let answer = Answering {
            sender: "Alex Rivera".into(),
            subject: "Invoice".into(),
            kind: AnswerKind::ReplyAll,
            to: vec!["attacker@evil.example".into()],
            cc: vec!["bob@corp.example".into()],
            redirected_from: Some("alex@corp.example".into()),
        };
        let mut danger = egui::Color32::PLACEHOLDER;
        let mut body = 0.0;
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            danger = theme::colors(ui).danger;
            body = egui::TextStyle::Body.resolve(ui.style()).size;
            answered(ui, &answer, false);
        });
        let texts: Vec<(String, bool)> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text) => Some((
                    text.galley.text().to_owned(),
                    text.galley.job.sections.iter().all(|section| {
                        section.format.color == danger && section.format.font_id.size == body
                    }),
                )),
                _ => None,
            })
            .collect();
        output.drop_without_applying_deltas();

        let shown = |wanted: &str| texts.iter().find(|(text, _)| text.contains(wanted));
        assert!(shown("To: attacker@evil.example").is_some());
        assert!(shown("Cc: bob@corp.example").is_some());
        let warning = shown("not to its sender, alex@corp.example").expect("redirect warning");
        assert!(warning.1, "the redirect warning stands out");
    }

    #[test]
    fn the_composer_names_recipients_whose_copy_is_not_encrypted() {
        use crate::app::Effect;
        use futures::executor::block_on;

        let context = egui::Context::default();
        theme::install(&context);
        let (mut app, _) = crate::app::App::boot(true, crate::settings::Settings::default());
        app.update(Message::OpenCompose);
        app.update(Message::ComposeChanged(
            ComposeField::To,
            "ada@proton.me, plain@example.com".into(),
        ));
        let shown = |app: &crate::app::App| {
            let mut messages = Vec::new();
            let mut danger = egui::Color32::PLACEHOLDER;
            let mut body = 0.0;
            let output = context.run_ui(egui::RawInput::default(), |ui| {
                danger = theme::colors(ui).danger;
                body = egui::TextStyle::Body.resolve(ui.style()).size;
                show(ui, app.compose().unwrap(), &mut messages);
            });
            // The warning is named and drawn in the danger color at full
            // size, not as a field label.
            let found = output.shapes.iter().any(|clipped| {
                matches!(&clipped.shape, egui::epaint::Shape::Text(text)
                    if text.galley.text().contains("not end-to-end encrypt this message for: plain@example.com")
                        && text.galley.job.sections.iter().all(|section| section.format.color == danger && section.format.font_id.size == body))
            });
            output.drop_without_applying_deltas();
            found
        };
        assert!(!shown(&app));

        let effect = app
            .update(Message::ComposeRecipientsLeft)
            .into_iter()
            .next()
            .unwrap();
        let Effect::Future(lookup) = effect else {
            panic!("the lookup runs off the UI thread");
        };
        app.update(block_on(lookup));

        assert!(shown(&app));
    }

    #[test]
    fn the_composer_shows_the_ascii_form_of_an_international_domain() {
        let context = egui::Context::default();
        theme::install(&context);
        let shown = |accept_international_domains| {
            let mut settings = crate::settings::Settings::default();
            settings.accept_international_domains = accept_international_domains;
            let (mut app, _) = crate::app::App::boot(true, settings);
            app.update(Message::OpenCompose);
            app.update(Message::ComposeChanged(
                ComposeField::To,
                "ceo@p\u{430}ypal.com".into(),
            ));
            let mut messages = Vec::new();
            let mut danger = egui::Color32::PLACEHOLDER;
            let mut body = 0.0;
            let output = context.run_ui(egui::RawInput::default(), |ui| {
                danger = theme::colors(ui).danger;
                body = egui::TextStyle::Body.resolve(ui.style()).size;
                show(ui, app.compose().unwrap(), &mut messages);
            });
            // The look-alike is named beside a form that is plainly not paypal.com.
            let found = output.shapes.iter().any(|clipped| {
                matches!(&clipped.shape, egui::epaint::Shape::Text(text)
                    if text.galley.text().contains("p\u{430}ypal.com is sent as xn--pypal-4ve.com")
                        && text.galley.job.sections.iter().all(|section| section.format.color == danger && section.format.font_id.size == body))
            });
            output.drop_without_applying_deltas();
            found
        };

        assert!(!shown(false));
        assert!(shown(true));
    }

    #[test]
    fn a_new_draft_puts_the_cursor_where_writing_starts() {
        use crate::mail::demo;

        let context = egui::Context::default();
        theme::install(&context);
        let focused_after_showing = |app: &crate::app::App| {
            context
                .run_ui(egui::RawInput::default(), |ui| {
                    show(ui, app.compose().unwrap(), &mut Vec::new());
                })
                .drop_without_applying_deltas();
            context.memory(|memory| memory.focused())
        };

        let (mut app, _) = crate::app::App::boot(true, crate::settings::Settings::default());
        app.update(Message::OpenCompose);
        let draft = app.compose().unwrap().id();
        assert_eq!(focused_after_showing(&app), Some(field_id("To", draft)));
        // Given once: after that the cursor stays wherever the writer puts it.
        context.memory_mut(|memory| memory.surrender_focus(field_id("To", draft)));
        assert_eq!(focused_after_showing(&app), None);

        // A reply knows its recipients, so it starts in the body.
        app.update(Message::DiscardCompose);
        let mailbox = demo::DemoMailbox::new();
        let page = mailbox.list_conversations(
            &crate::mail::Folder::INBOX,
            0,
            demo::PAGE_SIZE,
            demo::now(),
        );
        app.update(Message::ConversationsLoaded(1, page));
        app.update(Message::SelectConversation("demo-0".into()));
        let crate::app::ReaderState::Loading {
            conversation_id,
            request,
        } = app.mailbox().unwrap().reader_state()
        else {
            panic!("the conversation is loading");
        };
        let request = crate::app::ReaderRequest {
            id: *request,
            conversation_id: conversation_id.clone(),
            kind: crate::mail::SummaryKind::Conversation,
            messages: 1,
        };
        let detail = mailbox.conversation_detail(&request.conversation_id, demo::now());
        app.update(Message::ConversationLoaded(request, detail));
        app.update(Message::Answer {
            message_id: "demo-0-2".into(),
            kind: AnswerKind::Reply,
        });
        let reply = app.compose().expect("a reply is open").id();
        assert_eq!(focused_after_showing(&app), Some(body_id(reply)));
    }

    #[test]
    fn field_labels_are_drawn_at_body_size() {
        let context = egui::Context::default();
        theme::install(&context);
        let (mut app, _) = crate::app::App::boot(true, crate::settings::Settings::default());
        app.update(Message::OpenCompose);
        let mut messages = Vec::new();
        let mut body = 0.0;
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            body = egui::TextStyle::Body.resolve(ui.style()).size;
            show(ui, app.compose().unwrap(), &mut messages);
        });
        let size_of = |label: &str| {
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::epaint::Shape::Text(text) if text.galley.text() == label => text
                        .galley
                        .job
                        .sections
                        .first()
                        .map(|section| section.format.font_id.size),
                    _ => None,
                })
        };

        for label in ["To", "Subject"] {
            assert_eq!(size_of(label), Some(body), "{label}");
        }
        output.drop_without_applying_deltas();
    }

    #[test]
    fn leaving_a_recipient_field_asks_how_the_message_will_be_protected() {
        let context = egui::Context::default();
        theme::install(&context);
        let (mut app, _) = crate::app::App::boot(true, crate::settings::Settings::default());
        app.update(Message::OpenCompose);
        let frame = |events: Vec<egui::Event>| {
            let mut messages = Vec::new();
            context
                .run_ui(
                    egui::RawInput {
                        events,
                        ..Default::default()
                    },
                    |ui| show(ui, app.compose().unwrap(), &mut messages),
                )
                .drop_without_applying_deltas();
            messages
                .iter()
                .any(|message| matches!(message, Message::ComposeRecipientsLeft))
        };

        frame(Vec::new());
        let to = field_id("To", app.compose().unwrap().id());
        context.memory_mut(|memory| memory.request_focus(to));
        assert!(!frame(Vec::new()), "entering the field looks nothing up");
        assert!(!frame(Vec::new()), "staying in the field looks nothing up");

        let tab = egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        // Focus moves during the frame that sees the key; the field reports it by the next one.
        let asked = [frame(vec![tab]), frame(Vec::new())];
        assert!(asked.contains(&true));
        assert!(!frame(Vec::new()), "the lookup is asked for once");
    }

    #[test]
    fn compose_fields_start_at_the_left_and_are_taller_than_the_default() {
        let context = egui::Context::default();
        let mut geometry = None;
        context
            .run_ui(egui::RawInput::default(), |ui| {
                ui.set_width(400.0);
                let left = ui.available_rect_before_wrap().left();
                let mut messages = Vec::new();
                let field = line(
                    ui,
                    "To",
                    ComposeField::To,
                    &Compose::default(),
                    false,
                    &mut messages,
                );

                let mut plain = String::new();
                let plain = ui.add(egui::TextEdit::singleline(&mut plain));
                geometry = Some((left, field.rect, plain.rect.height()));
            })
            .drop_without_applying_deltas();

        let (left, field, plain_height) = geometry.expect("the field was laid out");
        assert!((field.left() - left).abs() <= 1.0);
        assert!(field.height() >= plain_height + 8.0);
    }
}
