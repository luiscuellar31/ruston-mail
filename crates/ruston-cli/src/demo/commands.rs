//! The demo's commands, an area to a function, as `commands::run` has them:
//! each answers from the demo's own mail, sending nothing anywhere.

use super::data::{demo_addresses, demo_contacts, demo_counts, demo_filters, demo_labels};
use super::{
    DEMO_EMAIL, DEMO_NAME, DemoMessage, demo_number, finds, ids_named, list_page, message_named,
    page_of, read_conversation, search_label, search_local, simulated_attachment,
};
use crate::cli::{
    AddressesCmd, AttachmentsCmd, ContactsCmd, ConversationsCmd, Ctx, DraftsCmd, FiltersCmd,
    LabelAction, LabelsCmd, MessagesCmd, SettingsCmd,
};
use crate::commands::contacts::{ATTACH_PUBLIC_KEY, SIGN};
use crate::mailbox::{someone_matches, within};
use crate::{render, terminal};
use ruston_core::mail::attachments::save_under_free_name;
use ruston_core::mail::organize::folder_id_among;
use ruston_core::mail::read::search_window;
use ruston_core::model::enums::{filter, label_ids, label_type};
use ruston_core::{
    Attachment, ContactEmail, Conversation, Error, Filter, Label, MessageMetadata, Result,
    SearchOpts,
};
use serde_json::json;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

/// Says the demo is signed in.
pub(super) fn login(ctx: &Ctx) -> Result<()> {
    if ctx.json {
        render::json_out(&json!({
            "status": "ok",
            "email": DEMO_EMAIL,
            "mode": "demo"
        }));
    } else {
        terminal::println!("Logged in as {DEMO_EMAIL} (offline demo mode)");
    }
    Ok(())
}

/// Says the demo session ended.
pub(super) fn logout(ctx: &Ctx) -> Result<()> {
    if ctx.json {
        render::json_out(&json!({
            "status": "ok",
            "message": "Demo session ended"
        }));
    } else {
        terminal::println!("Logged out of demo session.");
    }
    Ok(())
}

/// Names the demo's user.
pub(super) fn whoami(ctx: &Ctx) -> Result<()> {
    if ctx.json {
        render::json_out(&json!({
            "email": DEMO_EMAIL,
            "mode": "demo"
        }));
    } else {
        terminal::println!("{DEMO_EMAIL} (demo)");
    }
    Ok(())
}

/// `messages`, on the demo's mail.
pub(super) fn messages(ctx: &Ctx, cmd: MessagesCmd, messages: Vec<DemoMessage>) -> Result<()> {
    match cmd {
        MessagesCmd::List {
            folder,
            page,
            page_size,
            unread,
            cached: _,
        } => {
            let target_label = folder_id_among(&demo_labels(), &folder)?;
            let is_all = target_label == label_ids::ALL_MAIL
                || folder.eq_ignore_ascii_case("all")
                || folder.eq_ignore_ascii_case("allmail");

            let mut matching: Vec<MessageMetadata> = messages
                .into_iter()
                .filter(|m| is_all || m.meta.label_ids.contains(&target_label))
                .filter(|m| !unread || m.meta.is_unread())
                .map(|m| m.meta)
                .collect();

            matching.sort_by_key(|a| std::cmp::Reverse(a.time));
            let total = matching.len() as u32;
            let paged = page_of(matching, page, page_size);

            render::messages_list(ctx.json, total, &paged);
            Ok(())
        }
        MessagesCmd::Search(args) => {
            let window = search_window(args.after.as_deref(), args.before.as_deref())?;
            let label = search_label(args.folder.as_deref())?;
            let mut results: Vec<MessageMetadata> = messages
                .into_iter()
                .filter(|m| {
                    if let Some(kw) = &args.keyword {
                        let kw = kw.to_lowercase();
                        if !m.meta.subject.to_lowercase().contains(&kw)
                            && !m.body.to_lowercase().contains(&kw)
                        {
                            return false;
                        }
                    }
                    if let Some(from) = &args.from
                        && !someone_matches([&m.meta.sender], from)
                    {
                        return false;
                    }
                    let recipients = m.meta.to_list.iter().chain(&m.meta.cc_list);
                    if let Some(to) = &args.to
                        && !someone_matches(recipients.chain(&m.meta.bcc_list), to)
                    {
                        return false;
                    }
                    if let Some(subject) = &args.subject
                        && !m
                            .meta
                            .subject
                            .to_lowercase()
                            .contains(&subject.to_lowercase())
                    {
                        return false;
                    }
                    if args.unread && !m.meta.is_unread() {
                        return false;
                    }
                    within(window, m.meta.time)
                        && label
                            .as_ref()
                            .is_none_or(|label| m.meta.label_ids.contains(label))
                })
                .map(|m| m.meta)
                .collect();

            results.sort_by_key(|a| std::cmp::Reverse(a.time));
            if let Some(limit) = args.limit {
                results.truncate(limit as usize);
            }
            render::messages_list(ctx.json, results.len() as u32, &results);
            Ok(())
        }
        MessagesCmd::Read {
            reference,
            format,
            body_only,
            output,
        } => {
            let msg = message_named(&messages, &reference)?;

            let full = msg.to_full();
            if let Some(path) = output {
                fs::write(&path, &full.body)?;
                render::body_written(ctx.json, full.body.len(), &path);
            } else {
                render::full_message(
                    ctx.json,
                    &full,
                    format,
                    render::MessageView::chosen(body_only),
                );
            }
            Ok(())
        }
        MessagesCmd::Send(args) => {
            let id = "msg-demo-sent-101";
            match args.send_at {
                Some(at) => render::scheduled(ctx.json, id, at),
                None => render::sent(ctx.json, id),
            }
            Ok(())
        }
        // Named after the message they answer, found as the others are.
        MessagesCmd::Reply { reference, .. } => {
            let original = message_named(&messages, &reference)?;
            let id = format!("msg-demo-reply-{}", demo_number(original));
            render::sent(ctx.json, &id);
            Ok(())
        }
        MessagesCmd::Forward { reference, .. } => {
            let original = message_named(&messages, &reference)?;
            let id = format!("msg-demo-fwd-{}", demo_number(original));
            render::sent(ctx.json, &id);
            Ok(())
        }
        MessagesCmd::CancelSend { reference } => {
            let ids = ids_named(&messages, &[reference])?;
            render::action_result(ctx.json, "cancel-send", &ids);
            Ok(())
        }
        MessagesCmd::Trash { references } => {
            let ids = ids_named(&messages, &references)?;
            render::action_result(ctx.json, "trash", &ids);
            Ok(())
        }
        // The fictional mailbox loses nothing, so there is nothing to confirm.
        MessagesCmd::Delete { references, .. } => {
            let ids = ids_named(&messages, &references)?;
            render::action_result(ctx.json, "delete", &ids);
            Ok(())
        }
        MessagesCmd::Spam { references } => {
            let ids = ids_named(&messages, &references)?;
            render::action_result(ctx.json, "spam", &ids);
            Ok(())
        }
        MessagesCmd::Ham { references } => {
            let ids = ids_named(&messages, &references)?;
            render::action_result(ctx.json, "ham", &ids);
            Ok(())
        }
        MessagesCmd::Unsubscribe { reference } => {
            let ids = ids_named(&messages, &[reference])?;
            render::action_result(ctx.json, "unsubscribe", &ids);
            Ok(())
        }
        MessagesCmd::Empty { folder, .. } => {
            render::folder_emptied(ctx.json, folder);
            Ok(())
        }
        MessagesCmd::Label {
            action,
            label_id,
            references,
        } => {
            folder_id_among(&demo_labels(), &label_id)?;
            let ids = ids_named(&messages, &references)?;
            let action = match action {
                LabelAction::Add => "label",
                LabelAction::Rm => "unlabel",
            };
            render::action_result(ctx.json, action, &ids);
            Ok(())
        }
        MessagesCmd::Receipt { reference } => {
            let ids = ids_named(&messages, &[reference])?;
            render::action_result(ctx.json, "receipt", &ids);
            Ok(())
        }
        MessagesCmd::Move { references, dest } => {
            folder_id_among(&demo_labels(), &dest)?;
            let ids = ids_named(&messages, &references)?;
            render::action_result(ctx.json, "move", &ids);
            Ok(())
        }
        MessagesCmd::Mark {
            state: _,
            references,
        } => {
            let ids = ids_named(&messages, &references)?;
            render::action_result(ctx.json, "mark", &ids);
            Ok(())
        }
        MessagesCmd::Star { references } => {
            let ids = ids_named(&messages, &references)?;
            render::action_result(ctx.json, "star", &ids);
            Ok(())
        }
        MessagesCmd::Unstar { references } => {
            let ids = ids_named(&messages, &references)?;
            render::action_result(ctx.json, "unstar", &ids);
            Ok(())
        }
    }
}

/// `conversations`, on the demo's mail.
pub(super) fn conversations(
    ctx: &Ctx,
    cmd: ConversationsCmd,
    messages: Vec<DemoMessage>,
    conversations: Vec<Conversation>,
) -> Result<()> {
    match cmd {
        ConversationsCmd::List {
            folder,
            page,
            page_size,
            unread,
        } => {
            let target_label = folder_id_among(&demo_labels(), &folder)?;
            let is_all = target_label == label_ids::ALL_MAIL
                || folder.eq_ignore_ascii_case("all")
                || folder.eq_ignore_ascii_case("allmail");

            let (total, paged) = list_page(
                conversations,
                &target_label,
                is_all,
                unread,
                page,
                page_size,
            );
            render::conversations_list(ctx.json, total, &paged);
            Ok(())
        }
        ConversationsCmd::Search(args) => {
            let window = search_window(args.after.as_deref(), args.before.as_deref())?;
            let label = search_label(args.folder.as_deref())?;
            let opts = SearchOpts::from(args);
            let mut results: Vec<Conversation> = conversations
                .into_iter()
                .filter(|c| finds(c, &opts, window, label.as_deref()))
                .collect();

            results.sort_by_key(|a| std::cmp::Reverse(a.time));
            if let Some(limit) = opts.limit {
                results.truncate(limit as usize);
            }
            render::conversations_list(ctx.json, results.len() as u32, &results);
            Ok(())
        }
        ConversationsCmd::Read { id } => {
            let (conv, conv_messages) = read_conversation(messages, conversations, &id)?;
            render::conversation_read(ctx.json, &conv, &conv_messages);
            Ok(())
        }
        ConversationsCmd::Move { ids, dest } => {
            folder_id_among(&demo_labels(), &dest)?;
            render::action_result(ctx.json, "move", &ids);
            Ok(())
        }
        ConversationsCmd::Trash { ids } => {
            render::action_result(ctx.json, "trash", &ids);
            Ok(())
        }
        ConversationsCmd::Mark {
            state: _,
            folder,
            ids,
        } => {
            folder_id_among(&demo_labels(), &folder)?;
            render::action_result(ctx.json, "mark", &ids);
            Ok(())
        }
        ConversationsCmd::Star { ids } => {
            render::action_result(ctx.json, "star", &ids);
            Ok(())
        }
        ConversationsCmd::Unstar { ids } => {
            render::action_result(ctx.json, "unstar", &ids);
            Ok(())
        }
        ConversationsCmd::Snooze {
            ids,
            until,
            relative,
        } => {
            // Refuse what the live command refuses.
            crate::commands::conversations::snooze_time(until, relative, 0)?;
            render::action_result(ctx.json, "snooze", &ids);
            Ok(())
        }
        ConversationsCmd::Unsnooze { ids } => {
            render::action_result(ctx.json, "unsnooze", &ids);
            Ok(())
        }
    }
}

/// `attachments`, of the demo's messages.
pub(super) fn attachments(ctx: &Ctx, cmd: AttachmentsCmd, messages: &[DemoMessage]) -> Result<()> {
    match cmd {
        AttachmentsCmd::List {
            message,
            include_inline: _,
        } => {
            let msg = message_named(messages, &message)?;
            render::attachments_list(ctx.json, &msg.attachments);
            Ok(())
        }
        AttachmentsCmd::Download {
            message,
            attachment,
            output_dir,
            all: _,
            include_inline: _,
        } => {
            let msg = message_named(messages, &message)?;
            let atts: Vec<&Attachment> = if let Some(att_id) = &attachment {
                msg.attachments.iter().filter(|a| a.id == *att_id).collect()
            } else {
                msg.attachments.iter().collect()
            };

            if atts.is_empty() {
                return Err(Error::Other("no attachments found to download".into()));
            }

            let out_dir = output_dir.unwrap_or_else(|| PathBuf::from("."));
            fs::create_dir_all(&out_dir)?;

            // Saved as the real command saves: never over a file or
            // through a link already there.
            let mut written = Vec::new();
            for a in &atts {
                let path = save_under_free_name(&out_dir, &a.name, &simulated_attachment(&a.name))?;
                written.push(path.to_string_lossy().to_string());
            }
            render::downloaded(ctx.json, &written);
            Ok(())
        }
    }
}

/// `drafts`, among the demo's messages.
pub(super) fn drafts(ctx: &Ctx, cmd: DraftsCmd, messages: Vec<DemoMessage>) -> Result<()> {
    match cmd {
        DraftsCmd::List { page, page_size } => {
            let drafts: Vec<MessageMetadata> = messages
                .into_iter()
                .filter(|m| m.meta.label_ids.contains(&label_ids::DRAFTS.to_string()))
                .map(|m| m.meta)
                .collect();

            let total = drafts.len() as u32;
            let paged = page_of(drafts, page, page_size);

            render::messages_list(ctx.json, total, &paged);
            Ok(())
        }
        DraftsCmd::Save(_) => {
            render::draft_saved(ctx.json, "draft-demo-saved-01");
            Ok(())
        }
        DraftsCmd::Edit { id, .. } => {
            render::action_result(ctx.json, "edit", &[id]);
            Ok(())
        }
        DraftsCmd::Delete { ids, .. } => {
            render::action_result(ctx.json, "delete", &ids);
            Ok(())
        }
    }
}

/// `filters`, on the demo's filters.
pub(super) fn filters(ctx: &Ctx, cmd: FiltersCmd) -> Result<()> {
    match cmd {
        FiltersCmd::List => {
            render::filters_list(ctx.json, &demo_filters());
            Ok(())
        }
        FiltersCmd::Check { .. } => {
            if ctx.json {
                render::json_out(&json!({ "valid": true }));
            } else {
                terminal::println!("Sieve script is valid.");
            }
            Ok(())
        }
        FiltersCmd::Create { name, sieve: _ } => {
            let f = Filter {
                id: "filter-demo-02".to_string(),
                name,
                status: filter::ENABLED,
                version: filter::SIEVE_VERSION,
            };
            render::filter_created(ctx.json, &f);
            Ok(())
        }
        FiltersCmd::Delete { id } => {
            render::action_result(ctx.json, "delete", &[id]);
            Ok(())
        }
        FiltersCmd::Enable { id } => {
            render::action_result(ctx.json, "enable", &[id]);
            Ok(())
        }
        FiltersCmd::Disable { id } => {
            render::action_result(ctx.json, "disable", &[id]);
            Ok(())
        }
    }
}

/// `contacts`, on the demo's contacts.
pub(super) fn contacts(ctx: &Ctx, cmd: ContactsCmd) -> Result<()> {
    match cmd {
        ContactsCmd::List { page, page_size } => {
            let contacts = demo_contacts();
            let total = contacts.len() as u32;
            let paged = page_of(contacts, page, page_size);
            render::contacts_list(ctx.json, total, &paged);
            Ok(())
        }
        ContactsCmd::Emails { email } => {
            let emails: Vec<ContactEmail> = demo_contacts()
                .into_iter()
                .flat_map(|c| c.emails)
                .filter(|e| {
                    if let Some(target) = &email {
                        e.email.to_lowercase().contains(&target.to_lowercase())
                    } else {
                        true
                    }
                })
                .collect();
            render::contact_emails(ctx.json, &emails);
            Ok(())
        }
    }
}

/// `addresses`, on the demo's addresses.
pub(super) fn addresses(ctx: &Ctx, cmd: AddressesCmd) -> Result<()> {
    match cmd {
        AddressesCmd::List => {
            render::addresses_list(ctx.json, &demo_addresses());
            Ok(())
        }
        AddressesCmd::Update { id, .. } => {
            render::action_result(ctx.json, "update", &[id]);
            Ok(())
        }
    }
}

/// `settings`, as the demo's account has them.
pub(super) fn settings(ctx: &Ctx, cmd: SettingsCmd) -> Result<()> {
    match cmd {
        SettingsCmd::Get => {
            render::json_out(&json!({
                "DisplayName": DEMO_NAME,
                "Sign": 1,
                "AttachPublicKey": 0,
                "AutoSaveContacts": 1
            }));
            Ok(())
        }
        SettingsCmd::Sign { value } => {
            render::setting_changed(ctx.json, "sign", SIGN, value.is_on());
            Ok(())
        }
        SettingsCmd::AttachPublicKey { value } => {
            render::setting_changed(
                ctx.json,
                "attach_public_key",
                ATTACH_PUBLIC_KEY,
                value.is_on(),
            );
            Ok(())
        }
    }
}

/// `counts`, of the demo's mail.
pub(super) fn counts(ctx: &Ctx, conversations: bool) -> Result<()> {
    render::counts(ctx.json, &demo_counts(conversations));
    Ok(())
}

/// `labels`, on the demo's labels and folders.
pub(super) fn labels(ctx: &Ctx, cmd: LabelsCmd) -> Result<()> {
    match cmd {
        LabelsCmd::List { folders } => {
            let kind = render::LabelKind::chosen(folders);
            let listed = match kind {
                render::LabelKind::Labels => label_type::MESSAGE_LABEL,
                render::LabelKind::Folders => label_type::MESSAGE_FOLDER,
            };
            let labels: Vec<Label> = demo_labels()
                .into_iter()
                .filter(|l| l.label_type == listed)
                .collect();
            render::labels_list(ctx.json, &labels, kind);
            Ok(())
        }
        LabelsCmd::Create {
            name,
            color,
            folder,
            parent,
        } => {
            let l = Label {
                id: "label-demo-created".to_string(),
                name,
                color,
                path: "Custom".to_string(),
                label_type: if folder {
                    label_type::MESSAGE_FOLDER
                } else {
                    label_type::MESSAGE_LABEL
                },
                parent_id: parent,
                order: 99,
                notify: None,
            };
            render::label_created(ctx.json, &l);
            Ok(())
        }
        LabelsCmd::Delete { ids } => {
            render::action_result(ctx.json, "delete", &ids);
            Ok(())
        }
        LabelsCmd::Update { id, .. } => {
            render::action_result(ctx.json, "update", &[id]);
            Ok(())
        }
    }
}

/// `export`, of the demo's messages.
pub(super) fn export(
    ctx: &Ctx,
    folder: String,
    out: PathBuf,
    max: u32,
    messages: &[DemoMessage],
) -> Result<()> {
    fs::create_dir_all(&out)?;
    let target_label = folder_id_among(&demo_labels(), &folder)?;
    let is_all = target_label == label_ids::ALL_MAIL || folder.eq_ignore_ascii_case("all");

    let exported: Vec<&DemoMessage> = messages
        .iter()
        .filter(|m| is_all || m.meta.label_ids.contains(&target_label))
        .take(max as usize)
        .collect();

    let mut written = 0;
    let mut already_exported = 0;
    for m in &exported {
        let eml_path = out.join(format!("{}.eml", m.meta.id));
        let eml_content = format!(
            "From: {}\nTo: {}\nSubject: {}\nDate: {}\nMessage-ID: {}\n\n{}",
            m.meta.sender.address,
            m.meta
                .to_list
                .iter()
                .map(|r| r.address.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            m.meta.subject,
            render::fmt_time(m.meta.time),
            m.meta.id,
            m.body
        );
        let mut file = match ruston_core::mail::export::create_private(&eml_path) {
            Ok(file) => file,
            // As in the live command, running again keeps what is there.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                already_exported += 1;
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        file.write_all(eml_content.as_bytes())?;
        written += 1;
    }

    if ctx.json {
        render::json_out(&json!({
            "exported": written,
            "already_exported": already_exported,
            "skipped": [],
            "out": out.display().to_string(),
        }));
    } else {
        terminal::println!("Exported {} message(s) to {}", written, out.display());
        if already_exported > 0 {
            terminal::println!(
                "{} message(s) were already exported there",
                already_exported
            );
        }
    }
    Ok(())
}

/// `search`, in the demo's local index.
pub(super) fn search(ctx: &Ctx, query: String, limit: u32) -> Result<()> {
    let results = search_local(&query, limit);
    render::messages_list(ctx.json, results.len() as u32, &results);
    Ok(())
}

/// `sync`, with nothing to bring.
pub(super) fn sync(ctx: &Ctx, backfill: Option<String>) -> Result<()> {
    if ctx.json {
        render::json_out(&json!({
            "status": "ok",
            "synced": true,
            "backfill": backfill,
            "new_events": 0
        }));
    } else {
        terminal::println!("Demo cache sync complete (0 new events, offline mode).");
    }
    Ok(())
}

/// `index`, of the demo's `indexed` messages.
pub(super) fn index(ctx: &Ctx, indexed: usize) -> Result<()> {
    if ctx.json {
        render::json_out(&json!({ "indexed": indexed }));
    } else {
        terminal::println!("Indexed {indexed} message(s) for local search.");
    }
    Ok(())
}

/// `watch`, with no events to wait for.
pub(super) fn watch(ctx: &Ctx, interval: u64, folder: Option<String>) -> Result<()> {
    if ctx.json {
        render::json_out(&json!({
            "status": "ok",
            "watch": "demo_tick",
            "interval": interval,
            "folder": folder
        }));
    } else {
        terminal::println!(
            "[demo] Simulated watch event stream (interval: {interval}s). 0 new events."
        );
    }
    Ok(())
}
