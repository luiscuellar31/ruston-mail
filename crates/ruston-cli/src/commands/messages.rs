//! Message commands.

use crate::cli::Ctx;
use crate::cli::{
    DraftArgs, EmptyFolder, LabelAction, MessagesCmd, ReadFormat, ReadState, SendArgs,
};
use crate::commands::prompt::{can_prompt, confirm, confirm_deletion, require_consent};
use crate::commands::{read_body, resolve_all, search_options};
use crate::render;
use crate::terminal;
use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{
    BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use ruston_core::{AddressInfo, Client, Error, LocalCache, Quote, Result, SendOptions};
use std::cell::Cell;
use std::io::{self, BufRead, Write};
use std::path::Path;

fn prompt_sender(
    addresses: &[AddressInfo],
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> io::Result<String> {
    terminal::writeln!(output, "Choose a sender address:")?;
    for (index, address) in addresses.iter().enumerate() {
        let default = if index == 0 { " (default)" } else { "" };
        terminal::writeln!(output, "  {}. {}{default}", index + 1, address.email)?;
    }

    loop {
        terminal::write!(output, "Sender [1]: ")?;
        output.flush()?;
        let mut answer = String::new();
        if input.read_line(&mut answer)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "sender selection cancelled",
            ));
        }
        let answer = answer.trim();
        let index = if answer.is_empty() {
            Some(0)
        } else {
            answer.parse::<usize>().ok().and_then(|n| n.checked_sub(1))
        };
        if let Some(address) = index.and_then(|index| addresses.get(index)) {
            return Ok(address.email.clone());
        }
        terminal::writeln!(output, "Choose a number from 1 to {}.", addresses.len())?;
    }
}

#[derive(Default)]
struct HtmlDetector(Cell<bool>);

impl TokenSink for HtmlDetector {
    type Handle = ();

    fn process_token(&self, token: Token, _line_number: u64) -> TokenSinkResult<()> {
        if matches!(token, Token::DoctypeToken(_))
            || matches!(token, Token::TagToken(tag)
                if tag.kind == TagKind::StartTag
                    && matches!(&*tag.name, "html" | "body" | "p" | "div" | "span" | "br"
                        | "a" | "b" | "strong" | "i" | "em" | "h1" | "h2" | "h3"
                        | "ul" | "ol" | "li" | "table" | "img"))
        {
            self.0.set(true);
        }
        TokenSinkResult::Continue
    }
}

fn looks_like_html(body: &str) -> bool {
    let tokenizer = Tokenizer::new(HtmlDetector::default(), TokenizerOpts::default());
    let input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(body));
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    tokenizer.sink.0.get()
}

fn confirm_html(input: &mut impl BufRead, output: &mut impl Write) -> io::Result<bool> {
    confirm("Body looks like HTML. Send as HTML?", input, output)
}

/// Asks twice through `read`, as `rpassword::prompt_password` asks, for the
/// password that will protect a message, so a typo cannot lock its
/// recipients out.
fn ask_message_password(mut read: impl FnMut(&str) -> io::Result<String>) -> Result<String> {
    let password = read("Message password: ")?;
    if password.is_empty() {
        return Err(Error::Other(
            "a message password cannot be empty; nothing was sent".into(),
        ));
    }
    if read("Repeat the message password: ")? != password {
        return Err(Error::Other(
            "the two message passwords differ; nothing was sent".into(),
        ));
    }
    Ok(password)
}

/// The password that protects a message, if any. `--eo-password` without a
/// value arrives as an empty one and means "ask for it through `read`", so
/// the password never has to go on the command line.
fn message_password(
    given: Option<&str>,
    read: impl FnMut(&str) -> io::Result<String>,
) -> Result<Option<String>> {
    match given {
        Some("") => ask_message_password(read).map(Some),
        password => Ok(password.map(str::to_owned)),
    }
}

/// Writes a decrypted body to `path` as a file only its owner can read. What
/// is already there, a symbolic link included, is replaced rather than
/// written through: the file is created beside it and renamed over it.
fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "the output path names no file")
    })?;
    let folder = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    for attempt in 0..100 {
        let temporary = folder.join(format!(
            ".{}.{}.{attempt}.tmp",
            name.to_string_lossy(),
            std::process::id()
        ));
        let mut file = match ruston_core::mail::export::create_private(&temporary) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let written = file
            .write_all(contents)
            .and_then(|()| file.sync_all())
            .and_then(|()| std::fs::rename(&temporary, path));
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        return written;
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no temporary file name is available",
    ))
}

/// Lists, searches, reads, sends or answers messages, or changes the ones
/// named.
pub async fn run(ctx: &Ctx, cmd: MessagesCmd) -> Result<()> {
    match cmd {
        MessagesCmd::List {
            folder,
            page,
            page_size,
            unread,
            cached,
        } => {
            let (total, msgs) = if cached {
                LocalCache::open(&ctx.profile)
                    .await?
                    .messages_page(&folder, unread, page, page_size)?
            } else {
                let client = Client::resume(&ctx.profile).await?;
                let folder = client.folder_id(&folder).await?;
                client
                    .list_messages(&folder, page, page_size, unread)
                    .await?
            };
            render::messages_list(ctx.json, total, &msgs);
            Ok(())
        }
        MessagesCmd::Search(args) => {
            let client = Client::resume(&ctx.profile).await?;
            let opts = search_options(&client, args.into()).await?;
            let msgs = client.search_messages(&opts).await?;
            render::messages_list(ctx.json, msgs.len() as u32, &msgs);
            Ok(())
        }
        MessagesCmd::Read {
            reference,
            format,
            body_only,
            output,
        } => read(ctx, &reference, format, body_only, output.as_deref()).await,
        MessagesCmd::Send(args) => send(ctx, args).await,
        MessagesCmd::Reply {
            reference,
            all,
            from,
            body,
            attach,
            no_quote,
        } => {
            let opts = SendOptions {
                from,
                attachments: attach,
                quote: quote(no_quote),
                ..SendOptions::default()
            };
            answer(ctx, &reference, Answer::Reply { all }, &body, opts).await
        }
        MessagesCmd::Forward {
            reference,
            to,
            from,
            body,
            attach,
            no_quote,
            leave_out,
        } => {
            let opts = SendOptions {
                to,
                from,
                attachments: attach,
                quote: quote(no_quote),
                leave_out,
                ..SendOptions::default()
            };
            answer(ctx, &reference, Answer::Forward, &body, opts).await
        }
        MessagesCmd::CancelSend { reference } => {
            let client = Client::resume(&ctx.profile).await?;
            client.cancel_send(&reference).await?;
            render::action_result(ctx.json, "cancel-send", &[reference]);
            Ok(())
        }
        MessagesCmd::Move { references, dest } => {
            change_messages(ctx, &references, Change::Move { dest }).await
        }
        MessagesCmd::Trash { references } => change_messages(ctx, &references, Change::Trash).await,
        MessagesCmd::Mark { state, references } => {
            change_messages(ctx, &references, Change::Mark(state)).await
        }
        MessagesCmd::Star { references } => change_messages(ctx, &references, Change::Star).await,
        MessagesCmd::Unstar { references } => {
            change_messages(ctx, &references, Change::Unstar).await
        }
        MessagesCmd::Spam { references } => change_messages(ctx, &references, Change::Spam).await,
        MessagesCmd::Ham { references } => change_messages(ctx, &references, Change::Ham).await,
        MessagesCmd::Label {
            action,
            label_id,
            references,
        } => {
            let change = Change::Label {
                action,
                label: label_id,
            };
            change_messages(ctx, &references, change).await
        }
        MessagesCmd::Unsubscribe { reference } => {
            change_messages(ctx, &[reference], Change::Unsubscribe).await
        }
        MessagesCmd::Receipt { reference } => {
            change_messages(ctx, &[reference], Change::Receipt).await
        }
        MessagesCmd::Delete { references, yes } => delete(ctx, &references, yes).await,
        MessagesCmd::Empty { folder, yes } => empty(ctx, folder, yes).await,
        MessagesCmd::Undelete { ids } => {
            // `undelete` takes message IDs only, as its help says, so
            // nothing is looked up.
            let client = Client::resume(&ctx.profile).await?;
            client.undelete_messages(&ids).await?;
            render::action_result(ctx.json, "undelete", &ids);
            Ok(())
        }
    }
}

/// How a message is answered.
enum Answer {
    /// A reply to its sender, or with `all` to everyone it went to.
    Reply { all: bool },
    /// A forward to recipients of the writer's choosing, with its
    /// attachments.
    Forward,
}

/// Sends `opts` as an answer to the message `reference` names, with the
/// text `body` stands for.
async fn answer(
    ctx: &Ctx,
    reference: &str,
    how: Answer,
    body: &str,
    opts: SendOptions,
) -> Result<()> {
    let client = Client::resume(&ctx.profile).await?;
    let opts = SendOptions {
        body: read_body(body)?,
        ..opts
    };
    let id = match how {
        Answer::Reply { all } => client.reply(reference, all, &opts).await?,
        Answer::Forward => client.forward(reference, &opts).await?,
    };
    render::sent(ctx.json, &id);
    Ok(())
}

/// What a command changes about the messages its references name.
enum Change {
    /// Moves them to a folder, named as the folder options take it.
    Move {
        dest: String,
    },
    Trash,
    Mark(ReadState),
    Star,
    Unstar,
    /// Reports them as spam, which moves them to Spam.
    Spam,
    /// Reports them as not spam, which moves them to the Inbox.
    Ham,
    /// Puts a label on them or takes it off, named as above.
    Label {
        action: LabelAction,
        label: String,
    },
    /// Leaves the mailing list a message came from.
    Unsubscribe,
    /// Sends the read receipt a message asks for.
    Receipt,
}

impl Change {
    /// The command's own name, which the report gives.
    fn action(&self) -> &'static str {
        match self {
            Change::Move { .. } => "move",
            Change::Trash => "trash",
            Change::Mark(_) => "mark",
            Change::Star => "star",
            Change::Unstar => "unstar",
            Change::Spam => "spam",
            Change::Ham => "ham",
            Change::Label {
                action: LabelAction::Add,
                ..
            } => "label",
            Change::Label {
                action: LabelAction::Rm,
                ..
            } => "unlabel",
            Change::Unsubscribe => "unsubscribe",
            Change::Receipt => "receipt",
        }
    }

    /// Asks Proton to make the change to the messages `ids`, finding a
    /// folder or label it names first.
    async fn apply(&self, client: &Client, ids: &[String]) -> Result<()> {
        match self {
            Change::Move { dest } => {
                let dest = client.folder_id(dest).await?;
                client.move_messages(ids, &dest).await
            }
            Change::Trash => client.trash_messages(ids).await,
            Change::Mark(state) => client.mark_messages_read(ids, state.as_bool()).await,
            Change::Star => client.star_messages(ids, true).await,
            Change::Unstar => client.star_messages(ids, false).await,
            Change::Spam => client.report_spam(ids).await,
            Change::Ham => client.report_ham(ids).await,
            Change::Label { action, label } => {
                let label = client.folder_id(label).await?;
                match action {
                    LabelAction::Add => client.apply_label(ids, &label).await,
                    LabelAction::Rm => client.remove_label(ids, &label).await,
                }
            }
            // These commands name one message each.
            Change::Unsubscribe => {
                for id in ids {
                    client.unsubscribe(id).await?;
                }
                Ok(())
            }
            Change::Receipt => {
                for id in ids {
                    client.send_read_receipt(id).await?;
                }
                Ok(())
            }
        }
    }
}

/// What `--no-quote` asks of an answer.
fn quote(no_quote: bool) -> Quote {
    if no_quote {
        Quote::Omitted
    } else {
        Quote::Original
    }
}

/// Shows one message, or writes its body to `output` as a file only its
/// owner can read.
async fn read(
    ctx: &Ctx,
    reference: &str,
    format: ReadFormat,
    body_only: bool,
    output: Option<&Path>,
) -> Result<()> {
    let client = Client::resume(&ctx.profile).await?;
    let id = client.resolve_ref(reference).await?;
    let msg = client.read_message(&id).await?;
    if let Some(path) = output {
        write_private(path, msg.body.as_bytes())?;
        render::body_written(ctx.json, msg.body.len(), path);
    } else {
        render::full_message(
            ctx.json,
            &msg,
            format,
            render::MessageView::chosen(body_only),
        );
    }
    Ok(())
}

/// Sends a new message. On a terminal it first asks what was left out: the
/// address to send from, whether a body that looks like HTML is HTML, and a
/// message password asked for without a value.
async fn send(ctx: &Ctx, mut args: SendArgs) -> Result<()> {
    let client = Client::resume(&ctx.profile).await?;
    choose_sender(ctx, &client, &mut args.message)?;
    let message = &mut args.message;
    let body = read_body(&message.body)?;
    if !message.html && can_prompt(ctx) && looks_like_html(&body) {
        message.html = confirm_html(&mut io::stdin().lock(), &mut io::stderr().lock())?;
    }
    let opts = SendOptions {
        send_at: args.send_at,
        expires_in: args.expires,
        ..args.message.into_send_options(body)
    };
    let eo_password = message_password(args.eo_password.as_deref(), |prompt| {
        rpassword::prompt_password(prompt)
    })?;
    let id = match &eo_password {
        Some(pw) => client.send_eo(&opts, pw, args.eo_hint.as_deref()).await?,
        None => client.send(&opts).await?,
    };
    match opts.send_at {
        Some(at) => render::scheduled(ctx.json, &id, at),
        None => render::sent(ctx.json, &id),
    }
    Ok(())
}

/// With several addresses and a terminal to ask on, asks which one sends a
/// message that names none.
fn choose_sender(ctx: &Ctx, client: &Client, message: &mut DraftArgs) -> Result<()> {
    if message.from.is_none() && can_prompt(ctx) {
        let addresses = client.addresses();
        if addresses.len() > 1 {
            message.from = Some(prompt_sender(
                &addresses,
                &mut io::stdin().lock(),
                &mut io::stderr().lock(),
            )?);
        }
    }
    Ok(())
}

/// Deletes messages for good. Unless `yes` was given up front, it first
/// lists what the references matched and asks.
async fn delete(ctx: &Ctx, references: &[String], yes: bool) -> Result<()> {
    require_consent(ctx, yes)?;
    let client = Client::resume(&ctx.profile).await?;
    let ids = if yes {
        resolve_all(&client, references).await?
    } else {
        // A reference can be search text, so show what it matched. A
        // search hit carries what the list shows; only an ID is fetched.
        let mut matched = Vec::with_capacity(references.len());
        for reference in references {
            matched.push(client.resolve_ref_metadata(reference).await?);
        }
        let doomed: Vec<String> = matched
            .iter()
            .map(|meta| {
                format!(
                    "\"{}\" from {} (id {})",
                    meta.subject, meta.sender.address, meta.id
                )
            })
            .collect();
        if !confirm_deletion(&doomed, &format!("{} message(s)", matched.len()))? {
            return Ok(());
        }
        matched.into_iter().map(|meta| meta.id).collect()
    };
    client.delete_messages(&ids).await?;
    render::action_result(ctx.json, "delete", &ids);
    Ok(())
}

/// Deletes every message in `folder` for good, after asking unless `yes`
/// was given up front.
async fn empty(ctx: &Ctx, folder: EmptyFolder, yes: bool) -> Result<()> {
    require_consent(ctx, yes)?;
    if !yes && !confirm_deletion(&[], &format!("every message in {}", folder.name()))? {
        return Ok(());
    }
    let client = Client::resume(&ctx.profile).await?;
    client.empty_folder(folder.as_str()).await?;
    render::folder_emptied(ctx.json, folder);
    Ok(())
}

/// Makes `change` to the messages `references` name, each looked up first,
/// and reports it with the IDs they named.
async fn change_messages(ctx: &Ctx, references: &[String], change: Change) -> Result<()> {
    let client = Client::resume(&ctx.profile).await?;
    let ids = resolve_all(&client, references).await?;
    change.apply(&client, &ids).await?;
    render::action_result(ctx.json, change.action(), &ids);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        Change, ask_message_password, confirm_html, looks_like_html, message_password,
        prompt_sender, write_private,
    };
    use crate::cli::{LabelAction, ReadState};
    use ruston_core::AddressInfo;
    use std::io::{self, Cursor};

    #[test]
    fn each_change_is_reported_under_its_commands_name() {
        use clap::CommandFactory;

        let cli = crate::cli::Cli::command();
        let messages = cli.find_subcommand("messages").unwrap();
        let label = |action| Change::Label {
            action,
            label: "Work".into(),
        };
        for (change, name) in [
            (Change::Move { dest: "x".into() }, "move"),
            (Change::Trash, "trash"),
            (Change::Mark(ReadState::Unread), "mark"),
            (Change::Star, "star"),
            (Change::Unstar, "unstar"),
            (Change::Spam, "spam"),
            (Change::Ham, "ham"),
            (label(LabelAction::Add), "label"),
            (Change::Unsubscribe, "unsubscribe"),
            (Change::Receipt, "receipt"),
        ] {
            assert_eq!(change.action(), name);
            assert!(messages.find_subcommand(name).is_some(), "{name}");
        }
        // Taking a label off, `messages label rm`, is reported as `unlabel`.
        assert_eq!(label(LabelAction::Rm).action(), "unlabel");
    }

    #[test]
    fn sender_prompt_does_not_emit_terminal_controls() {
        let untrusted = "sender\x1b]52;c;Y2xpcGJvYXJk\x07\u{9b}2J\r@example.test";
        let addresses = [AddressInfo {
            id: "a".into(),
            email: untrusted.into(),
        }];
        let mut output = Vec::new();
        let selected = prompt_sender(&addresses, &mut Cursor::new("\n"), &mut output).unwrap();
        assert_eq!(selected, untrusted);
        let displayed = String::from_utf8(output).unwrap();
        assert!(
            displayed
                .chars()
                .all(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        );
    }

    #[test]
    fn sender_prompt_selects_defaults_and_cancels_on_eof() {
        let addresses = [
            AddressInfo {
                id: "primary".into(),
                email: "primary@example.test".into(),
            },
            AddressInfo {
                id: "alias".into(),
                email: "alias@example.test".into(),
            },
        ];

        for (answer, expected) in [
            ("\n", "primary@example.test"),
            ("2\n", "alias@example.test"),
            ("3\n2\n", "alias@example.test"),
        ] {
            let selected = prompt_sender(&addresses, &mut Cursor::new(answer), &mut Vec::new())
                .expect("a valid choice should select a sender");
            assert_eq!(selected, expected);
        }

        let error = prompt_sender(&addresses, &mut Cursor::new(""), &mut Vec::new())
            .expect_err("EOF must cancel the send");
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn html_detection_ignores_plain_text_comparisons() {
        assert!(looks_like_html(
            "<p class=\"note\">Hello <strong>world</strong></p>"
        ));
        assert!(looks_like_html(
            "<!DOCTYPE html><html><body>Hello</body></html>"
        ));
        assert!(!looks_like_html("2 < 3 and 5 > 4"));
        assert!(!looks_like_html("Please write <alias> in the field"));
    }

    #[test]
    fn html_confirmation_requires_an_explicit_yes() {
        for (answer, expected) in [
            ("y\n", true),
            ("no\n", false),
            ("\n", false),
            ("?\nyes\n", true),
        ] {
            assert_eq!(
                confirm_html(&mut Cursor::new(answer), &mut Vec::new()).unwrap(),
                expected
            );
        }
        let error = confirm_html(&mut Cursor::new(""), &mut Vec::new()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn a_message_password_is_asked_for_twice_and_both_must_match() {
        let typed = |answers: &'static [&'static str]| {
            let mut answers = answers.iter();
            move |_prompt: &str| Ok::<_, io::Error>(answers.next().unwrap().to_string())
        };
        assert_eq!(
            ask_message_password(typed(&["s3cret", "s3cret"])).unwrap(),
            "s3cret"
        );
        assert!(ask_message_password(typed(&["s3cret", "s3creT"])).is_err());
        assert!(ask_message_password(typed(&[""])).is_err());
    }

    #[test]
    fn only_an_empty_message_password_is_asked_for() {
        let asked = std::cell::Cell::new(0);
        let typed = |_prompt: &str| {
            asked.set(asked.get() + 1);
            Ok::<_, io::Error>("s3cret".to_owned())
        };
        assert_eq!(
            message_password(Some(""), typed).unwrap().as_deref(),
            Some("s3cret")
        );
        assert_eq!(asked.get(), 2);

        let never = |_prompt: &str| -> io::Result<String> { panic!("nothing to ask") };
        assert_eq!(
            message_password(Some("given"), never).unwrap().as_deref(),
            Some("given")
        );
        assert_eq!(message_password(None, never).unwrap(), None);
    }

    #[test]
    fn a_body_written_to_a_file_is_private_and_never_written_through_a_link() {
        let dir = std::env::temp_dir().join(format!("ruston-output-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let output = dir.join("message.txt");

        // A file already there is replaced.
        std::fs::write(&output, b"older").unwrap();
        write_private(&output, b"decrypted body").unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), b"decrypted body");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&output).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);

            // A link at the path is replaced; what it points to is untouched.
            let elsewhere = dir.join("elsewhere.txt");
            std::fs::write(&elsewhere, b"keep").unwrap();
            std::fs::remove_file(&output).unwrap();
            std::os::unix::fs::symlink(&elsewhere, &output).unwrap();
            write_private(&output, b"decrypted body").unwrap();
            assert_eq!(std::fs::read(&elsewhere).unwrap(), b"keep");
            assert!(!std::fs::symlink_metadata(&output).unwrap().is_symlink());
            assert_eq!(std::fs::read(&output).unwrap(), b"decrypted body");
        }
        // On Windows the file keeps its own access list through the rename:
        // the owner, the system and administrators, none inherited.
        #[cfg(windows)]
        {
            let icacls = std::path::Path::new(&std::env::var_os("SystemRoot").unwrap())
                .join("System32")
                .join("icacls.exe");
            let listed = std::process::Command::new(icacls)
                .arg(&output)
                .output()
                .unwrap();
            let listed = String::from_utf8_lossy(&listed.stdout);
            let entries: Vec<_> = listed.lines().filter(|line| line.contains(":(")).collect();
            assert_eq!(entries.len(), 3, "{listed}");
            assert!(
                entries.iter().all(|entry| !entry.contains("(I)")),
                "{listed}"
            );
        }
        // No temporary file is left behind.
        assert!(std::fs::read_dir(&dir).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
