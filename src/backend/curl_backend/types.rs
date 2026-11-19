//! Mail backend types: failures, sync batches, operations, and the
//! backend trait.

use super::AccountConfig;
use crate::model::types::{AttachmentPayloadId, MailboxKind, Message};
use enzui_iced_core::blocking_task::{BlockingTask, BlockingTaskCancellation, BlockingTaskError};
use enzui_services::credential_store::Secret;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MailFailureKind {
    Authentication,
    // The bounded transport pool refused this generation because every
    // worker and queue slot was already occupied. Distinct from
    // [`MailFailureKind::Worker`] so the reducer can present a
    // try-again-shortly outcome rather than an unrecoverable worker error.
    Busy,
    Cancelled,
    Configuration,
    // The SMTP transfer failed after DATA began, so server acceptance is unknown.
    DeliveryUncertain,
    // An IMAP command may have reached the server, so retry must reconcile
    // the recorded UID identity before it can issue another command.
    MutationUncertain,
    Envelope,
    MessageTooLarge,
    Network,
    Unavailable,
    Protocol,
    ResponseTooLarge,
    Timeout,
    Tls,
    Worker,
    WorkerShutdown,
    WorkerPanicked,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MailFailure {
    pub(crate) kind: MailFailureKind,
    diagnostic: String,
    sync_progress: Option<(usize, usize)>,
}

impl MailFailure {
    pub(crate) fn new(kind: MailFailureKind, diagnostic: impl Into<String>) -> Self {
        Self {
            kind,
            diagnostic: diagnostic.into(),
            sync_progress: None,
        }
    }

    pub(super) fn with_sync_progress(mut self, completed: usize, total: usize) -> Self {
        self.sync_progress = Some((completed, total));
        self
    }

    pub(crate) fn user_message(&self) -> &'static str {
        match self.kind {
            MailFailureKind::Authentication => "The mail server rejected the account credentials.",
            MailFailureKind::Busy => "Another mail operation is running. Try again in a moment.",
            MailFailureKind::Cancelled => "The mail operation was cancelled.",
            MailFailureKind::Configuration => "The mail server settings are not supported.",
            MailFailureKind::DeliveryUncertain => {
                "Server acceptance is unknown. The Outbox item is kept with the same Message-ID for an explicit Retry."
            }
            MailFailureKind::MutationUncertain => {
                "The server mutation outcome is unknown. Its durable identity was kept for reconciliation before retry."
            }
            MailFailureKind::Envelope => "The outgoing sender or recipient list is invalid.",
            MailFailureKind::MessageTooLarge => "The outgoing message exceeds the safety limit.",
            MailFailureKind::Network => "The mail server could not be reached.",
            MailFailureKind::Unavailable => {
                "This build cannot safely perform the requested mail operation."
            }
            MailFailureKind::Protocol => "The mail server returned an invalid response.",
            MailFailureKind::ResponseTooLarge => {
                "The mail server response exceeded the safety limit."
            }
            MailFailureKind::Timeout => "The mail server did not respond before the timeout.",
            MailFailureKind::Tls => {
                "A secure connection to the mail server could not be established."
            }
            MailFailureKind::Worker => "The mail operation could not be started.",
            MailFailureKind::WorkerShutdown => "The mail worker is unavailable.",
            MailFailureKind::WorkerPanicked => "The mail worker stopped unexpectedly.",
        }
    }

    pub(crate) fn diagnostic(&self) -> &str {
        &self.diagnostic
    }

    pub(crate) fn sync_progress(&self) -> Option<(usize, usize)> {
        self.sync_progress
    }

    #[cfg(test)]
    pub(crate) fn for_test(kind: MailFailureKind) -> Self {
        Self::new(kind, "generated test failure")
    }
}

pub(crate) type MailResult<T> = Result<T, MailFailure>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SyncCoverage {
    Complete,
    Capped { completed: usize, total: usize },
}

impl SyncCoverage {
    pub(crate) fn is_complete(self) -> bool {
        self == Self::Complete
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ImapContentIdentity {
    pub(crate) account_id: String,
    pub(crate) mailbox: MailboxKind,
    pub(crate) uid_validity: u32,
    pub(crate) uid: u32,
    pub(crate) part: String,
    pub(crate) sync_generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AttachmentCompletion {
    pub(crate) identity: ImapContentIdentity,
    pub(crate) payload_id: Option<AttachmentPayloadId>,
    pub(crate) payload_identity: Option<crate::model::types::AttachmentPayloadIdentity>,
    pub(crate) decoded_bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(crate) struct FetchedMessage {
    pub(crate) message: Message,
    pub(crate) body_completion: ImapContentIdentity,
    pub(crate) attachment_completions: Vec<AttachmentCompletion>,
}

#[derive(Clone, Debug)]
pub(crate) struct SyncBatch {
    pub(crate) older_than_uid: Option<u32>,
    pub(crate) retained: Vec<RetainedMessage>,
    pub(crate) account_id: String,
    pub(crate) mailbox: MailboxKind,
    pub(crate) uid_validity: u32,
    pub(crate) sync_generation: u64,
    pub(crate) coverage: SyncCoverage,
    pub(crate) messages: Vec<FetchedMessage>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MutationDisposition {
    Applied,
    AlreadyApplied,
    StaleIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MutationResult {
    pub(crate) mutation_id: u64,
    pub(crate) disposition: MutationDisposition,
    pub(crate) missing_target_ids: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) enum MailOperationOutput {
    Authenticated,
    Inbox(SyncBatch),
    Search(SyncBatch),
    Mutation(MutationResult),
    Submitted,
}

const SMTP_PRE_WIRE: u8 = 0;
const SMTP_WIRE_ENTERED: u8 = 1;
const SMTP_CANCELLED_PRE_WIRE: u8 = 2;

#[derive(Debug)]
pub(super) struct SmtpWireState {
    phase: AtomicU8,
}

impl SmtpWireState {
    pub(super) fn new() -> Self {
        Self {
            phase: AtomicU8::new(SMTP_PRE_WIRE),
        }
    }

    pub(super) fn enter_wire(&self) -> bool {
        match self.phase.compare_exchange(
            SMTP_PRE_WIRE,
            SMTP_WIRE_ENTERED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) | Err(SMTP_WIRE_ENTERED) => true,
            Err(_) => false,
        }
    }

    pub(super) fn cancel_is_prewire(&self) -> bool {
        match self.phase.compare_exchange(
            SMTP_PRE_WIRE,
            SMTP_CANCELLED_PRE_WIRE,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) | Err(SMTP_CANCELLED_PRE_WIRE) => true,
            Err(_) => false,
        }
    }

    pub(super) fn entered(&self) -> bool {
        self.phase.load(Ordering::Acquire) == SMTP_WIRE_ENTERED
    }
}

#[derive(Clone)]
pub(crate) struct OperationCancel {
    pub(super) cancelled: Arc<AtomicBool>,
    // SMTP cancellation must not settle the framework future after DATA began;
    // the transfer result owns the authoritative uncertain settlement then.
    pub(super) smtp_wire: Option<Arc<SmtpWireState>>,
    // Pre-wire SMTP and every non-SMTP cancellation settle queued blocking
    // work immediately. Post-DATA SMTP leaves settlement to the transport.
    pub(super) task_cancel: Option<BlockingTaskCancellation<MailResult<MailOperationOutput>>>,
}

impl std::fmt::Debug for OperationCancel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OperationCancel")
            .field("cancelled", &self.cancelled.load(Ordering::Relaxed))
            .field(
                "smtp_wire_entered",
                &self.smtp_wire.as_ref().is_some_and(|wire| wire.entered()),
            )
            .field("task_cancel", &self.task_cancel.is_some())
            .finish()
    }
}

impl OperationCancel {
    // After SMTP DATA begins, cancellation must settle as accepted or delivery-uncertain.
    pub(crate) fn cancel(&self) {
        if let Some(smtp_wire) = self.smtp_wire.as_ref() {
            let prewire = smtp_wire.cancel_is_prewire();
            self.cancelled.store(true, Ordering::Release);
            if prewire && let Some(task_cancel) = self.task_cancel.as_ref() {
                task_cancel.cancel();
            }
            return;
        }
        // Non-SMTP operations have no ambiguous server-acceptance phase, so
        // cancellation may publish before the cooperative transfer exits.
        if let Some(task_cancel) = self.task_cancel.as_ref() {
            task_cancel.cancel();
        }
        self.cancelled.store(true, Ordering::Release);
    }
}

// Handle to one in-flight or already-settled mail transport generation.
//
// The transport runs on a bounded [`enzui_iced_core::blocking_task::BlockingTaskDispatcher`] worker, so the
// generation stays charged against the pool until the underlying libcurl call
// exits (cancellation cannot forcibly interrupt an in-progress transfer).
pub(crate) struct MailOperation {
    pub(super) generation: u64,
    pub(super) cancel: OperationCancel,
    pub(super) inner: MailOperationInner,
}

pub(super) enum MailOperationInner {
    Dispatched(BlockingTask<MailResult<MailOperationOutput>>),
    #[cfg(test)]
    Settled(MailResult<MailOperationOutput>),
}

impl MailOperation {
    pub(crate) fn cancel_handle(&self) -> OperationCancel {
        self.cancel.clone()
    }

    // Awaits the one terminal settlement for this generation.
    //
    // A cancellation is reported only when it settled before the transport
    // published an outcome; a completed submission or an authoritative server
    // failure is still reported as such when a cancel arrives afterwards.
    pub(crate) async fn finish(self) -> (u64, MailResult<MailOperationOutput>) {
        let MailOperation {
            generation, inner, ..
        } = self;
        let result = match inner {
            #[cfg(test)]
            MailOperationInner::Settled(result) => result,
            MailOperationInner::Dispatched(task) => match task.await.into_result() {
                Ok(result) => result,
                Err(BlockingTaskError::Busy) => Err(MailFailure::new(
                    MailFailureKind::Busy,
                    "mail transport worker queue is busy",
                )),
                Err(BlockingTaskError::Cancelled) => Err(MailFailure::new(
                    MailFailureKind::Cancelled,
                    "mail worker cancelled before completion",
                )),
                Err(BlockingTaskError::Shutdown) => Err(MailFailure::new(
                    MailFailureKind::WorkerShutdown,
                    "mail transport worker owner shut down",
                )),
                Err(BlockingTaskError::Panicked) => Err(MailFailure::new(
                    MailFailureKind::WorkerPanicked,
                    "mail worker panicked",
                )),
                Err(_) => Err(MailFailure::new(
                    MailFailureKind::Worker,
                    "mail worker settled with an unrecognised terminal state",
                )),
            },
        };
        (generation, result)
    }
}

#[derive(Debug)]
pub(crate) struct SendRequest {
    pub(crate) config: AccountConfig,
    pub(crate) envelope_from: String,
    pub(crate) recipients: Vec<String>,
    pub(crate) mime: Vec<u8>,
    pub(crate) secret: Secret,
}

#[derive(Debug)]
pub(crate) struct SyncRequest {
    pub(crate) config: AccountConfig,
    pub(crate) secret: Secret,
    // `(uid_validity, uid)` of messages the app already holds in the mailbox
    // being synced; a sync fetches only their flags.
    pub(crate) known: Vec<(u32, u32)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RetainedMessage {
    pub(crate) uid: u32,
    pub(crate) seen: bool,
    pub(crate) flagged: bool,
}

#[derive(Debug)]
pub(crate) struct MutationRequest {
    pub(crate) config: AccountConfig,
    pub(crate) mutation: crate::local_state::PendingServerMutation,
    pub(crate) secret: Secret,
}

pub(crate) trait MailBackend: Send + Sync {
    fn validate_configuration(&self, config: &AccountConfig) -> MailResult<()>;
    fn validate_submission(&self, config: &AccountConfig) -> MailResult<()>;
    fn probe(&self, generation: u64, request: SyncRequest) -> MailOperation;
    fn sync_mailbox(
        &self,
        generation: u64,
        request: SyncRequest,
        mailbox: MailboxKind,
    ) -> MailOperation;
    fn mutate(&self, generation: u64, request: MutationRequest) -> MailOperation;
    fn submit(&self, generation: u64, request: SendRequest) -> MailOperation;
    fn search_mailbox(
        &self,
        generation: u64,
        request: SyncRequest,
        mailbox: MailboxKind,
        criteria: String,
    ) -> MailOperation;
    // Start watching `config`'s Inbox for server pushes, or `None` where this
    // backend cannot; the app then relies on its periodic sync.
    fn watch_inbox(
        &self,
        _config: &AccountConfig,
        _secret: Secret,
    ) -> Option<crate::backend::inbox_watch::InboxWatch> {
        None
    }
}
