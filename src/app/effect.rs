use std::future::Future;
use std::pin::Pin;

use crate::mail::LoginRequest;

use super::{AuthAttempt, Message};

/// Work requested by the state machine and performed by the desktop shell.
/// This boundary deliberately contains no GUI types.
pub enum Effect {
    Future(Pin<Box<dyn Future<Output = Message> + Send>>),
    Background(Pin<Box<dyn Future<Output = ()> + Send>>),
    SignIn(AuthAttempt, LoginRequest),
    CancelSignIn,
    Ui(UiEffect),
}

/// Work that must happen on the GUI thread.
#[derive(Debug, Clone, PartialEq)]
pub enum UiEffect {
    CopyText(String),
    FocusSearch,
    ScrollReaderTop,
    RevealConversation(String),
    NotifyNewMail { sender: String, subject: String },
    PickComposeAttachments,
}

/// A flat collection makes combining independent effects explicit and keeps
/// tests able to assert how much work an interaction starts.
#[derive(Default)]
pub struct Effects(Vec<Effect>);

impl Effects {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn perform<F, T>(future: F, map: T) -> Self
    where
        F: Future + Send + 'static,
        F::Output: Send,
        T: FnOnce(F::Output) -> Message + Send + 'static,
    {
        Self(vec![Effect::Future(Box::pin(
            async move { map(future.await) },
        ))])
    }

    pub fn background(future: impl Future<Output = ()> + Send + 'static) -> Self {
        Self(vec![Effect::Background(Box::pin(future))])
    }

    pub fn sign_in(attempt: AuthAttempt, request: LoginRequest) -> Self {
        Self(vec![Effect::SignIn(attempt, request)])
    }

    pub fn cancel_sign_in() -> Self {
        Self(vec![Effect::CancelSignIn])
    }

    pub fn ui(effect: UiEffect) -> Self {
        Self(vec![Effect::Ui(effect)])
    }

    pub fn batch(effects: impl IntoIterator<Item = Self>) -> Self {
        Self(effects.into_iter().flat_map(|effects| effects.0).collect())
    }

    #[cfg(test)]
    pub fn units(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn into_iter(self) -> impl Iterator<Item = Effect> {
        self.0.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;

    use super::*;

    #[test]
    fn batches_are_flat() {
        let effects = Effects::batch([
            Effects::ui(UiEffect::FocusSearch),
            Effects::batch([Effects::none(), Effects::ui(UiEffect::ScrollReaderTop)]),
        ]);

        assert_eq!(effects.units(), 2);
    }

    #[test]
    fn futures_map_their_result_to_one_message() {
        let effect = Effects::perform(async { true }, Message::ShowSettings)
            .into_iter()
            .next()
            .expect("one effect");
        let Effect::Future(future) = effect else {
            panic!("expected future");
        };

        assert!(matches!(block_on(future), Message::ShowSettings(true)));
    }

    #[test]
    fn ui_effects_keep_their_payload() {
        let effect = Effects::ui(UiEffect::CopyText("copy me".into()))
            .into_iter()
            .next()
            .expect("one effect");

        assert!(matches!(
            effect,
            Effect::Ui(UiEffect::CopyText(text)) if text == "copy me"
        ));

        let notify = Effects::ui(UiEffect::NotifyNewMail {
            sender: "Alice".into(),
            subject: "Update".into(),
        })
        .into_iter()
        .next()
        .expect("one effect");
        assert!(matches!(
            notify,
            Effect::Ui(UiEffect::NotifyNewMail { sender, subject })
                if sender == "Alice" && subject == "Update"
        ));

        let pick = Effects::ui(UiEffect::PickComposeAttachments)
            .into_iter()
            .next()
            .expect("one effect");
        assert!(matches!(pick, Effect::Ui(UiEffect::PickComposeAttachments)));
    }
}
