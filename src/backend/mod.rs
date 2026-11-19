//! Validated account configuration and the bounded libcurl mail backend.
//!
//! Credentials remain outside these configuration types and are supplied to
//! one cancellable worker operation at a time.

pub(crate) mod config;
pub(crate) mod curl_backend;
pub(crate) mod discovery;
pub(crate) mod inbox_watch;
mod public_https;
pub(crate) mod unsubscribe;

#[allow(
    unused_imports,
    reason = "The module facade serves production and feature-specific tests."
)]
pub(crate) use config::{
    AccountConfig, AuthMethod, Encryption, ImapConfig, IncomingConfig, MailProtocol, ParsedEmail,
    ProviderPreset, SmtpConfig,
};
