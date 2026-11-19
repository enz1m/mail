use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PendingUndoToast {
    pub(crate) id: ToastId,
    pub(crate) generation: u64,
}

#[derive(Clone, Debug)]
pub(crate) enum AttachmentPreview {
    Loading,
    Image(iced::widget::image::Handle),
    Text(String),
    Unavailable,
}

#[derive(Clone, Debug)]
pub(crate) enum PreviewPayload {
    Image(iced::widget::image::Handle),
    Text(String),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) enum ServerSearch {
    #[default]
    Idle,
    Running {
        query: String,
    },
    Found {
        query: String,
        count: usize,
    },
    Failed {
        query: String,
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum UnsubscribeProgress {
    Sending,
    Done,
    Failed(String),
}

// A move the person can still take back: the server mutations it queued are
// held unsent until its toast settles.
#[derive(Clone, Debug)]
pub(crate) struct UndoMove {
    pub(crate) toast: ToastId,
    pub(crate) previous: Vec<(String, model::types::MessageLocation)>,
    pub(crate) mutations: Vec<u64>,
    pub(crate) reopen: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CloseSaveKey {
    DurableState,
}

#[derive(Clone, Debug)]
pub(crate) enum ClosePhase {
    Draining,
    Failed(String),
}

#[derive(Clone, Debug)]
pub(crate) struct CloseSettlement {
    pub(crate) generation: u64,
    pub(crate) attempt: enzui_iced_core::close_guard::SaveAttemptToken,
    pub(crate) deadline: Instant,
    pub(crate) next_retry: Instant,
    pub(crate) phase: ClosePhase,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LocalTimestampRetry {
    SaveDraft,
    AutosaveDraft,
    CommitPendingSend,
}

#[derive(Clone, Debug)]
pub(crate) enum PersistenceDebt {
    LocalState(local_state::LocalStateToken),
    SyncAdoption(u64),
    Preferences(preferences::SaveOrigin, preferences::PreferencesToken),
    Shortcuts(u64),
}

#[derive(Clone, Debug)]
pub(crate) enum AttachmentRetry {
    Stage {
        generation: u64,
        paths: Vec<std::path::PathBuf>,
        existing_count: usize,
        existing_bytes: u64,
    },
    Export {
        attachment: model::types::Attachment,
        destination: std::path::PathBuf,
    },
    Cleanup,
    Reconcile,
    StartupSurvey,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LocalTimestampFailure {
    pub(crate) retry: LocalTimestampRetry,
    pub(crate) error: enzui_iced_core::local_time::LocalCivilTimeError,
}

#[derive(Clone)]
pub(crate) struct CredentialStoreHandle(pub(crate) Arc<dyn CredentialStore>);

impl fmt::Debug for CredentialStoreHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("CredentialStoreHandle").finish()
    }
}

pub(crate) struct CredentialCompletion(pub(crate) Mutex<Option<CredentialOutcome>>);

impl CredentialCompletion {
    pub(crate) fn new(outcome: CredentialOutcome) -> Self {
        Self(Mutex::new(Some(outcome)))
    }

    pub(crate) fn take(&self) -> Option<CredentialOutcome> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

impl fmt::Debug for CredentialCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("CredentialCompletion").finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CredentialStage {
    StoreAccount,
    LoadForOperation,
    DeleteAbandonedAccount,
    DeleteAccount,
    VerifyAccountDeleted,
    RecoverConfirmCurrent,
    RecoverDeleteExact,
    RecoverConfirmAbsent,
    LoadOldForEdit,
    StoreEditCandidate,
    StoreAuthenticatedEdit,
    LoadEditCandidate,
    DeleteEditCandidate,
    RestoreOldAfterEdit,
    VerifyEditCandidateDeleted,
    VerifyRestoredOldAfterEdit,
    DeleteOldAfterEdit,
    VerifyOldDeletedAfterEdit,
}

#[derive(Clone, Debug)]
pub(crate) struct AccountRemoval {
    pub(crate) account: model::types::Account,
    pub(crate) credential_key: CredentialKey,
    pub(crate) phase: AccountRemovalPhase,
}

#[derive(Clone, Debug)]
pub(crate) enum AccountRemovalPhase {
    Confirming,
    AwaitingJournal(u64),
    Publishing(u64),
    Deleting(u64),
    VerifyingDeletion(u64),
    Failed(String),
}

pub(crate) struct AccountEdit {
    pub(crate) old_account: model::types::Account,
    pub(crate) old_key: CredentialKey,
    pub(crate) candidate: backend::AccountConfig,
    pub(crate) new_key: CredentialKey,
    pub(crate) old_secret: Option<Secret>,
    pub(crate) candidate_secret: Option<Secret>,
    pub(crate) phase: AccountEditPhase,
}

#[derive(Clone, Debug)]
pub(crate) enum AccountEditPhase {
    AwaitingJournal(u64),
    LoadingOld(u64),
    StoringCandidate(u64),
    LoadingCandidate(u64),
    AuthenticatingStoredCandidate(u64),
    AuthenticatingBeforeStore(u64),
    StoringAuthenticatedCandidate(u64),
    Publishing {
        local: local_state::LocalStateToken,
        preferences: preferences::PreferencesToken,
    },
    DeletingCandidate {
        generation: u64,
        failure: String,
    },
    RestoringOld {
        generation: u64,
        failure: String,
    },
    VerifyingCandidateDeleted {
        generation: u64,
        failure: String,
    },
    VerifyingRestoredOld {
        generation: u64,
        failure: String,
    },
    DeletingOld(u64),
    VerifyingOldDeleted(u64),
    Failed(String),
    OldCleanupFailed(String),
}

#[derive(Clone, Debug)]
pub(crate) struct CredentialRecovery {
    pub(crate) generation: u64,
    pub(crate) key: credential_mutation::DurableCredentialKey,
    pub(crate) delete_after: Option<credential_mutation::DurableCredentialKey>,
}

#[derive(Clone, Debug)]
pub(crate) enum MailIntent {
    ProbeAccount(backend::AccountConfig),
    SyncMailbox {
        config: backend::AccountConfig,
        mailbox: MailboxKind,
        refresh_generation: u64,
    },
    ReconcileServerMutation {
        config: backend::AccountConfig,
        mutation_id: u64,
    },
    ProbeAccountEdit(backend::AccountConfig),
    SubmitOutbox {
        config: backend::AccountConfig,
        pending_id: String,
    },
    SearchServer {
        config: backend::AccountConfig,
        mailbox: MailboxKind,
        criteria: String,
        query: String,
    },
}

impl MailIntent {
    pub(crate) fn config(&self) -> &backend::AccountConfig {
        match self {
            Self::ProbeAccount(config)
            | Self::ProbeAccountEdit(config)
            | Self::SyncMailbox { config, .. }
            | Self::ReconcileServerMutation { config, .. }
            | Self::SearchServer { config, .. } => config,
            Self::SubmitOutbox { config, .. } => config,
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::ProbeAccount(_) => "account authentication",
            Self::ProbeAccountEdit(_) => "updated account authentication",
            Self::SyncMailbox { .. } => "mailbox synchronization",
            Self::ReconcileServerMutation { .. } => "server mutation reconciliation",
            Self::SubmitOutbox { .. } => "message delivery",
            Self::SearchServer { .. } => "server search",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct MailNotice {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) retryable: bool,
    pub(crate) can_cancel_outbox: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingSyncAdoption {
    pub(crate) announce: bool,
    pub(crate) generation: u64,
    pub(crate) account_id: String,
    pub(crate) mailbox: MailboxKind,
    pub(crate) base_token: local_state::LocalStateToken,
    pub(crate) candidate_token: local_state::LocalStateToken,
    pub(crate) candidate_revision: u64,
    pub(crate) candidate_store: MailboxStore,
    pub(crate) candidate_state: local_state::LocalState,
    pub(crate) fetched_payloads: Vec<model::types::AttachmentPayloadRef>,
}

// The production paths of this app's three persisted frames.
//
// A path lets a refused load take identity-bound custody of the existing
// bytes. `deferred` marks the production route, whose reads leave the boot
// thread.
#[derive(Clone, Debug, Default)]
pub(crate) struct PersistedFrames {
    pub(crate) local_state: Option<std::path::PathBuf>,
    pub(crate) preferences: Option<std::path::PathBuf>,
    pub(crate) shortcuts: Option<std::path::PathBuf>,
    pub(crate) deferred: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FrameKind {
    LocalState,
    Preferences,
    Shortcuts,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FrameStatus {
    // The bounded owner holds the read; nothing derived from this frame is
    // installed and this frame publishes nothing.
    Loading,
    // The read was refused and custody of the unreadable bytes is in flight.
    Retaining,
    Publishable,
    // The frame could not be read, or could not be retained: it stays
    // write-blocked for the rest of the session.
    WriteBlocked,
}

const FRAME_READ_FAILED: &str = "Saved state could not be read";
// Shown when the bounded owner cannot admit a frame read.
const FRAME_READ_BUSY: &str = "Saved state is already being read";
const FRAME_READ_SHUTDOWN: &str = "The saved-state worker is unavailable";
const FRAME_READ_PANICKED: &str = "The saved-state worker stopped unexpectedly";

pub(crate) struct FrameOutcome {
    pub(crate) summary: Option<String>,
    // The unreadable bytes this frame still owes custody of.
    pub(crate) custody: Option<(
        std::path::PathBuf,
        enzui_iced_core::quarantine::ExpectedSourceIdentity,
    )>,
}

impl FrameOutcome {
    pub(crate) fn settled(
        summary: Option<String>,
        custody: Option<(
            std::path::PathBuf,
            enzui_iced_core::quarantine::ExpectedSourceIdentity,
        )>,
    ) -> Self {
        Self { summary, custody }
    }
}

pub(crate) struct FrameLoad<T> {
    pub(crate) status: FrameStatus,
    pub(crate) path: Option<std::path::PathBuf>,
    pub(crate) identity: Option<enzui_iced_core::quarantine::ExpectedSourceIdentity>,
    pub(crate) staged: Option<T>,
    pub(crate) read_succeeded: bool,
}

impl<T> FrameLoad<T> {
    pub(crate) fn inline(read_succeeded: bool) -> Self {
        Self {
            status: FrameStatus::Publishable,
            path: None,
            identity: None,
            staged: None,
            read_succeeded,
        }
    }

    // A frame whose read must leave the boot thread.
    pub(crate) fn deferred(path: Option<std::path::PathBuf>) -> Self {
        Self {
            status: FrameStatus::Loading,
            path,
            identity: None,
            staged: None,
            read_succeeded: false,
        }
    }

    pub(crate) fn publishable(&self) -> bool {
        self.status == FrameStatus::Publishable
    }

    pub(crate) fn pending(&self) -> bool {
        matches!(self.status, FrameStatus::Loading | FrameStatus::Retaining)
    }

    // Records the worker refusal that produced no read.
    pub(crate) fn refuse_worker(
        &mut self,
        error: enzui_iced_core::blocking_task::BlockingTaskError,
    ) -> String {
        self.status = FrameStatus::WriteBlocked;
        let message = match error {
            enzui_iced_core::blocking_task::BlockingTaskError::Busy => FRAME_READ_BUSY,
            enzui_iced_core::blocking_task::BlockingTaskError::Shutdown => FRAME_READ_SHUTDOWN,
            enzui_iced_core::blocking_task::BlockingTaskError::Panicked => FRAME_READ_PANICKED,
            enzui_iced_core::blocking_task::BlockingTaskError::Cancelled => FRAME_READ_FAILED,
            _ => FRAME_READ_FAILED,
        };
        crate::l10n::tr(message)
    }

    // Folds one completed load into this frame.
    //
    // Only a successful read stages a document; a refused frame stays
    // write-blocked until custody of its existing bytes is proven.
    pub(crate) fn settle(
        &mut self,
        outcome: enzui_iced_core::persist::LoadOutcome<T>,
    ) -> FrameOutcome {
        let summary = outcome.user_summary();
        match outcome {
            enzui_iced_core::persist::LoadOutcome::Loaded(value) => {
                self.staged = Some(value);
                self.read_succeeded = true;
                self.status = FrameStatus::Publishable;
                FrameOutcome::settled(None, None)
            }
            enzui_iced_core::persist::LoadOutcome::Absent { .. } => {
                self.read_succeeded = true;
                self.status = FrameStatus::Publishable;
                FrameOutcome::settled(None, None)
            }
            _ => match (self.path.clone(), self.identity) {
                (Some(path), Some(identity)) => {
                    self.status = FrameStatus::Retaining;
                    FrameOutcome::settled(summary, Some((path, identity)))
                }
                _ => {
                    self.status = FrameStatus::WriteBlocked;
                    FrameOutcome::settled(summary, None)
                }
            },
        }
    }
}

pub(crate) struct StartupLoads {
    pub(crate) local_state: FrameLoad<local_state::LocalState>,
    pub(crate) preferences: FrameLoad<preferences::Preferences>,
    pub(crate) shortcuts: FrameLoad<enzui_iced_core::ShortcutOverrides>,
}

impl StartupLoads {
    pub(crate) fn pending(&self) -> bool {
        self.local_state.pending() || self.preferences.pending() || self.shortcuts.pending()
    }

    pub(crate) fn publishable(&self) -> bool {
        self.local_state.publishable()
            && self.preferences.publishable()
            && self.shortcuts.publishable()
    }
}

pub(crate) enum FrameCompletion {
    LocalState(
        Option<enzui_iced_core::quarantine::ExpectedSourceIdentity>,
        Box<enzui_iced_core::persist::LoadOutcome<local_state::LocalState>>,
    ),
    Preferences(
        Option<enzui_iced_core::quarantine::ExpectedSourceIdentity>,
        enzui_iced_core::persist::LoadOutcome<preferences::Preferences>,
    ),
    Shortcuts(
        Option<enzui_iced_core::quarantine::ExpectedSourceIdentity>,
        enzui_iced_core::persist::LoadOutcome<enzui_iced_core::ShortcutOverrides>,
    ),
}

// Carries one completion through the reducer exactly once.
//
// A completion may not be cloned, and a second delivery must not install a
// second time, so the payload is taken on first use.
pub(crate) struct FrameLoadHandoff(Mutex<Option<FrameCompletion>>);

impl FrameLoadHandoff {
    pub(crate) fn new(completion: FrameCompletion) -> Self {
        Self(Mutex::new(Some(completion)))
    }

    pub(crate) fn take(&self) -> Option<FrameCompletion> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

impl fmt::Debug for FrameLoadHandoff {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FrameLoadHandoff")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct AddressMenuTarget {
    pub(crate) message_id: String,
    pub(crate) recipient: Option<usize>,
    pub(crate) name: String,
    pub(crate) email: String,
}
