//! Startup frame loads and credential tasks.

use super::*;

impl State {
    // Starts one bounded-owner read per deferred frame.
    //
    // A worker refusal is terminal for its frame: that frame stays
    // write-blocked and visibly unreadable rather than silently empty.
    pub(crate) fn begin_startup_loads(&mut self) -> Task<Message> {
        let local_state = if self.startup.local_state.status == FrameStatus::Loading {
            let store = self.local_state_gate.store().clone();
            let path = self.startup.local_state.path.clone();
            Task::perform(
                self.submit_blocking(move || {
                    let identity = path.as_deref().and_then(local_state::source_identity);
                    (identity, local_state::load_with_previews(&store))
                }),
                |settlement| match settlement.into_result() {
                    Ok((identity, outcome)) => {
                        Message::FrameLoadSettled(Arc::new(FrameLoadHandoff::new(
                            FrameCompletion::LocalState(identity, Box::new(outcome)),
                        )))
                    }
                    Err(error) => Message::FrameLoadFailed(FrameKind::LocalState, error),
                },
            )
        } else {
            Task::none()
        };

        let preferences = if self.startup.preferences.status == FrameStatus::Loading {
            let store = self.preferences_gate.store().clone();
            let path = self.startup.preferences.path.clone();
            Task::perform(
                self.submit_blocking(move || {
                    let identity = path.as_deref().and_then(local_state::source_identity);
                    (identity, store.load())
                }),
                |settlement| match settlement.into_result() {
                    Ok((identity, outcome)) => Message::FrameLoadSettled(Arc::new(
                        FrameLoadHandoff::new(FrameCompletion::Preferences(identity, outcome)),
                    )),
                    Err(error) => Message::FrameLoadFailed(FrameKind::Preferences, error),
                },
            )
        } else {
            Task::none()
        };

        let shortcuts = if self.startup.shortcuts.status == FrameStatus::Loading {
            let store = self.shortcut_store.clone();
            let path = self.startup.shortcuts.path.clone();
            Task::perform(
                self.submit_blocking(move || {
                    let identity = path.as_deref().and_then(local_state::source_identity);
                    (identity, store.load())
                }),
                |settlement| match settlement.into_result() {
                    Ok((identity, outcome)) => Message::FrameLoadSettled(Arc::new(
                        FrameLoadHandoff::new(FrameCompletion::Shortcuts(identity, outcome)),
                    )),
                    Err(error) => Message::FrameLoadFailed(FrameKind::Shortcuts, error),
                },
            )
        } else {
            Task::none()
        };

        Task::batch([local_state, preferences, shortcuts])
    }

    // Folds one settled frame and, on refusal, starts its custody.
    pub(crate) fn finish_frame_load(&mut self, completion: FrameCompletion) -> Task<Message> {
        let (kind, outcome) = match completion {
            FrameCompletion::LocalState(identity, outcome) => {
                self.startup.local_state.identity = identity;
                let outcome = self.startup.local_state.settle(*outcome);
                self.local_state_load_error.clone_from(&outcome.summary);
                (FrameKind::LocalState, outcome)
            }
            FrameCompletion::Preferences(identity, outcome) => {
                self.startup.preferences.identity = identity;
                let outcome = self.startup.preferences.settle(outcome);
                self.preferences_load_error.clone_from(&outcome.summary);
                (FrameKind::Preferences, outcome)
            }
            FrameCompletion::Shortcuts(identity, outcome) => {
                self.startup.shortcuts.identity = identity;
                let outcome = self.startup.shortcuts.settle(outcome);
                self.shortcut_load_error.clone_from(&outcome.summary);
                (FrameKind::Shortcuts, outcome)
            }
        };
        match outcome.custody {
            Some((path, identity)) => self.start_frame_custody(kind, path, identity),
            None => self.install_settled_frames(),
        }
    }

    // Retains the unreadable bytes of one refused frame off the reducer.
    fn start_frame_custody(
        &mut self,
        kind: FrameKind,
        path: std::path::PathBuf,
        identity: enzui_iced_core::quarantine::ExpectedSourceIdentity,
    ) -> Task<Message> {
        Task::perform(
            self.submit_blocking(move || local_state::retain_unreadable_state(&path, identity)),
            move |settlement| {
                Message::FrameCustodySettled(
                    kind,
                    settlement.into_result().map(|custody| {
                        matches!(custody, local_state::UnreadableStateCustody::Retained)
                    }),
                )
            },
        )
    }

    // Authorizes replacement only for a frame whose bytes are retained.
    pub(crate) fn finish_frame_custody(
        &mut self,
        kind: FrameKind,
        retained: Result<bool, enzui_iced_core::blocking_task::BlockingTaskError>,
    ) -> Task<Message> {
        let retained = match retained {
            Ok(retained) => retained,
            Err(error) => {
                let summary = match kind {
                    FrameKind::LocalState => self.startup.local_state.refuse_worker(error),
                    FrameKind::Preferences => self.startup.preferences.refuse_worker(error),
                    FrameKind::Shortcuts => self.startup.shortcuts.refuse_worker(error),
                };
                match kind {
                    FrameKind::LocalState => self.local_state_load_error = Some(summary),
                    FrameKind::Preferences => self.preferences_load_error = Some(summary),
                    FrameKind::Shortcuts => self.shortcut_load_error = Some(summary),
                }
                return self.install_settled_frames();
            }
        };
        let status = if retained {
            FrameStatus::Publishable
        } else {
            FrameStatus::WriteBlocked
        };
        match kind {
            FrameKind::LocalState => {
                if retained {
                    self.local_state_gate.store().authorize_replacement();
                    self.local_state_retained_for_recovery = true;
                }
                self.startup.local_state.status = status;
            }
            FrameKind::Preferences => {
                if retained {
                    self.preferences_gate.store().authorize_replacement();
                }
                self.startup.preferences.status = status;
            }
            FrameKind::Shortcuts => {
                if retained {
                    self.shortcut_store.authorize_replacement();
                }
                self.startup.shortcuts.status = status;
            }
        }
        self.install_settled_frames()
    }

    pub(crate) fn install_settled_frames(&mut self) -> Task<Message> {
        if self.startup.pending() {
            return Task::none();
        }
        if let Some(loaded) = self.startup.local_state.staged.take() {
            self.install_local_state(loaded);
        }
        if let Some(loaded) = self.startup.preferences.staged.take() {
            self.install_preferences(loaded);
        }
        if let Some(overrides) = self.startup.shortcuts.staged.take() {
            self.install_shortcut_overrides(overrides);
        }
        if !self.startup.local_state.read_succeeded {
            return Task::none();
        }
        let referenced = self.attachment_records();
        Task::batch([
            self.start_attachment_startup_survey(referenced),
            self.continue_credential_mutation(),
        ])
    }

    fn install_local_state(&mut self, loaded: local_state::LocalState) {
        let mut loaded = loaded;
        loaded.recover_ordinals();
        let local_state::LocalState {
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
        } = loaded;
        self.store = MailboxStore::new(accounts, messages, today);
        self.pending_send = pending_send;
        self.next_ordinal = next_message_ordinal;
        self.next_account_ordinal = next_account_ordinal;
        self.active_account_id = active_account_id;
        self.send_generation = send_generation;
        self.pending_server_mutations = pending_server_mutations;
        self.next_server_mutation_ordinal = next_mutation_ordinal;
        self.credential_mutations =
            credential_mutation::CredentialMutationOwner::new(credential_journal);
        self.credential_recovery = None;
        self.pending_undo_toast = if self.pending_send.is_some() {
            let id = self.undo_send_toasts.push(undo_send_toast_spec());
            Some(PendingUndoToast {
                id,
                generation: self.send_generation,
            })
        } else {
            None
        };
        let _ = self
            .undo_send_toasts
            .sync_expiry(&mut self.undo_send_expiry, std::time::Instant::now());
        self.reconcile_selection();
        // The installed document is what the file already holds, so the gate
        // must treat it as published rather than as a first change.
        let token = self.local_state_token();
        self.local_state_gate.rebaseline(token.clone());
        self.local_state_published = token;
    }

    fn install_preferences(&mut self, loaded: preferences::Preferences) {
        let privacy = loaded.privacy_state();
        let mut layout = loaded.sidebar_layout.clone();
        layout.prune(&self.store.accounts);
        self.rules_count_label = crate::settings_view::rules_count_label(loaded.rules.len());
        self.rules_ui.next_id = next_rule_id_seed(&loaded.rules);
        self.rules = enzui_iced_core::persist::Tracked::new(loaded.rules);
        self.privacy = enzui_iced_core::persist::Tracked::new(privacy);
        self.sidebar_layout = enzui_iced_core::persist::Tracked::new(layout);
        self.compose_format = enzui_iced_core::persist::Tracked::new(loaded.compose_format);
        self.prefer_plain_text = enzui_iced_core::persist::Tracked::new(loaded.prefer_plain_text);
        self.dark_html = enzui_iced_core::persist::Tracked::new(loaded.dark_html);
        self.notifications = enzui_iced_core::persist::Tracked::new(loaded.notifications);
        // Fresh `Tracked` values start at revision zero, which is the baseline
        // token the gate was constructed with.
        let token = self.preferences_token();
        self.preferences_gate.rebaseline(token);
        self.preferences_published = token;
    }

    fn install_shortcut_overrides(&mut self, overrides: enzui_iced_core::ShortcutOverrides) {
        self.shortcuts = shortcuts::catalog_with_overrides(overrides);
        self.shortcut_revision = 0;
        self.shortcut_published = 0;
        // Adopting the loaded overrides supersedes any pre-adoption save so
        // the new baseline's token 0 remains committed rather than skipped.
        self.shortcut_gate.rebaseline(0);
    }

    // Surveys attachment custody once at startup on the bounded owner.
    //
    // Removes unreferenced leftovers; until this settles the session owes a
    // reconcile pass, so close settlement cannot treat custody as proven. A
    // referenced payload that went missing is reported when it is opened.
    pub(crate) fn start_attachment_startup_survey(
        &mut self,
        attachments: Vec<model::types::Attachment>,
    ) -> Task<Message> {
        let referenced = attachments
            .iter()
            .filter_map(model::types::Attachment::payload_ref)
            .map(|payload| (payload.id, payload.identity))
            .collect::<std::collections::HashMap<_, _>>();
        if !self.attachment_store.needs_blocking_maintenance() {
            let cleanup = self.attachment_store.cleanup_unreferenced(&referenced);
            self.apply_attachment_startup_survey(cleanup);
            return Task::none();
        }
        self.attachment_cleanup_inflight = self.attachment_cleanup_inflight.saturating_add(1);
        self.attachment_reconcile_needed = true;
        let store = self.attachment_store.clone();
        Task::perform(
            self.submit_blocking(move || store.cleanup_unreferenced(&referenced)),
            |settlement| match settlement.into_result() {
                Ok(cleanup) => Message::AttachmentStartupSurveyFinished(cleanup),
                Err(error) => Message::AttachmentStartupSurveyFinished(Err(
                    Self::attachment_worker_error(error),
                )),
            },
        )
    }

    #[cfg(test)]
    pub(crate) fn with_link_opener(mut self, opener: link_launcher::LinkOpener) -> Self {
        self.link_opener = opener;
        self
    }

    pub(crate) fn recipient_field_mut(&mut self, field: RecipientField) -> &mut String {
        match field {
            RecipientField::To => &mut self.compose.to,
            RecipientField::Cc => &mut self.compose.cc,
            RecipientField::Bcc => &mut self.compose.bcc,
        }
    }

    pub(crate) fn compose_attachment_failure(&self) -> Option<attachment_store::AttachmentFailure> {
        if self.compose.attachments.len() > attachment_store::MAX_ATTACHMENT_COUNT {
            return Some(attachment_store::AttachmentFailure::TooMany);
        }
        let total = self
            .compose
            .attachments
            .iter()
            .fold(0_u64, |total, attachment| {
                total.saturating_add(attachment.size)
            });
        if total > attachment_store::MAX_TOTAL_ATTACHMENT_BYTES {
            return Some(attachment_store::AttachmentFailure::TotalTooLarge);
        }
        self.compose
            .attachments
            .iter()
            .find_map(|attachment| match attachment.payload_ref() {
                Some(payload)
                    if payload.id.is_valid() && payload.identity.size == attachment.size =>
                {
                    None
                }
                Some(_) => Some(attachment_store::AttachmentFailure::InvalidPayload),
                None => Some(attachment_store::AttachmentFailure::MissingPayload),
            })
    }

    pub(crate) fn attachment_payloads(
        &self,
    ) -> std::collections::HashMap<
        model::types::AttachmentPayloadId,
        model::types::AttachmentPayloadIdentity,
    > {
        self.store
            .messages
            .iter()
            .flat_map(|message| message.attachments.iter())
            .chain(self.compose.attachments.iter())
            .chain(
                self.pending_send
                    .iter()
                    .flat_map(|pending| pending.attachments.iter()),
            )
            .filter_map(model::types::Attachment::payload_ref)
            .map(|payload| (payload.id, payload.identity))
            .collect()
    }

    pub(crate) fn attachment_records(&self) -> Vec<model::types::Attachment> {
        self.store
            .messages
            .iter()
            .flat_map(|message| message.attachments.iter())
            .chain(self.compose.attachments.iter())
            .chain(
                self.pending_send
                    .iter()
                    .flat_map(|pending| pending.attachments.iter()),
            )
            .cloned()
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn pending_send_attachment_handoff(
        &self,
    ) -> Result<Vec<u8>, attachment_store::AttachmentFailure> {
        let pending = self
            .pending_send
            .as_ref()
            .ok_or(attachment_store::AttachmentFailure::NoPendingSend)?;
        self.attachment_store.outgoing_mime_bytes(pending)
    }

    pub(crate) fn pending_send_targets(&self) -> Option<Vec<String>> {
        self.pending_send.as_ref().map(|pending| {
            let mut targets = vec![pending.id.clone()];
            if let Some(draft_id) = &pending.restored_draft_id {
                targets.push(draft_id.clone());
            }
            targets
        })
    }

    pub(crate) fn is_fixture_mode(&self) -> bool {
        self.fixture_mode
    }

    pub(crate) fn credential_key(config: &backend::AccountConfig) -> CredentialKey {
        CredentialKey::new(
            "io.github.enzui.Mail",
            config.credential_owner.clone(),
            "password",
        )
    }

    // Reserves the next mail-operation generation, or returns `None` when the
    // counter is exhausted so the caller can refuse admission before any
    // credential or network work begins. Every returned value is nonzero.
    pub(crate) fn advance_mail_generation(&mut self) -> Option<u64> {
        let next = self.mail_generation.checked_add(1)?;
        self.mail_generation = next;
        Some(next)
    }

    pub(crate) fn refuse_exhausted_mail_generation(&mut self) -> Task<Message> {
        self.mail_notice = Some(MailNotice {
            title: crate::l10n::tr("Mail operation queue exhausted"),
            body: crate::l10n::tr(
                "The mail-operation counter has no room for another attempt. Restart the app to reissue mail work.",
            ),
            retryable: false,
            can_cancel_outbox: self.pending_send.is_some(),
        });
        Task::none()
    }

    pub(crate) fn credential_task(
        &mut self,
        generation: u64,
        stage: CredentialStage,
        operation: enzui_services::credential_store::PendingCredentialOp,
    ) -> Task<Message> {
        let cancel = operation.cancel_handle();
        self.credential_inflight = Some((generation, cancel, stage));
        Task::perform(operation, move |outcome| {
            Message::CredentialFinished(
                generation,
                stage,
                Arc::new(CredentialCompletion::new(outcome)),
            )
        })
    }

    pub(crate) fn credential_keys_in_public_state(
        &self,
    ) -> Vec<credential_mutation::DurableCredentialKey> {
        self.store
            .accounts
            .iter()
            .filter_map(|account| account.config.as_ref())
            .map(Self::credential_key)
            .map(|key| credential_mutation::DurableCredentialKey::from_key(&key))
            .collect()
    }

    fn current_account_credential_key(
        &self,
        account_id: &str,
    ) -> Option<credential_mutation::DurableCredentialKey> {
        self.store
            .account(account_id)
            .and_then(|account| account.config.as_ref())
            .map(Self::credential_key)
            .map(|key| credential_mutation::DurableCredentialKey::from_key(&key))
    }

    pub(crate) fn finish_credential_transition(&mut self, generation: u64) -> Task<Message> {
        let sync = self
            .credential_mutations
            .journal()
            .active()
            .filter(|record| {
                record.sequence == generation
                    && self.credential_mutations.journal().public_generation()
                        == record.target_public_generation
            })
            .and_then(|record| {
                (record.kind == credential_mutation::CredentialMutationKind::Add
                    || (record.kind == credential_mutation::CredentialMutationKind::Edit
                        && record.config_changed))
                    .then(|| {
                        self.store
                            .account(&record.account_id)
                            .and_then(|account| account.config.clone())
                    })
                    .flatten()
            });
        if !self.credential_mutations.finish(generation) {
            return Task::none();
        }
        self.credential_recovery = None;
        self.mail_notice = Some(MailNotice {
            title: crate::l10n::tr("Credential state reconciled"),
            body: "The account configuration and its exact credential keys are reconciled; the journal update is being saved."
                .to_owned(),
            retryable: false,
            can_cancel_outbox: false,
        });
        let save = self.save_local_state();
        let sync = sync.map_or_else(Task::none, |config| self.queue_inbox_sync(config));
        Task::batch([save, sync])
    }

    pub(crate) fn continue_credential_mutation(&mut self) -> Task<Message> {
        if self.startup.pending()
            || !self.startup.local_state.publishable()
            || !self.local_state_is_published()
            || !self.credential_store_ready
            || self.credential_inflight.is_some()
            || self.mail_cancel.is_some()
        {
            return Task::none();
        }

        if let Some(AccountRemoval {
            account,
            phase: AccountRemovalPhase::AwaitingJournal(generation),
            ..
        }) = self.account_removal.as_ref()
        {
            let generation = *generation;
            let account = account.clone();
            // The removal was confirmed for exactly this account; if it changed during the
            // journal save, the removal is abandoned and its journal retired.
            if self.store.account(&account.id) != Some(&account) {
                self.account_removal = None;
                self.mail_notice = Some(MailNotice {
                    title: crate::l10n::tr("Account removal stopped"),
                    body: "The account changed before its removal was saved, so it was kept."
                        .to_owned(),
                    retryable: false,
                    can_cancel_outbox: false,
                });
                return self.finish_credential_transition(generation);
            }
            if !self.credential_mutations.mark_public(generation) {
                return Task::none();
            }
            let task = self.remove_account_owned_state(account);
            if let Some(removal) = self.account_removal.as_mut() {
                removal.phase = AccountRemovalPhase::Publishing(generation);
            }
            self.mail_notice = Some(MailNotice {
                title: crate::l10n::tr("Configuration saved; credential cleanup pending"),
                body: "The account is removed from local state. Its exact keyring entry will be deleted after that state is durable."
                    .to_owned(),
                retryable: false,
                can_cancel_outbox: false,
            });
            return Task::batch([task, self.save_local_state()]);
        }
        if let Some(AccountRemoval {
            credential_key,
            phase: AccountRemovalPhase::Publishing(generation),
            ..
        }) = self.account_removal.as_ref()
        {
            let generation = *generation;
            let key = credential_key.clone();
            let public_matches =
                self.credential_mutations
                    .journal()
                    .active()
                    .is_some_and(|record| {
                        record.sequence == generation
                            && self.credential_mutations.journal().public_generation()
                                == record.target_public_generation
                    });
            if !public_matches {
                return Task::none();
            }
            if let Some(removal) = self.account_removal.as_mut() {
                removal.phase = AccountRemovalPhase::Deleting(generation);
            }
            let operation = self.credential_store.0.delete(key);
            return self.credential_task(generation, CredentialStage::DeleteAccount, operation);
        }
        if let Some(AccountEdit {
            old_key,
            phase: AccountEditPhase::AwaitingJournal(generation),
            ..
        }) = self.account_edit.as_ref()
        {
            let generation = *generation;
            let key = old_key.clone();
            if let Some(edit) = self.account_edit.as_mut() {
                edit.phase = AccountEditPhase::LoadingOld(generation);
            }
            let operation = self.credential_store.0.get(key);
            return self.credential_task(generation, CredentialStage::LoadOldForEdit, operation);
        }
        if let Some(secret) = self.credential_secret.take()
            && let Some(MailIntent::ProbeAccount(config)) = self.mail_intent.as_ref()
        {
            let Some(record) = self.credential_mutations.journal().active() else {
                return Task::none();
            };
            if record.kind != credential_mutation::CredentialMutationKind::Add {
                return Task::none();
            }
            let generation = record.sequence;
            let operation = self
                .credential_store
                .0
                .set(Self::credential_key(config), secret);
            return self.credential_task(generation, CredentialStage::StoreAccount, operation);
        }
        if self.account_edit.is_some()
            || self.account_removal.is_some()
            || self.mail_intent.is_some()
        {
            return Task::none();
        }
        let Some(record) = self.credential_mutations.journal().active().cloned() else {
            return Task::none();
        };
        let current = self.current_account_credential_key(&record.account_id);
        let referenced = self.credential_keys_in_public_state();
        match self
            .credential_mutations
            .recovery_plan(&record, current.as_ref(), &referenced)
        {
            credential_mutation::RecoveryPlan::Clear => {
                self.finish_credential_transition(record.sequence)
            }
            credential_mutation::RecoveryPlan::ConfirmCurrent { key, delete_after } => {
                let operation = self.credential_store.0.get(key.to_key());
                self.credential_recovery = Some(CredentialRecovery {
                    generation: record.sequence,
                    key,
                    delete_after,
                });
                self.mail_notice = Some(MailNotice {
                    title: crate::l10n::tr("Credential reconciliation pending"),
                    body: "The saved account configuration is being checked against its exact keyring entry."
                        .to_owned(),
                    retryable: false,
                    can_cancel_outbox: false,
                });
                self.credential_task(
                    record.sequence,
                    CredentialStage::RecoverConfirmCurrent,
                    operation,
                )
            }
            credential_mutation::RecoveryPlan::DeleteExact(key) => {
                let operation = self.credential_store.0.delete(key.to_key());
                self.credential_recovery = Some(CredentialRecovery {
                    generation: record.sequence,
                    key,
                    delete_after: None,
                });
                self.mail_notice = Some(MailNotice {
                    title: crate::l10n::tr("Configuration saved; credential cleanup pending"),
                    body: "Only the exact unreferenced key recorded by the durable transition journal is being removed."
                        .to_owned(),
                    retryable: false,
                    can_cancel_outbox: false,
                });
                self.credential_task(
                    record.sequence,
                    CredentialStage::RecoverDeleteExact,
                    operation,
                )
            }
        }
    }

    pub(crate) fn complete_credential_recovery(
        &mut self,
        generation: u64,
        stage: CredentialStage,
        outcome: CredentialOutcome,
    ) -> Task<Message> {
        let Some(recovery) = self.credential_recovery.as_ref() else {
            return Task::none();
        };
        if recovery.generation != generation {
            return Task::none();
        }
        match stage {
            CredentialStage::RecoverConfirmCurrent => match outcome {
                CredentialOutcome::Retrieved(_) => {
                    if let Some(key) = recovery.delete_after.clone() {
                        let operation = self.credential_store.0.delete(key.to_key());
                        if let Some(recovery) = self.credential_recovery.as_mut() {
                            recovery.key = key;
                            recovery.delete_after = None;
                        }
                        self.credential_task(
                            generation,
                            CredentialStage::RecoverDeleteExact,
                            operation,
                        )
                    } else {
                        self.finish_credential_transition(generation)
                    }
                }
                _ => {
                    self.mail_notice = Some(MailNotice {
                        title: crate::l10n::tr("Credential state uncertain"),
                        body: "The current account key could not be confirmed. The durable reconciliation record was retained for retry."
                            .to_owned(),
                        retryable: true,
                        can_cancel_outbox: false,
                    });
                    Task::none()
                }
            },
            CredentialStage::RecoverDeleteExact => {
                let operation = self.credential_store.0.get(recovery.key.to_key());
                self.credential_task(generation, CredentialStage::RecoverConfirmAbsent, operation)
            }
            CredentialStage::RecoverConfirmAbsent => match outcome {
                CredentialOutcome::NotFound => self.finish_credential_transition(generation),
                _ => {
                    self.mail_notice = Some(MailNotice {
                        title: crate::l10n::tr("Credential cleanup pending"),
                        body: "Deletion of the exact retired key was not confirmed. The durable reconciliation record was retained for retry."
                            .to_owned(),
                        retryable: true,
                        can_cancel_outbox: false,
                    });
                    Task::none()
                }
            },
            _ => Task::none(),
        }
    }

    pub(crate) fn start_credential_load(&mut self, intent: MailIntent) -> Task<Message> {
        if self.close_settlement.is_some()
            && !matches!(&intent, MailIntent::ReconcileServerMutation { .. })
        {
            return Task::none();
        }
        let belongs_to_active_add = self
            .credential_mutations
            .journal()
            .active()
            .is_some_and(|record| {
                record.kind == credential_mutation::CredentialMutationKind::Add
                    && matches!(&intent, MailIntent::ProbeAccount(config) if config.id == record.account_id)
            });
        if self.credential_mutations.journal().active().is_some() && !belongs_to_active_add {
            self.mail_queue.push_back(intent);
            return Task::none();
        }
        if self.account_edit_has_active_work()
            || self.pending_sync_adoption.is_some()
            || self.content_sync_inflight.is_some()
        {
            self.mail_queue.push_back(intent);
            return Task::none();
        }
        if !self.credential_store_ready {
            self.mail_queue.push_back(intent);
            return Task::none();
        }
        let availability = match &intent {
            MailIntent::SubmitOutbox { config, .. } => {
                self.mail_backend.validate_submission(config)
            }
            MailIntent::ProbeAccount(config)
            | MailIntent::SyncMailbox { config, .. }
            | MailIntent::ProbeAccountEdit(config)
            | MailIntent::ReconcileServerMutation { config, .. }
            | MailIntent::SearchServer { config, .. } => {
                self.mail_backend.validate_configuration(config)
            }
        };
        if let Err(error) = availability {
            self.mail_notice = Some(MailNotice {
                title: crate::l10n::message(
                    "mail-notice-could-not-start",
                    &[("value1", (intent.label()).to_owned())],
                ),
                body: crate::l10n::tr(error.user_message()),
                retryable: false,
                can_cancel_outbox: matches!(&intent, MailIntent::SubmitOutbox { .. }),
            });
            self.mail_retry = matches!(&intent, MailIntent::SubmitOutbox { .. }).then_some(intent);
            return Task::none();
        }
        if self.credential_inflight.is_some() || self.mail_cancel.is_some() {
            self.mail_queue.push_back(intent);
            return Task::none();
        }
        // Encoding precedes credential acquisition: a message that cannot be
        // built must never unlock a password.
        if let MailIntent::SubmitOutbox { pending_id, .. } = &intent
            && !self
                .outgoing_mime
                .as_ref()
                .is_some_and(|(prepared, _)| prepared == pending_id)
        {
            return self.start_outgoing_mime(pending_id.clone());
        }
        let Some(generation) = self.advance_mail_generation() else {
            self.mail_queue.push_front(intent);
            return self.refuse_exhausted_mail_generation();
        };
        let key = Self::credential_key(intent.config());
        let operation = self.credential_store.0.get(key);
        self.mail_intent = Some(intent);
        self.credential_task(generation, CredentialStage::LoadForOperation, operation)
    }

    // Encodes the durable Outbox entry on the bounded blocking owner.
    //
    // The Outbox copy stays authoritative throughout; a failure to encode is a
    // retryable notice, never a send.
    fn start_outgoing_mime(&mut self, pending_id: String) -> Task<Message> {
        let Some(pending) = self
            .pending_send
            .as_ref()
            .filter(|pending| pending.id == pending_id)
            .cloned()
        else {
            return self.start_next_mail_operation();
        };
        self.mail_notice = Some(MailNotice {
            title: crate::l10n::tr("Preparing message"),
            body: crate::l10n::tr("The durable Outbox copy is being encoded for submission."),
            retryable: false,
            can_cancel_outbox: true,
        });
        let store = self.attachment_store.clone();
        Task::perform(
            self.submit_blocking(move || store.outgoing_mime_bytes(&pending)),
            move |settlement| {
                Message::OutgoingMimePrepared(pending_id, Self::attachment_settlement(settlement))
            },
        )
    }

    // Folds one startup custody answer into the visible attachment state.
    //
    // A cleanup failure keeps the reconcile pass owed, so close settlement
    // cannot treat unproven custody as settled.
    pub(crate) fn apply_attachment_startup_survey(
        &mut self,
        cleanup: Result<(), attachment_store::AttachmentFailure>,
    ) {
        match cleanup {
            Ok(()) => {
                self.attachment_reconcile_needed = false;
                if self.pending_payload_cleanup.is_empty() && self.attachment_cleanup_inflight == 0
                {
                    self.attachment_cleanup_error = None;
                }
            }
            Err(error) => {
                self.attachment_reconcile_needed = true;
                self.attachment_cleanup_error = Some(error);
                self.attachment_notice = Some(crate::l10n::tr(error.user_message()));
            }
        }
    }
}
