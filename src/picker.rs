//! XDG desktop portal ownership for attachment import and export.
//!
//! The portal backend is session-owned because it is not `Send`; completion is
//! driven by its event signal rather than frame polling.

#[cfg(test)]
use enzui_iced_widgets::file_chooser::PortalBackend;

use enzui_iced_widgets::file_chooser::{
    CancelReason, ChooserOptions, ChooserParent, ChooserSession, FileResult, PortalOperation,
    PortalOutcome, PortalWake,
};

// Chooser intent retained until the terminal portal outcome is routed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PickerTarget {
    ComposeAttachment(u64),
    SaveAttachment(crate::model::types::Attachment),
}

impl PickerTarget {
    pub(crate) fn operation(&self) -> PortalOperation {
        match self {
            Self::ComposeAttachment(_) => PortalOperation::OpenFile,
            Self::SaveAttachment(_) => PortalOperation::SaveFile,
        }
    }

    pub(crate) fn options(&self) -> ChooserOptions {
        match self {
            Self::ComposeAttachment(_) => {
                ChooserOptions::new().title(crate::l10n::tr("Attach a file"))
            }
            Self::SaveAttachment(attachment) => ChooserOptions::new()
                .title(crate::l10n::tr("Save attachment"))
                .current_name(safe_name(&attachment.name)),
        }
    }
}

fn safe_name(name: &str) -> String {
    crate::attachment_store::bounded_display_name(name)
}

pub(crate) use enzui_iced_widgets::file_chooser::failure_text;
pub(crate) use enzui_iced_widgets::file_chooser::failure_text as save_failure_text;

// Attachment intent and stale-result protection around the shared chooser lifecycle.
#[derive(Debug)]
pub(crate) struct Picker {
    session: ChooserSession<PickerTarget>,
    target: Option<PickerTarget>,
    generation: u64,
    parent_request: Option<enzui_iced_widgets::file_chooser::PortalCancel>,
    #[cfg(not(test))]
    exports: enzui_iced_widgets::export_pool::ExportPoolOwner,
}

impl Drop for Picker {
    fn drop(&mut self) {
        self.cancel(CancelReason::Application);
    }
}

impl Default for Picker {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ParentReady {
    generation: u64,
    window: Option<iced::window::Id>,
    parent: enzui_parent_lease::Handoff<Option<ChooserParent>>,
}

impl Picker {
    #[cfg(not(test))]
    pub(crate) fn request(&mut self, target: PickerTarget) -> iced::Task<crate::Message> {
        use enzui_iced_widgets::portal_parent::{ParentExport, export_parent, export_parent_task};
        if self.is_pending() && self.target.as_ref() == Some(&target) {
            return iced::Task::none();
        }
        let Some(generation) = self.generation.checked_add(1) else {
            return iced::Task::done(crate::Message::AttachPickerOutcome(
                target,
                PortalOutcome::Failed,
            ));
        };
        self.cancel(CancelReason::Application);
        self.generation = generation;
        self.target = Some(target);
        let cancel = enzui_iced_widgets::file_chooser::PortalCancel::new();
        self.parent_request = Some(cancel.clone());
        let pool = self.exports.dispatcher();
        iced::window::latest().then(move |window| {
            let ready = move |parent| {
                crate::Message::AttachParentReady(ParentReady {
                    generation,
                    window,
                    parent: enzui_parent_lease::Handoff::new(parent),
                })
            };
            let Some(window) = window else {
                return iced::Task::done(ready(None));
            };
            let pool = pool.clone();
            let cancel = cancel.clone();
            iced::window::run(window, |handle| {
                handle
                    .window_handle()
                    .ok()
                    .map(|handle| export_parent(&handle.as_raw()))
            })
            .then(move |encoded| match encoded {
                Some(ParentExport::Ready(handle)) => {
                    iced::Task::done(ready(Some(ChooserParent::X11 { window, handle })))
                }
                Some(ParentExport::NeedsWaylandExport) => export_parent_task(
                    &pool,
                    generation,
                    window,
                    cancel.clone(),
                    std::time::Duration::from_secs(5),
                )
                .map(move |handoff| {
                    let parent = handoff
                        .take()
                        .and_then(|settled| settled.into_result().ok())
                        .and_then(|adoption| match adoption {
                            enzui_wayland::ParentAdoption::Parented { export, .. } => {
                                Some(ChooserParent::Wayland { window, export })
                            }
                            _ => None,
                        });
                    ready(parent)
                }),
                _ => iced::Task::done(ready(None)),
            })
        })
    }

    pub(crate) fn parent_ready(
        &mut self,
        ready: ParentReady,
    ) -> Option<(PickerTarget, PortalOutcome<FileResult>)> {
        if ready.generation != self.generation || self.parent_request.is_none() {
            return None;
        }
        self.parent_request = None;
        let target = self.target.clone()?;
        let Some(parent) = ready.parent.take().flatten() else {
            self.target = None;
            return Some((target, PortalOutcome::ParentUnavailable));
        };
        let outcome = self.session.start_parented(
            target.clone(),
            target.operation(),
            target.options(),
            parent,
            |window| Some(window) == ready.window,
        );
        if let Err(outcome) = outcome {
            self.target = None;
            return Some((target, outcome));
        }
        None
    }
    pub(crate) fn new() -> Self {
        Self {
            session: ChooserSession::new().with_fallback(),
            target: None,
            generation: 0,
            parent_request: None,
            #[cfg(not(test))]
            exports: export_pool(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_backend(backend: Box<dyn PortalBackend>) -> Self {
        Self {
            session: ChooserSession::with_backend(backend),
            target: None,
            generation: 0,
            parent_request: None,
            #[cfg(not(test))]
            exports: export_pool(),
        }
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.parent_request.is_some() || self.session.is_pending()
    }

    #[cfg(test)]
    pub(crate) fn start(&mut self, target: PickerTarget) -> Result<(), PortalOutcome<FileResult>> {
        if self.is_pending() && self.target.as_ref() == Some(&target) {
            return Ok(());
        }
        let Some(generation) = self.generation.checked_add(1) else {
            return Err(PortalOutcome::Failed);
        };
        #[cfg(test)]
        if tests::backend_construction_is_forced_to_fail() {
            return Err(PortalOutcome::Unavailable);
        }
        self.cancel(CancelReason::Application);
        self.session
            .start(target.clone(), target.operation(), target.options())?;
        self.target = Some(target);
        self.generation = generation;
        Ok(())
    }

    pub(crate) fn cancel(&mut self, reason: CancelReason) {
        if let Some(cancel) = self.parent_request.take() {
            cancel.cancel(reason);
        }
        self.session.cancel(reason);
        self.target = None;
    }

    pub(crate) fn parent_closed(&mut self, window: iced::window::Id) {
        self.session.parent_closed(window);
        self.cancel(CancelReason::ParentClosed);
    }

    pub(crate) fn completion_subscription(&self) -> iced::Subscription<PortalWake> {
        self.session.subscription()
    }

    pub(crate) fn pump(&mut self) -> Option<(PickerTarget, PortalOutcome<FileResult>)> {
        let mut captured = None;
        let _task: iced::Task<()> = self.session.pump(|target, outcome| {
            captured = Some((target, outcome));
        });
        if captured.is_some() {
            self.target = None;
        }
        captured
    }

    pub(crate) fn view<'a>(
        &'a self,
        content: iced::Element<'a, crate::Message>,
    ) -> iced::Element<'a, crate::Message> {
        self.session.view(content, crate::Message::AttachFallback)
    }

    pub(crate) fn handle(
        &mut self,
        event: enzui_iced_widgets::file_chooser::FallbackEvent,
    ) -> iced::Task<crate::Message> {
        self.session
            .handle(event, crate::Message::AttachPickerOutcome)
    }
}

#[cfg(not(test))]
fn export_pool() -> enzui_iced_widgets::export_pool::ExportPoolOwner {
    enzui_iced_widgets::export_pool::ExportPoolOwner::start()
        .unwrap_or_else(|_| enzui_iced_widgets::export_pool::ExportPoolOwner::unavailable())
}

impl crate::State {
    pub(crate) fn request_attachment_picker(
        &mut self,
        target: PickerTarget,
    ) -> iced::Task<crate::Message> {
        #[cfg(test)]
        {
            if let Err(outcome) = self.attach_chooser.start(target.clone()) {
                return self.finish_attach_pick(target, outcome);
            }
            iced::Task::none()
        }
        #[cfg(not(test))]
        self.attach_chooser.request(target)
    }
}

#[cfg(test)]
pub(crate) mod tests {

    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    static FORCE_BACKEND_FAILURE: AtomicBool = AtomicBool::new(false);

    pub(super) fn backend_construction_is_forced_to_fail() -> bool {
        FORCE_BACKEND_FAILURE.load(Ordering::Relaxed)
    }

    use enzui_iced_widgets::file_chooser::{
        BackendError, BackendReply, CancelReason, ChooserOptions, ParentHandle, PortalBackend,
        PortalOperation, PortalResponse, RequestId,
    };

    #[derive(Debug, Default)]
    pub(crate) struct StartLog {
        pub(crate) started: Vec<PortalOperation>,
        pub(crate) closes: usize,
    }

    pub(crate) struct ScriptedBackend {
        log: Arc<Mutex<StartLog>>,

        reply: Option<BackendReply>,
        next_id: u64,
    }

    impl ScriptedBackend {
        fn silent() -> Self {
            Self {
                log: Arc::new(Mutex::new(StartLog::default())),
                reply: None,
                next_id: 0,
            }
        }

        pub(crate) fn replying(response: PortalResponse, uris: Vec<String>) -> Self {
            Self {
                reply: Some(BackendReply { response, uris }),
                ..Self::silent()
            }
        }

        pub(crate) fn completed(uris: Vec<String>) -> Self {
            Self::replying(PortalResponse::Success, uris)
        }

        pub(crate) fn log(&self) -> Arc<Mutex<StartLog>> {
            Arc::clone(&self.log)
        }
    }

    impl PortalBackend for ScriptedBackend {
        fn start(&mut self, operation: PortalOperation) -> Result<RequestId, BackendError> {
            self.start_with_options(operation, &ChooserOptions::new(), &ParentHandle::none())
        }

        fn start_with_options(
            &mut self,
            operation: PortalOperation,
            _options: &ChooserOptions,
            _parent: &ParentHandle,
        ) -> Result<RequestId, BackendError> {
            self.next_id += 1;
            let mut log = self.log.lock().map_err(|_| BackendError::Failed)?;
            log.started.push(operation);
            Ok(RequestId::new(self.next_id))
        }

        fn poll_reply(&mut self, _id: RequestId) -> Option<BackendReply> {
            self.reply.take()
        }

        fn close(&mut self, _id: RequestId, _reason: CancelReason) {
            self.log.lock().unwrap().closes += 1;
        }

        fn describe(&self) -> String {
            "scripted (in-process, no D-Bus)".to_owned()
        }
    }
}
