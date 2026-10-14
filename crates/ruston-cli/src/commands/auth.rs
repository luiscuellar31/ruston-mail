//! Authentication commands: login, logout, whoami.

use crate::cli::{ClientPreset, Ctx};
use crate::commands::{prompt_line, resume};
use crate::render;
use crate::terminal;
use ruston_core::{Client, Error, LoginOptions, Result, TotpPrompt};
use secrecy::SecretString;
use serde_json::json;
use std::sync::Arc;

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

pub async fn login(ctx: &Ctx) -> Result<()> {
    let username = match env_nonempty("PROTON_USER") {
        Some(u) => u,
        None => prompt_line("Proton username/email: ")?,
    };
    let password = match env_nonempty("PROTON_PASSWORD") {
        Some(p) => p,
        None => rpassword::prompt_password("Proton password: ")?,
    };

    // Explicit flags win; otherwise fall back to a --client preset.
    let app_version = ctx
        .app_version
        .clone()
        .or_else(|| ctx.client.map(|c| c.app_version().to_string()));
    let user_agent = resolve_user_agent(ctx.user_agent.clone(), ctx.client);

    let opts = LoginOptions {
        username,
        password: SecretString::from(password),
        totp: ctx.totp.clone().map(SecretString::from),
        mailbox_password: ctx.mailbox_password.clone().map(SecretString::from),
        profile: ctx.profile.clone(),
        base_url: ctx.api_url.clone(),
        app_version,
        user_agent: Some(user_agent),
        hv: Some(crate::hv::resolver(
            ctx.captcha_token.clone(),
            ctx.api_url
                .clone()
                .unwrap_or_else(|| "https://mail.proton.me/api".to_string()),
            ctx.captcha_chrome,
        )),
    };

    let client = Client::login_with_totp_prompt(opts, totp_prompt()).await?;
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

/// What logging out told Proton.
#[derive(Debug)]
enum Revocation {
    /// Proton was asked to end the session.
    Requested,
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
        Ok(client) => {
            client.logout().await?;
            Ok(Revocation::Requested)
        }
        Err(error) => {
            forget().await?;
            Ok(Revocation::Skipped(error.to_string()))
        }
    }
}

pub async fn logout(ctx: &Ctx) -> Result<()> {
    let revocation = log_out(resume(&ctx.profile).await, || {
        Client::forget_session(&ctx.profile)
    })
    .await?;
    let revoked = matches!(revocation, Revocation::Requested);
    if ctx.json {
        render::json_out(&json!({ "status": "ok", "revoked": revoked }));
    } else if let Revocation::Skipped(reason) = revocation {
        terminal::println!("Logged out on this device");
        terminal::eprintln!(
            "Proton was not asked to end the session, which could not be resumed: {reason}"
        );
    } else {
        terminal::println!("Logged out");
    }
    Ok(())
}

pub async fn whoami(ctx: &Ctx) -> Result<()> {
    let client = resume(&ctx.profile).await?;
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
