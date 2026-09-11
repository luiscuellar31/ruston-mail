#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connected,
    Disconnected,
}

pub trait MailService {
    fn connection_status(&self) -> ConnectionStatus;
}
