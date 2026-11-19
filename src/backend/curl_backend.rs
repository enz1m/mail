//! Safe libcurl IMAP and SMTP transport with bounded I/O and cancellation.

use std::time::Duration;

use super::{AccountConfig, AuthMethod, Encryption, MailProtocol};

pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const TOTAL_TIMEOUT: Duration = Duration::from_secs(45);
pub(crate) const LOW_SPEED_TIME: Duration = Duration::from_secs(15);
pub(crate) const LOW_SPEED_BYTES_PER_SECOND: u32 = 32;
pub(crate) const MAX_CONTROL_RESPONSE_BYTES: usize = 256 * 1024;
pub(crate) const MAX_PROTOCOL_HEADER_BYTES: usize = 128 * 1024;
pub(crate) const MAX_FETCH_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_SYNC_AGGREGATE_FETCH_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_SYNC_RETAINED_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_MESSAGE_BODY_BYTES: u32 = 512 * 1024;
pub(crate) const MAX_MIME_HEADER_BYTES: usize = 32 * 1024;
pub(crate) const MAX_MIME_HEADER_COUNT: usize = 128;
pub(crate) const MAX_MIME_PARTS: usize = 64;
pub(crate) const MAX_MIME_DEPTH: usize = 8;
pub(crate) const MAX_MESSAGES_PER_SYNC: usize = 50;
pub(crate) const MAX_UIDS_PER_MAILBOX_SNAPSHOT: usize = 4_096;
pub(crate) const SYNC_WINDOW_MESSAGES: u32 = 200;
pub(crate) const MAX_CREDENTIAL_BYTES: usize = 16 * 1024;
pub(crate) const MAX_SMTP_MESSAGE_BYTES: usize = crate::attachment_store::MAX_OUTGOING_MIME_BYTES;

#[path = "curl_backend/fetch.rs"]
mod fetch;
#[path = "curl_backend/mime.rs"]
mod mime;
#[path = "curl_backend/oauth.rs"]
pub(crate) mod oauth;
#[path = "curl_backend/operations.rs"]
mod operations;
#[path = "curl_backend/transport.rs"]
mod transport;
#[path = "curl_backend/types.rs"]
mod types;

#[cfg(test)]
use fetch::{
    bounded_sync_coverage, parse_fetch_messages, parse_responses, parse_search_uids,
    parse_uid_validity,
};
#[cfg(test)]
use mime::{MimeParseFailure, decode_transfer, parse_mime_message};
#[cfg(test)]
pub(crate) use operations::ready_operation;
pub(crate) use operations::{CurlMailBackend, WatchProgress, watch_inbox};
#[cfg(test)]
use operations::{internal_owner, submit_smtp_operation};
#[cfg(test)]
use transport::{SmtpEndpoint, imap_endpoint_url, smtp_endpoint};
#[cfg(test)]
pub(crate) use types::{
    AttachmentCompletion, FetchedMessage, MailOperation, MailResult, MutationResult,
    RetainedMessage,
};
pub(crate) use types::{
    ImapContentIdentity, MailBackend, MailFailure, MailFailureKind, MailOperationOutput,
    MutationDisposition, MutationRequest, OperationCancel, SendRequest, SyncBatch, SyncCoverage,
    SyncRequest,
};

#[cfg(test)]
#[path = "curl_backend_tests.rs"]
mod tests;
