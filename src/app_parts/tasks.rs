use super::classify::{
    classify_closed_search_event, classify_draft_event, classify_open_search_event,
};
use super::*;

impl State {
    // Starts the browser sign-in for the account being added. The wait and token
    // exchange run on a blocking worker until the browser returns, the person
    // cancels, or five minutes pass.
    pub(crate) fn start_oauth_sign_in(&mut self) -> Task<Message> {
        use backend::curl_backend::oauth;
        self.end_oauth_sign_in();
        let add = &mut self.settings.add_account;
        let Some(provider) = add.oauth_provider() else {
            add.oauth = OAuthSignInStatus::Failed(
                "Mail has no browser sign-in for this server; use a password.".into(),
            );
            return Task::none();
        };
        let start = match oauth::OAuthClient::configured(provider)
            .and_then(|client| oauth::begin_sign_in(client, add.draft.email.trim()))
        {
            Ok(start) => start,
            Err(message) => {
                add.oauth = OAuthSignInStatus::Failed(message);
                return Task::none();
            }
        };
        if let Err(outcome) = self.link_opener.start(&start.authorization_url) {
            self.settings.add_account.oauth = OAuthSignInStatus::Failed(
                link_launcher::failure_text(&outcome)
                    .unwrap_or_else(|| "The browser could not be opened.".into()),
            );
            return Task::none();
        }
        let Some(generation) = self.oauth_generation.checked_add(1) else {
            return Task::none();
        };
        self.oauth_generation = generation;
        let binding = model::settings::OAuthBinding::of(&self.settings.add_account);
        self.oauth_sign_in = Some((generation, start.cancel, binding));
        self.settings.add_account.oauth = OAuthSignInStatus::Waiting {
            provider: provider.label(),
        };
        let sign_in = start.sign_in;
        Task::perform(
            self.submit_blocking(move || sign_in.finish(oauth::SIGN_IN_WAIT)),
            move |settlement| {
                Message::AddAccountOAuthFinished(generation, OAuthHandoff::new(settlement))
            },
        )
    }

    // Look up the typed address's servers off the UI thread. Without a
    // network to ask, the servers stay the user's to enter.
    pub(crate) fn look_up_servers(&mut self) -> Task<Message> {
        let Some(net) = self.discovery_net.clone() else {
            return Task::none();
        };
        let Some((generation, email)) = self.settings.add_account.begin_lookup() else {
            return Task::none();
        };
        Task::perform(
            self.submit_blocking(move || crate::backend::discovery::discover(&email, net.as_ref())),
            move |settlement| {
                let found = settlement.into_result().unwrap_or_default();
                Message::AddAccountServersFound(generation, found)
            },
        )
    }

    // Ends any sign-in in flight and drops a grant that was never committed:
    // whenever the address, server or authentication it was for may change,
    // or the flow closes.
    pub(crate) fn end_oauth_sign_in(&mut self) {
        if let Some((_, cancel, _)) = self.oauth_sign_in.take() {
            cancel.cancel();
        }
        self.oauth_grant = None;
        self.settings.add_account.oauth = OAuthSignInStatus::NotStarted;
    }

    pub(crate) fn remove_payloads_task(
        &mut self,
        payloads: Vec<model::types::AttachmentPayloadRef>,
    ) -> Task<Message> {
        self.attachment_cleanup_inflight = self.attachment_cleanup_inflight.saturating_add(1);
        self.attachment_cleanup_error = None;
        let store = self.attachment_store.clone();
        let submitted = payloads.clone();
        Task::perform(
            self.submit_blocking(move || store.remove_payloads(&payloads)),
            move |settlement| {
                Message::AttachmentCleanupFinished(
                    submitted,
                    Self::attachment_settlement(settlement),
                )
            },
        )
    }

    pub(crate) fn reconcile_attachment_store_task(&mut self) -> Task<Message> {
        self.attachment_cleanup_inflight = self.attachment_cleanup_inflight.saturating_add(1);
        self.attachment_cleanup_error = None;
        let store = self.attachment_store.clone();
        let referenced = self.attachment_payloads();
        Task::perform(
            self.submit_blocking(move || store.cleanup_unreferenced(&referenced)),
            |settlement| {
                Message::AttachmentReconcileFinished(Self::attachment_settlement(settlement))
            },
        )
    }

    // Number of blocking submissions still charged against the shared pool.
    //
    // [`enzui_iced_core::blocking_task::BlockingTaskCancellation`] settles a
    // future as `Cancelled` before the synchronous call exits, so a closure can
    // still be inside libcurl after `mail_cancel` cleared; close settlement reads
    // this real occupancy.
    pub(crate) fn blocking_pool_live(&self) -> usize {
        self.blocking_tasks.dispatcher().snapshot().live()
    }

    pub(crate) fn durable_state_is_settled(&self) -> bool {
        !self.startup.pending()
            && self.startup.publishable()
            && self.local_state_is_published()
            && self.local_state_error.is_none()
            && self.local_timestamp_failure.is_none()
            && self.preferences_published == self.preferences_token()
            && self.preferences_error.is_none()
            && self.shortcut_published == self.shortcut_revision
            && self.shortcut_error_kind.is_none()
            && self.pending_payload_cleanup.is_empty()
            && !self.attachment_store.has_cleanup_debt()
            && self.attachment_stage_inflight == 0
            && self.attachment_export_pending.is_none()
            && self.attachment_open_held.is_none()
            && self.attachment_cleanup_inflight == 0
            && self.attachment_cleanup_error.is_none()
            && !self.attachment_reconcile_needed
            && self.credential_store_ready
            && self.credential_inflight.is_none()
            && self.credential_mutations.journal().active().is_none()
            && self.credential_recovery.is_none()
            && self.mail_cancel.is_none()
            && !self.mailbox_sync_retry_pending()
            && self.pending_sync_adoption.is_none()
            && self.content_sync_inflight.is_none()
            && !self.blocking_generation_exhausted
            && self.blocking_pool_live() == 0
    }

    pub(crate) fn persistence_error_is_transient(
        kind: enzui_iced_core::persist::PersistErrorKind,
    ) -> bool {
        matches!(
            kind,
            enzui_iced_core::persist::PersistErrorKind::MissingDestination
                | enzui_iced_core::persist::PersistErrorKind::Unknown
        )
    }

    pub(crate) fn attachment_error_is_transient(
        error: attachment_store::AttachmentFailure,
    ) -> bool {
        matches!(
            error,
            attachment_store::AttachmentFailure::StoreUnavailable
                | attachment_store::AttachmentFailure::StoreUncertain
                | attachment_store::AttachmentFailure::MaintenanceDeferred
                | attachment_store::AttachmentFailure::MaintenanceIncomplete
                | attachment_store::AttachmentFailure::MissingPayload
                | attachment_store::AttachmentFailure::PayloadChanged
                | attachment_store::AttachmentFailure::Read(
                    enzui_iced_core::io_class::IoFailureCategory::NotFound
                        | enzui_iced_core::io_class::IoFailureCategory::Unknown
                )
                | attachment_store::AttachmentFailure::Write(
                    enzui_iced_core::io_class::IoFailureCategory::NotFound
                        | enzui_iced_core::io_class::IoFailureCategory::Unknown
                )
        )
    }

    // Route one attach-chooser terminal outcome; every path converges here.
    //
    // Success appends the chosen local files to the open compose, and a result
    // with no local file becomes a notice. User cancellation leaves compose
    // untouched with no notice. Every failure class shows
    // [`picker::failure_text`].
    pub(crate) fn finish_attach_pick(
        &mut self,
        target: picker::PickerTarget,
        outcome: enzui_iced_widgets::file_chooser::PortalOutcome<
            enzui_iced_widgets::file_chooser::FileResult,
        >,
    ) -> Task<Message> {
        use enzui_iced_widgets::file_chooser::PortalOutcome;
        match outcome {
            PortalOutcome::Completed(result) => match target {
                picker::PickerTarget::ComposeAttachment(generation)
                    if self.compose.open && generation == self.attachment_picker_generation =>
                {
                    let paths = result.local_paths();
                    if paths.is_empty() {
                        self.compose_attach_notice = Some(crate::l10n::tr(
                            "That location is not a local file — nothing to attach.",
                        ));
                        return Task::none();
                    }
                    let existing_count = self.compose.attachments.len();
                    let existing_bytes = self
                        .compose
                        .attachments
                        .iter()
                        .fold(0_u64, |total, attachment| {
                            total.saturating_add(attachment.size)
                        });
                    self.attachment_stage_inflight =
                        self.attachment_stage_inflight.saturating_add(1);
                    self.stage_attachments_task(AttachmentRetry::Stage {
                        generation,
                        paths,
                        existing_count,
                        existing_bytes,
                    })
                }
                picker::PickerTarget::ComposeAttachment(_) => Task::none(),
                picker::PickerTarget::SaveAttachment(attachment) => {
                    let Some(destination) = result.first_local_path() else {
                        self.attachment_notice = Some(
                            attachment_store::AttachmentFailure::InvalidDestination
                                .user_message()
                                .to_owned(),
                        );
                        return Task::none();
                    };
                    let Some(payload) = attachment.payload_ref() else {
                        self.attachment_notice = Some(
                            attachment_store::AttachmentFailure::MissingPayload
                                .user_message()
                                .to_owned(),
                        );
                        return Task::none();
                    };
                    self.attachment_export_pending = Some(payload);
                    self.export_attachment_task(AttachmentRetry::Export {
                        attachment,
                        destination,
                    })
                }
            },
            PortalOutcome::Cancelled(_) => Task::none(),
            other => {
                match target {
                    picker::PickerTarget::ComposeAttachment(_) => {
                        self.compose_attach_notice = picker::failure_text(&other);
                    }
                    picker::PickerTarget::SaveAttachment(_) => {
                        self.attachment_notice = picker::save_failure_text(&other);
                    }
                }
                Task::none()
            }
        }
    }

    pub(crate) fn reconcile_selection(&mut self) {
        use enzui_iced_core::drag::ItemId;
        use enzui_iced_core::list_selection::ClickModel;
        let convos = self.store.conversations_for(&self.selection);
        let membership: HashMap<&str, &str> = convos
            .iter()
            .flat_map(|conversation| {
                conversation
                    .message_ids
                    .iter()
                    .map(move |id| (id.as_str(), conversation.id.as_str()))
            })
            .collect();
        let selected = self
            .selected_conversation
            .as_deref()
            .and_then(|id| membership.get(id).copied())
            .or_else(|| convos.first().map(|conversation| conversation.id.as_str()))
            .map(str::to_owned);
        if selected != self.selected_conversation {
            if self.quick_reply_for.is_some()
                && self.quick_reply_for == self.selected_conversation
                && self
                    .selected_conversation
                    .as_deref()
                    .is_some_and(|id| membership.contains_key(id))
            {
                self.quick_reply_for.clone_from(&selected);
            }
            self.selected_conversation = selected;
        }
        let remap = |id: &ItemId| {
            membership
                .get(id.as_str())
                .map(|id| ItemId::new((*id).to_owned()))
        };
        let selected = self
            .row_selection
            .iter()
            .filter_map(remap)
            .collect::<Vec<_>>();
        let anchor = self.row_selection.anchor().and_then(remap);
        let active = self.row_selection.active().and_then(remap);
        self.row_selection.replace(selected, anchor, active);
        let model = ClickModel::new(
            convos
                .iter()
                .map(|conversation| ItemId::new(conversation.id.clone())),
        );
        let _ = self.row_selection.reconcile(&model);
    }

    pub(crate) fn mode(&self) -> LayoutMode {
        layout_mode(self.window_size.width)
    }

    pub(crate) fn theme_ref(&self) -> &EnzTheme {
        self.themes.theme()
    }

    pub(crate) fn theme(&self) -> Theme {
        self.themes.iced_theme()
    }

    pub(crate) fn subscription(&self) -> Subscription<Message> {
        let resize = if self.search_open {
            iced::event::listen_with(classify_open_search_event)
        } else {
            iced::event::listen_with(classify_closed_search_event)
        };
        let mut subs: Vec<Subscription<Message>> = vec![resize, self.desktop.subscription()];
        if self.compose.open || self.quick_reply_mode.is_some() {
            subs.push(iced::event::listen_with(classify_draft_event));
        }
        if self.html_reader_active() {
            subs.push(self.html_reader.subscription().map_or_else(
                || iced::time::every(self.html_reader.poll_interval()).map(|_| Message::HtmlTick),
                |subscription| subscription.map(Message::HtmlNativeReady),
            ));
        }
        if !self.startup.pending()
            && self.close_settlement.is_none()
            && !self.mailbox_sync_generation_exhausted
            && self.account_edit.is_none()
            && self.account_removal.is_none()
            && self.settings.edit_account.account_id.is_none()
            && self.selected_mailbox_sync().is_some()
        {
            let generation = self.mailbox_sync_generation;
            subs.push(
                iced::time::every(MAILBOX_SYNC_INTERVAL)
                    .with(generation)
                    .map(|(generation, now)| Message::PeriodicMailboxSync(generation, now)),
            );
        }
        if let Some(deadline) = self.mailbox_sync_retry_at
            && self.close_settlement.is_none()
        {
            let generation = self.mailbox_sync_generation;
            let wait = deadline
                .saturating_duration_since(Instant::now())
                .max(Duration::from_millis(50));
            subs.push(
                iced::time::every(wait)
                    .with(generation)
                    .map(|(generation, now)| Message::MailboxSyncRetryTick(generation, now)),
            );
        }
        if !self.persistence_retries.is_empty()
            || !self.attachment_retries.is_empty()
            || self.mail_busy
            || self.attachment_open_busy
        {
            subs.push(
                iced::time::every(BLOCKING_RETRY_INTERVAL).map(|_| Message::BlockingRetryTick),
            );
        }
        if let Some(settlement) = self.close_settlement.as_ref()
            && matches!(settlement.phase, ClosePhase::Draining)
        {
            let generation = settlement.generation;
            subs.push(
                iced::time::every(CLOSE_RETRY_INTERVAL)
                    .with(generation)
                    .map(|(generation, _)| {
                        Message::CloseSettlementTick(generation, Instant::now())
                    }),
            );
        }
        if let Some(wait) = self.next_snooze_wait() {
            subs.push(iced::time::every(wait).map(|_| Message::SnoozeWake));
        }
        for watch in self.inbox_watches.values() {
            subs.push(watch.subscription().map(Message::InboxWatch));
        }
        subs.push(
            self.new_mail
                .wake_subscription()
                .map(|_| Message::NewMailNotificationWake),
        );
        if let Some((deadline, wait)) = self.undo_send_expiry.armed() {
            subs.push(
                iced::time::every(wait)
                    .with(deadline)
                    .map(|(_deadline, now)| Message::Toast(ToastEvent::Tick(now))),
            );
        }
        let attach_wake = self.attach_chooser.completion_subscription();
        subs.push(attach_wake.map(|_| Message::AttachPortalTick));
        let link_open_wake = self.link_opener.completion_subscription();
        subs.push(link_open_wake.map(Message::OpenUrlPortalTick));
        let attachment_open_wake = self.attachment_opener.completion_subscription();
        subs.push(attachment_open_wake.map(Message::AttachmentOpenPortalTick));
        if self.compose.open {
            subs.push(iced::time::every(AUTOSAVE_INTERVAL).map(|_| Message::AutosaveDraftTick));
        }
        subs.push(
            self.themes
                .subscription()
                .map(|event| Message::Theme(Box::new(event))),
        );

        #[cfg(feature = "center")]
        subs.push(self.center_subscription());
        #[cfg(feature = "center")]
        subs.push(self.agent_subscription());
        Subscription::batch(subs)
    }
}
