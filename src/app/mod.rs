use crate::mail::{ConnectionStatus, MailService};

pub struct App {
    mail_service: Box<dyn MailService>,
}

impl App {
    pub fn new(mail_service: impl MailService + 'static) -> Self {
        Self {
            mail_service: Box::new(mail_service),
        }
    }

    pub fn connection_status(&self) -> ConnectionStatus {
        self.mail_service.connection_status()
    }
}
