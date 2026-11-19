use super::*;

impl State {
    fn close_permanent_error(&self) -> Option<String> {
        if self.blocking_generation_exhausted {
            return Some(crate::l10n::tr(
                "Background-work identity was exhausted; restart is required.",
            ));
        }
        if !self.startup.local_state.publishable()
            && let Some(summary) = self.local_state_load_error.as_deref()
        {
            return Some(crate::l10n::message(
                "mail-notice-local-mail-state-summary",
                &[("summary", (summary).to_owned())],
            ));
        }
        if !self.startup.preferences.publishable()
            && let Some(summary) = self.preferences_load_error.as_deref()
        {
            return Some(crate::l10n::message(
                "mail-notice-preferences-summary",
                &[("summary", (summary).to_owned())],
            ));
        }
        if !self.startup.shortcuts.publishable()
            && let Some(summary) = self.shortcut_load_error.as_deref()
        {
            return Some(crate::l10n::message(
                "mail-notice-keyboard-shortcuts-summary",
                &[("summary", (summary).to_owned())],
            ));
        }
        if let Some(failure) = self.local_timestamp_failure {
            return Some(format!(
                "{}. {}",
                Self::timestamp_failure_title(failure.retry),
                failure.error
            ));
        }
        if !self.local_state_is_published()
            && let Some(error) = &self.local_state_error
            && !Self::persistence_error_is_transient(error.kind())
        {
            return Some(crate::l10n::message(
                "mail-notice-local-mail-state",
                &[("value1", (error.user_summary()))],
            ));
        }
        if self.preferences_published != self.preferences_token()
            && let Some(error) = &self.preferences_error
            && !Self::persistence_error_is_transient(error.kind())
        {
            return Some(crate::l10n::message(
                "mail-notice-preferences",
                &[("value1", (error.kind()).to_string())],
            ));
        }
        if self.shortcut_published != self.shortcut_revision
            && let Some(kind) = self.shortcut_error_kind
            && !Self::persistence_error_is_transient(kind)
        {
            return Some(crate::l10n::message(
                "mail-notice-keyboard-shortcuts-kind",
                &[("kind", (kind).to_string())],
            ));
        }
        if let Some(error) = self.attachment_cleanup_error
            && !Self::attachment_error_is_transient(error)
        {
            return Some(crate::l10n::tr(error.user_message()));
        }
        None
    }

    fn close_failure_summary(&self) -> String {
        if let Some(error) = self.close_permanent_error() {
            return error;
        }
        if let Some(error) = &self.local_state_error
            && !self.local_state_is_published()
        {
            return crate::l10n::message(
                "mail-notice-local-mail-state-was-not-saved",
                &[("value1", (error.user_summary()))],
            );
        }
        if let Some(error) = &self.preferences_error
            && self.preferences_published != self.preferences_token()
        {
            return crate::l10n::message(
                "mail-notice-preferences-were-not-saved",
                &[("value1", (error.kind()).to_string())],
            );
        }
        if let Some(kind) = self.shortcut_error_kind
            && self.shortcut_published != self.shortcut_revision
        {
            return crate::l10n::message(
                "mail-notice-keyboard-shortcuts-were-not-saved-kind",
                &[("kind", (kind).to_string())],
            );
        }
        if let Some(error) = self.attachment_cleanup_error {
            return crate::l10n::tr(error.user_message());
        }
        if self.credential_mutations.journal().active().is_some() {
            return "Credential reconciliation remains durably journaled and was not confirmed before the close deadline."
                .to_owned();
        }
        if self.pending_sync_adoption.is_some() {
            return "The synchronized mailbox snapshot was not saved before the close deadline."
                .to_owned();
        }
        if self.mailbox_sync_retry_pending() {
            return "Mailbox synchronization remains resumable; retry it or keep the window open."
                .to_owned();
        }
        if !self.credential_store_ready
            || self.credential_inflight.is_some()
            || self.mail_cancel.is_some()
        {
            return crate::l10n::tr(
                "A mail service operation did not stop before the close deadline.",
            );
        }
        let pool_live = self.blocking_pool_live();
        if pool_live > 0 {
            return crate::l10n::count("mail-close-pending-operations", pool_live);
        }
        crate::l10n::tr("Saving did not finish before the close deadline.")
    }

    fn schedule_close_saves(&mut self) -> Task<Message> {
        let local = if self.pending_sync_adoption.is_none()
            && !self.local_state_is_published()
            && self
                .local_state_error
                .as_ref()
                .is_none_or(|error| Self::persistence_error_is_transient(error.kind()))
        {
            self.save_local_state()
        } else {
            Task::none()
        };
        let preferences = if self.preferences_published != self.preferences_token()
            && self
                .preferences_error
                .as_ref()
                .is_none_or(|error| Self::persistence_error_is_transient(error.kind()))
        {
            self.save_preferences(preferences::SaveOrigin::CloseSettlement)
        } else {
            Task::none()
        };
        let shortcuts = if self.shortcut_published != self.shortcut_revision
            && !self
                .shortcut_saves_inflight
                .contains(&self.shortcut_revision)
            && self
                .shortcut_error_kind
                .is_none_or(Self::persistence_error_is_transient)
        {
            self.save_shortcut_overrides()
        } else {
            Task::none()
        };
        let sync_adoption = if self.pending_sync_adoption.is_some() {
            self.save_sync_adoption()
        } else {
            Task::none()
        };
        let credentials = self.continue_credential_mutation();
        let server_mutations = self.start_next_mail_operation();
        let attachments = if self.local_state_is_published()
            && self.attachment_cleanup_inflight == 0
            && self.content_sync_inflight.is_none()
            && self
                .attachment_cleanup_error
                .is_none_or(Self::attachment_error_is_transient)
        {
            if self.attachment_reconcile_needed || self.attachment_store.has_cleanup_debt() {
                self.reconcile_attachment_store_task()
            } else if !self.pending_payload_cleanup.is_empty() {
                self.cleanup_retired_payloads()
            } else {
                Task::none()
            }
        } else {
            Task::none()
        };
        Task::batch([
            local,
            preferences,
            shortcuts,
            sync_adoption,
            credentials,
            server_mutations,
            attachments,
        ])
    }

    fn close_action_task(
        &mut self,
        action: enzui_iced_core::close_guard::CloseGuardAction<CloseSaveKey>,
    ) -> Task<Message> {
        match action {
            enzui_iced_core::close_guard::CloseGuardAction::Close(window) => {
                self.close_committed = true;
                #[cfg(feature = "center")]
                self.shutdown_center();
                iced::window::close(window)
            }
            enzui_iced_core::close_guard::CloseGuardAction::Hold
            | enzui_iced_core::close_guard::CloseGuardAction::Save { .. } => Task::none(),
        }
    }

    fn start_close_attempt(&mut self, now: Instant) -> Task<Message> {
        let action = self
            .close_guard
            .decide(enzui_iced_core::close_guard::CloseDecision::SaveAll);
        let attempt = match action {
            enzui_iced_core::close_guard::CloseGuardAction::Save { attempt, .. } => attempt,
            other => return self.close_action_task(other),
        };
        self.close_generation = self.close_generation.wrapping_add(1);
        self.stop_inbox_watches();
        self.close_settlement = Some(CloseSettlement {
            generation: self.close_generation,
            attempt,
            deadline: now + CLOSE_DRAIN_TIMEOUT,
            next_retry: now + CLOSE_RETRY_INTERVAL,
            phase: ClosePhase::Draining,
        });
        self.schedule_close_saves()
    }

    pub(crate) fn request_window_close(
        &mut self,
        window: iced::window::Id,
        now: Instant,
    ) -> Task<Message> {
        if self.close_guard.is_pending() {
            return Task::none();
        }
        self.invalidate_mailbox_syncs_for_close();
        self.attach_chooser.parent_closed(window);
        self.attachment_opener
            .cancel(enzui_iced_core::portal::CancelReason::Application);
        if let Some(generation) = self.attachment_open_generation.checked_add(1) {
            self.attachment_open_generation = generation;
        }
        self.attachment_open_pending = None;
        if let Some(opened) = self.attachment_open_held.take()
            && let Err(error) = opened.verify_path()
        {
            self.attachment_store
                .retain_cleanup_authority(&opened.payload_ref());
            self.attachment_reconcile_needed = true;
            self.attachment_notice = Some(crate::l10n::tr(error.user_message()));
        }
        self.attachment_open_busy = false;
        self.mail_queue.clear();
        self.mail_busy = false;
        let mut credential_cleanup = Task::none();
        if matches!(self.mail_retry, Some(MailIntent::ProbeAccount(_)))
            && let Some(generation) = self.advance_mail_generation()
            && let Some(MailIntent::ProbeAccount(config)) = self.mail_retry.take()
        {
            credential_cleanup = self.delete_account_credential(generation, &config);
        }
        if !matches!(
            self.mail_intent,
            Some(MailIntent::ReconcileServerMutation { .. })
        ) {
            if let Some((generation, cancel, _)) = &self.credential_inflight {
                cancel.cancel();
                if matches!(
                    self.mail_intent,
                    Some(MailIntent::ProbeAccount(_) | MailIntent::SyncMailbox { .. })
                ) {
                    self.mail_discard_generation = Some(*generation);
                }
            }
            if let Some((generation, cancel)) = &self.mail_cancel {
                cancel.cancel();
                if matches!(
                    self.mail_intent,
                    Some(MailIntent::ProbeAccount(_) | MailIntent::SyncMailbox { .. })
                ) {
                    self.mail_discard_generation = Some(*generation);
                }
            }
        }
        if self.compose.open
            && self.compose.can_save_draft()
            && self.autosave_compose_draft()
            && let Some(id) = self.compose.editing_draft_id.clone()
        {
            self.draft_save_pending = true;
            self.queue_server_mutation(local_state::ServerMutationKind::DraftUpserted, vec![id]);
        }
        let dirty = (!self.durable_state_is_settled()).then(|| {
            enzui_iced_core::close_guard::UnsavedItem::new(
                CloseSaveKey::DurableState,
                "Mail data and settings",
            )
        });
        match self.close_guard.request(
            enzui_iced_core::close_guard::CloseRequest::for_window(window),
            dirty,
        ) {
            action @ enzui_iced_core::close_guard::CloseGuardAction::Close(_) => {
                Task::batch([credential_cleanup, self.close_action_task(action)])
            }
            enzui_iced_core::close_guard::CloseGuardAction::Hold => {
                Task::batch([credential_cleanup, self.start_close_attempt(now)])
            }
            enzui_iced_core::close_guard::CloseGuardAction::Save { .. } => Task::none(),
        }
    }

    fn fail_close_attempt(&mut self, message: String) -> Task<Message> {
        let Some(settlement) = self.close_settlement.as_mut() else {
            return Task::none();
        };
        settlement.phase = ClosePhase::Failed(message);
        let attempt = settlement.attempt.clone();
        let _ = self.close_guard.save_finished(
            &CloseSaveKey::DurableState,
            &attempt,
            enzui_iced_core::close_guard::SaveOutcome::Failed,
        );
        Task::none()
    }

    pub(crate) fn advance_close_settlement(&mut self, now: Instant) -> Task<Message> {
        let Some((attempt, deadline, next_retry, failed)) =
            self.close_settlement.as_ref().map(|settlement| {
                (
                    settlement.attempt.clone(),
                    settlement.deadline,
                    settlement.next_retry,
                    matches!(settlement.phase, ClosePhase::Failed(_)),
                )
            })
        else {
            return Task::none();
        };
        if failed {
            return Task::none();
        }
        if self.durable_state_is_settled() {
            match self.close_guard.save_finished(
                &CloseSaveKey::DurableState,
                &attempt,
                enzui_iced_core::close_guard::SaveOutcome::Saved,
            ) {
                Ok(action) => {
                    self.close_settlement = None;
                    return self.close_action_task(action);
                }
                Err(_) => return Task::none(),
            }
        }
        if let Some(error) = self.close_permanent_error() {
            return self.fail_close_attempt(error);
        }
        if now >= deadline {
            return self.fail_close_attempt(self.close_failure_summary());
        }
        if now >= next_retry {
            if let Some(settlement) = self.close_settlement.as_mut() {
                settlement.next_retry = now + CLOSE_RETRY_INTERVAL;
            }
            return self.schedule_close_saves();
        }
        Task::none()
    }

    pub(crate) fn close_is_committed(&self) -> bool {
        self.close_committed
    }

    pub(crate) fn close_notice(&self) -> Option<(String, bool)> {
        if self.close_committed {
            return Some(("Closing…".to_owned(), false));
        }
        self.close_settlement
            .as_ref()
            .map(|settlement| match &settlement.phase {
                ClosePhase::Draining => (
                    crate::l10n::tr("Settling mail operations and saving data before closing…"),
                    false,
                ),
                ClosePhase::Failed(error) => (
                    crate::l10n::message(
                        "mail-notice-could-not-safely-close-error",
                        &[("error", (error).clone())],
                    ),
                    true,
                ),
            })
    }

    pub(crate) fn cancel_close_settlement(&mut self) {
        let _ = self
            .close_guard
            .decide(enzui_iced_core::close_guard::CloseDecision::Cancel);
        self.close_settlement = None;
        self.close_committed = false;
    }

    pub(crate) fn discard_close_settlement(&mut self) -> Task<Message> {
        if self.credential_mutations.journal().active().is_some()
            || self.credential_secret.is_some()
            || self.credential_inflight.is_some()
            || self.mail_cancel.is_some()
            || self.mailbox_sync_retry_pending()
            || self.pending_sync_adoption.is_some()
            || self.content_sync_inflight.is_some()
            || self.blocking_pool_live() > 0
            || self.account_edit_has_active_work()
            || self.account_removal.is_some()
        {
            return self.fail_close_attempt(
                "Mail or credential reconciliation cannot be discarded; retry or keep the window open."
                    .to_owned(),
            );
        }
        let action = self
            .close_guard
            .decide(enzui_iced_core::close_guard::CloseDecision::Discard);
        self.close_settlement = None;
        self.close_action_task(action)
    }

    pub(crate) fn retry_close_settlement(&mut self, now: Instant) -> Task<Message> {
        let Some(settlement) = self.close_settlement.as_ref() else {
            return Task::none();
        };
        if !matches!(settlement.phase, ClosePhase::Failed(_)) {
            return Task::none();
        }
        self.local_state_error = None;
        if self.local_timestamp_failure.is_some() {
            let _ = self.retry_local_timestamp();
        }
        self.preferences_error = None;
        self.shortcut_error_kind = None;
        self.shortcuts.finish_save(Ok(()));
        self.attachment_cleanup_error = None;

        if !self.startup.local_state.publishable() && self.local_state_load_error.is_some() {
            self.startup.local_state.status = FrameStatus::Loading;
            self.local_state_load_error = None;
        }
        if !self.startup.preferences.publishable() && self.preferences_load_error.is_some() {
            self.startup.preferences.status = FrameStatus::Loading;
            self.preferences_load_error = None;
        }
        if !self.startup.shortcuts.publishable() && self.shortcut_load_error.is_some() {
            self.startup.shortcuts.status = FrameStatus::Loading;
            self.shortcut_load_error = None;
        }
        let loads = self.begin_startup_loads();
        let close = self.start_close_attempt(now);
        Task::batch([loads, close])
    }

    pub(crate) fn close_message_allowed(&self, message: &Message) -> bool {
        if self.close_committed {
            return false;
        }
        if self.close_settlement.is_none() {
            return true;
        }
        matches!(
            message,
            Message::WindowCloseResolved(_)
                | Message::CloseSettlementTick(..)
                | Message::RetryCloseSettlement
                | Message::DiscardCloseSettlement
                | Message::CancelCloseSettlement
                | Message::LocalStateSaved(..)
                | Message::SyncAdoptionSaved(..)
                | Message::PreferencesSaved(..)
                | Message::ShortcutsSaved(..)
                | Message::PersistenceBusy(..)
                | Message::BlockingRetryTick
                | Message::AttachmentsStaged(..)
                | Message::InboundContentStaged(..)
                | Message::AttachmentSaved(..)
                | Message::AttachmentPathResolved(..)
                | Message::AttachmentBusy(..)
                | Message::AttachmentCleanupFinished(..)
                | Message::AttachmentReconcileFinished(..)
                | Message::AttachmentStartupSurveyFinished(..)
                | Message::CredentialStoreReady(..)
                | Message::CredentialFinished(..)
                | Message::MailOperationFinished(..)
                | Message::OutgoingMimePrepared(..)
                // A close during startup still needs the real document, so frame reads must
                // finish; blocking them would make close wait forever.
                | Message::FrameLoadSettled(..)
                | Message::FrameLoadFailed(..)
                | Message::FrameCustodySettled(..)
                // Closing a corner notice touches only view state.
                | Message::HideNotice(..)
        )
    }

    // Whether one message may run while a persisted frame is still pending.
    //
    // `install_settled_frames` replaces the mailbox, preferences and shortcuts
    // wholesale, so anything mutating persisted state is refused first. An
    // allowlist, so a new message is refused until classified.
    pub(crate) fn startup_message_allowed(&self, message: &Message) -> bool {
        if !self.startup.pending() {
            return true;
        }
        matches!(
            message,
            Message::Theme(..)
                | Message::WindowResized(..)
                | Message::Window(..)
                | Message::WindowCloseResolved(_)
                | Message::CloseSettlementTick(..)
                | Message::RetryCloseSettlement
                | Message::DiscardCloseSettlement
                | Message::CancelCloseSettlement
                                | Message::FrameLoadSettled(..)
                | Message::FrameLoadFailed(..)
                | Message::FrameCustodySettled(..)
                // Settlements of work started outside the loading window; dropping one would
                // leak its custody or leave its progress entry unresolved.
                | Message::LocalStateSaved(..)
                | Message::SyncAdoptionSaved(..)
                | Message::PreferencesSaved(..)
                | Message::ShortcutsSaved(..)
                | Message::PersistenceBusy(..)
                | Message::BlockingRetryTick
                | Message::AttachmentsStaged(..)
                | Message::InboundContentStaged(..)
                | Message::AttachmentSaved(..)
                | Message::AttachmentBusy(..)
                | Message::AttachmentCleanupFinished(..)
                | Message::AttachmentReconcileFinished(..)
                | Message::AttachmentStartupSurveyFinished(..)
                | Message::AttachmentOpenPortalTick(..)
                | Message::AttachmentOpenFinished(..)
                | Message::CredentialStoreReady(..)
                | Message::CredentialFinished(..)
                | Message::MailOperationFinished(..)
                | Message::OutgoingMimePrepared(..)
                | Message::SettingsExitSettled(..)
                | Message::ClipboardWritten
                // Navigation, focus and search touch only transient view state.
                | Message::SelectSidebar(..)
                | Message::SelectConversation { .. }
                | Message::SelectNextConversation
                | Message::SelectPreviousConversation
                | Message::NarrowBack
                | Message::SidebarState(..)
                | Message::ToggleSidebarSection(..)
                | Message::SelectAllConversations
                | Message::ClearSelection
                | Message::ClearMessageSelection
                | Message::ToggleMailboxOverflow
                | Message::CloseMailboxOverflow
                | Message::SetMessagePlainText(..)
                | Message::ToggleQuotedText(..)
                | Message::ToggleReaderOverflow(_)
                | Message::CloseReaderOverflow(_)
                | Message::FocusNext
                | Message::FocusPrevious
                | Message::FocusSearchField
                | Message::SearchEdited(..)
                | Message::SearchCleared
                | Message::SelectSearchScope(..)
                | Message::SearchActivated
                | Message::ToggleTitlebarOverflow
                | Message::CloseTitlebarOverflow
                | Message::SearchDismissed
                | Message::SearchNavigate(..)
                | Message::SearchCommitHighlighted
                | Message::SelectSearchResult(..)
                                | Message::RequestOpenUrl(..)
                | Message::ConfirmOpenUrl(..)
                | Message::OpenUrlFinished(..)
                | Message::OpenUrlPortalTick(..)
                | Message::DismissOpenUrlNotice
                | Message::DismissMailNotice
                | Message::HideNotice(..)
                | Message::DismissAttachmentNotice
                // Key and shortcut decoding re-enters this gate with the
                // resolved message, so a mutating chord is refused there.
                | Message::Shortcut(..)
                | Message::KeyPressed { .. }
        )
    }

    // States that a refused action was not applied, and will work once loading
    // ends.
    pub(crate) fn notify_startup_loading(&mut self) {
        self.mail_notice = Some(MailNotice {
            title: crate::l10n::tr("Still loading your saved mail"),
            body: crate::l10n::tr(
                "That action was not applied. Please try again once your saved mail, preferences and shortcuts have finished loading.",
            ),
            retryable: false,
            can_cancel_outbox: false,
        });
    }

    pub(crate) fn close_progress_time(&self, message: &Message) -> Option<Instant> {
        match message {
            Message::CloseSettlementTick(generation, now) => self
                .close_settlement
                .as_ref()
                .filter(|settlement| settlement.generation == *generation)
                .map(|_| *now),
            Message::LocalStateSaved(token, _) if token != &self.local_state_token() => None,
            Message::SyncAdoptionSaved(generation, token, _)
                if self.pending_sync_adoption.as_ref().is_none_or(|pending| {
                    pending.generation != *generation || &pending.candidate_token != token
                }) =>
            {
                None
            }
            Message::PreferencesSaved(_, token, _) if token != &self.preferences_token() => None,
            Message::ShortcutsSaved(revision, _) if *revision != self.shortcut_revision => None,
            _ => Some(Instant::now()),
        }
    }

    pub(crate) fn queue_server_mutation(
        &mut self,
        kind: local_state::ServerMutationKind,
        target_ids: Vec<String>,
    ) {
        if target_ids.is_empty() {
            return;
        }
        if matches!(kind, local_state::ServerMutationKind::DraftUpserted)
            && self.pending_server_mutations.iter().any(|pending| {
                matches!(
                    &pending.kind,
                    local_state::ServerMutationKind::DraftUpserted
                ) && pending.target_ids == target_ids
            })
        {
            return;
        }
        if self.pending_server_mutations.len() >= local_state::LOCAL_STATE_MUTATIONS_MAX {
            return;
        }
        let id = self.next_server_mutation_ordinal;
        let Some(next) = id.checked_add(1) else {
            return;
        };
        self.next_server_mutation_ordinal = next;
        self.pending_server_mutations
            .push(local_state::PendingServerMutation {
                id,
                kind,
                target_ids,
                server_targets: Vec::new(),
            });
    }

    pub(crate) fn queue_server_mutations_for_messages(
        &mut self,
        kind: local_state::ServerMutationKind,
        target_ids: &[String],
    ) -> bool {
        if self.pending_server_mutations.iter().any(|mutation| {
            matches!(
                &mutation.kind,
                local_state::ServerMutationKind::Move(_)
                    | local_state::ServerMutationKind::EmptyMailbox(_)
            ) && (mutation
                .server_targets
                .iter()
                .any(|target| target_ids.contains(&target.message_id))
                || mutation
                    .target_ids
                    .iter()
                    .any(|pending| pending.starts_with("imap-") && target_ids.contains(pending)))
        }) {
            self.mail_notice = Some(MailNotice {
                title: crate::l10n::tr("Server change already pending"),
                body: "Wait for the recorded mutation to reconcile before changing the same message again."
                    .to_owned(),
                retryable: false,
                can_cancel_outbox: false,
            });
            return false;
        }
        let mut server_targets = Vec::new();
        for message in target_ids
            .iter()
            .filter_map(|id| self.store.message(id))
            .filter(|message| {
                self.store
                    .account(&message.account_id)
                    .and_then(|account| account.config.as_ref())
                    .is_some_and(|config| config.protocol == backend::MailProtocol::Imap)
            })
        {
            let Some(target) = Self::imap_server_target(message) else {
                self.mail_notice = Some(MailNotice {
                    title: crate::l10n::tr("Server action not started"),
                    body: "The selected message has no current IMAP mailbox and UID identity. Synchronize the mailbox before retrying."
                        .to_owned(),
                    retryable: false,
                    can_cancel_outbox: false,
                });
                return false;
            };
            server_targets.push(target);
        }
        if server_targets.is_empty() {
            self.queue_server_mutation(kind, target_ids.to_vec());
            return true;
        }
        self.queue_exact_server_targets(kind, server_targets)
    }

    fn queue_exact_server_targets(
        &mut self,
        kind: local_state::ServerMutationKind,
        server_targets: Vec<local_state::ServerMutationTarget>,
    ) -> bool {
        if self.pending_server_mutations.iter().any(|mutation| {
            matches!(
                &mutation.kind,
                local_state::ServerMutationKind::Move(_)
                    | local_state::ServerMutationKind::EmptyMailbox(_)
            ) && mutation.server_targets.iter().any(|pending| {
                server_targets
                    .iter()
                    .any(|target| target.message_id == pending.message_id)
            })
        }) {
            return false;
        }
        let mut groups: Vec<Vec<local_state::ServerMutationTarget>> = Vec::new();
        for target in server_targets {
            let group = groups.iter_mut().find(|group| {
                group.first().is_some_and(|first| {
                    first.account_id == target.account_id
                        && first.mailbox == target.mailbox
                        && first.uid_validity == target.uid_validity
                }) && group.len()
                    < local_state::LOCAL_STATE_TARGETS_PER_MUTATION_MAX
                        .min(backend::curl_backend::MAX_MESSAGES_PER_SYNC)
            });
            if let Some(group) = group {
                group.push(target);
            } else {
                groups.push(vec![target]);
            }
        }
        let Some(total) = self
            .pending_server_mutations
            .len()
            .checked_add(groups.len())
        else {
            return false;
        };
        let Some(group_count) = u64::try_from(groups.len()).ok() else {
            return false;
        };
        let Some(next) = self.next_server_mutation_ordinal.checked_add(group_count) else {
            return false;
        };
        if total > local_state::LOCAL_STATE_MUTATIONS_MAX {
            return false;
        }
        let mut id = self.next_server_mutation_ordinal;
        for targets in groups {
            self.pending_server_mutations
                .push(local_state::PendingServerMutation {
                    id,
                    kind: kind.clone(),
                    target_ids: targets
                        .iter()
                        .map(|target| target.message_id.clone())
                        .collect(),
                    server_targets: targets,
                });
            id = id.saturating_add(1);
        }
        self.next_server_mutation_ordinal = next;
        true
    }

    pub(crate) fn apply_rules_with_server_journal(&mut self) -> Option<usize> {
        let before = self.store.clone();
        let affected = model::rules::apply_rules(&mut self.store, &self.rules);
        let mut intents = Vec::new();
        let mut local_only = Vec::new();
        for original in &before.messages {
            let Some((updated_location, updated_unread, updated_flagged)) = self
                .store
                .message(&original.id)
                .map(|message| (message.location.clone(), message.unread, message.flagged))
            else {
                continue;
            };
            if original.location == updated_location
                && original.unread == updated_unread
                && original.flagged == updated_flagged
            {
                continue;
            }
            let configured_imap = before
                .account(&original.account_id)
                .and_then(|account| account.config.as_ref())
                .is_some_and(|config| config.protocol == backend::MailProtocol::Imap);
            if !configured_imap {
                local_only.push(original.id.clone());
                continue;
            }
            let Some(target) = Self::imap_server_target(original) else {
                self.store = before.clone();
                self.mail_notice = Some(MailNotice {
                    title: crate::l10n::tr("Rules not applied"),
                    body: "A matched server message has no exact IMAP identity. Synchronize its mailbox before applying rules."
                        .to_owned(),
                    retryable: false,
                    can_cancel_outbox: false,
                });
                return None;
            };
            if original.unread != updated_unread {
                intents.push((
                    local_state::ServerMutationKind::SetUnread(updated_unread),
                    target.clone(),
                ));
            }
            if original.flagged != updated_flagged {
                intents.push((
                    local_state::ServerMutationKind::SetFlagged(updated_flagged),
                    target.clone(),
                ));
            }
            if original.location != updated_location {
                let model::types::MessageLocation::Mailbox(destination) = &updated_location else {
                    self.store = before.clone();
                    self.mail_notice = Some(MailNotice {
                        title: crate::l10n::tr("Rules not applied"),
                        body: "Moving server mail to a local-only folder is not supported."
                            .to_owned(),
                        retryable: false,
                        can_cancel_outbox: false,
                    });
                    return None;
                };
                intents.push((local_state::ServerMutationKind::Move(*destination), target));
            }
        }
        let journal_len = self.pending_server_mutations.len();
        let next_ordinal = self.next_server_mutation_ordinal;
        for (kind, target) in intents {
            if !self.queue_exact_server_targets(kind, vec![target]) {
                self.pending_server_mutations.truncate(journal_len);
                self.next_server_mutation_ordinal = next_ordinal;
                self.store = before;
                self.mail_notice = Some(MailNotice {
                    title: crate::l10n::tr("Rules not applied"),
                    body: "The bounded server-mutation journal is busy. Reconcile pending changes before applying rules again."
                        .to_owned(),
                    retryable: true,
                    can_cancel_outbox: false,
                });
                return None;
            }
        }
        if !local_only.is_empty() {
            self.queue_server_mutation(local_state::ServerMutationKind::RulesApplied, local_only);
        }
        Some(affected)
    }

    pub(crate) fn preferences_token(&self) -> preferences::PreferencesToken {
        (
            self.rules.revision(),
            self.privacy.revision(),
            self.sidebar_layout.revision(),
            self.compose_format.revision(),
            self.prefer_plain_text.revision(),
            self.dark_html.revision(),
            self.notifications.revision(),
        )
    }

    // Write the rule set and privacy decisions off the UI thread when the
    // gate token changed. A failure lands in `preferences_error`, and
    // `origin` returns on `PreferencesSaved` so it is reported where the
    // decision was made.
    pub(crate) fn save_preferences(&mut self, origin: preferences::SaveOrigin) -> Task<Message> {
        if !self.startup.preferences.publishable() {
            return Task::none();
        }
        let completed_token = self.preferences_token();
        let Some(plan) = self
            .preferences_gate
            .reserve_save_if_changed(completed_token)
        else {
            return Task::none();
        };
        self.preferences_error = None;
        let rules = self.rules.get().clone();
        let policy = self.privacy.policy;
        let allow_list = self.privacy.allow_list.clone();
        let block_list = self.privacy.block_list.clone();
        let sidebar_layout = self.sidebar_layout.get().clone();
        let compose_format = *self.compose_format.get();
        let prefer_plain_text = *self.prefer_plain_text.get();
        let dark_html = *self.dark_html.get();
        let notifications = self.notifications.get().clone();
        Task::perform(
            self.submit_blocking(move || {
                plan.prepare_and_execute(move || preferences::Preferences {
                    rules,
                    policy,
                    allow_list,
                    block_list,
                    sidebar_layout,
                    compose_format,
                    prefer_plain_text,
                    dark_html,
                    notifications,
                })
            }),
            move |settlement| {
                if settlement.result().err()
                    == Some(enzui_iced_core::blocking_task::BlockingTaskError::Busy)
                {
                    Message::PersistenceBusy(PersistenceDebt::Preferences(origin, completed_token))
                } else {
                    Message::PreferencesSaved(
                        origin,
                        completed_token,
                        Self::persist_settlement(settlement),
                    )
                }
            },
        )
    }

    // Write the override map off the UI thread, reporting the outcome back
    // into the catalog so a failure is visible rather than swallowed.
    pub(crate) fn save_shortcut_change(&mut self) -> Task<Message> {
        self.shortcut_revision = self.shortcut_revision.wrapping_add(1);
        self.save_shortcut_overrides()
    }

    pub(crate) fn save_shortcut_overrides(&mut self) -> Task<Message> {
        if !self.startup.shortcuts.publishable() {
            return Task::none();
        }
        let revision = self.shortcut_revision;
        if self.shortcut_saves_inflight.contains(&revision) {
            return Task::none();
        }
        // Reserve the save sequence on the reducer so publication order
        // matches submission order across concurrent workers; a Busy or shut
        // owner drops the plan unrun and retires the generation.
        let Some(plan) = self.shortcut_gate.reserve_save_if_changed(revision) else {
            return Task::none();
        };
        let overrides = self.shortcuts.overrides().clone();
        self.shortcut_saves_inflight.insert(revision);
        Task::perform(
            self.submit_blocking(move || plan.prepare_and_execute(move || overrides)),
            move |settlement| {
                if settlement.result().err()
                    == Some(enzui_iced_core::blocking_task::BlockingTaskError::Busy)
                {
                    Message::PersistenceBusy(PersistenceDebt::Shortcuts(revision))
                } else {
                    Message::ShortcutsSaved(revision, Self::persist_settlement(settlement))
                }
            },
        )
    }
}
