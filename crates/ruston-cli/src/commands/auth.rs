//! Signing in and out: `login`, `logout` and `whoami`.

use crate::cli::{ClientPreset, Ctx, LoginArgs};
use crate::commands::prompt::prompt_line;
use crate::render;
use crate::terminal;
use ruston_core::{
    Client, Error, LoginOptions, LoginPrompts, MailboxPasswordPrompt, Result, TotpPrompt,
};
use secrecy::SecretString;
use serde_json::json;
use std::io::IsTerminal;
use std::sync::Arc;

/// The value of the environment variable `name` when it is set and not
/// empty; otherwise what `ask` gets from the person at the terminal.
fn from_env_or_ask(
    name: &str,
    ask: impl FnOnce() -> std::io::Result<String>,
) -> std::io::Result<String> {
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => Ok(value),
        _ => ask(),
    }
}

pub async fn login(ctx: &Ctx, args: LoginArgs) -> Result<()> {
    let username = from_env_or_ask("PROTON_USER", || prompt_line("Proton username/email: "))?;
    let password = from_env_or_ask("PROTON_PASSWORD", || {
        rpassword::prompt_password("Proton password: ")
    })?;

    // An option given wins over what the --client preset would send.
    let preset_version = args.client.map(|preset| preset.app_version().to_owned());
    let app_version = args.app_version.or(preset_version);
    let user_agent = resolve_user_agent(args.user_agent, args.client);

    let opts = LoginOptions {
        username,
        password: SecretString::from(password),
        totp: args.totp.map(SecretString::from),
        mailbox_password: args.mailbox_password.map(SecretString::from),
        profile: ctx.profile.clone(),
        base_url: args.api_url.clone(),
        app_version,
        user_agent: Some(user_agent),
        hv: Some(crate::hv::resolver(
            args.captcha_token,
            args.api_url
                .unwrap_or_else(|| "https://mail.proton.me/api".to_string()),
            args.captcha_chrome,
        )),
    };

    let prompts = LoginPrompts {
        totp: Some(totp_prompt()),
        // Without a terminal, core's error asks for --mailbox-password.
        mailbox_password: std::io::stdin().is_terminal().then(mailbox_password_prompt),
    };
    let client = Client::login_with_prompts(opts, prompts).await?;
    let email = client.primary_email().unwrap_or("(unknown)");
    if ctx.json {
        render::json_out(&json!({ "status": "ok", "email": email }));
    } else {
        terminal::println!("Logged in as {email}");
    }
    Ok(())
}

/// Explicit `--user-agent` wins, then a `--client` preset, then an honest
/// `ruston-cli/<version> (<os>)`.
fn resolve_user_agent(explicit: Option<String>, preset: Option<ClientPreset>) -> String {
    explicit
        .or_else(|| preset.map(|c| c.user_agent().to_string()))
        .unwrap_or_else(|| {
            format!(
                "ruston-cli/{} ({})",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS
            )
        })
}

/// Ask for the 2FA code only once the server requires it, so it is still
/// valid after a CAPTCHA.
fn totp_prompt() -> TotpPrompt {
    Arc::new(|| {
        Box::pin(async {
            let code =
                tokio::task::spawn_blocking(|| rpassword::prompt_password("Proton 2FA code: "))
                    .await
                    .map_err(|e| Error::Other(format!("2FA prompt: {e}")))??;
            Ok(SecretString::from(code))
        })
    })
}

/// Ask for a separate mailbox password only when the account has one, and
/// hidden, so it never has to go on the command line.
fn mailbox_password_prompt() -> MailboxPasswordPrompt {
    Arc::new(|| {
        Box::pin(async {
            let password =
                tokio::task::spawn_blocking(|| rpassword::prompt_password("Mailbox password: "))
                    .await
                    .map_err(|e| Error::Other(format!("mailbox password prompt: {e}")))??;
            Ok(SecretString::from(password))
        })
    })
}

/// What logging out told Proton.
#[derive(Debug)]
enum Revocation {
    /// Proton confirmed that the session ended.
    Confirmed,
    /// Proton was asked, but did not confirm in time or refused.
    Unconfirmed,
    /// The saved session could not be resumed, so only this device forgot it.
    Skipped(String),
}

/// Signs out with the resumed session, or, when it cannot be resumed, still
/// removes it from this device: local cleanup never depends on Proton.
async fn log_out<F, Fut>(resumed: Result<Client>, forget: F) -> Result<Revocation>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    match resumed {
        Ok(client) => Ok(if client.logout().await? {
            Revocation::Confirmed
        } else {
            Revocation::Unconfirmed
        }),
        Err(error) => {
            forget().await?;
            Ok(Revocation::Skipped(error.to_string()))
        }
    }
}

pub async fn logout(ctx: &Ctx) -> Result<()> {
    let revocation = log_out(Client::resume(&ctx.profile).await, || {
        Client::forget_session(&ctx.profile)
    })
    .await?;
    let revoked = matches!(revocation, Revocation::Confirmed);
    if ctx.json {
        render::json_out(&json!({ "status": "ok", "revoked": revoked }));
    } else {
        match revocation {
            Revocation::Confirmed => terminal::println!("Logged out"),
            Revocation::Unconfirmed => {
                terminal::println!("Logged out on this device");
                terminal::eprintln!(
                    "Proton did not confirm that the session ended; you can end it from your Proton account's security settings"
                );
            }
            Revocation::Skipped(reason) => {
                terminal::println!("Logged out on this device");
                terminal::eprintln!(
                    "Proton was not asked to end the session, which could not be resumed: {reason}"
                );
            }
        }
    }
    Ok(())
}

pub async fn whoami(ctx: &Ctx) -> Result<()> {
    let client = Client::resume(&ctx.profile).await?;
    let email = client.primary_email().unwrap_or("(unknown)");
    if ctx.json {
        render::json_out(&json!({ "email": email }));
    } else {
        terminal::println!("{email}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_session_that_cannot_resume_is_still_removed_from_this_device() {
        let forgot = std::cell::Cell::new(false);
        let outcome = log_out(Err(Error::Unauthorized), || async {
            forgot.set(true);
            Ok(())
        })
        .await
        .unwrap();
        assert!(forgot.get(), "local credentials must go even offline");
        assert!(matches!(outcome, Revocation::Skipped(reason) if reason.contains("unauthorized")));

        let failed = log_out(Err(Error::Unauthorized), || async {
            Err(Error::Session("keyring delete: locked".into()))
        })
        .await;
        assert!(
            matches!(failed, Err(Error::Session(_))),
            "a failed local cleanup is reported"
        );
    }

    #[test]
    fn user_agent_defaults_to_honest_cli_identity() {
        assert_eq!(
            resolve_user_agent(None, None),
            format!(
                "ruston-cli/{} ({})",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS
            )
        );
    }

    #[test]
    fn user_agent_explicit_then_preset_win() {
        assert_eq!(
            resolve_user_agent(Some("custom/1".into()), Some(ClientPreset::Web)),
            "custom/1"
        );
        assert_eq!(
            resolve_user_agent(None, Some(ClientPreset::Ios)),
            ClientPreset::Ios.user_agent()
        );
    }
}
