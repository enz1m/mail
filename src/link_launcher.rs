//! Cancellable portal launches for confirmed links and retained local files.
//!
//! Hostile HTML links reach [`LinkOpener::start`] only after confirmation;
//! attachment opens reach [`LinkOpener::start_file`] with descriptor authority.
//! The framework owns transport, correlation, and exact-once cancellation while
//! this adapter owns the shared request generation and completion subscription.

use enzui_iced_core::portal::completion::CompletionSignal;
use enzui_iced_core::portal::open_uri::{
    OpenFileRequest, OpenUri, OpenUriBackend, OpenUriRequest, PendingOpenUri, RetainedFile,
};
use enzui_iced_core::portal::{CancelReason, PortalOutcome, PortalParent};

pub(crate) type LinkOpenOutcome = PortalOutcome<()>;

// Queued wakes carry app request generations; a shared backend signal counter cannot identify their owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct LinkWake {
    request: u64,
}

impl LinkWake {
    // The app request generation this wake was emitted for.
    #[must_use]
    pub(crate) fn request(self) -> u64 {
        self.request
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WakeVerdict {
    // The wake named a request that is no longer the live one (superseded or
    // already settled). Nothing was polled, so a queued old wake cannot
    // consume the replacement's reply.
    Stale,
    Pending,
    Finished(LinkOpenOutcome),
}

// A user-facing sentence for a failure outcome, or `None` for success and
// cancellation. Every arm is named, so a new [`PortalOutcome`] variant fails
// to compile here.
pub(crate) fn failure_text(outcome: &LinkOpenOutcome) -> Option<String> {
    let s = match outcome {
        PortalOutcome::Completed(()) | PortalOutcome::Cancelled(_) => return None,
        PortalOutcome::Unavailable => "No desktop link opener is available on this system.",
        PortalOutcome::Unsupported => {
            "This desktop's portal does not support opening external links."
        }
        PortalOutcome::NotDeclared => "This build is not permitted to open external links.",
        PortalOutcome::Denied => "The portal refused permission to open the link.",
        PortalOutcome::ParentUnavailable => "The link could not be attached to this window.",
        PortalOutcome::Failed => "The link could not be opened.",
    };
    Some(crate::l10n::tr(s))
}

pub(crate) struct LinkOpener {
    // Built on first use so merely constructing the app spawns no tokio
    // runtime (the render tests never confirm a link).
    backend: Option<Box<dyn OpenUriBackend>>,
    pending: Option<PendingOpenUri>,
    // Identity of the live request. Bumped whenever a new request is
    // installed, so iced tears down the completion subscription bound to
    // the finished request instead of reusing its stream for the new one.
    generation: u64,
}

impl std::fmt::Debug for LinkOpener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkOpener")
            .field(
                "backend",
                &self.backend.as_ref().map(|b| b.describe_open_uri()),
            )
            .field("has_pending", &self.pending.is_some())
            .field("generation", &self.generation)
            .finish()
    }
}

impl Default for LinkOpener {
    fn default() -> Self {
        Self::new()
    }
}

impl LinkOpener {
    pub(crate) fn new() -> Self {
        Self {
            backend: None,
            pending: None,
            generation: 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_backend(backend: Box<dyn OpenUriBackend>) -> Self {
        Self {
            backend: Some(backend),
            pending: None,
            generation: 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn is_pending(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(PendingOpenUri::is_pending)
    }

    // A start refusal is terminal and follows the same completion route as a late portal reply.
    pub(crate) fn start(&mut self, href: &str) -> Result<(), LinkOpenOutcome> {
        // Parse before replacing the live request: malformed hostile input
        // cannot evict a valid operation already owned by the app.
        let uri = OpenUri::parse(href).map_err(|_| PortalOutcome::Failed)?;
        let Some(next_generation) = self.generation.checked_add(1) else {
            return Err(PortalOutcome::Failed);
        };
        // A second confirmed destination supersedes the first. Cancellation
        // and settlement happen synchronously against the owned backend, so
        // neither a later portal wake nor another reducer pass is required to
        // close the exact old RequestId.
        self.cancel_and_settle(CancelReason::Application);
        let Some(backend) = self.backend_mut() else {
            return Err(PortalOutcome::Unavailable);
        };
        let backend: &mut dyn OpenUriBackend = &mut **backend;
        // `PortalParent::None` resolves to `Unparented` regardless of the
        // predicate (it is only consulted for `Window(id)`), so a constant is
        // honest rather than a fake window lookup.
        match OpenUriRequest::new(uri, PortalParent::None).start(backend, |_| true) {
            Ok(pending) => {
                self.pending = Some(pending);
                self.generation = next_generation;
                Ok(())
            }
            Err(outcome) => Err(outcome),
        }
    }

    // Sends an already-retained file descriptor through the framework's
    // OpenFile request without converting it to a pathname.
    pub(crate) fn start_file(&mut self, file: RetainedFile) -> Result<(), LinkOpenOutcome> {
        let Some(next_generation) = self.generation.checked_add(1) else {
            return Err(PortalOutcome::Failed);
        };
        self.cancel_and_settle(CancelReason::Application);
        let Some(backend) = self.backend_mut() else {
            return Err(PortalOutcome::Unavailable);
        };
        let backend: &mut dyn OpenUriBackend = &mut **backend;
        match OpenFileRequest::new(file, PortalParent::None).start(backend, |_| true) {
            Ok(pending) => {
                self.pending = Some(pending);
                self.generation = next_generation;
                Ok(())
            }
            Err(outcome) => Err(outcome),
        }
    }

    // Only a live asynchronous request owns a completion subscription.
    pub(crate) fn completion_subscription(&self) -> iced::Subscription<LinkWake> {
        let Some(pending) = self.pending.as_ref() else {
            return iced::Subscription::none();
        };
        if !pending.is_unsettled() {
            return iced::Subscription::none();
        }
        let Some(signal) = self
            .backend
            .as_deref()
            .and_then(OpenUriBackend::open_uri_completion_signal)
        else {
            return iced::Subscription::none();
        };
        completion_stream(self.generation, signal)
    }

    // A superseded wake polls nothing, so it cannot consume the replacement request’s reply.
    pub(crate) fn pump(&mut self, wake: LinkWake) -> WakeVerdict {
        if wake.request() != self.generation || self.pending.is_none() {
            return WakeVerdict::Stale;
        }
        let pending = self
            .pending
            .as_mut()
            .expect("the live generation was just checked to be pending");
        let backend = self
            .backend
            .as_deref_mut()
            .expect("a pending request implies a backend was built");

        // Poll can itself observe a cancellation that won concurrently while
        // the backend poll was in progress. It then returns `Cancelled`
        // directly; cancellation that was already terminal before this pass
        // instead returns `None`, so adopt the shared lifecycle's reason.
        let outcome = pending
            .poll(backend)
            .or_else(|| pending.cancel_reason().map(PortalOutcome::Cancelled));

        // Settle before deciding whether to release the slot, as the file chooser
        // does: `settle` is idempotent, and a racing cancellation may still owe a
        // Close.
        let _ = pending.settle(backend);

        let Some(outcome) = outcome else {
            return WakeVerdict::Pending;
        };
        self.pending = None;
        WakeVerdict::Finished(outcome)
    }

    pub(crate) fn cancel(&mut self, reason: CancelReason) {
        self.cancel_and_settle(reason);
    }

    #[cfg(test)]
    pub(crate) fn live_wake(&self) -> LinkWake {
        LinkWake {
            request: self.generation,
        }
    }

    // Cancels and settles the exact current generation before releasing it.
    // Portal backend methods are non-blocking by contract, so this is safe in
    // replacement and `Drop` paths.
    fn cancel_and_settle(&mut self, reason: CancelReason) {
        let Some(mut pending) = self.pending.take() else {
            return;
        };
        let _ = pending.cancel(reason);
        if let Some(backend) = self.backend.as_deref_mut() {
            let _ = pending.settle(backend);
        }
    }

    // The backend, built on first use. A refused tokio runtime is reported as
    // [`PortalOutcome::Unavailable`] through `Self::start` instead of aborting.
    fn backend_mut(&mut self) -> Option<&mut Box<dyn OpenUriBackend>> {
        if self.backend.is_none() {
            self.backend =
                Some(enzui_iced_core::portal::open_uri::desktop_launcher::host_open_uri_backend());
        }
        self.backend.as_mut()
    }
}

impl Drop for LinkOpener {
    fn drop(&mut self) {
        self.cancel_and_settle(CancelReason::Application);
    }
}

// One priming wake, then one wake per subsequent portal edge.
//
// Mirrors [`enzui_iced_widgets::file_chooser::completion_subscription`],
// inlined because the widgets crate exposes no subscription over
// [`PendingOpenUri`]; the edge comes from
// [`OpenUriBackend::open_uri_completion_signal`]. Every wake carries `key`,
// the request generation the stream was built for.
fn completion_stream(key: u64, signal: CompletionSignal) -> iced::Subscription<LinkWake> {
    #[derive(Clone)]
    struct CompletionKey {
        key: u64,
        signal: CompletionSignal,
    }
    impl std::hash::Hash for CompletionKey {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
            // Not the signal: renders of the same live request must hash equal, or iced
            // rebuilds the stream every frame.
            self.key.hash(state);
        }
    }
    iced::Subscription::run_with(CompletionKey { key, signal }, |data| {
        let signal = data.signal.clone();
        let request = data.key;
        iced::futures::stream::unfold(
            (signal, None::<u64>),
            move |(signal, seen): (CompletionSignal, Option<u64>)| async move {
                // The portal signal's generation says only "something moved",
                // and is shared by every request on this backend; it drives the
                // wait but never identifies the request.
                let generation = match seen {
                    None => signal.generation(),
                    Some(seen) => signal.wait_after(seen).await,
                };
                Some((LinkWake { request }, (signal, Some(generation))))
            },
        )
    })
}

#[cfg(test)]
pub(crate) mod tests {

    use super::*;
    use std::sync::{Arc, Mutex};

    use enzui_iced_core::portal::backend::{BackendError, RequestId};
    use enzui_iced_core::portal::open_uri::{MockOpenUriBackend, OpenUriBackend, OpenUriReply};
    use enzui_iced_core::portal::parent::ParentHandle;

    #[derive(Clone)]
    pub(crate) struct SharedMockBackend {
        inner: Arc<Mutex<MockOpenUriBackend>>,
        signal: CompletionSignal,
    }

    impl SharedMockBackend {
        pub(crate) fn new() -> Self {
            Self {
                inner: Arc::new(Mutex::new(MockOpenUriBackend::new())),
                signal: CompletionSignal::new(),
            }
        }

        pub(crate) fn boxed(&self) -> Box<dyn OpenUriBackend> {
            Box::new(self.clone())
        }

        pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, MockOpenUriBackend> {
            self.inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }

        pub(crate) fn script_and_notify(&self, id: RequestId, reply: OpenUriReply) {
            self.lock().script(id, reply);
            self.signal.notify();
        }
    }

    impl OpenUriBackend for SharedMockBackend {
        fn start_open_uri(
            &mut self,
            uri: &OpenUri,
            parent: &ParentHandle,
        ) -> Result<RequestId, BackendError> {
            self.lock().start_open_uri(uri, parent)
        }
        fn start_open_file(
            &mut self,
            file: &RetainedFile,
            parent: &ParentHandle,
            activation: Option<&enzui_iced_core::activation::ActivationContext>,
        ) -> Result<RequestId, BackendError> {
            self.lock().start_open_file(file, parent, activation)
        }
        fn poll_open_uri(&mut self, id: RequestId) -> Option<OpenUriReply> {
            self.lock().poll_open_uri(id)
        }
        fn close_open_uri(&mut self, id: RequestId, reason: CancelReason) {
            self.lock().close_open_uri(id, reason);
        }
        fn open_uri_completion_signal(&self) -> Option<CompletionSignal> {
            Some(self.signal.clone())
        }
        fn describe_open_uri(&self) -> String {
            self.lock().describe_open_uri()
        }
    }
}
