//! An IMAP IDLE worker signals changes; ordinary sync owns adoption.
//! Authentication refusal or unsupported IDLE falls back to periodic sync.

use std::io::{Read as _, Write as _};
use std::num::NonZeroUsize;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use enzui_iced_core::blocking_task::{BlockingTask, BlockingTaskOwner};
use enzui_iced_core::worker_bridge::{self, WorkerBridgeClose, WorkerReceiver, WorkerSender};
use enzui_services::credential_store::Secret;
use iced::Subscription;

use super::AccountConfig;
use super::curl_backend::{MailFailure, MailFailureKind, WatchProgress, watch_inbox};

// The first wait before reconnecting after a dropped connection; each
// further failure doubles it up to [`MAX_BACKOFF`].
const FIRST_BACKOFF: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);
const ONE: NonZeroUsize = NonZeroUsize::new(1).unwrap();

#[derive(Clone, Debug)]
pub(crate) enum InboxWatchEvent {
    Changed,
    Ended(MailFailure),
}

#[derive(Clone, Debug)]
pub(crate) struct InboxWatchUpdate {
    pub(crate) account_id: String,
    pub(crate) watch_id: u64,
    pub(crate) event: InboxWatchEvent,
}

pub(crate) struct InboxWatch {
    config: AccountConfig,
    id: u64,
    wake: UnixStream,
    cancel: Arc<AtomicBool>,
    // iced deduplicates recipes by id; only the first stream takes this.
    receiver: Arc<Mutex<Option<WorkerReceiver<InboxWatchEvent>>>>,
    close: WorkerBridgeClose<InboxWatchEvent>,
    _task: BlockingTask<()>,
    // Declared last, so it is dropped after `Drop::drop` has woken the work.
    _owner: BlockingTaskOwner,
}

impl std::fmt::Debug for InboxWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboxWatch")
            .field("account_id", &self.config.id)
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

static NEXT_WATCH_ID: AtomicU64 = AtomicU64::new(1);

impl InboxWatch {
    // Start watching `config`'s Inbox with `secret`.
    //
    // # Errors
    //
    // When the wake socket pair or the worker cannot be created.
    pub(crate) fn spawn(config: AccountConfig, secret: Secret) -> std::io::Result<Self> {
        let (wake, wake_worker) = UnixStream::pair()?;
        let (sender, receiver, close) = worker_bridge::channel(ONE);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_config = config.clone();
        let worker_cancel = Arc::clone(&cancel);
        // A watch of its own: its one worker blocks for hours, which would
        // hold a shared pool's slot for good. Dropping the owner waits briefly
        // for the woken worker, then leaves it to finish logging in alone.
        let owner = BlockingTaskOwner::new("mail-inbox-watch", ONE, ONE)?;
        // The sender drops when the work ends, even by panic, which ends the
        // stream after its last event; a close guard would discard `Ended`.
        let task = owner.submit(0, move |_| {
            run(
                &worker_config,
                &secret,
                &worker_cancel,
                wake_worker,
                &sender,
            );
        });
        Ok(Self {
            config,
            id: NEXT_WATCH_ID.fetch_add(1, Ordering::Relaxed),
            wake,
            cancel,
            receiver: Arc::new(Mutex::new(Some(receiver))),
            close,
            _task: task,
            _owner: owner,
        })
    }

    pub(crate) fn config(&self) -> &AccountConfig {
        &self.config
    }

    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub(crate) fn subscription(&self) -> Subscription<InboxWatchUpdate> {
        use iced::advanced::subscription::{EventStream, Hasher, Recipe, from_recipe};
        use iced::futures::stream::{BoxStream, StreamExt as _};
        struct WatchRecipe {
            id: u64,
            account_id: String,
            receiver: Arc<Mutex<Option<WorkerReceiver<InboxWatchEvent>>>>,
        }
        impl Recipe for WatchRecipe {
            type Output = InboxWatchUpdate;
            fn hash(&self, state: &mut Hasher) {
                use std::hash::Hash;
                "mail-app::inbox-watch".hash(state);
                self.id.hash(state);
            }
            fn stream(self: Box<Self>, _events: EventStream) -> BoxStream<'static, Self::Output> {
                let receiver = self
                    .receiver
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                let Some(receiver) = receiver else {
                    return iced::futures::stream::empty().boxed();
                };
                let (account_id, watch_id) = (self.account_id, self.id);
                receiver
                    .map(move |event| InboxWatchUpdate {
                        account_id: account_id.clone(),
                        watch_id,
                        event,
                    })
                    .boxed()
            }
        }
        from_recipe(WatchRecipe {
            id: self.id,
            account_id: self.config.id.clone(),
            receiver: Arc::clone(&self.receiver),
        })
    }
}

impl Drop for InboxWatch {
    fn drop(&mut self) {
        // The cancel flag stops a watch still logging in; the byte, or this
        // end of the pair closing, ends a wait in `IDLE` or in a backoff.
        self.cancel.store(true, Ordering::Relaxed);
        let _ = (&self.wake).write_all(&[1]);
        self.close.close();
    }
}

fn run(
    config: &AccountConfig,
    secret: &Secret,
    cancel: &AtomicBool,
    wake: UnixStream,
    sender: &WorkerSender<InboxWatchEvent>,
) {
    let mut failures = 0u32;
    loop {
        let reconnecting = failures > 0;
        let result = watch_inbox(config, secret, cancel, &wake, |progress| {
            if progress == WatchProgress::Changed || reconnecting {
                let _ = sender.send_progress(InboxWatchEvent::Changed);
            }
            if progress == WatchProgress::Ready {
                failures = 0;
            }
        });
        let failure = match result {
            Ok(()) => return,
            Err(failure) => failure,
        };
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        if !worth_retrying(failure.kind) {
            let _ = sender.finish(None, InboxWatchEvent::Ended(failure));
            return;
        }
        failures = failures.saturating_add(1);
        if !rest(&wake, backoff(failures)) {
            return;
        }
    }
}

// A dropped connection is worth another try; a refused login, a server
// without `IDLE` or a bad configuration is not.
fn worth_retrying(kind: MailFailureKind) -> bool {
    !matches!(
        kind,
        MailFailureKind::Authentication
            | MailFailureKind::Unavailable
            | MailFailureKind::Configuration
            | MailFailureKind::Cancelled
    )
}

fn backoff(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    FIRST_BACKOFF
        .saturating_mul(1 << doublings)
        .min(MAX_BACKOFF)
}

fn rest(wake: &UnixStream, wait: Duration) -> bool {
    if wake.set_read_timeout(Some(wait)).is_err() {
        return false;
    }
    let mut byte = [0u8; 1];
    let rested = matches!(
        (&*wake).read(&mut byte),
        Err(error) if matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        )
    );
    let _ = wake.set_read_timeout(None);
    rested
}

#[cfg(test)]
#[path = "inbox_watch_tests.rs"]
mod tests;
