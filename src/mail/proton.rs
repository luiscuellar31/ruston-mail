use super::{ConnectionStatus, MailService};

#[derive(Default)]
pub struct ProtonMailService {
    client: Option<proton_core::Client>,
}

impl MailService for ProtonMailService {
    fn connection_status(&self) -> ConnectionStatus {
        if self.client.is_some() {
            ConnectionStatus::Connected
        } else {
            ConnectionStatus::Disconnected
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_disconnected() {
        let service = ProtonMailService::default();

        assert_eq!(service.connection_status(), ConnectionStatus::Disconnected);
    }
}
