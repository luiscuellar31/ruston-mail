//! Message commands.

use crate::cli::Ctx;
use crate::cli::{LabelAction, MessagesCmd, SearchArgs, SendArgs};
use crate::commands::{read_body, resolve_all, resume};
use crate::render;
use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{
    BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use ruston_core::{AddressInfo, Client, Result, SearchOpts, SendOptions};
use std::cell::Cell;
use std::io::{self, BufRead, IsTerminal, Write};

fn search_opts(a: SearchArgs) -> SearchOpts {
    SearchOpts {
        keyword: a.keyword,
        from: a.from,
        to: a.to,
        subject: a.subject,
        after: a.after,
        before: a.before,
        folder: a.folder,
        unread: a.unread,
        limit: a.limit,
    }
}

pub(crate) fn send_options(a: SendArgs, body: String) -> SendOptions {
    SendOptions {
        to: a.to,
        cc: a.cc,
        bcc: a.bcc,
        from: a.from,
        subject: a.subject,
        body,
        html: a.html,
        attachments: a.attach,
        send_at: a.send_at,
        expires_in: a.expires,
    }
}

fn prompt_sender(
    addresses: &[AddressInfo],
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> io::Result<String> {
    writeln!(output, "Choose a sender address:")?;
    for (index, address) in addresses.iter().enumerate() {
        let default = if index == 0 { " (default)" } else { "" };
        writeln!(output, "  {}. {}{default}", index + 1, address.email)?;
    }

    loop {
        write!(output, "Sender [1]: ")?;
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
        writeln!(output, "Choose a number from 1 to {}.", addresses.len())?;
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

fn confirm_html(input: &mut impl BufRead, output: &mut impl Write) -> io::Result<bool> {
    loop {
        write!(output, "Body looks like HTML. Send as HTML? [y/N]: ")?;
        output.flush()?;
        let mut answer = String::new();
        if input.read_line(&mut answer)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "HTML format confirmation cancelled",
            ));
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => return Ok(true),
            "" | "n" | "no" => return Ok(false),
            _ => writeln!(output, "Enter y or n.")?,
        }
    }
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
            let client = resume(&ctx.profile).await?;
            if cached {
                let msgs = client.cached_messages(&folder, unread, page_size)?;
                render::messages_list(ctx.json, msgs.len() as u32, &msgs);
            } else {
                let (total, msgs) = client
                    .list_messages(&folder, page, page_size, unread)
                    .await?;
                render::messages_list(ctx.json, total, &msgs);
            }
            Ok(())
        }
        MessagesCmd::Search(args) => {
            let client = resume(&ctx.profile).await?;
            let opts = search_opts(args);
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
                std::fs::write(&path, &msg.body)?;
                if !ctx.json {
                    println!("Wrote {} bytes to {}", msg.body.len(), path.display());
                }
            } else {
                render::full_message(ctx.json, &msg, format, body_only);
            }
            Ok(())
        }
        MessagesCmd::Send(mut args) => {
            let client = resume(&ctx.profile).await?;
            if args.from.is_none() && can_prompt(ctx) {
                let addresses = client.addresses();
                if addresses.len() > 1 {
                    args.from = Some(prompt_sender(
                        &addresses,
                        &mut io::stdin().lock(),
                        &mut io::stderr().lock(),
                    )?);
                }
            }
            let body = read_body(&args.body)?;
            if !args.html && can_prompt(ctx) && looks_like_html(&body) {
                args.html = confirm_html(&mut io::stdin().lock(), &mut io::stderr().lock())?;
            }
            let eo_password = args.eo_password.clone();
            let eo_hint = args.eo_hint.clone();
            let opts = send_options(args, body);
            let id = match &eo_password {
                Some(pw) => client.send_eo(&opts, pw, eo_hint.as_deref()).await?,
                None => client.send(&opts).await?,
            };
            render::sent(ctx.json, &id);
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
        MessagesCmd::Delete { references } => {
            organize(ctx, &references, "delete", |c, ids| async move {
                c.delete_messages(&ids).await
            })
            .await
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
        MessagesCmd::Empty { folder } => {
            let client = resume(&ctx.profile).await?;
            client.empty_folder(&folder).await?;
            render::action_result(ctx.json, "emptied", std::slice::from_ref(&folder));
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
    use super::{confirm_html, looks_like_html, prompt_sender};
    use ruston_core::AddressInfo;
    use std::io::{self, Cursor};

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
}
