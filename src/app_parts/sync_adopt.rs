use super::*;

impl State {
    pub(crate) fn imap_server_target(
        message: &model::types::Message,
    ) -> Option<local_state::ServerMutationTarget> {
        let mailbox = match message.location {
            model::types::MessageLocation::Mailbox(mailbox) => mailbox,
            model::types::MessageLocation::Folder(_) => return None,
        };
        let prefix = format!("imap-{}-", message.account_id);
        let suffix = message.id.strip_prefix(&prefix)?;
        let mailbox_prefix = match mailbox {
            MailboxKind::Inbox => None,
            MailboxKind::Drafts => Some("drafts-"),
            MailboxKind::Sent => Some("sent-"),
            MailboxKind::Junk => Some("junk-"),
            MailboxKind::Trash => Some("trash-"),
            MailboxKind::Archive => Some("archive-"),
        };
        let (mailbox, identity) = mailbox_prefix
            .and_then(|prefix| {
                suffix
                    .strip_prefix(prefix)
                    .map(|identity| (mailbox, identity))
            })
            .unwrap_or((MailboxKind::Inbox, suffix));
        let (uid_validity, uid) = identity.split_once('-')?;
        let uid_validity = uid_validity
            .parse::<u32>()
            .ok()
            .filter(|value| *value != 0)?;
        let uid = uid.parse::<u32>().ok().filter(|value| *value != 0)?;
        Some(local_state::ServerMutationTarget {
            message_id: message.id.clone(),
            account_id: message.account_id.clone(),
            mailbox,
            uid_validity,
            uid,
        })
    }

    fn publish_inbound_attachments(
        store: &attachment_store::AttachmentStore,
        mut batch: backend::curl_backend::SyncBatch,
    ) -> Result<backend::curl_backend::SyncBatch, attachment_store::AttachmentFailure> {
        let mut published = Vec::new();
        for fetched in &mut batch.messages {
            if fetched.attachment_completions.len() != fetched.message.attachments.len() {
                let _ = store.remove_payloads(&published);
                return Err(attachment_store::AttachmentFailure::InvalidPayload);
            }
            for (completion, attachment) in fetched
                .attachment_completions
                .iter_mut()
                .zip(&mut fetched.message.attachments)
            {
                if completion.payload_id.is_some()
                    || completion.payload_identity.is_some()
                    || attachment.payload_id.is_some()
                    || attachment.payload_identity.is_some()
                    || u64::try_from(completion.decoded_bytes.len()).unwrap_or(u64::MAX)
                        != attachment.size
                {
                    let _ = store.remove_payloads(&published);
                    return Err(attachment_store::AttachmentFailure::InvalidPayload);
                }
                let staged = match store.stage_inbound(
                    &attachment.name,
                    &attachment.mime,
                    &completion.decoded_bytes,
                ) {
                    Ok(staged) => staged,
                    Err(error) => {
                        let cleanup = store.remove_payloads(&published);
                        return Err(cleanup.err().unwrap_or(error));
                    }
                };
                let Some(payload) = staged.payload_ref() else {
                    let _ = store.remove_payloads(&published);
                    return Err(attachment_store::AttachmentFailure::InvalidPayload);
                };
                published.push(payload.clone());
                *attachment = staged;
                completion.payload_id = Some(payload.id);
                completion.payload_identity = Some(payload.identity);
                completion.decoded_bytes.clear();
            }
        }
        Ok(batch)
    }

    fn start_inbound_content_staging(
        &mut self,
        generation: u64,
        refresh_generation: u64,
        config: backend::AccountConfig,
        batch: backend::curl_backend::SyncBatch,
    ) -> Task<Message> {
        if batch.sync_generation != generation {
            self.retain_mailbox_sync_retry(
                MailIntent::SyncMailbox {
                    config,
                    mailbox: batch.mailbox,
                    refresh_generation,
                },
                Instant::now(),
            );
            self.mail_notice = Some(MailNotice {
                title: crate::l10n::tr("Mailbox synchronization not adopted"),
                body: crate::l10n::tr(
                    "A fetched mailbox carried a stale synchronization generation.",
                ),
                retryable: true,
                can_cancel_outbox: false,
            });
            return Task::none();
        }
        if batch
            .messages
            .iter()
            .all(|fetched| fetched.attachment_completions.is_empty())
        {
            return self.stage_sync_adoption(generation, config, batch);
        }
        self.content_sync_inflight = Some((generation, refresh_generation));
        let mailbox = batch.mailbox;
        let store = self.attachment_store.clone();
        Task::perform(
            self.submit_blocking(move || Self::publish_inbound_attachments(&store, batch)),
            move |settlement| {
                Message::InboundContentStaged(
                    generation,
                    refresh_generation,
                    Box::new(config),
                    mailbox,
                    Self::attachment_settlement(settlement),
                )
            },
        )
    }

    pub(crate) fn sync_payloads(
        batch: &backend::curl_backend::SyncBatch,
    ) -> Vec<model::types::AttachmentPayloadRef> {
        batch
            .messages
            .iter()
            .flat_map(|fetched| fetched.message.attachments.iter())
            .filter_map(model::types::Attachment::payload_ref)
            .collect()
    }

    // Bodies incomplete at the current cap are retained; a larger cap or newer parser permits refetch.
    pub(crate) fn known_uids(&self, account_id: &str, mailbox: MailboxKind) -> Vec<(u32, u32)> {
        use model::types::MessageBodyState;
        self.store
            .messages
            .iter()
            .filter(|message| {
                message.account_id == account_id
                    && message.location == model::types::MessageLocation::Mailbox(mailbox)
                    && match message.body_state {
                        MessageBodyState::Malformed => false,
                        MessageBodyState::Incomplete { limit_bytes, .. } => {
                            limit_bytes >= backend::curl_backend::MAX_MESSAGE_BODY_BYTES
                        }
                        _ => true,
                    }
            })
            .filter_map(Self::imap_server_target)
            .filter(|target| target.mailbox == mailbox)
            .map(|target| (target.uid_validity, target.uid))
            .collect()
    }

    pub(crate) fn complete_sync_candidate(
        &self,
        generation: u64,
        batch: backend::curl_backend::SyncBatch,
    ) -> Result<MailboxStore, &'static str> {
        let backend::curl_backend::SyncBatch {
            older_than_uid,
            retained,
            account_id,
            mailbox,
            uid_validity,
            sync_generation,
            coverage,
            messages: fetched_messages,
        } = batch;
        if sync_generation != generation {
            return Err("a fetched mailbox carried a stale synchronization generation");
        }
        if let backend::curl_backend::SyncCoverage::Capped { total, .. } = coverage
            && fetched_messages.len() < total
        {
            return Err("an incomplete mailbox response cannot replace local state");
        }
        let protected = self
            .pending_server_mutations
            .iter()
            .flat_map(|mutation| mutation.server_targets.iter())
            .map(|target| {
                (
                    target.account_id.clone(),
                    target.mailbox,
                    target.uid_validity,
                    target.uid,
                )
            })
            .collect::<HashSet<_>>();
        let mut population = HashSet::new();
        let retained_flags = retained
            .iter()
            .map(|kept| (kept.uid, (kept.seen, kept.flagged)))
            .collect::<std::collections::HashMap<_, _>>();
        population.extend(retained_flags.keys().copied());
        for fetched in &fetched_messages {
            let message = &fetched.message;
            let identity = Self::imap_server_target(message)
                .ok_or("a fetched message has no exact IMAP identity")?;
            if identity.account_id != account_id
                || identity.mailbox != mailbox
                || identity.uid_validity != uid_validity
                || !population.insert(identity.uid)
            {
                return Err("a fetched message crossed or duplicated its mailbox identity");
            }
            let completion_matches = |completion: &backend::curl_backend::ImapContentIdentity| {
                completion.account_id == identity.account_id
                    && completion.mailbox == identity.mailbox
                    && completion.uid_validity == identity.uid_validity
                    && completion.uid == identity.uid
                    && completion.sync_generation == generation
                    && !completion.part.is_empty()
            };
            if !completion_matches(&fetched.body_completion)
                || fetched.attachment_completions.len() != message.attachments.len()
                || fetched
                    .attachment_completions
                    .iter()
                    .zip(&message.attachments)
                    .any(|(completion, attachment)| {
                        !completion_matches(&completion.identity)
                            || completion.payload_id != attachment.payload_id
                            || completion.payload_identity != attachment.payload_identity
                            || !completion.decoded_bytes.is_empty()
                    })
            {
                return Err("a MIME completion crossed its exact message or part identity");
            }
            let mut part_ids = vec![fetched.body_completion.part.as_str()];
            part_ids.extend(
                fetched
                    .attachment_completions
                    .iter()
                    .map(|completion| completion.identity.part.as_str()),
            );
            part_ids.sort_unstable();
            if part_ids.windows(2).any(|pair| pair[0] == pair[1]) {
                return Err("a MIME completion duplicated its part identity");
            }
        }
        let messages = fetched_messages
            .into_iter()
            .map(|fetched| fetched.message)
            .collect::<Vec<_>>();
        let mut candidate = self.store.clone();
        candidate.messages.retain(|message| {
            if message.account_id != account_id
                || message.location != model::types::MessageLocation::Mailbox(mailbox)
            {
                return true;
            }
            let Some(identity) = Self::imap_server_target(message) else {
                return true;
            };
            let exact = (
                identity.account_id,
                identity.mailbox,
                identity.uid_validity,
                identity.uid,
            );
            protected.contains(&exact)
                || self.server_search_ids.contains(&message.id)
                || (identity.uid_validity == uid_validity
                    && (population.contains(&identity.uid)
                        || older_than_uid.is_some_and(|start| identity.uid < start)))
        });
        // A kept message takes the server's flags unless a local change to it
        // is still on its way there.
        for message in &mut candidate.messages {
            if message.account_id != account_id
                || message.location != model::types::MessageLocation::Mailbox(mailbox)
            {
                continue;
            }
            let Some(identity) = Self::imap_server_target(message) else {
                continue;
            };
            if identity.uid_validity != uid_validity
                || protected.contains(&(
                    identity.account_id.clone(),
                    identity.mailbox,
                    identity.uid_validity,
                    identity.uid,
                ))
            {
                continue;
            }
            if let Some(&(seen, flagged)) = retained_flags.get(&identity.uid) {
                message.unread = !seen;
                message.flagged = flagged;
            }
        }
        for mut message in messages {
            let identity = Self::imap_server_target(&message)
                .ok_or("a fetched message has no exact IMAP identity")?;
            let exact = (
                identity.account_id,
                identity.mailbox,
                identity.uid_validity,
                identity.uid,
            );
            if protected.contains(&exact) {
                continue;
            }
            if let Some(existing) = candidate
                .messages
                .iter_mut()
                .find(|item| item.id == message.id)
            {
                message.snoozed_until = existing.snoozed_until;
                message.send_at = existing.send_at;
                *existing = message;
            } else {
                candidate.messages.push(message);
            }
        }
        candidate.mark_dirty();
        Ok(candidate)
    }

    pub(crate) fn stage_sync_adoption(
        &mut self,
        generation: u64,
        config: backend::AccountConfig,
        batch: backend::curl_backend::SyncBatch,
    ) -> Task<Message> {
        // A message larger than the sync's probe arrives marked incomplete, so a batch
        // holding every message of its population is adopted; missing messages never
        // authorize their absence.
        if let backend::curl_backend::SyncCoverage::Capped { completed, total } = batch.coverage
            && batch.messages.len() < total
        {
            let stale_payloads = Self::sync_payloads(&batch);
            self.mail_notice = Some(MailNotice {
                title: crate::l10n::tr("Mailbox synchronization incomplete"),
                body: crate::l10n::message(
                    "mail-notice-the-server-advertised-total-messages-the-bounded-fetch-completed-complet",
                    &[
                        ("total", crate::l10n::number(total)),
                        ("completed", crate::l10n::number(completed)),
                    ],
                ),
                retryable: false,
                can_cancel_outbox: false,
            });
            let cleanup = if stale_payloads.is_empty() {
                Task::none()
            } else {
                self.remove_payloads_task(stale_payloads)
            };
            let next = self.start_next_mail_operation();
            return Task::batch([cleanup, next]);
        }
        let announce = batch.older_than_uid.is_none()
            && self
                .store
                .messages
                .iter()
                .filter(|message| {
                    message.account_id == batch.account_id
                        && message.location == model::types::MessageLocation::Mailbox(batch.mailbox)
                })
                .filter_map(Self::imap_server_target)
                .all(|target| target.uid_validity == batch.uid_validity);
        let mailbox = batch.mailbox;
        let fetched_payloads = Self::sync_payloads(&batch);
        let candidate_store = match self.complete_sync_candidate(generation, batch) {
            Ok(candidate) => candidate,
            Err(body) => {
                self.retain_mailbox_sync_retry(
                    MailIntent::SyncMailbox {
                        config,
                        mailbox,
                        refresh_generation: self.mailbox_sync_generation,
                    },
                    Instant::now(),
                );
                self.mail_notice = Some(MailNotice {
                    title: crate::l10n::tr("Mailbox synchronization not adopted"),
                    body: body.to_owned(),
                    retryable: true,
                    can_cancel_outbox: false,
                });
                return if fetched_payloads.is_empty() {
                    Task::none()
                } else {
                    self.remove_payloads_task(fetched_payloads)
                };
            }
        };
        let base_token = self.local_state_token();
        let Some(candidate_revision) = self.sync_adoption_revision.checked_add(1) else {
            self.mail_notice = Some(MailNotice {
                title: crate::l10n::tr("Mailbox synchronization needs restart"),
                body: "The synchronization adoption counter is exhausted; the last-good mailbox was retained."
                    .to_owned(),
                retryable: false,
                can_cancel_outbox: false,
            });
            return if fetched_payloads.is_empty() {
                Task::none()
            } else {
                self.remove_payloads_task(fetched_payloads)
            };
        };
        let candidate_token =
            self.local_state_token_for_revision(&candidate_store, candidate_revision);
        let candidate_state = local_state::LocalState::snapshot(
            &candidate_store,
            self.pending_send.as_ref(),
            &self.pending_server_mutations,
            local_state::LocalStateMetadata {
                next_message_ordinal: self.next_ordinal,
                next_account_ordinal: self.next_account_ordinal,
                active_account_id: &self.active_account_id,
                send_generation: self.send_generation,
                next_mutation_ordinal: self.next_server_mutation_ordinal,
                credential_journal: self.credential_mutations.journal(),
            },
        );
        self.pending_sync_adoption = Some(PendingSyncAdoption {
            announce,
            generation,
            account_id: config.id,
            mailbox,
            base_token,
            candidate_token,
            candidate_revision,
            candidate_store,
            candidate_state,
            fetched_payloads,
        });
        // The last saved mailbox stays visible until the snapshot is durable;
        // a routine save is not news.
        self.mail_notice = None;
        self.save_sync_adoption()
    }

    pub(crate) fn save_sync_adoption(&mut self) -> Task<Message> {
        let Some(pending) = self.pending_sync_adoption.as_ref() else {
            return Task::none();
        };
        if pending.base_token != self.local_state_token() {
            let Some(pending) = self.pending_sync_adoption.take() else {
                return Task::none();
            };
            let account_id = pending.account_id.clone();
            let mailbox = pending.mailbox;
            if let Some(config) = self
                .store
                .account(&account_id)
                .and_then(|account| account.config.clone())
            {
                self.retain_mailbox_sync_retry(
                    MailIntent::SyncMailbox {
                        config,
                        mailbox,
                        refresh_generation: self.mailbox_sync_generation,
                    },
                    Instant::now(),
                );
            }
            let retryable = self.mailbox_sync_retry.is_some();
            self.mail_notice = Some(MailNotice {
                title: crate::l10n::tr("Mailbox synchronization needs retry"),
                body: "Local mail changed before the synchronized snapshot became durable; the last-good view was retained."
                    .to_owned(),
                retryable,
                can_cancel_outbox: false,
            });
            return if pending.fetched_payloads.is_empty() {
                Task::none()
            } else {
                self.remove_payloads_task(pending.fetched_payloads)
            };
        }
        let generation = pending.generation;
        let token = pending.candidate_token.clone();
        let state = pending.candidate_state.clone();
        let Some(plan) = self.local_state_gate.reserve_save_if_changed(token.clone()) else {
            return Task::none();
        };
        self.local_state_error = None;
        Task::perform(
            self.submit_blocking(move || plan.prepare_and_execute(move || state)),
            move |settlement| {
                if settlement.result().err()
                    == Some(enzui_iced_core::blocking_task::BlockingTaskError::Busy)
                {
                    Message::PersistenceBusy(PersistenceDebt::SyncAdoption(generation))
                } else {
                    Message::SyncAdoptionSaved(
                        generation,
                        token,
                        Self::persist_settlement(settlement),
                    )
                }
            },
        )
    }

    // Search results are added, never adopted as a snapshot: they say nothing
    // about the rest of the mailbox. Attachment bytes are not staged; a found
    // message lists its attachments until a sync fetches it in full.
    fn complete_server_search(
        &mut self,
        query: String,
        result: Result<
            backend::curl_backend::MailOperationOutput,
            backend::curl_backend::MailFailure,
        >,
    ) -> Task<Message> {
        let current = self.search_query.trim() == query;
        match result {
            Ok(backend::curl_backend::MailOperationOutput::Search(batch)) if current => {
                if query.is_empty() && batch.messages.is_empty() {
                    self.older_mailboxes_exhausted
                        .insert((batch.account_id.clone(), batch.mailbox));
                }
                let mut count = 0;
                for fetched in batch.messages {
                    let mut message = fetched.message;
                    if self.store.message(&message.id).is_some() {
                        continue;
                    }
                    for attachment in &mut message.attachments {
                        attachment.payload_id = None;
                        attachment.payload_identity = None;
                    }
                    self.server_search_ids.insert(message.id.clone());
                    self.store.messages.push(message);
                    count += 1;
                }
                if count > 0 {
                    self.store.mark_dirty();
                    self.reconcile_after_mutation();
                }
                self.server_search = ServerSearch::Found { query, count };
            }
            Ok(_) => {}
            Err(_) if current => {
                self.server_search = ServerSearch::Failed {
                    query,
                    reason: "The server could not be searched. Check the connection and try again."
                        .to_owned(),
                };
            }
            Err(_) => {}
        }
        self.start_next_mail_operation()
    }

    // Nothing reached the server, which still holds the messages in their
    // source mailbox, so the local move is reverted rather than retried.
    fn restore_unplaceable_move(
        &mut self,
        intent: &MailIntent,
        mutation: &local_state::PendingServerMutation,
    ) -> Task<Message> {
        self.pending_server_mutations
            .retain(|pending| pending.id != mutation.id);
        let mut restored = false;
        for target in &mutation.server_targets {
            if let Some(message) = self
                .store
                .messages
                .iter_mut()
                .find(|message| message.id == target.message_id)
            {
                message.location = model::types::MessageLocation::Mailbox(target.mailbox);
                restored = true;
            }
        }
        if restored {
            self.store.mark_dirty();
            self.reconcile_after_mutation();
        }
        self.mail_retry = None;
        self.mail_notice = Some(MailNotice {
            title: crate::l10n::tr("Mailbox not found on server"),
            body: "The server has no mailbox to receive this move. The messages remain in their original mailbox."
                .to_owned(),
            retryable: false,
            can_cancel_outbox: false,
        });
        match mutation.server_targets.first() {
            Some(target) => self.queue_mailbox_sync(intent.config().clone(), target.mailbox),
            None => self.start_next_mail_operation(),
        }
    }

    pub(crate) fn complete_mail_operation(
        &mut self,
        generation: u64,
        result: Result<
            backend::curl_backend::MailOperationOutput,
            backend::curl_backend::MailFailure,
        >,
    ) -> Task<Message> {
        if self
            .mail_cancel
            .as_ref()
            .is_none_or(|(active, _)| *active != generation)
        {
            let stale_payloads = match &result {
                Ok(backend::curl_backend::MailOperationOutput::Inbox(batch)) => {
                    Self::sync_payloads(batch)
                }
                _ => Vec::new(),
            };
            return if stale_payloads.is_empty() {
                Task::none()
            } else {
                self.remove_payloads_task(stale_payloads)
            };
        }
        self.mail_cancel = None;
        let Some(intent) = self.mail_intent.take() else {
            return self.start_next_mail_operation();
        };
        if matches!(&intent, MailIntent::ProbeAccountEdit(_)) {
            return self.complete_edit_authentication(generation, result);
        }
        let discard = self.mail_discard_generation.take() == Some(generation);
        if discard {
            match (&intent, &result) {
                (
                    MailIntent::SubmitOutbox { .. },
                    Ok(backend::curl_backend::MailOperationOutput::Submitted),
                ) => {}
                (MailIntent::SubmitOutbox { .. }, Err(error))
                    if error.kind == backend::curl_backend::MailFailureKind::DeliveryUncertain => {}
                (MailIntent::SubmitOutbox { .. }, _) => {
                    self.mail_notice = None;
                    return self.restore_cancelled_outbox();
                }
                (MailIntent::ProbeAccount(config), _) => {
                    let config = config.clone();
                    self.mail_intent = None;
                    return self.delete_account_credential(generation, &config);
                }
                (MailIntent::SyncMailbox { .. }, _) => {
                    return self.start_next_mail_operation();
                }
                (MailIntent::ReconcileServerMutation { .. }, _) => {
                    self.mail_retry = Some(intent.clone());
                    self.mail_notice = Some(MailNotice {
                        title: crate::l10n::tr("Server mutation reconciliation pending"),
                        body: "Cancellation did not discard the durable mutation. Retry will confirm the recorded UID identity before another command is issued."
                            .to_owned(),
                        retryable: true,
                        can_cancel_outbox: false,
                    });
                    return Task::none();
                }
                (MailIntent::ProbeAccountEdit(_), _) => return Task::none(),
                (MailIntent::SearchServer { .. }, _) => {
                    self.server_search = ServerSearch::Idle;
                    return self.start_next_mail_operation();
                }
            }
        }
        if let MailIntent::SearchServer { query, .. } = &intent {
            let query = query.clone();
            return self.complete_server_search(query, result);
        }
        match result {
            Ok(backend::curl_backend::MailOperationOutput::Authenticated)
                if matches!(&intent, MailIntent::ProbeAccount(_)) =>
            {
                let MailIntent::ProbeAccount(config) = intent else {
                    return Task::none();
                };
                let id = match commit_account_config(
                    &mut self.store,
                    &mut self.next_account_ordinal,
                    config.clone(),
                ) {
                    Ok(id) => id,
                    Err(error) => {
                        self.settings.add_account.error =
                            Some(crate::l10n::tr(error.user_message()));
                        self.mail_notice = Some(MailNotice {
                            title: crate::l10n::tr("Account not added"),
                            body: crate::l10n::tr(error.user_message()),
                            retryable: false,
                            can_cancel_outbox: false,
                        });
                        return self.delete_account_credential(generation, &config);
                    }
                };
                self.store.mark_dirty();
                self.settings.add_account.reset();
                if self.active_account_id.is_empty() {
                    self.active_account_id = id;
                }
                let Some((record_generation, record_account)) = self
                    .credential_mutations
                    .journal()
                    .active()
                    .filter(|record| {
                        record.kind == credential_mutation::CredentialMutationKind::Add
                    })
                    .map(|record| (record.sequence, record.account_id.clone()))
                else {
                    return Task::none();
                };
                if record_account != config.id
                    || !self.credential_mutations.mark_public(record_generation)
                {
                    return Task::none();
                }
                self.mail_notice = Some(MailNotice {
                    title: crate::l10n::tr("Configuration saved; credential reconciliation pending"),
                    body: "Authentication succeeded. The account configuration is being published before the credential journal is cleared."
                        .to_owned(),
                    retryable: false,
                    can_cancel_outbox: false,
                });
                Task::none()
            }
            Ok(backend::curl_backend::MailOperationOutput::Inbox(batch))
                if matches!(
                    &intent,
                    MailIntent::SyncMailbox { config, mailbox, .. }
                        if config.id == batch.account_id && *mailbox == batch.mailbox
                ) =>
            {
                let MailIntent::SyncMailbox {
                    config,
                    refresh_generation,
                    ..
                } = intent
                else {
                    return Task::none();
                };
                if refresh_generation != self.mailbox_sync_generation {
                    return self.start_next_mail_operation();
                }
                self.mailbox_sync_retry_at = None;
                self.mailbox_sync_retry_attempt = 0;
                self.start_inbound_content_staging(generation, refresh_generation, config, batch)
            }
            Ok(backend::curl_backend::MailOperationOutput::Mutation(outcome))
                if matches!(
                    &intent,
                    MailIntent::ReconcileServerMutation { mutation_id, .. }
                        if *mutation_id == outcome.mutation_id
                ) =>
            {
                let Some(mutation) = self
                    .pending_server_mutations
                    .iter()
                    .find(|mutation| mutation.id == outcome.mutation_id)
                    .cloned()
                else {
                    return self.start_next_mail_operation();
                };
                if outcome.disposition
                    == backend::curl_backend::MutationDisposition::DestinationMissing
                {
                    return self.restore_unplaceable_move(&intent, &mutation);
                }
                let stale_identity = outcome.disposition
                    == backend::curl_backend::MutationDisposition::StaleIdentity;
                // A moved message keeps its local copy in the destination until
                // that mailbox's sync below replaces it with the server's copy.
                let remove_ids = if stale_identity
                    || matches!(
                        &mutation.kind,
                        local_state::ServerMutationKind::EmptyMailbox(_)
                    ) {
                    mutation.target_ids.iter().cloned().collect::<HashSet<_>>()
                } else {
                    outcome
                        .missing_target_ids
                        .iter()
                        .cloned()
                        .collect::<HashSet<_>>()
                };
                if !remove_ids.is_empty() {
                    self.store
                        .messages
                        .retain(|message| !remove_ids.contains(&message.id));
                    self.store.mark_dirty();
                    self.reconcile_after_mutation();
                }
                self.pending_server_mutations
                    .retain(|pending| pending.id != outcome.mutation_id);
                self.mail_retry = None;
                self.mail_notice = stale_identity.then(|| MailNotice {
                    title: crate::l10n::tr("Mailbox identity changed"),
                    body: "The saved UIDVALIDITY no longer matches the server. No mutation command was sent; stale local messages were removed for synchronization."
                        .to_owned(),
                    retryable: false,
                    can_cancel_outbox: false,
                });
                let config = intent.config().clone();
                let mut mailboxes = mutation
                    .server_targets
                    .first()
                    .map(|target| vec![target.mailbox])
                    .unwrap_or_default();
                if let local_state::ServerMutationKind::Move(destination) = mutation.kind
                    && !mailboxes.contains(&destination)
                {
                    mailboxes.push(destination);
                }
                Task::batch(
                    mailboxes
                        .into_iter()
                        .map(|mailbox| self.queue_mailbox_sync(config.clone(), mailbox)),
                )
            }
            Ok(backend::curl_backend::MailOperationOutput::Submitted)
                if matches!(&intent, MailIntent::SubmitOutbox { .. }) =>
            {
                let MailIntent::SubmitOutbox { pending_id, .. } = &intent else {
                    return Task::none();
                };
                if self
                    .pending_send
                    .as_ref()
                    .is_none_or(|pending| pending.id != *pending_id)
                {
                    self.mail_notice = Some(MailNotice {
                        title: crate::l10n::tr("Mail completion ignored"),
                        body: "The accepted message no longer matches the current Outbox item."
                            .to_owned(),
                        retryable: false,
                        can_cancel_outbox: false,
                    });
                    return self.start_next_mail_operation();
                }
                if self.commit_pending_send() {
                    self.mail_retry = None;
                    self.mail_notice = Some(MailNotice {
                        title: crate::l10n::tr("Message sent"),
                        body: crate::l10n::tr("The SMTP server accepted the message."),
                        retryable: false,
                        can_cancel_outbox: false,
                    });
                } else {
                    self.mail_notice = Some(MailNotice {
                        title: crate::l10n::tr("Message accepted; local copy pending"),
                        body: crate::l10n::tr(
                            "The server accepted the message, but the local Sent copy still needs a valid local timestamp.",
                        ),
                        retryable: false,
                        can_cancel_outbox: false,
                    });
                }
                self.start_next_mail_operation()
            }
            Ok(_) => {
                self.mail_notice = Some(MailNotice {
                    title: crate::l10n::tr("Mail protocol mismatch"),
                    body: crate::l10n::tr("The mail worker returned an unexpected completion."),
                    retryable: true,
                    can_cancel_outbox: matches!(&intent, MailIntent::SubmitOutbox { .. }),
                });
                if matches!(&intent, MailIntent::SyncMailbox { .. }) {
                    self.retain_mailbox_sync_retry(intent, Instant::now());
                } else {
                    self.mail_retry = Some(intent);
                }
                Task::none()
            }
            Err(error) => {
                if matches!(&intent, MailIntent::ProbeAccount(_)) {
                    self.settings.add_account.error = Some(crate::l10n::tr(error.user_message()));
                }
                if let MailIntent::SyncMailbox {
                    refresh_generation, ..
                } = &intent
                {
                    if *refresh_generation != self.mailbox_sync_generation {
                        return self.start_next_mail_operation();
                    }
                    let progress =
                        error
                            .sync_progress()
                            .map_or_else(String::new, |(done, total)| {
                                crate::l10n::message("mail-notice-the-bounded-fetch-stopped-after-done-of-total-messages", &[("done", crate::l10n::number(done)), ("total", crate::l10n::number(total))])
                            });
                    let automatic = matches!(
                        error.kind,
                        backend::curl_backend::MailFailureKind::Busy
                            | backend::curl_backend::MailFailureKind::Network
                            | backend::curl_backend::MailFailureKind::Timeout
                            | backend::curl_backend::MailFailureKind::Worker
                    );
                    let continuation = if automatic {
                        "a bounded retry is scheduled"
                    } else {
                        "Refresh can retry the bounded synchronization"
                    };
                    self.mail_notice = Some(MailNotice {
                        title: crate::l10n::tr("Mailbox synchronization paused"),
                        body: crate::l10n::message(
                            "mail-notice-the-last-good-mailbox-remains-visible-continuation",
                            &[
                                ("value1", (error.user_message()).to_owned()),
                                ("value2", (progress)),
                                ("continuation", (continuation).to_owned()),
                            ],
                        ),
                        retryable: true,
                        can_cancel_outbox: false,
                    });
                    if automatic {
                        self.retain_mailbox_sync_retry(intent, Instant::now());
                    } else {
                        self.mailbox_sync_retry = Some(intent);
                        self.mailbox_sync_retry_at = None;
                    }
                    return Task::none();
                }
                let retry_when_owner_drains =
                    error.kind == backend::curl_backend::MailFailureKind::Busy;
                let uncertain = matches!(
                    error.kind,
                    backend::curl_backend::MailFailureKind::DeliveryUncertain
                        | backend::curl_backend::MailFailureKind::MutationUncertain
                );
                self.mail_notice = Some(if uncertain {
                    // The durable Outbox or IMAP journal remains authoritative
                    // when cancellation or transport failure races the wire.
                    MailNotice {
                        title: if matches!(&intent, MailIntent::SubmitOutbox { .. }) {
                            crate::l10n::tr("Delivery not confirmed")
                        } else {
                            crate::l10n::tr("Server change not confirmed")
                        },
                        body: crate::l10n::tr(error.user_message()),
                        retryable: true,
                        can_cancel_outbox: matches!(&intent, MailIntent::SubmitOutbox { .. }),
                    }
                } else {
                    MailNotice {
                        title: crate::l10n::message(
                            "mail-notice-could-not-complete",
                            &[("value1", (intent.label()).to_owned())],
                        ),
                        body: crate::l10n::tr(error.user_message()),
                        retryable: error.kind != backend::curl_backend::MailFailureKind::Cancelled,
                        can_cancel_outbox: matches!(&intent, MailIntent::SubmitOutbox { .. }),
                    }
                });
                self.mail_retry = Some(intent);
                // `mail_busy` is the auto-retry gate for `Busy` alone; every
                // other failure, including uncertain delivery, waits for an
                // explicit user Retry.
                self.mail_busy = retry_when_owner_drains;
                Task::none()
            }
        }
    }
}
