mod proton;
mod service;

pub use proton::{ProtonMailService, ResumeOutcome, SignInOutcome};
pub use service::{AuthError, LoginRequest};
