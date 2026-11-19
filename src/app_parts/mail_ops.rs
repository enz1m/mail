use super::*;

impl State {
    pub(crate) fn start_next_mail_operation(&mut self) -> Task<Message> {
        if self.account_edit_has_active_work()
            || self.credential_mutations.journal().active().is_some()
            || self.credential_inflight.is_some()
            || self.mail_cancel.is_some()
            || self.mail_retry.is_some()
            || self.mailbox_sync_retry.is_some()
            || self.pending_sync_adoption.is_some()
            || self.content_sync_inflight.is_some()
        {
            return Task::none();
        }
        if self.local_state_is_published()
            && let Some(mutation) = self.next_dispatchable_mutation()
            && let Some(account_id) = mutation
                .server_targets
                .first()
                .map(|target| target.account_id.as_str())
            && let Some(config) = self
                .store
                .account(account_id)
                .and_then(|account| account.config.clone())
        {
            return self.start_credential_load(MailIntent::ReconcileServerMutation {
                config,
                mutation_id: mutation.id,
            });
        }
        if self.close_settlement.is_some() {
            return Task::none();
        }
        self.mail_queue
            .pop_front()
            .map_or_else(Task::none, |intent| self.start_credential_load(intent))
    }

    // The first journaled server change that may go to the server now. A move
    // still offering Undo waits, unless the app is closing.
    pub(crate) fn next_dispatchable_mutation(&self) -> Option<&local_state::PendingServerMutation> {
        self.pending_server_mutations.iter().find(|mutation| {
            !mutation.server_targets.is_empty()
                && (self.close_settlement.is_some() || !self.held_mutations.contains(&mutation.id))
        })
    }

    pub(crate) fn search_server(&mut self) -> Task<Message> {
        let query = self.search_query.trim().to_owned();
        if query.is_empty() || matches!(self.server_search, ServerSearch::Running { .. }) {
            return Task::none();
        }
        let Some(criteria) = model::search::parse(&query).imap_criteria(self.store.today) else {
            self.server_search = ServerSearch::Failed {
                query,
                reason: crate::l10n::tr("Nothing in this search can be asked of the server."),
            };
            return Task::none();
        };
        let Some((config, mailbox)) = self.server_search_target() else {
            self.server_search = ServerSearch::Failed {
                query,
                reason: crate::l10n::tr("No mail server is set up to ask."),
            };
            return Task::none();
        };
        self.server_search = ServerSearch::Running {
            query: query.clone(),
        };
        self.mail_queue.push_back(MailIntent::SearchServer {
            config,
            mailbox,
            criteria,
            query,
        });
        self.start_next_mail_operation()
    }

    fn server_search_target(&self) -> Option<(backend::AccountConfig, MailboxKind)> {
        let imap = |account: &model::types::Account| {
            account
                .config
                .clone()
                .filter(|config| config.protocol == backend::MailProtocol::Imap)
        };
        if let SidebarSelection::AccountMailbox {
            account_id,
            mailbox,
        } = &self.selection
        {
            return self
                .store
                .account(account_id)
                .and_then(imap)
                .map(|config| (config, *mailbox));
        }
        self.store
            .accounts
            .iter()
            .find_map(imap)
            .map(|config| (config, MailboxKind::Inbox))
    }

    pub(crate) fn clear_server_search(&mut self) {
        self.server_search = ServerSearch::Idle;
        if self.server_search_ids.is_empty() {
            return;
        }
        let pending: HashSet<&str> = self
            .pending_server_mutations
            .iter()
            .flat_map(|mutation| mutation.target_ids.iter().map(String::as_str))
            .collect();
        let ids = std::mem::take(&mut self.server_search_ids);
        let before = self.store.messages.len();
        self.store
            .messages
            .retain(|message| !ids.contains(&message.id) || pending.contains(message.id.as_str()));
        if self.store.messages.len() != before {
            self.store.mark_dirty();
            self.reconcile_after_mutation();
        }
    }

    pub(crate) fn start_backend_with_secret(&mut self, secret: Secret) -> Task<Message> {
        let Some(intent) = self.mail_intent.clone() else {
            return Task::none();
        };
        let generation = self.mail_generation;
        if let MailIntent::SyncMailbox {
            config,
            mailbox: MailboxKind::Inbox,
            ..
        } = &intent
        {
            self.start_inbox_watch(config, &secret);
        }
        let operation = match &intent {
            MailIntent::ProbeAccount(config) => self.mail_backend.probe(
                generation,
                backend::curl_backend::SyncRequest {
                    config: config.clone(),
                    secret,
                    known: Vec::new(),
                },
            ),
            MailIntent::SyncMailbox {
                config, mailbox, ..
            } => self.mail_backend.sync_mailbox(
                generation,
                backend::curl_backend::SyncRequest {
                    config: config.clone(),
                    secret,
                    known: self.known_uids(&config.id, *mailbox),
                },
                *mailbox,
            ),
            MailIntent::ProbeAccountEdit(config) => self.mail_backend.probe(
                generation,
                backend::curl_backend::SyncRequest {
                    config: config.clone(),
                    secret,
                    known: Vec::new(),
                },
            ),
            MailIntent::SearchServer {
                config,
                mailbox,
                criteria,
                ..
            } => self.mail_backend.search_mailbox(
                generation,
                backend::curl_backend::SyncRequest {
                    config: config.clone(),
                    secret,
                    known: self.known_uids(&config.id, *mailbox),
                },
                *mailbox,
                criteria.clone(),
            ),
            MailIntent::ReconcileServerMutation { mutation_id, .. } => {
                let Some(mutation) = self
                    .pending_server_mutations
                    .iter()
                    .find(|mutation| mutation.id == *mutation_id)
                    .cloned()
                else {
                    self.mail_intent = None;
                    return self.start_next_mail_operation();
                };
                self.mail_backend.mutate(
                    generation,
                    backend::curl_backend::MutationRequest {
                        config: intent.config().clone(),
                        mutation,
                        secret,
                    },
                )
            }
            MailIntent::SubmitOutbox { config, pending_id } => {
                let Some(pending) = self
                    .pending_send
                    .as_ref()
                    .filter(|pending| pending.id == *pending_id)
                else {
                    self.mail_intent = None;
                    return self.start_next_mail_operation();
                };
                let recipients = match pending.smtp_envelope_recipients() {
                    Ok(recipients) => recipients,
                    Err(error) => {
                        self.mail_retry = None;
                        self.mail_intent = None;
                        self.mail_notice = Some(Self::envelope_refusal_notice(error));
                        return self.start_next_mail_operation();
                    }
                };
                let envelope_from = pending.sender_email.clone();
                let config = config.clone();
                let prepared = self
                    .outgoing_mime
                    .take()
                    .filter(|(prepared, _)| prepared == pending_id);
                let Some((_, mime)) = prepared else {
                    self.mail_retry = Some(intent.clone());
                    self.mail_intent = None;
                    self.mail_notice = Some(MailNotice {
                        title: crate::l10n::tr("Message remains in Outbox"),
                        body: crate::l10n::tr("The message was not encoded for submission."),
                        retryable: true,
                        can_cancel_outbox: true,
                    });
                    return Task::none();
                };
                self.mail_backend.submit(
                    generation,
                    backend::curl_backend::SendRequest {
                        config,
                        envelope_from,
                        recipients,
                        mime,
                        secret,
                    },
                )
            }
        };
        self.mail_cancel = Some((generation, operation.cancel_handle()));
        Task::perform(operation.finish(), |(generation, result)| {
            Message::MailOperationFinished(generation, result)
        })
    }

    pub(crate) fn queue_mailbox_sync(
        &mut self,
        config: backend::AccountConfig,
        mailbox: MailboxKind,
    ) -> Task<Message> {
        let refresh_generation = self.mailbox_sync_generation;
        let already_queued = self.mail_queue.iter().any(|intent| {
            matches!(intent, MailIntent::SyncMailbox {
                config: queued,
                mailbox: queued_mailbox,
                refresh_generation: queued_generation,
            } if queued.id == config.id
                && *queued_mailbox == mailbox
                && *queued_generation == refresh_generation)
        }) || self.mail_intent.as_ref().is_some_and(|intent| {
            matches!(intent, MailIntent::SyncMailbox {
                config: active,
                mailbox: active_mailbox,
                refresh_generation: active_generation,
            } if active.id == config.id
                && *active_mailbox == mailbox
                && *active_generation == refresh_generation)
        }) || self.mailbox_sync_retry.as_ref().is_some_and(|intent| {
            matches!(intent, MailIntent::SyncMailbox {
                config: retry,
                mailbox: retry_mailbox,
                refresh_generation: retry_generation,
            } if retry.id == config.id
                && *retry_mailbox == mailbox
                && *retry_generation == refresh_generation)
        });
        if already_queued {
            return Task::none();
        }
        if self.mailbox_sync_retry.is_some() {
            self.mailbox_sync_retry = None;
            self.mailbox_sync_retry_at = None;
            self.mailbox_sync_retry_attempt = 0;
        }
        self.mail_queue
            .retain(|intent| !matches!(intent, MailIntent::SyncMailbox { .. }));
        self.start_credential_load(MailIntent::SyncMailbox {
            config,
            mailbox,
            refresh_generation,
        })
    }

    pub(crate) fn mailbox_sync_running(&self) -> bool {
        let syncing = |intent: &MailIntent| matches!(intent, MailIntent::SyncMailbox { .. });
        self.mail_intent.as_ref().is_some_and(syncing)
            || self.mail_queue.iter().any(syncing)
            || self.pending_sync_adoption.is_some()
    }

    pub(crate) fn queue_inbox_sync(&mut self, config: backend::AccountConfig) -> Task<Message> {
        self.queue_mailbox_sync(config, MailboxKind::Inbox)
    }

    pub(crate) fn selected_mailbox_sync(&self) -> Option<(backend::AccountConfig, MailboxKind)> {
        let (account_id, mailbox) = match &self.selection {
            SidebarSelection::AccountMailbox {
                account_id,
                mailbox,
            } => (account_id.as_str(), *mailbox),
            SidebarSelection::Favorite(favorite) => {
                let mailbox = match favorite {
                    model::types::FavoriteKind::Inbox | model::types::FavoriteKind::Flagged => {
                        MailboxKind::Inbox
                    }
                    model::types::FavoriteKind::Drafts => MailboxKind::Drafts,
                    model::types::FavoriteKind::Sent => MailboxKind::Sent,
                };
                (self.active_account_id.as_str(), mailbox)
            }
            SidebarSelection::SmartMailbox(_) | SidebarSelection::Folder { .. } => {
                (self.active_account_id.as_str(), MailboxKind::Inbox)
            }
        };
        self.store
            .account(account_id)
            .and_then(|account| account.config.clone())
            .map(|config| (config, mailbox))
    }

    pub(crate) fn invalidate_mailbox_syncs(&mut self, cancel_active: bool) -> bool {
        let advanced = if let Some(generation) = self.mailbox_sync_generation.checked_add(1) {
            self.mailbox_sync_generation = generation;
            true
        } else {
            self.mailbox_sync_generation_exhausted = true;
            self.mail_notice = Some(MailNotice {
                title: crate::l10n::tr("Mailbox refresh unavailable"),
                body:
                    "The mailbox-refresh identity is exhausted. Restart the app to refresh again."
                        .to_owned(),
                retryable: false,
                can_cancel_outbox: false,
            });
            false
        };
        self.mailbox_sync_retry_at = None;
        self.mailbox_sync_retry_attempt = 0;
        self.mail_queue
            .retain(|intent| !matches!(intent, MailIntent::SyncMailbox { .. }));
        if self.mailbox_sync_retry.is_some() {
            self.mailbox_sync_retry = None;
        }
        if cancel_active
            && matches!(
                self.mail_intent.as_ref(),
                Some(MailIntent::SyncMailbox { .. })
            )
        {
            if let Some((active, cancel)) = &self.mail_cancel {
                cancel.cancel();
                self.mail_discard_generation = Some(*active);
            } else if let Some((active, cancel, CredentialStage::LoadForOperation)) =
                &self.credential_inflight
            {
                cancel.cancel();
                self.mail_discard_generation = Some(*active);
            }
        }
        advanced
    }

    pub(crate) fn invalidate_mailbox_syncs_for_close(&mut self) {
        let resumable_sync = match self.mailbox_sync_retry.as_ref() {
            Some(MailIntent::SyncMailbox {
                config, mailbox, ..
            }) => Some((config.clone(), *mailbox)),
            _ => None,
        };
        let retry_at = self.mailbox_sync_retry_at;
        let retry_attempt = self.mailbox_sync_retry_attempt;
        let _ = self.invalidate_mailbox_syncs(false);
        if let Some((config, mailbox)) = resumable_sync {
            self.mailbox_sync_retry = Some(MailIntent::SyncMailbox {
                config,
                mailbox,
                refresh_generation: self.mailbox_sync_generation,
            });
            self.mailbox_sync_retry_at = retry_at;
            self.mailbox_sync_retry_attempt = retry_attempt;
        }
    }

    pub(crate) fn older_mailbox_cursor(
        &self,
    ) -> Option<(backend::AccountConfig, MailboxKind, u32)> {
        if !self.search_query.trim().is_empty() {
            return None;
        }
        let (config, mailbox) = self.selected_mailbox_sync()?;
        if self
            .older_mailboxes_exhausted
            .contains(&(config.id.clone(), mailbox))
        {
            return None;
        }
        let uid = self
            .known_uids(&config.id, mailbox)
            .iter()
            .map(|(_, uid)| *uid)
            .min()?;
        (uid > 1).then_some((config, mailbox, uid))
    }

    pub(crate) fn load_older_mailbox(&mut self) -> Task<Message> {
        if self.mail_intent.is_some() || self.mail_cancel.is_some() || !self.mail_queue.is_empty() {
            return Task::none();
        }
        let Some((config, mailbox, uid)) = self.older_mailbox_cursor() else {
            return Task::none();
        };
        self.mail_queue.push_back(MailIntent::SearchServer {
            config,
            mailbox,
            criteria: format!("X-MAIL-OLDER {uid}"),
            query: String::new(),
        });
        self.start_next_mail_operation()
    }

    pub(crate) fn manual_mailbox_refresh(&mut self) -> Task<Message> {
        let Some((config, mailbox)) = self.selected_mailbox_sync() else {
            self.mail_notice = Some(MailNotice {
                title: crate::l10n::tr("Mailbox refresh unavailable"),
                body: crate::l10n::tr("The current mailbox has no configured account transport."),
                retryable: false,
                can_cancel_outbox: false,
            });
            return Task::none();
        };
        if !self.invalidate_mailbox_syncs(true) {
            return Task::none();
        }
        // The refresh button spins while this runs; a finished or failed
        // sync replaces any notice an earlier one left.
        self.mail_notice = None;
        self.queue_mailbox_sync(config, mailbox)
    }

    pub(crate) fn periodic_mailbox_refresh(
        &mut self,
        generation: u64,
        _now: Instant,
    ) -> Task<Message> {
        if generation != self.mailbox_sync_generation
            || self.mailbox_sync_generation_exhausted
            || self.close_settlement.is_some()
        {
            return Task::none();
        }
        let Some((config, mailbox)) = self.selected_mailbox_sync() else {
            return Task::none();
        };
        if self.mailbox_sync_retry.is_some() {
            return Task::none();
        }
        self.queue_mailbox_sync(config, mailbox)
    }

    pub(crate) fn retain_mailbox_sync_retry(&mut self, intent: MailIntent, now: Instant) {
        let MailIntent::SyncMailbox {
            refresh_generation, ..
        } = &intent
        else {
            return;
        };
        if *refresh_generation != self.mailbox_sync_generation {
            return;
        }
        let delay = MAILBOX_SYNC_RETRY_DELAYS[self
            .mailbox_sync_retry_attempt
            .min(MAILBOX_SYNC_RETRY_DELAYS.len() - 1)];
        self.mailbox_sync_retry_attempt = self
            .mailbox_sync_retry_attempt
            .saturating_add(1)
            .min(MAILBOX_SYNC_RETRY_DELAYS.len() - 1);
        self.mailbox_sync_retry_at = Some(now + delay);
        self.mailbox_sync_retry = Some(intent);
    }

    pub(crate) fn mailbox_sync_retry_pending(&self) -> bool {
        self.mailbox_sync_retry.is_some()
    }

    pub(crate) fn retry_mailbox_sync_if_due(
        &mut self,
        generation: u64,
        now: Instant,
    ) -> Task<Message> {
        if generation != self.mailbox_sync_generation
            || self.mailbox_sync_generation_exhausted
            || self
                .mailbox_sync_retry_at
                .is_none_or(|deadline| now < deadline)
        {
            return Task::none();
        }
        self.mailbox_sync_retry_at = None;
        let Some(intent @ MailIntent::SyncMailbox { .. }) = self.mailbox_sync_retry.take() else {
            return Task::none();
        };
        self.start_credential_load(intent)
    }

    pub(crate) fn start_pending_send(&mut self) -> Task<Message> {
        let Some(pending) = self.pending_send.as_ref() else {
            return Task::none();
        };
        if !self.local_state_is_published() {
            self.mail_notice = Some(MailNotice {
                title: crate::l10n::tr("Saving Outbox"),
                body: crate::l10n::tr("Delivery will start after the queued message is durable."),
                retryable: false,
                can_cancel_outbox: true,
            });
            return Task::none();
        }
        let Some(config) = self
            .store
            .account(&pending.account_id)
            .and_then(|account| account.config.clone())
        else {
            self.mail_notice = Some(MailNotice {
                title: crate::l10n::tr("Message remains in Outbox"),
                body: crate::l10n::tr("The selected account has no configured mail transport."),
                retryable: false,
                can_cancel_outbox: true,
            });
            return Task::none();
        };
        if let Err(error) = pending.smtp_envelope_recipients() {
            self.mail_retry = None;
            self.mail_notice = Some(Self::envelope_refusal_notice(error));
            return Task::none();
        }
        let intent = MailIntent::SubmitOutbox {
            config,
            pending_id: pending.id.clone(),
        };
        self.mail_retry = None;
        self.mail_notice = Some(MailNotice {
            title: crate::l10n::tr("Sending message"),
            body: crate::l10n::tr("The durable Outbox copy is being submitted."),
            retryable: false,
            can_cancel_outbox: true,
        });
        self.start_credential_load(intent)
    }

    fn envelope_refusal_notice(error: model::compose::EnvelopeRefusal) -> MailNotice {
        MailNotice {
            title: crate::l10n::tr("Message remains in Outbox"),
            body: crate::l10n::tr(error.user_message()),
            retryable: false,
            can_cancel_outbox: true,
        }
    }

    pub(crate) fn credential_failure_notice(
        intent: &MailIntent,
        outcome: CredentialOutcome,
    ) -> MailNotice {
        let body = match &outcome {
            CredentialOutcome::NotFound => "No password is stored for this account.",
            CredentialOutcome::Unavailable => "The system credential service is unavailable.",
            CredentialOutcome::PermissionDenied => "The system credential service denied access.",
            CredentialOutcome::UnlockCancelled => "The credential unlock prompt was cancelled.",
            CredentialOutcome::Cancelled => "The credential request was cancelled.",
            CredentialOutcome::Saturated => "The credential service is busy. Retry this operation.",
            CredentialOutcome::OwnerShutdown => "The credential service is shutting down.",
            CredentialOutcome::MutationUncertain => {
                "The credential change could not be confirmed and requires reconciliation."
            }
            CredentialOutcome::Failed(_) => "The system credential service failed.",
            CredentialOutcome::Retrieved(_)
            | CredentialOutcome::Stored
            | CredentialOutcome::Deleted => {
                "The credential operation returned an unexpected result."
            }
        };
        MailNotice {
            title: crate::l10n::message(
                "mail-notice-could-not-complete",
                &[("value1", (intent.label()).to_owned())],
            ),
            body: body.to_owned(),
            retryable: !matches!(
                outcome,
                CredentialOutcome::Cancelled
                    | CredentialOutcome::OwnerShutdown
                    | CredentialOutcome::MutationUncertain
            ),
            can_cancel_outbox: matches!(intent, MailIntent::SubmitOutbox { .. }),
        }
    }

    pub(crate) fn restore_cancelled_outbox(&mut self) -> Task<Message> {
        let restored = self.compose_ctl().undo_pending_send();
        if !restored {
            return Task::none();
        }
        self.compose_generation = self.compose_generation.wrapping_add(1);
        if self.autosave_compose_draft()
            && let Some(id) = self.compose.editing_draft_id.clone()
        {
            self.draft_save_pending = true;
            self.queue_server_mutation(local_state::ServerMutationKind::DraftUpserted, vec![id]);
        }
        self.sync_body_editor();
        self.arm_compose_focus()
    }

    pub(crate) fn request_account_removal(&mut self, account_id: &str) -> Task<Message> {
        if self.account_removal.is_some() || self.settings.edit_account.account_id.is_some() {
            return Task::none();
        }
        let Some(account) = self.store.account(account_id).cloned() else {
            return Task::none();
        };
        let Some(config) = account.config.as_ref() else {
            return Task::none();
        };
        self.account_removal = Some(AccountRemoval {
            credential_key: Self::credential_key(config),
            account,
            phase: AccountRemovalPhase::Confirming,
        });
        self.confirm_remove_account = enzui_iced_core::confirm::ConfirmState::new(None);
        Task::none()
    }

    pub(crate) fn account_removal_confirmation(&self) -> Option<(&str, &str)> {
        self.account_removal.as_ref().and_then(|removal| {
            matches!(&removal.phase, AccountRemovalPhase::Confirming).then_some((
                removal.account.display_name.as_str(),
                removal.account.email.as_str(),
            ))
        })
    }

    pub(crate) fn account_removal_status(
        &self,
        account_id: &str,
    ) -> Option<(&'static str, &str, bool)> {
        let removal = self
            .account_removal
            .as_ref()
            .filter(|removal| removal.account.id == account_id)?;
        match &removal.phase {
            AccountRemovalPhase::Confirming => None,
            AccountRemovalPhase::AwaitingJournal(_) => Some((
                "Preparing account removal",
                "Saving the exact account and credential transition before removal…",
                false,
            )),
            AccountRemovalPhase::Publishing(_) => Some((
                "Configuration saved; credential cleanup pending",
                "The local account removal is being published before its exact key is deleted.",
                false,
            )),
            AccountRemovalPhase::Deleting(_) | AccountRemovalPhase::VerifyingDeletion(_) => Some((
                "Configuration saved; credential cleanup pending",
                "The account is removed locally while deletion of its exact keyring entry is confirmed…",
                false,
            )),
            AccountRemovalPhase::Failed(message) => {
                Some(("Account not removed", message.as_str(), true))
            }
        }
    }

    pub(crate) fn start_account_removal(&mut self) -> Task<Message> {
        let Some(removal) = self.account_removal.as_ref() else {
            return Task::none();
        };
        if !matches!(
            &removal.phase,
            AccountRemovalPhase::Confirming | AccountRemovalPhase::Failed(_)
        ) {
            return Task::none();
        }
        let account = removal.account.clone();
        let account_id = account.id.clone();
        let key = removal.credential_key.clone();
        if account
            .config
            .as_ref()
            .is_some_and(|config| config.auth == backend::AuthMethod::OAuth2)
        {
            // Its access token must not outlive it in memory; other accounts
            // just refresh.
            backend::curl_backend::oauth::forget_tokens();
        }
        if matches!(&removal.phase, AccountRemovalPhase::Failed(_))
            && self.store.account(&account_id).is_none()
            && let Some(record) = self.credential_mutations.journal().active()
            && record.account_id == account_id
            && self.credential_mutations.journal().public_generation()
                == record.target_public_generation
        {
            let generation = record.sequence;
            if let Some(removal) = self.account_removal.as_mut() {
                removal.phase = AccountRemovalPhase::Publishing(generation);
            }
            return self.continue_credential_mutation();
        }
        let current_matches = self
            .store
            .account(&account_id)
            .is_some_and(|current| current == &account);
        if !current_matches {
            self.account_removal = None;
            self.mail_notice = Some(MailNotice {
                title: crate::l10n::tr("Account not removed"),
                body: crate::l10n::tr("The account changed after removal was requested."),
                retryable: false,
                can_cancel_outbox: false,
            });
            return Task::none();
        }
        let owned_message_ids = self
            .store
            .messages
            .iter()
            .filter(|message| message.account_id == account_id)
            .map(|message| message.id.as_str())
            .collect::<HashSet<_>>();
        let pending_send_id = self
            .pending_send
            .as_ref()
            .filter(|pending| pending.account_id == account_id)
            .map(|pending| pending.id.as_str());
        let has_pending_server_work = self.pending_server_mutations.iter().any(|mutation| {
            mutation
                .server_targets
                .iter()
                .any(|target| target.account_id == account_id)
                || mutation.target_ids.iter().any(|target| {
                    target == &account_id
                        || owned_message_ids.contains(target.as_str())
                        || pending_send_id == Some(target.as_str())
                })
        });
        if has_pending_server_work {
            if let Some(removal) = self.account_removal.as_mut() {
                removal.phase = AccountRemovalPhase::Failed(
                    "Server changes for this account remain durably journaled. Reconcile them before removing the account."
                        .to_owned(),
                );
            }
            return Task::none();
        }
        if !self.credential_store_ready {
            if let Some(removal) = self.account_removal.as_mut() {
                removal.phase = AccountRemovalPhase::Failed(
                    "The system credential service is still starting. Retry account removal when it is available."
                        .to_owned(),
                );
            }
            return Task::none();
        }
        if self.credential_inflight.is_some()
            || self.mail_cancel.is_some()
            || self.pending_sync_adoption.is_some()
            || self.content_sync_inflight.is_some()
            || self.credential_mutations.journal().active().is_some()
        {
            if let Some(removal) = self.account_removal.as_mut() {
                removal.phase = AccountRemovalPhase::Failed(
                    "Finish or cancel the current mail or credential operation, then retry account removal."
                        .to_owned(),
                );
            }
            return Task::none();
        }
        let referenced = self.credential_keys_in_public_state();
        let generation = match self.credential_mutations.reserve(
            credential_mutation::CredentialMutationKind::Remove,
            account_id,
            true,
            &referenced,
            |_| credential_mutation::TransitionKeys {
                old_key: Some(key),
                new_key: None,
            },
        ) {
            Ok(reservation) => reservation.sequence,
            Err(message) => {
                if let Some(removal) = self.account_removal.as_mut() {
                    removal.phase = AccountRemovalPhase::Failed(message.to_owned());
                }
                return Task::none();
            }
        };
        if let Some(removal) = self.account_removal.as_mut() {
            removal.phase = AccountRemovalPhase::AwaitingJournal(generation);
        }
        Task::none()
    }

    fn account_removal_failure(outcome: CredentialOutcome) -> String {
        match outcome {
            CredentialOutcome::Unavailable => {
                "The account configuration is removed, but the credential service is unavailable. Exact-key cleanup remains journaled."
            }
            CredentialOutcome::PermissionDenied => {
                "The account configuration is removed, but the credential service denied exact-key cleanup."
            }
            CredentialOutcome::UnlockCancelled => {
                "The account configuration is removed, but credential unlocking was cancelled."
            }
            CredentialOutcome::Cancelled => {
                "The account configuration is removed, but credential deletion was cancelled."
            }
            CredentialOutcome::Saturated => {
                "The account configuration is removed, but credential cleanup is busy and remains journaled."
            }
            CredentialOutcome::OwnerShutdown => {
                "The account configuration is removed, but credential cleanup stopped and remains journaled."
            }
            CredentialOutcome::MutationUncertain => {
                "The account configuration is removed, but credential deletion is uncertain and remains journaled for reconciliation."
            }
            CredentialOutcome::Failed(_) => {
                "The account configuration is removed, but credential deletion was not confirmed."
            }
            CredentialOutcome::Retrieved(_) => {
                "The account configuration is removed, but the retired credential is still present."
            }
            CredentialOutcome::NotFound => {
                "Credential absence was returned at an unexpected deletion stage."
            }
            CredentialOutcome::Stored | CredentialOutcome::Deleted => {
                "Credential cleanup returned an unexpected verification result."
            }
        }
        .to_owned()
    }

    pub(crate) fn remove_account_owned_state(
        &mut self,
        account: model::types::Account,
    ) -> Task<Message> {
        let account_id = account.id.clone();
        let display_name = account.display_name;
        let removes_pending = self
            .pending_send
            .as_ref()
            .is_some_and(|pending| pending.account_id == account_id);
        if removes_pending {
            self.pending_send = None;
        }
        let removes_compose = self.compose.from_account_id == account_id;
        if removes_compose {
            self.compose = ComposeState::default();
            self.compose_generation = self.compose_generation.wrapping_add(1);
            self.compose_overflow_open = false;
            self.draft_save_pending = false;
            self.sync_body_editor();
        }

        self.store
            .messages
            .retain(|message| message.account_id != account_id);
        self.store.accounts.retain(|item| item.id != account_id);
        self.forget_inbox_watch(&account_id);
        self.mail_queue
            .retain(|intent| intent.config().id != account_id);
        if self
            .mail_retry
            .as_ref()
            .is_some_and(|intent| intent.config().id == account_id)
        {
            self.mail_retry = None;
        }
        if self
            .mailbox_sync_retry
            .as_ref()
            .is_some_and(|intent| intent.config().id == account_id)
        {
            self.mailbox_sync_retry = None;
            self.mailbox_sync_retry_at = None;
            self.mailbox_sync_retry_attempt = 0;
        }
        self.collapsed_sections.remove(&account_id);
        self.settings.expanded_account = None;
        self.settings.edit_account.reset();
        self.sync_signature_editor(None);

        if self.active_account_id == account_id {
            self.active_account_id = self
                .store
                .accounts
                .first()
                .map(|account| account.id.clone())
                .unwrap_or_default();
        }
        if matches!(
            &self.selection,
            SidebarSelection::AccountMailbox {
                account_id: selected,
                ..
            } | SidebarSelection::Folder {
                account_id: selected,
                ..
            } if selected == &account_id
        ) {
            self.selection = SidebarSelection::Favorite(model::types::FavoriteKind::Inbox);
        }
        if self.store.accounts.is_empty() {
            self.selection = SidebarSelection::Favorite(model::types::FavoriteKind::Inbox);
            self.selected_conversation = None;
            let _ = self.row_selection.clear();
        }

        let mut sidebar_layout = self.sidebar_layout.get().clone();
        let preferences_changed = sidebar_layout.prune(&self.store.accounts);
        if preferences_changed {
            *self.sidebar_layout.get_mut() = sidebar_layout;
        }
        self.store.mark_dirty();
        let sidebar = self.reconcile_sidebar_selection();
        self.reconcile_after_mutation();
        self.mail_notice = Some(MailNotice {
            title: crate::l10n::tr("Account removed"),
            body: crate::l10n::message(
                "mail-notice-removed-display-name-and-its-local-mail",
                &[("display_name", (display_name))],
            ),
            retryable: false,
            can_cancel_outbox: false,
        });
        let preferences = if preferences_changed {
            self.save_preferences(preferences::SaveOrigin::SidebarLayout)
        } else {
            Task::none()
        };
        let compose = if removes_compose {
            self.dismiss_compose_focus(DismissCause::DefaultAction)
        } else {
            Task::none()
        };
        Task::batch([sidebar, preferences, compose])
    }

    pub(crate) fn complete_account_removal(
        &mut self,
        generation: u64,
        stage: CredentialStage,
        outcome: CredentialOutcome,
    ) -> Task<Message> {
        let Some(removal) = self.account_removal.as_ref() else {
            return Task::none();
        };
        match stage {
            CredentialStage::DeleteAccount if matches!(&removal.phase, AccountRemovalPhase::Deleting(active) if *active == generation) =>
            {
                let key = removal.credential_key.clone();
                if let Some(removal) = self.account_removal.as_mut() {
                    removal.phase = AccountRemovalPhase::VerifyingDeletion(generation);
                }
                let operation = self.credential_store.0.get(key);
                self.credential_task(generation, CredentialStage::VerifyAccountDeleted, operation)
            }
            CredentialStage::VerifyAccountDeleted if matches!(&removal.phase, AccountRemovalPhase::VerifyingDeletion(active) if *active == generation) => {
                if matches!(outcome, CredentialOutcome::NotFound) {
                    self.account_removal = None;
                    self.finish_credential_transition(generation)
                } else {
                    let message = Self::account_removal_failure(outcome);
                    self.account_removal = None;
                    self.mail_notice = Some(MailNotice {
                        title: crate::l10n::tr("Configuration saved; credential cleanup pending"),
                        body: message,
                        retryable: true,
                        can_cancel_outbox: false,
                    });
                    Task::none()
                }
            }
            _ => Task::none(),
        }
    }

    pub(crate) fn delete_account_credential(
        &mut self,
        generation: u64,
        config: &backend::AccountConfig,
    ) -> Task<Message> {
        let key =
            credential_mutation::DurableCredentialKey::from_key(&Self::credential_key(config));
        let journal_generation = self
            .credential_mutations
            .journal()
            .active()
            .filter(|record| {
                record.kind == credential_mutation::CredentialMutationKind::Add
                    && record.account_id == config.id
            })
            .map_or(generation, |record| record.sequence);
        let operation = self.credential_store.0.delete(key.to_key());
        self.credential_recovery = Some(CredentialRecovery {
            generation: journal_generation,
            key,
            delete_after: None,
        });
        self.credential_task(
            journal_generation,
            CredentialStage::RecoverDeleteExact,
            operation,
        )
    }
}
