use super::*;

impl State {
    pub(crate) fn compose_ctl(&mut self) -> ComposeController<'_> {
        ComposeController {
            store: &mut self.store,
            compose: &mut self.compose,
            pending_send: &mut self.pending_send,
            next_ordinal: &mut self.next_ordinal,
            active_account_id: &self.active_account_id,
            clock: self.local_clock.as_ref(),
        }
    }

    fn record_local_timestamp_failure(
        &mut self,
        retry: LocalTimestampRetry,
        error: enzui_iced_core::local_time::LocalCivilTimeError,
    ) {
        self.local_timestamp_failure = Some(LocalTimestampFailure { retry, error });
    }

    pub(crate) fn open_compose(&mut self, mode: ComposeMode) -> bool {
        if self.pending_send.is_some() && !self.fixture_mode {
            self.mail_notice = Some(MailNotice {
                title: crate::l10n::tr("Message remains in Outbox"),
                body: crate::l10n::tr(
                    "Retry or cancel the queued message before composing another.",
                ),
                retryable: self.mail_retry.is_some(),
                can_cancel_outbox: true,
            });
            return false;
        }
        match self.compose_ctl().try_open(mode) {
            Ok(()) => {
                self.local_timestamp_failure = None;
                true
            }
            Err(error) => {
                self.record_local_timestamp_failure(LocalTimestampRetry::CommitPendingSend, error);
                false
            }
        }
    }

    pub(crate) fn send_compose(&mut self) -> bool {
        match self.compose_ctl().try_send_compose() {
            Ok(sent) => {
                self.local_timestamp_failure = None;
                sent
            }
            Err(error) => {
                self.record_local_timestamp_failure(LocalTimestampRetry::CommitPendingSend, error);
                false
            }
        }
    }

    pub(crate) fn commit_pending_send(&mut self) -> bool {
        match self.compose_ctl().try_commit_pending_send() {
            Ok(committed) => {
                self.local_timestamp_failure = None;
                committed
            }
            Err(error) => {
                self.record_local_timestamp_failure(LocalTimestampRetry::CommitPendingSend, error);
                false
            }
        }
    }

    pub(crate) fn save_compose_draft(&mut self) -> Option<String> {
        match self.compose_ctl().try_save_draft() {
            Ok(saved) => {
                self.local_timestamp_failure = None;
                saved
            }
            Err(error) => {
                self.record_local_timestamp_failure(LocalTimestampRetry::SaveDraft, error);
                None
            }
        }
    }

    pub(crate) fn autosave_compose_draft(&mut self) -> bool {
        match self.compose_ctl().try_autosave_draft() {
            Ok(saved) => {
                self.local_timestamp_failure = None;
                saved
            }
            Err(error) => {
                self.record_local_timestamp_failure(LocalTimestampRetry::AutosaveDraft, error);
                false
            }
        }
    }

    pub(crate) fn checkpoint_compose_draft(&mut self) {
        if self.pending_send.is_some() {
            return;
        }
        if self.autosave_compose_draft()
            && let Some(id) = self.compose.editing_draft_id.clone()
        {
            self.draft_save_pending = true;
            self.queue_server_mutation(local_state::ServerMutationKind::DraftUpserted, vec![id]);
        }
    }

    pub(crate) fn retry_local_timestamp(&mut self) -> Task<Message> {
        let Some(failure) = self.local_timestamp_failure else {
            return Task::none();
        };
        match failure.retry {
            LocalTimestampRetry::SaveDraft => {
                let was_open = self.compose.open;
                if let Some(id) = self.save_compose_draft() {
                    self.queue_server_mutation(
                        local_state::ServerMutationKind::DraftUpserted,
                        vec![id],
                    );
                    self.compose_overflow_open = false;
                    self.sync_body_editor();
                    self.reconcile_selection();
                    if was_open {
                        return self.dismiss_compose_focus(DismissCause::DefaultAction);
                    }
                }
            }
            LocalTimestampRetry::AutosaveDraft => {
                if self.autosave_compose_draft()
                    && let Some(id) = self.compose.editing_draft_id.clone()
                {
                    self.draft_save_pending = true;
                    self.queue_server_mutation(
                        local_state::ServerMutationKind::DraftUpserted,
                        vec![id],
                    );
                }
            }
            LocalTimestampRetry::CommitPendingSend => {
                let _ = self.commit_pending_send();
                self.reconcile_selection();
            }
        }
        Task::none()
    }

    pub(crate) fn timestamp_failure_title(retry: LocalTimestampRetry) -> &'static str {
        match retry {
            LocalTimestampRetry::SaveDraft | LocalTimestampRetry::AutosaveDraft => {
                "Could not timestamp this draft"
            }
            LocalTimestampRetry::CommitPendingSend => "Could not create the local Sent copy",
        }
    }

    fn local_timestamp_notice_details(&self) -> Option<(String, String)> {
        self.local_timestamp_failure.map(|failure| {
            let recovery = match failure.retry {
                LocalTimestampRetry::SaveDraft | LocalTimestampRetry::AutosaveDraft => {
                    "The draft remains open and has not been created with a fallback time."
                }
                LocalTimestampRetry::CommitPendingSend => {
                    "The pending send is retained and nothing was timestamped with a fallback time."
                }
            };
            (
                Self::timestamp_failure_title(failure.retry).to_owned(),
                format!("{recovery} {}", failure.error),
            )
        })
    }

    // Reported in the bottom-right corner, over the composer too.
    pub(crate) fn local_timestamp_notice(&self) -> Option<(String, String)> {
        if self.close_settlement.is_some() {
            None
        } else {
            self.local_timestamp_notice_details()
        }
    }

    pub(crate) fn arm_undo_send_toast(&mut self, now: std::time::Instant) {
        let id = self.undo_send_toasts.push_at(undo_send_toast_spec(), now);
        self.pending_undo_toast = Some(PendingUndoToast {
            id,
            generation: self.send_generation,
        });
    }

    fn resolve_undo_send_toast(&mut self, id: ToastId, discard: bool) -> (bool, bool) {
        let Some(tracked) = self.pending_undo_toast else {
            return (false, false);
        };
        if tracked.id != id
            || tracked.generation != self.send_generation
            || self.pending_send.is_none()
        {
            return (false, false);
        }

        self.pending_undo_toast = None;
        if discard {
            self.send_generation = self.send_generation.wrapping_add(1);
            let restored = self.compose_ctl().undo_pending_send();
            if restored {
                self.compose_generation = self.compose_generation.wrapping_add(1);
            }
            if restored
                && self.autosave_compose_draft()
                && let Some(id) = self.compose.editing_draft_id.clone()
            {
                self.draft_save_pending = true;
                self.queue_server_mutation(
                    local_state::ServerMutationKind::DraftUpserted,
                    vec![id],
                );
            }
            self.compose_overflow_open = false;
            self.sync_body_editor();
            (restored, false)
        } else if self.fixture_mode {
            let pending_targets = self.pending_send_targets();
            if self.commit_pending_send()
                && let Some(targets) = pending_targets
            {
                self.queue_server_mutation(local_state::ServerMutationKind::Send, targets);
            }
            self.reconcile_selection();
            (false, false)
        } else {
            (false, true)
        }
    }

    pub(crate) fn update_undo_send_toast(&mut self, event: ToastEvent) -> Task<Message> {
        self.undo_move_from_toast(&event);
        let discard = match &event {
            ToastEvent::Action(id, label) if label != SEND_NOW_ACTION => Some(*id),
            _ => None,
        };
        let event = match event {
            ToastEvent::Action(id, label)
                if label == SEND_NOW_ACTION
                    && self.pending_undo_toast.is_some_and(|pending| {
                        pending.id == id && pending.generation == self.send_generation
                    }) =>
            {
                ToastEvent::Dismiss(id)
            }
            event => event,
        };
        let removed = self.undo_send_toasts.update(event);
        let mut restored = false;
        let mut transmit = false;
        for id in removed {
            let resolution = self.resolve_undo_send_toast(id, discard == Some(id));
            restored |= resolution.0;
            transmit |= resolution.1;
        }
        let focus = if restored {
            self.arm_compose_focus()
        } else {
            Task::none()
        };
        let send = if transmit {
            self.start_pending_send()
        } else {
            Task::none()
        };
        Task::batch([focus, send])
    }

    pub(crate) fn reconcile_undo_send_toast_removals(&mut self) -> Task<Message> {
        let mut unavailable = self.undo_send_toasts.take_dropped();
        unavailable.extend(self.undo_send_toasts.take_replaced());
        let mut transmit = false;
        for id in unavailable {
            transmit |= self.resolve_undo_send_toast(id, false).1;
        }

        if self.pending_send.is_none()
            && let Some(tracked) = self.pending_undo_toast
            && self.undo_send_toasts.visible_ids().contains(&tracked.id)
        {
            self.undo_send_toasts.dismiss(tracked.id);
            self.pending_undo_toast = None;
        }
        if transmit {
            self.start_pending_send()
        } else {
            Task::none()
        }
    }

    // Rebuild the body editor `Content` from `compose.body` when the reducer
    // replaces the whole compose state; per-keystroke edits flow the other way so
    // the caret is preserved.
    pub(crate) fn sync_body_editor(&mut self) {
        self.body_editor = text_editor::Content::with_text(&self.compose.body);
        // A seed or undo restoring a rich compose must rebuild the controller too.
        match self.compose.format {
            model::compose::ComposeFormat::Rich | model::compose::ComposeFormat::Html => {
                // A stored HTML body is read back through the emitter's reader so a reopened
                // draft keeps its styles, inks and alignment; seeding from plain text would let
                // the next save overwrite the stored HTML. A refusal falls back to the plain
                // body.
                let doc = self
                    .compose
                    .body_html
                    .as_deref()
                    .and_then(|html| enzui_iced_widgets::rich_edit::from_html(html).ok())
                    .unwrap_or_else(|| {
                        enzui_iced_widgets::rich_edit::RichEditDocument::from_plain(
                            &self.compose.body,
                        )
                    });
                doc.text().clone_into(&mut self.compose.body);
                self.rich_controller = enzui_iced_widgets::rich_edit::RichEditController::new(doc);
                self.rich_editor = text_editor::Content::with_text(&self.compose.body);
                self.body_editor = text_editor::Content::with_text(&self.compose.body);
            }
            model::compose::ComposeFormat::Plain => {}
        }
    }

    pub(crate) fn position_compose_caret(&mut self, offset: usize) {
        use enzui_iced_widgets::rich_edit::{
            RichEditController, RichEditDocument, sync_content_cursor_from_controller,
        };
        let mut plain = RichEditController::new(RichEditDocument::from_plain(&self.compose.body));
        plain.set_selection(offset..offset);
        sync_content_cursor_from_controller(&plain, &mut self.body_editor);
        self.rich_controller.set_selection(offset..offset);
        sync_content_cursor_from_controller(&self.rich_controller, &mut self.rich_editor);
    }

    // Re-seed the signature editor from the open account's expander; `None`
    // resets it so a stale buffer cannot reach the next account.
    pub(crate) fn sync_signature_editor(&mut self, account_id: Option<&str>) {
        let text = account_id
            .and_then(|id| self.store.account(id))
            .map(|a| a.signature.clone())
            .unwrap_or_default();
        self.signature_editor = text_editor::Content::with_text(&text);
    }

    pub(crate) fn resolve_key(
        &self,
        event: &iced::keyboard::Event,
        captured: bool,
    ) -> Option<shortcuts::ShortcutAction> {
        shortcuts::dispatch_in(
            self.shortcuts.resolved(),
            event,
            shortcuts::ShortcutContext::new(captured, self.modal_owns_keyboard()),
        )
    }

    // Background keyboard ownership follows the scrimmed presentations, not
    // widget capture: a focused dialog button can leave bare keys uncaptured.
    pub(crate) fn modal_owns_keyboard(&self) -> bool {
        self.compose.open || self.blocking_presentation_open()
    }

    pub(crate) fn blocking_presentation_open(&self) -> bool {
        self.settings.open
            || self.rules_ui.open
            || self.shortcuts_open
            || self.bulk.pending_empty.is_some()
            || self.pending_open_url.is_some()
            || self.pending_block_sender.is_some()
            || !self.pending_move_targets.is_empty()
            || self
                .message_dialogs
                .is_open(&crate::modal_focus::MessageDialog::Snooze)
            || self.schedule_editor.is_some()
    }

    // One report from the shared editable-shortcuts pane.
    //
    // [`enzui_iced_core::EditableShortcutCatalog`] refuses before it mutates, so a
    // refused candidate changes only `shortcut_feedback`, shown through
    // `RebindRefusal::message()` verbatim.
    pub(crate) fn update_shortcut_settings(
        &mut self,
        event: enzui_iced_widgets::editable_shortcuts::EditableShortcutsEvent,
    ) -> Task<Message> {
        use enzui_iced_widgets::editable_shortcuts::EditableShortcutsEvent as Ev;
        use enzui_iced_widgets::key_capture_chip::KeyCapture;

        match event {
            Ev::Capture { action, event } => match event {
                KeyCapture::Started => {
                    // One recorder per window: arming this row disarms the previous one and clears
                    // a stale refusal.
                    self.shortcut_armed = Some(action);
                    self.shortcut_feedback = None;
                    Task::none()
                }
                KeyCapture::Cancelled => {
                    self.shortcut_armed = None;
                    self.shortcut_feedback = None;
                    Task::none()
                }
                KeyCapture::Cleared => {
                    // There is no unbound state; name the affordance that exists instead of
                    // ignoring the press.
                    self.shortcut_feedback = Some((action, shortcuts::CLEAR_REFUSED.to_owned()));
                    Task::none()
                }
                KeyCapture::Candidate(chord) => {
                    match self
                        .shortcuts
                        .try_rebind(&action, chord, shortcuts::validate)
                    {
                        Ok(()) => {
                            self.shortcut_armed = None;
                            self.shortcut_feedback = None;
                            self.save_shortcut_change()
                        }
                        Err(refusal) => {
                            self.shortcut_feedback = Some((action, refusal.message()));
                            Task::none()
                        }
                    }
                }
            },
            Ev::Reset { action } => {
                self.shortcut_feedback = None;
                if self.shortcuts.reset(&action) {
                    self.save_shortcut_change()
                } else {
                    Task::none()
                }
            }
            Ev::ResetAll => {
                self.shortcut_armed = None;
                self.shortcut_feedback = None;
                if self.shortcuts.reset_all() {
                    self.save_shortcut_change()
                } else {
                    Task::none()
                }
            }
        }
    }

    pub(crate) fn local_state_token_for_revision(
        &self,
        store: &MailboxStore,
        sync_adoption_revision: u64,
    ) -> local_state::LocalStateToken {
        local_state::LocalState::token(
            store,
            sync_adoption_revision,
            self.pending_send.as_ref(),
            self.pending_server_mutations.len(),
            local_state::LocalStateMetadata {
                next_message_ordinal: self.next_ordinal,
                next_account_ordinal: self.next_account_ordinal,
                active_account_id: &self.active_account_id,
                send_generation: self.send_generation,
                next_mutation_ordinal: self.next_server_mutation_ordinal,
                credential_journal: self.credential_mutations.journal(),
            },
        )
    }

    pub(crate) fn local_state_token(&self) -> local_state::LocalStateToken {
        self.local_state_token_for_revision(&self.store, self.sync_adoption_revision)
    }

    pub(crate) fn local_state_is_published(&self) -> bool {
        self.local_state_published == self.local_state_token()
    }

    // Submits one synchronous call to the bounded blocking owner.
    //
    // The generation is per-submission, so a settlement can never be mistaken
    // for another submission's answer. Exhaustion returns a terminal shutdown
    // settlement without dispatching or reusing a token.
    pub(crate) fn submit_blocking<T, F>(
        &mut self,
        work: F,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = enzui_iced_core::blocking_task::BlockingTaskSettlement<T>,
                > + Send,
        >,
    >
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let Some(generation) = self.blocking_generation.checked_add(1) else {
            self.blocking_generation_exhausted = true;
            self.attachment_notice = Some(crate::l10n::tr(
                "Background work is unavailable until the application restarts.",
            ));
            return Box::pin(std::future::ready(
                enzui_iced_core::blocking_task::BlockingTaskSettlement::from_error(
                    u64::MAX,
                    enzui_iced_core::blocking_task::BlockingTaskError::Shutdown,
                ),
            ));
        };
        self.blocking_generation = generation;
        let localizer = self.localizer.clone();
        Box::pin(
            self.blocking_tasks
                .submit(generation, move |_| localizer.scope(work)),
        )
    }

    pub(crate) fn retain_persistence_retry(&mut self, debt: PersistenceDebt) {
        self.persistence_retries.retain(|pending| {
            !matches!(
                (pending, &debt),
                (
                    PersistenceDebt::LocalState(_),
                    PersistenceDebt::LocalState(_)
                ) | (
                    PersistenceDebt::SyncAdoption(_),
                    PersistenceDebt::SyncAdoption(_)
                ) | (
                    PersistenceDebt::Preferences(..),
                    PersistenceDebt::Preferences(..)
                ) | (PersistenceDebt::Shortcuts(_), PersistenceDebt::Shortcuts(_))
            )
        });
        self.persistence_retries.push_back(debt);
    }

    pub(crate) fn retain_attachment_retry(&mut self, retry: AttachmentRetry) {
        self.attachment_retries.retain(|pending| {
            !matches!(
                (pending, &retry),
                (AttachmentRetry::Stage { .. }, AttachmentRetry::Stage { .. })
                    | (
                        AttachmentRetry::Export { .. },
                        AttachmentRetry::Export { .. }
                    )
                    | (AttachmentRetry::Cleanup, AttachmentRetry::Cleanup)
                    | (AttachmentRetry::Reconcile, AttachmentRetry::Reconcile)
                    | (
                        AttachmentRetry::StartupSurvey,
                        AttachmentRetry::StartupSurvey
                    )
            )
        });
        self.attachment_retries.push_back(retry);
    }

    pub(crate) fn retry_blocking_work(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        let debts = std::mem::take(&mut self.persistence_retries);
        for debt in debts {
            match debt {
                PersistenceDebt::LocalState(token)
                    if token == self.local_state_token() && !self.local_state_is_published() =>
                {
                    tasks.push(self.save_local_state());
                }
                PersistenceDebt::SyncAdoption(generation)
                    if self
                        .pending_sync_adoption
                        .as_ref()
                        .is_some_and(|pending| pending.generation == generation) =>
                {
                    tasks.push(self.save_sync_adoption());
                }
                PersistenceDebt::Preferences(origin, token)
                    if token == self.preferences_token() && self.preferences_published != token =>
                {
                    tasks.push(self.save_preferences(origin));
                }
                PersistenceDebt::Shortcuts(revision)
                    if revision == self.shortcut_revision
                        && self.shortcut_published != revision =>
                {
                    tasks.push(self.save_shortcut_overrides());
                }
                _ => {}
            }
        }
        let attachment_retries = std::mem::take(&mut self.attachment_retries);
        for retry in attachment_retries {
            match retry {
                retry @ AttachmentRetry::Stage { .. } => {
                    tasks.push(self.stage_attachments_task(retry));
                }
                retry @ AttachmentRetry::Export { .. } => {
                    tasks.push(self.export_attachment_task(retry));
                }
                AttachmentRetry::Cleanup
                    if self.attachment_cleanup_inflight == 0
                        && self.content_sync_inflight.is_none() =>
                {
                    tasks.push(self.cleanup_retired_payloads());
                }
                AttachmentRetry::Reconcile
                    if self.attachment_cleanup_inflight == 0
                        && self.content_sync_inflight.is_none() =>
                {
                    tasks.push(self.reconcile_attachment_store_task());
                }
                AttachmentRetry::StartupSurvey
                    if self.attachment_cleanup_inflight == 0
                        && self.content_sync_inflight.is_none() =>
                {
                    let attachments = self.attachment_records();
                    tasks.push(self.start_attachment_startup_survey(attachments));
                }
                retry @ (AttachmentRetry::Cleanup
                | AttachmentRetry::Reconcile
                | AttachmentRetry::StartupSurvey) => self.retain_attachment_retry(retry),
            }
        }
        if self.mail_busy {
            self.mail_busy = false;
            if let Some(intent) = self.mail_retry.take() {
                tasks.push(self.start_credential_load(intent));
            }
        }
        if self.credential_mutations.journal().active().is_some() {
            tasks.push(self.continue_credential_mutation());
        }
        if self.attachment_open_busy {
            self.attachment_open_busy = false;
            if let Some(payload) = self.attachment_open_pending.clone() {
                tasks.push(self.open_attachment_task(self.attachment_open_generation, payload));
            }
        }
        Task::batch(tasks)
    }

    // A blocking settlement that produced no publication is a retryable
    // persistence failure, never a silent success.
    pub(crate) fn persist_settlement(
        settlement: enzui_iced_core::blocking_task::BlockingTaskSettlement<
            Result<(), enzui_iced_core::persist::PersistError>,
        >,
    ) -> Result<(), enzui_iced_core::persist::PersistError> {
        use enzui_iced_core::blocking_task::BlockingTaskError;

        match settlement.into_result() {
            Ok(result) => result,
            Err(BlockingTaskError::Busy) => Err(enzui_iced_core::persist::PersistError::new(
                "blocking persistence owner is busy",
            )),
            Err(BlockingTaskError::Cancelled) => Err(enzui_iced_core::persist::PersistError::new(
                "persistence work was cancelled",
            )),
            Err(BlockingTaskError::Shutdown) => Err(enzui_iced_core::persist::PersistError::new(
                "persistence worker is unavailable",
            )),
            Err(BlockingTaskError::Panicked) => Err(enzui_iced_core::persist::PersistError::new(
                "persistence worker panicked",
            )),
            Err(_) => Err(enzui_iced_core::persist::PersistError::new(
                "persistence worker returned an unknown terminal state",
            )),
        }
    }

    // A blocking settlement that ran no attachment maintenance leaves the work
    // owed, so the deferred outcome stays distinct from a store failure.
    pub(crate) fn attachment_worker_error(
        error: enzui_iced_core::blocking_task::BlockingTaskError,
    ) -> attachment_store::AttachmentFailure {
        use enzui_iced_core::blocking_task::BlockingTaskError;

        match error {
            BlockingTaskError::Busy => attachment_store::AttachmentFailure::MaintenanceDeferred,
            BlockingTaskError::Cancelled => attachment_store::AttachmentFailure::WorkerCancelled,
            BlockingTaskError::Shutdown => attachment_store::AttachmentFailure::WorkerShutdown,
            BlockingTaskError::Panicked => attachment_store::AttachmentFailure::WorkerPanicked,
            _ => attachment_store::AttachmentFailure::WorkerShutdown,
        }
    }

    pub(crate) fn attachment_settlement<T>(
        settlement: enzui_iced_core::blocking_task::BlockingTaskSettlement<
            Result<T, attachment_store::AttachmentFailure>,
        >,
    ) -> Result<T, attachment_store::AttachmentFailure> {
        match settlement.into_result() {
            Ok(result) => result,
            Err(error) => Err(Self::attachment_worker_error(error)),
        }
    }

    pub(crate) fn open_attachment_task(
        &mut self,
        generation: u64,
        attachment: model::types::Attachment,
    ) -> Task<Message> {
        let Some(completed_payload) = attachment.payload_ref() else {
            return Task::none();
        };
        let store = self.attachment_store.clone();
        Task::perform(
            self.submit_blocking(move || store.open(&attachment)),
            move |settlement| {
                Message::AttachmentPathResolved(
                    generation,
                    completed_payload,
                    Self::attachment_settlement(settlement).map(Arc::new),
                )
            },
        )
    }

    pub(crate) fn stage_attachments_task(&mut self, intent: AttachmentRetry) -> Task<Message> {
        let AttachmentRetry::Stage {
            generation,
            paths,
            existing_count,
            existing_bytes,
        } = &intent
        else {
            return Task::none();
        };
        let generation = *generation;
        let paths = paths.clone();
        let existing_count = *existing_count;
        let existing_bytes = *existing_bytes;
        let retry = intent.clone();
        let store = self.attachment_store.clone();
        Task::perform(
            self.submit_blocking(move || store.stage_paths(paths, existing_count, existing_bytes)),
            move |settlement| match settlement.into_result() {
                Ok(result) => Message::AttachmentsStaged(generation, result),
                Err(enzui_iced_core::blocking_task::BlockingTaskError::Busy) => {
                    Message::AttachmentBusy(retry)
                }
                Err(error) => Message::AttachmentsStaged(
                    generation,
                    Err(Self::attachment_worker_error(error)),
                ),
            },
        )
    }

    pub(crate) fn export_attachment_task(&mut self, intent: AttachmentRetry) -> Task<Message> {
        let AttachmentRetry::Export {
            attachment,
            destination,
        } = &intent
        else {
            return Task::none();
        };
        let attachment = attachment.clone();
        let Some(completed_payload) = attachment.payload_ref() else {
            return Task::none();
        };
        let destination = destination.clone();
        let retry = intent.clone();
        let store = self.attachment_store.clone();
        Task::perform(
            self.submit_blocking(move || store.export(&attachment, &destination)),
            move |settlement| match settlement.into_result() {
                Ok(result) => Message::AttachmentSaved(completed_payload, result),
                Err(enzui_iced_core::blocking_task::BlockingTaskError::Busy) => {
                    Message::AttachmentBusy(retry)
                }
                Err(error) => Message::AttachmentSaved(
                    completed_payload,
                    Err(Self::attachment_worker_error(error)),
                ),
            },
        )
    }

    pub(crate) fn save_local_state(&mut self) -> Task<Message> {
        // Nothing publishes before this frame's own read settles, so a loading
        // or unreadable session cannot overwrite the file with defaults.
        if !self.startup.local_state.publishable() {
            return Task::none();
        }
        let token = self.local_state_token();
        let Some(plan) = self.local_state_gate.reserve_save_if_changed(token.clone()) else {
            return Task::none();
        };
        self.local_state_error = None;
        let accounts = self.store.accounts.clone();
        let messages = self.store.messages.clone();
        let today = self.store.today;
        let pending_send = self.pending_send.clone();
        let next_message_ordinal = self.next_ordinal;
        let next_account_ordinal = self.next_account_ordinal;
        let active_account_id = self.active_account_id.clone();
        let send_generation = self.send_generation;
        let pending_server_mutations = self.pending_server_mutations.clone();
        let next_mutation_ordinal = self.next_server_mutation_ordinal;
        let credential_journal = self.credential_mutations.journal().clone();
        let completed_token = token;
        Task::perform(
            self.submit_blocking(move || {
                plan.prepare_and_execute(move || {
                    local_state::LocalState::from_owned(
                        accounts,
                        messages,
                        today,
                        pending_send,
                        next_message_ordinal,
                        next_account_ordinal,
                        active_account_id,
                        send_generation,
                        pending_server_mutations,
                        next_mutation_ordinal,
                        credential_journal,
                    )
                })
            }),
            move |settlement| {
                if settlement.result().err()
                    == Some(enzui_iced_core::blocking_task::BlockingTaskError::Busy)
                {
                    Message::PersistenceBusy(PersistenceDebt::LocalState(completed_token))
                } else {
                    Message::LocalStateSaved(completed_token, Self::persist_settlement(settlement))
                }
            },
        )
    }

    pub(crate) fn cleanup_retired_payloads(&mut self) -> Task<Message> {
        let referenced = self.attachment_payloads();
        self.pending_payload_cleanup
            .retain(|payload| referenced.get(&payload.id) != Some(&payload.identity));
        if self.pending_payload_cleanup.is_empty() {
            return Task::none();
        }
        let payloads = std::mem::take(&mut self.pending_payload_cleanup);
        self.remove_payloads_task(payloads)
    }
}
