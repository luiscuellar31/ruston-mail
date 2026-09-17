use std::sync::{Arc, Mutex, mpsc};

use eframe::egui;
use futures::StreamExt;

use crate::app::{AuthAttempt, Effect, Effects, Message, UiEffect};
use crate::mail::{ProtonMailService, SignInEvent};

/// Executes application effects away from the rendering thread and wakes
/// egui as soon as a result is ready.
pub struct Runtime {
    executor: Option<tokio::runtime::Runtime>,
    sign_in: Mutex<Option<tokio::task::JoinHandle<()>>>,
    sender: EventSender,
    receiver: mpsc::Receiver<Message>,
}

#[derive(Clone)]
struct EventSender {
    messages: mpsc::Sender<Message>,
    context: Arc<Mutex<Option<egui::Context>>>,
}

impl EventSender {
    fn send(&self, message: Message) {
        let _ = self.messages.send(message);
        if let Some(context) = self.context.lock().ok().and_then(|guard| guard.clone()) {
            context.request_repaint();
        }
    }
}

impl Runtime {
    pub fn new() -> std::io::Result<Self> {
        let executor = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("ruston-worker")
            .build()?;
        let (messages, receiver) = mpsc::channel();

        Ok(Self {
            executor: Some(executor),
            sign_in: Mutex::new(None),
            sender: EventSender {
                messages,
                context: Arc::new(Mutex::new(None)),
            },
            receiver,
        })
    }

    pub fn attach(&self, context: &egui::Context) {
        if let Ok(mut current) = self.sender.context.lock() {
            *current = Some(context.clone());
        }
    }

    pub fn drain(&self) -> impl Iterator<Item = Message> + '_ {
        self.receiver.try_iter()
    }

    /// Starts background work and returns the effects that belong to egui.
    pub fn execute(&self, effects: Effects) -> Vec<UiEffect> {
        let mut ui = Vec::new();
        for effect in effects.into_iter() {
            match effect {
                Effect::Future(future) => {
                    let sender = self.sender.clone();
                    self.executor()
                        .spawn(async move { sender.send(future.await) });
                }
                Effect::Background(future) => {
                    self.executor().spawn(future);
                }
                Effect::SignIn(attempt, request) => self.sign_in(attempt, request),
                Effect::CancelSignIn => self.cancel_sign_in(),
                Effect::Ui(effect) => ui.push(effect),
            }
        }
        ui
    }

    fn sign_in(&self, attempt: AuthAttempt, request: crate::mail::LoginRequest) {
        let sender = self.sender.clone();
        let task = self.executor().spawn(async move {
            let (events, mut incoming) = futures::channel::mpsc::channel(1);
            let forward = async move {
                while let Some(event) = incoming.next().await {
                    let message = match event {
                        SignInEvent::Prompt(prompt) => Message::SignInPrompt(attempt, prompt),
                        SignInEvent::Finished(outcome) => Message::SignInFinished(attempt, outcome),
                    };
                    sender.send(message);
                }
            };

            futures::future::join(ProtonMailService::sign_in(request, events), forward).await;
        });
        let mut current = self.sign_in.lock().expect("sign-in task lock poisoned");
        if let Some(previous) = current.replace(task) {
            previous.abort();
        }
    }

    fn cancel_sign_in(&self) {
        if let Some(task) = self
            .sign_in
            .lock()
            .expect("sign-in task lock poisoned")
            .take()
        {
            task.abort();
        }
    }

    fn executor(&self) -> &tokio::runtime::Runtime {
        self.executor.as_ref().expect("runtime is alive")
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(executor) = self.executor.take() {
            executor.shutdown_background();
        }
    }
}
