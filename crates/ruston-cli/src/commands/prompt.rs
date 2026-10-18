//! Questions asked on the terminal: a line, a yes or no, and consent to
//! delete for good.

use crate::cli::Ctx;
use crate::terminal;
use ruston_core::{Error, Result};
use std::io::{self, BufRead, IsTerminal, Write};

/// Prompt on stderr (keeping stdout clean) and read a trimmed line.
pub fn prompt_line(prompt: &str) -> std::io::Result<String> {
    terminal::eprint!("{prompt}");
    std::io::stderr().flush()?;
    let mut s = String::new();
    std::io::stdin().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

/// Whether there is someone to ask: a terminal on both ends, and output
/// that is not JSON for a script.
pub(crate) fn can_prompt(ctx: &Ctx) -> bool {
    !ctx.json && io::stdin().is_terminal() && io::stderr().is_terminal()
}

/// Asks a yes/no question; anything short of an explicit yes is a no.
pub(crate) fn confirm(
    question: &str,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> io::Result<bool> {
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

/// Lists what is about to be deleted for good, then asks on the terminal
/// whether to delete `what`, such as "2 draft(s)". A refusal says that
/// nothing was deleted.
pub(crate) fn confirm_deletion(listed: &[String], what: &str) -> io::Result<bool> {
    ask_deletion(
        listed,
        what,
        &mut io::stdin().lock(),
        &mut io::stderr().lock(),
    )
}

/// Asks as [`confirm_deletion`] does, through `input` and `output`.
fn ask_deletion(
    listed: &[String],
    what: &str,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> io::Result<bool> {
    for line in listed {
        terminal::writeln!(output, "  {line}")?;
    }
    let question = format!("Permanently delete {what}? This cannot be undone.");
    let agreed = confirm(&question, input, output)?;
    if !agreed {
        terminal::writeln!(output, "Nothing was deleted.")?;
    }
    Ok(agreed)
}

#[cfg(test)]
mod tests {
    use super::{ask_deletion, require_consent};
    use crate::cli::Ctx;
    use std::io::Cursor;

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
        let agreed = ask_deletion(&doomed, "1 message(s)", &mut Cursor::new("\n"), &mut output);
        assert!(!agreed.unwrap(), "only an explicit yes deletes");
        let shown = String::from_utf8(output).unwrap();
        assert!(shown.contains("Invoice") && shown.contains("evil@example.test"));
        assert!(
            shown
                .chars()
                .all(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        );
        assert!(shown.ends_with(
            "Permanently delete 1 message(s)? This cannot be undone. [y/N]: \
             Nothing was deleted.\n"
        ));
        let mut output = Vec::new();
        assert!(
            ask_deletion(
                &doomed,
                "1 message(s)",
                &mut Cursor::new("yes\n"),
                &mut output
            )
            .unwrap()
        );
        assert!(
            !String::from_utf8(output)
                .unwrap()
                .contains("Nothing was deleted.")
        );
    }
}
