//! Message commands.

use crate::cli::Ctx;
use crate::cli::{DraftArgs, LabelAction, MessagesCmd};
use crate::commands::{read_body, resolve_all, resume};
use crate::render;
use crate::terminal;
use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{
    BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use ruston_core::{AddressInfo, Client, Error, LocalCache, Result, SendOptions};
use std::cell::Cell;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;

pub(crate) fn send_options(a: DraftArgs, body: String) -> SendOptions {
    SendOptions {
        to: a.to,
        cc: a.cc,
        bcc: a.bcc,
        from: a.from,
        subject: a.subject,
        body,
        html: a.html,
        attachments: a.attach,
        ..SendOptions::default()
    }
}

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

fn can_prompt(ctx: &Ctx) -> bool {
    !ctx.json && io::stdin().is_terminal() && io::stderr().is_terminal()
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

/// Asks a yes/no question; anything short of an explicit yes is a no.
fn confirm(question: &str, input: &mut impl BufRead, output: &mut impl Write) -> io::Result<bool> {
    loop {
        terminal::write!(output, "{question} [y/N]: ")?;
        output.flush()?;
        let mut answer = String::new();
        if input.read_line(&mut answer)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "confirmation cancelled",
            ));
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => return Ok(true),
            "" | "n" | "no" => return Ok(false),
            _ => terminal::writeln!(output, "Enter y or n.")?,
        }
    }
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

/// A permanent delete needs someone to agree to it: a terminal to ask on, or
/// `--yes` given up front.
pub(crate) fn require_consent(ctx: &Ctx, yes: bool) -> Result<()> {
    if yes || can_prompt(ctx) {
        Ok(())
    } else {
        Err(Error::Other(
            "permanent deletion cannot ask here; pass --yes to confirm it".into(),
        ))
    }
}

/// Lists what is about to be deleted for good, then asks.
pub(crate) fn confirm_deletion(
    doomed: &[String],
    question: &str,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> io::Result<bool> {
    for line in doomed {
        terminal::writeln!(output, "  {line}")?;
    }
    confirm(question, input, output)
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
                resume(&ctx.profile)
                    .await?
                    .list_messages(&folder, page, page_size, unread)
                    .await?
            };
            render::messages_list(ctx.json, total, &msgs);
            Ok(())
        }
        MessagesCmd::Search(args) => {
            let client = resume(&ctx.profile).await?;
            let opts = args.into();
            let msgs = client.search_messages(&opts).await?;
            render::messages_list(ctx.json, msgs.len() as u32, &msgs);
            Ok(())
        }
        MessagesCmd::Read {
            reference,
            format,
            body_only,
            output,
        } => {
            let client = resume(&ctx.profile).await?;
            let id = client.resolve_ref(&reference).await?;
            let msg = client.read_message(&id).await?;
            if let Some(path) = output {
                write_private(&path, msg.body.as_bytes())?;
                if !ctx.json {
                    terminal::println!("Wrote {} bytes to {}", msg.body.len(), path.display());
                }
            } else {
                render::full_message(ctx.json, &msg, format, body_only);
            }
            Ok(())
        }
        MessagesCmd::Send(mut args) => {
            let client = resume(&ctx.profile).await?;
            let message = &mut args.message;
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
            let body = read_body(&message.body)?;
            if !message.html && can_prompt(ctx) && looks_like_html(&body) {
                message.html = confirm_html(&mut io::stdin().lock(), &mut io::stderr().lock())?;
            }
            let opts = SendOptions {
                send_at: args.send_at,
                expires_in: args.expires,
                ..send_options(args.message, body)
            };
            let eo_password = match args.eo_password.as_deref() {
                // Asked for, so it never has to go on the command line.
                Some("") => Some(ask_message_password(|prompt| {
                    rpassword::prompt_password(prompt)
                })?),
                password => password.map(str::to_owned),
            };
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
        MessagesCmd::Reply {
            reference,
            all,
            from,
            body,
            attach,
        } => {
            let client = resume(&ctx.profile).await?;
            let body = read_body(&body)?;
            let opts = SendOptions {
                from,
                body,
                attachments: attach,
                ..Default::default()
            };
            let id = client.reply(&reference, all, &opts).await?;
            render::sent(ctx.json, &id);
            Ok(())
        }
        MessagesCmd::Forward {
            reference,
            to,
            from,
            body,
            attach,
        } => {
            let client = resume(&ctx.profile).await?;
            let body = read_body(&body)?;
            let opts = SendOptions {
                to,
                from,
                body,
                attachments: attach,
                ..Default::default()
            };
            let id = client.forward(&reference, &opts).await?;
            render::sent(ctx.json, &id);
            Ok(())
        }
        MessagesCmd::CancelSend { reference } => {
            let client = resume(&ctx.profile).await?;
            client.cancel_send(&reference).await?;
            render::action_result(ctx.json, "cancel-send", std::slice::from_ref(&reference));
            Ok(())
        }
        MessagesCmd::Move { references, dest } => {
            organize(ctx, &references, "move", move |c, ids| async move {
                c.move_messages(&ids, &dest).await
            })
            .await
        }
        MessagesCmd::Trash { references } => {
            organize(ctx, &references, "trash", |c, ids| async move {
                c.trash_messages(&ids).await
            })
            .await
        }
        MessagesCmd::Delete { references, yes } => {
            require_consent(ctx, yes)?;
            let client = resume(&ctx.profile).await?;
            let ids = if yes {
                resolve_all(&client, &references).await?
            } else {
                // A reference can be search text, so show what it matched. A
                // search hit carries what the list shows; only an ID is fetched.
                let mut matched = Vec::with_capacity(references.len());
                for reference in &references {
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
                let question = format!(
                    "Permanently delete {} message(s)? This cannot be undone.",
                    matched.len()
                );
                let mut output = io::stderr().lock();
                if !confirm_deletion(&doomed, &question, &mut io::stdin().lock(), &mut output)? {
                    terminal::writeln!(output, "Nothing was deleted.")?;
                    return Ok(());
                }
                matched.into_iter().map(|meta| meta.id).collect()
            };
            client.delete_messages(&ids).await?;
            render::action_result(ctx.json, "delete", &ids);
            Ok(())
        }
        MessagesCmd::Mark { state, references } => {
            let read = state.as_bool();
            organize(ctx, &references, "mark", move |c, ids| async move {
                c.mark_messages_read(&ids, read).await
            })
            .await
        }
        MessagesCmd::Star { references } => {
            organize(ctx, &references, "star", |c, ids| async move {
                c.star_messages(&ids, true).await
            })
            .await
        }
        MessagesCmd::Unstar { references } => {
            organize(ctx, &references, "unstar", |c, ids| async move {
                c.star_messages(&ids, false).await
            })
            .await
        }
        MessagesCmd::Spam { references } => {
            organize(ctx, &references, "spam", |c, ids| async move {
                c.report_spam(&ids).await
            })
            .await
        }
        MessagesCmd::Ham { references } => {
            organize(ctx, &references, "ham", |c, ids| async move {
                c.report_ham(&ids).await
            })
            .await
        }
        MessagesCmd::Unsubscribe { reference } => {
            let client = resume(&ctx.profile).await?;
            let id = client.resolve_ref(&reference).await?;
            client.unsubscribe(&id).await?;
            render::action_result(ctx.json, "unsubscribed", std::slice::from_ref(&id));
            Ok(())
        }
        MessagesCmd::Empty { folder, yes } => {
            require_consent(ctx, yes)?;
            if !yes {
                let question = format!(
                    "Permanently delete every message in {}? This cannot be undone.",
                    folder.name()
                );
                let mut output = io::stderr().lock();
                if !confirm_deletion(&[], &question, &mut io::stdin().lock(), &mut output)? {
                    terminal::writeln!(output, "Nothing was deleted.")?;
                    return Ok(());
                }
            }
            let client = resume(&ctx.profile).await?;
            client.empty_folder(folder.as_str()).await?;
            render::action_result(ctx.json, "emptied", &[folder.as_str().to_owned()]);
            Ok(())
        }
        MessagesCmd::Label {
            action,
            label_id,
            references,
        } => {
            let name = match action {
                LabelAction::Add => "label",
                LabelAction::Rm => "unlabel",
            };
            organize(ctx, &references, name, move |c, ids| async move {
                match action {
                    LabelAction::Add => c.apply_label(&ids, &label_id).await,
                    LabelAction::Rm => c.remove_label(&ids, &label_id).await,
                }
            })
            .await
        }
        MessagesCmd::Undelete { ids } => {
            let client = resume(&ctx.profile).await?;
            client.undelete_messages(&ids).await?;
            render::action_result(ctx.json, "undeleted", &ids);
            Ok(())
        }
        MessagesCmd::Receipt { reference } => {
            let client = resume(&ctx.profile).await?;
            let id = client.resolve_ref(&reference).await?;
            client.send_read_receipt(&id).await?;
            render::action_result(ctx.json, "receipt sent", std::slice::from_ref(&id));
            Ok(())
        }
    }
}

/// Resolve references, run a bulk action over the resulting IDs, and report.
async fn organize<F, Fut>(ctx: &Ctx, refs: &[String], action: &str, op: F) -> Result<()>
where
    F: FnOnce(Client, Vec<String>) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let client = resume(&ctx.profile).await?;
    let ids = resolve_all(&client, refs).await?;
    op(client, ids.clone()).await?;
    render::action_result(ctx.json, action, &ids);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        ask_message_password, confirm_deletion, confirm_html, looks_like_html, prompt_sender,
        require_consent, write_private,
    };
    use crate::cli::Ctx;
    use ruston_core::AddressInfo;
    use std::io::{self, Cursor};

    #[test]
    fn permanent_deletion_needs_a_terminal_or_yes() {
        // JSON output is for scripts, so it never stops to ask.
        let ctx = Ctx {
            profile: "test".into(),
            json: true,
            demo: false,
            api_url: None,
            app_version: None,
            totp: None,
            mailbox_password: None,
            captcha_token: None,
            client: None,
            user_agent: None,
            captcha_chrome: false,
        };
        assert!(require_consent(&ctx, false).is_err());
        assert!(require_consent(&ctx, true).is_ok());
    }

    #[test]
    fn deletion_lists_messages_without_terminal_controls_and_defaults_to_no() {
        let doomed =
            ["\"Invoice\x1b]52;c;Y2xpcGJvYXJk\x07\" from evil@example.test (id 1)".to_owned()];
        let mut output = Vec::new();
        let question = "Permanently delete 1 message(s)?";
        let agreed = confirm_deletion(&doomed, question, &mut Cursor::new("\n"), &mut output);
        assert!(!agreed.unwrap(), "only an explicit yes deletes");
        let shown = String::from_utf8(output).unwrap();
        assert!(shown.contains("Invoice") && shown.contains("evil@example.test"));
        assert!(
            shown
                .chars()
                .all(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        );
        assert!(
            confirm_deletion(
                &doomed,
                question,
                &mut Cursor::new("yes\n"),
                &mut Vec::new()
            )
            .unwrap()
        );
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
    fn a_body_written_to_a_file_is_private_and_never_written_through_a_link() {
        let dir = std::env::temp_dir().join(format!("ruston-output-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let output = dir.join("message.txt");

        // A file already there is replaced, as before.
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
