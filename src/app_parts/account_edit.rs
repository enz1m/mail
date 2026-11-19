use super::*;

impl State {
    fn edit_failure_text(outcome: &CredentialOutcome) -> &'static str {
        match outcome {
            CredentialOutcome::NotFound => "The existing account credential was not found.",
            CredentialOutcome::Unavailable => "The system credential service is unavailable.",
            CredentialOutcome::PermissionDenied => {
                "The system credential service denied the account update."
            }
            CredentialOutcome::UnlockCancelled => "Credential unlocking was cancelled.",
            CredentialOutcome::Cancelled => "The account update was cancelled.",
            CredentialOutcome::Saturated => {
                "The credential service is busy. Retry the account update."
            }
            CredentialOutcome::OwnerShutdown => "The credential service is shutting down.",
            CredentialOutcome::MutationUncertain => {
                "The credential change is uncertain and requires reconciliation."
            }
            CredentialOutcome::Failed(_) => "The credential operation failed.",
            CredentialOutcome::Retrieved(_)
            | CredentialOutcome::Stored
            | CredentialOutcome::Deleted => {
                "The credential operation returned an unexpected result."
            }
        }
    }

    pub(crate) fn begin_account_edit(&mut self, account_id: &str) -> Task<Message> {
        if self.account_edit.is_some() || self.account_removal.is_some() {
            return Task::none();
        }
        let Some(config) = self
            .store
            .account(account_id)
            .and_then(|account| account.config.as_ref())
        else {
            return Task::none();
        };
        self.settings.edit_account.begin(config);
        Task::none()
    }

    pub(crate) fn account_edit_has_active_work(&self) -> bool {
        self.account_edit.as_ref().is_some_and(|edit| {
            !matches!(
                &edit.phase,
                AccountEditPhase::Failed(_) | AccountEditPhase::OldCleanupFailed(_)
            )
        })
    }

    pub(crate) fn account_edit_status(
        &self,
        account_id: &str,
    ) -> Option<(&'static str, &str, bool)> {
        let edit = self
            .account_edit
            .as_ref()
            .filter(|edit| edit.old_account.id == account_id)?;
        match &edit.phase {
            AccountEditPhase::AwaitingJournal(_) => Some((
                "Preparing account update",
                "Saving the exact old and new credential keys before contacting the keyring.",
                false,
            )),
            AccountEditPhase::LoadingOld(_)
            | AccountEditPhase::StoringCandidate(_)
            | AccountEditPhase::LoadingCandidate(_)
            | AccountEditPhase::AuthenticatingStoredCandidate(_)
            | AccountEditPhase::AuthenticatingBeforeStore(_)
            | AccountEditPhase::StoringAuthenticatedCandidate(_) => Some((
                "Checking account changes",
                "The candidate credential and incoming server are being verified.",
                false,
            )),
            AccountEditPhase::Publishing { .. } => Some((
                "Saving account changes",
                "The authenticated account metadata is being published before old credential cleanup.",
                false,
            )),
            AccountEditPhase::DeletingCandidate { .. }
            | AccountEditPhase::RestoringOld { .. }
            | AccountEditPhase::VerifyingCandidateDeleted { .. }
            | AccountEditPhase::VerifyingRestoredOld { .. } => Some((
                "Keeping existing account",
                "The rejected candidate credential is being reconciled against the exact recorded key.",
                false,
            )),
            AccountEditPhase::DeletingOld(_) | AccountEditPhase::VerifyingOldDeleted(_) => Some((
                "Configuration saved; credential cleanup pending",
                "The updated account is active; absence of the exact old credential is being confirmed.",
                false,
            )),
            AccountEditPhase::Failed(message) => {
                Some(("Account not updated", message.as_str(), true))
            }
            AccountEditPhase::OldCleanupFailed(message) => {
                Some(("Old credential not removed", message.as_str(), true))
            }
        }
    }

    pub(crate) fn start_account_edit(&mut self) -> Task<Message> {
        if self.account_edit.is_some() {
            return Task::none();
        }
        let Some(account_id) = self.settings.edit_account.account_id.clone() else {
            return Task::none();
        };
        let has_pending_server_work = self.pending_server_mutations.iter().any(|mutation| {
            mutation
                .server_targets
                .iter()
                .any(|target| target.account_id == account_id)
        });
        if self.account_removal.is_some()
            || self.credential_inflight.is_some()
            || self.mail_cancel.is_some()
            || self.pending_sync_adoption.is_some()
            || self.content_sync_inflight.is_some()
            || !self.credential_store_ready
            || self.credential_mutations.journal().active().is_some()
            || has_pending_server_work
        {
            self.settings.edit_account.error = Some(
                "Reconcile pending server, credential, or mail work before updating the account."
                    .to_owned(),
            );
            return Task::none();
        }
        let Some(old_account) = self.store.account(&account_id).cloned() else {
            self.settings.edit_account.reset();
            return Task::none();
        };
        let Some(old_config) = old_account.config.as_ref() else {
            self.settings.edit_account.reset();
            return Task::none();
        };
        let mut candidate = match self
            .settings
            .edit_account
            .draft
            .validate(account_id.clone())
        {
            Ok(candidate) => candidate,
            Err(error) => {
                self.settings.edit_account.error = Some(error.message.to_owned());
                return Task::none();
            }
        };
        candidate
            .credential_owner
            .clone_from(&old_config.credential_owner);
        let password_changed = !self.settings.edit_account.password.is_empty();
        let config_changed = &candidate != old_config;
        if !config_changed && !password_changed {
            self.settings.edit_account.reset();
            return Task::none();
        }
        if let Err(error) = self.mail_backend.validate_configuration(&candidate) {
            self.settings.edit_account.error = Some(crate::l10n::tr(error.user_message()));
            return Task::none();
        }
        // An OAuth account's credential is its stored grant; a typed
        // password would replace it with something XOAUTH2 cannot use.
        if password_changed && candidate.auth == backend::AuthMethod::OAuth2 {
            self.settings.edit_account.error = Some(
                "This account signs in through the browser. Remove it and add it again to sign in anew."
                    .to_owned(),
            );
            return Task::none();
        }
        if self.settings.edit_account.password.len() > backend::curl_backend::MAX_CREDENTIAL_BYTES {
            self.settings.edit_account.error = Some(crate::l10n::tr(
                "The account password exceeds the supported size.",
            ));
            return Task::none();
        }
        let old_key = Self::credential_key(old_config);
        let referenced = self.credential_keys_in_public_state();
        let reservation = self.credential_mutations.reserve(
            credential_mutation::CredentialMutationKind::Edit,
            account_id.clone(),
            config_changed,
            &referenced,
            |sequence| {
                if config_changed {
                    candidate.credential_owner = format!("{account_id}-credential-{sequence}");
                }
                credential_mutation::TransitionKeys {
                    old_key: Some(old_key.clone()),
                    new_key: Some(Self::credential_key(&candidate)),
                }
            },
        );
        let generation = match reservation {
            Ok(reservation) => reservation.sequence,
            Err(message) => {
                self.settings.edit_account.error = Some(message.to_owned());
                return Task::none();
            }
        };
        let new_key = Self::credential_key(&candidate);
        let candidate_secret = if password_changed {
            Some(Secret::new(
                std::mem::take(&mut self.settings.edit_account.password).into_bytes(),
            ))
        } else {
            None
        };
        self.settings.edit_account.password_revealed = false;
        self.account_edit = Some(AccountEdit {
            old_account,
            old_key: old_key.clone(),
            candidate,
            new_key,
            old_secret: None,
            candidate_secret,
            phase: AccountEditPhase::AwaitingJournal(generation),
        });
        self.settings.edit_account.error = Some(crate::l10n::tr(
            "Saving the credential transition before contacting the keyring…",
        ));
        Task::none()
    }

    fn delete_edit_candidate(&mut self, generation: u64, failure: String) -> Task<Message> {
        let Some(edit) = self.account_edit.as_mut() else {
            return Task::none();
        };
        let operation = if edit.old_key == edit.new_key {
            let Some(old_secret) = edit.old_secret.as_ref() else {
                edit.phase = AccountEditPhase::Failed(crate::l10n::tr(
                    "The previous credential could not be restored.",
                ));
                return Task::none();
            };
            edit.phase = AccountEditPhase::RestoringOld {
                generation,
                failure,
            };
            self.credential_store.0.set(
                edit.old_key.clone(),
                Secret::new(old_secret.expose_secret().to_vec()),
            )
        } else {
            edit.phase = AccountEditPhase::DeletingCandidate {
                generation,
                failure,
            };
            self.credential_store.0.delete(edit.new_key.clone())
        };
        let stage = if edit.old_key == edit.new_key {
            CredentialStage::RestoreOldAfterEdit
        } else {
            CredentialStage::DeleteEditCandidate
        };
        self.credential_task(generation, stage, operation)
    }

    fn start_edit_probe(
        &mut self,
        generation: u64,
        authenticate_before_store: bool,
        secret: Secret,
    ) -> Task<Message> {
        let Some(edit) = self.account_edit.as_mut() else {
            return Task::none();
        };
        let config = edit.candidate.clone();
        edit.phase = if authenticate_before_store {
            AccountEditPhase::AuthenticatingBeforeStore(generation)
        } else {
            AccountEditPhase::AuthenticatingStoredCandidate(generation)
        };
        self.mail_intent = Some(MailIntent::ProbeAccountEdit(config.clone()));
        let operation = self.mail_backend.probe(
            generation,
            backend::curl_backend::SyncRequest {
                config,
                secret,
                known: Vec::new(),
            },
        );
        self.mail_cancel = Some((generation, operation.cancel_handle()));
        Task::perform(operation.finish(), |(generation, result)| {
            Message::MailOperationFinished(generation, result)
        })
    }

    pub(crate) fn complete_edit_credential(
        &mut self,
        generation: u64,
        stage: CredentialStage,
        outcome: CredentialOutcome,
    ) -> Task<Message> {
        let phase_matches = self.account_edit.as_ref().is_some_and(|edit| {
            matches!(
                (&edit.phase, stage),
                (AccountEditPhase::LoadingOld(active), CredentialStage::LoadOldForEdit)
                    | (AccountEditPhase::StoringCandidate(active), CredentialStage::StoreEditCandidate)
                    | (AccountEditPhase::StoringAuthenticatedCandidate(active), CredentialStage::StoreAuthenticatedEdit)
                    | (AccountEditPhase::LoadingCandidate(active), CredentialStage::LoadEditCandidate)
                    | (AccountEditPhase::DeletingCandidate { generation: active, .. }, CredentialStage::DeleteEditCandidate)
                    | (AccountEditPhase::RestoringOld { generation: active, .. }, CredentialStage::RestoreOldAfterEdit)
                    | (AccountEditPhase::VerifyingCandidateDeleted { generation: active, .. }, CredentialStage::VerifyEditCandidateDeleted)
                    | (AccountEditPhase::VerifyingRestoredOld { generation: active, .. }, CredentialStage::VerifyRestoredOldAfterEdit)
                    | (AccountEditPhase::DeletingOld(active), CredentialStage::DeleteOldAfterEdit)
                    | (AccountEditPhase::VerifyingOldDeleted(active), CredentialStage::VerifyOldDeletedAfterEdit)
                    if *active == generation
            )
        });
        if !phase_matches {
            return Task::none();
        }
        match stage {
            CredentialStage::LoadOldForEdit => match outcome {
                CredentialOutcome::Retrieved(old_secret) => {
                    let Some(edit) = self.account_edit.as_mut() else {
                        return Task::none();
                    };
                    edit.old_secret = Some(Secret::new(old_secret.expose_secret().to_vec()));
                    if edit.candidate_secret.is_none() {
                        edit.candidate_secret =
                            Some(Secret::new(old_secret.expose_secret().to_vec()));
                    }
                    let Some(candidate_secret) = edit.candidate_secret.as_ref() else {
                        return Task::none();
                    };
                    let candidate_secret = Secret::new(candidate_secret.expose_secret().to_vec());
                    if edit.old_key == edit.new_key {
                        return self.start_edit_probe(generation, true, candidate_secret);
                    }
                    let operation = self
                        .credential_store
                        .0
                        .set(edit.new_key.clone(), candidate_secret);
                    edit.phase = AccountEditPhase::StoringCandidate(generation);
                    self.credential_task(generation, CredentialStage::StoreEditCandidate, operation)
                }
                outcome => {
                    let message = Self::edit_failure_text(&outcome).to_owned();
                    self.account_edit = None;
                    self.settings.edit_account.error = Some(message);
                    self.finish_credential_transition(generation)
                }
            },
            CredentialStage::StoreEditCandidate => match outcome {
                CredentialOutcome::Stored => {
                    let Some(edit) = self.account_edit.as_mut() else {
                        return Task::none();
                    };
                    let operation = self.credential_store.0.get(edit.new_key.clone());
                    edit.phase = AccountEditPhase::LoadingCandidate(generation);
                    self.credential_task(generation, CredentialStage::LoadEditCandidate, operation)
                }
                outcome => self.delete_edit_candidate(
                    generation,
                    Self::edit_failure_text(&outcome).to_owned(),
                ),
            },
            CredentialStage::StoreAuthenticatedEdit => match outcome {
                CredentialOutcome::Stored => self.publish_authenticated_edit(generation),
                outcome => self.delete_edit_candidate(
                    generation,
                    Self::edit_failure_text(&outcome).to_owned(),
                ),
            },
            CredentialStage::LoadEditCandidate => match outcome {
                CredentialOutcome::Retrieved(secret) => {
                    self.start_edit_probe(generation, false, secret)
                }
                outcome => self.delete_edit_candidate(
                    generation,
                    Self::edit_failure_text(&outcome).to_owned(),
                ),
            },
            CredentialStage::DeleteEditCandidate | CredentialStage::RestoreOldAfterEdit => {
                let Some(edit) = self.account_edit.as_mut() else {
                    return Task::none();
                };
                let (key, failure, verify_stage) = match &edit.phase {
                    AccountEditPhase::DeletingCandidate { failure, .. } => (
                        edit.new_key.clone(),
                        failure.clone(),
                        CredentialStage::VerifyEditCandidateDeleted,
                    ),
                    AccountEditPhase::RestoringOld { failure, .. } => (
                        edit.old_key.clone(),
                        failure.clone(),
                        CredentialStage::VerifyRestoredOldAfterEdit,
                    ),
                    _ => return Task::none(),
                };
                edit.phase = match verify_stage {
                    CredentialStage::VerifyEditCandidateDeleted => {
                        AccountEditPhase::VerifyingCandidateDeleted {
                            generation,
                            failure,
                        }
                    }
                    CredentialStage::VerifyRestoredOldAfterEdit => {
                        AccountEditPhase::VerifyingRestoredOld {
                            generation,
                            failure,
                        }
                    }
                    _ => return Task::none(),
                };
                let operation = self.credential_store.0.get(key);
                self.credential_task(generation, verify_stage, operation)
            }
            CredentialStage::VerifyEditCandidateDeleted
            | CredentialStage::VerifyRestoredOldAfterEdit => {
                let cleanup_confirmed = matches!(
                    (&stage, &outcome),
                    (
                        CredentialStage::VerifyEditCandidateDeleted,
                        CredentialOutcome::NotFound
                    ) | (
                        CredentialStage::VerifyRestoredOldAfterEdit,
                        CredentialOutcome::Retrieved(_)
                    )
                );
                let Some(edit) = self.account_edit.as_mut() else {
                    return Task::none();
                };
                let mut message = match &edit.phase {
                    AccountEditPhase::VerifyingCandidateDeleted { failure, .. }
                    | AccountEditPhase::VerifyingRestoredOld { failure, .. } => failure.clone(),
                    _ => return Task::none(),
                };
                if cleanup_confirmed {
                    self.account_edit = None;
                    self.settings.edit_account.error = Some(message);
                    return self.finish_credential_transition(generation);
                }
                message.push_str(" Credential rollback or cleanup was not confirmed.");
                edit.phase = AccountEditPhase::Failed(message.clone());
                self.settings.edit_account.error = Some(message);
                Task::none()
            }
            CredentialStage::DeleteOldAfterEdit => {
                let Some(edit) = self.account_edit.as_mut() else {
                    return Task::none();
                };
                let old_key = edit.old_key.clone();
                edit.phase = AccountEditPhase::VerifyingOldDeleted(generation);
                let operation = self.credential_store.0.get(old_key);
                self.credential_task(
                    generation,
                    CredentialStage::VerifyOldDeletedAfterEdit,
                    operation,
                )
            }
            CredentialStage::VerifyOldDeletedAfterEdit => {
                if matches!(outcome, CredentialOutcome::NotFound) {
                    let config = self
                        .account_edit
                        .as_ref()
                        .map(|edit| edit.candidate.clone());
                    let current_matches = config.as_ref().is_some_and(|candidate| {
                        self.store
                            .account(&candidate.id)
                            .and_then(|account| account.config.as_ref())
                            == Some(candidate)
                    });
                    self.account_edit = None;
                    self.settings.edit_account.reset();
                    if !current_matches {
                        self.mail_notice = Some(MailNotice {
                            title: crate::l10n::tr("Account update completion ignored"),
                            body: "The account changed while old credential cleanup was settling."
                                .to_owned(),
                            retryable: false,
                            can_cancel_outbox: false,
                        });
                        return Task::none();
                    }
                    self.finish_credential_transition(generation)
                } else {
                    let message = crate::l10n::tr(
                        "The updated account configuration is saved, but absence of the exact old credential was not confirmed.",
                    );
                    if let Some(edit) = self.account_edit.as_mut() {
                        edit.phase = AccountEditPhase::OldCleanupFailed(message.clone());
                    }
                    self.settings.edit_account.error = Some(message.clone());
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

    fn edit_snapshot_is_current(&self) -> bool {
        self.account_edit.as_ref().is_some_and(|edit| {
            self.store
                .account(&edit.old_account.id)
                .is_some_and(|current| current == &edit.old_account)
        })
    }

    fn publish_authenticated_edit(&mut self, generation: u64) -> Task<Message> {
        let journal_config_changed = self
            .credential_mutations
            .journal()
            .active()
            .filter(|record| {
                record.kind == credential_mutation::CredentialMutationKind::Edit
                    && record.sequence == generation
            })
            .map(|record| record.config_changed);
        let phase_matches = self.account_edit.as_ref().is_some_and(|edit| {
            matches!(
                &edit.phase,
                AccountEditPhase::AuthenticatingStoredCandidate(active)
                    | AccountEditPhase::StoringAuthenticatedCandidate(active)
                    if *active == generation
            )
        });
        if !phase_matches || !self.edit_snapshot_is_current() {
            return self.delete_edit_candidate(
                generation,
                crate::l10n::tr("The account changed while authentication was settling."),
            );
        }
        let Some(edit) = self.account_edit.as_ref() else {
            return Task::none();
        };
        let account_id = edit.old_account.id.clone();
        let candidate = edit.candidate.clone();
        let config_changed = edit
            .old_account
            .config
            .as_ref()
            .is_some_and(|old| old != &candidate);
        if journal_config_changed != Some(config_changed) {
            return self.delete_edit_candidate(
                generation,
                "The recorded account-update shape did not match the authenticated candidate."
                    .to_owned(),
            );
        }
        if config_changed {
            let connection_changed = edit
                .old_account
                .config
                .as_ref()
                .is_some_and(|old| old.incoming != candidate.incoming);
            if connection_changed {
                let prefix = format!("imap-{account_id}-");
                self.store.messages.retain(|message| {
                    message.account_id != account_id || !message.id.starts_with(&prefix)
                });
            }
            let Some(account) = self
                .store
                .accounts
                .iter_mut()
                .find(|account| account.id == account_id)
            else {
                return self.delete_edit_candidate(
                    generation,
                    crate::l10n::tr("The account was removed while authentication was settling."),
                );
            };
            account.display_name.clone_from(&candidate.display_name);
            account.email.clone_from(&candidate.email);
            account.config = Some(candidate);
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
            self.store.mark_dirty();
            if !self.credential_mutations.mark_public(generation) {
                return Task::none();
            }
            self.reconcile_after_mutation();
        }
        let local = self.local_state_token();
        let preferences = self.preferences_token();
        if let Some(edit) = self.account_edit.as_mut() {
            edit.candidate_secret = None;
            edit.old_secret = None;
            edit.phase = AccountEditPhase::Publishing { local, preferences };
        }
        self.settings.edit_account.error = if config_changed {
            Some(crate::l10n::tr("Saving the updated account…"))
        } else {
            Some(crate::l10n::tr(
                "Confirming the password-only credential update…",
            ))
        };
        if config_changed {
            Task::none()
        } else {
            self.continue_account_edit_publication()
        }
    }

    pub(crate) fn complete_edit_authentication(
        &mut self,
        generation: u64,
        result: Result<
            backend::curl_backend::MailOperationOutput,
            backend::curl_backend::MailFailure,
        >,
    ) -> Task<Message> {
        let authenticate_before_store =
            self.account_edit
                .as_ref()
                .and_then(|edit| match &edit.phase {
                    AccountEditPhase::AuthenticatingBeforeStore(active)
                        if *active == generation =>
                    {
                        Some(true)
                    }
                    AccountEditPhase::AuthenticatingStoredCandidate(active)
                        if *active == generation =>
                    {
                        Some(false)
                    }
                    _ => None,
                });
        let Some(authenticate_before_store) = authenticate_before_store else {
            return Task::none();
        };
        match result {
            Ok(backend::curl_backend::MailOperationOutput::Authenticated)
                if authenticate_before_store =>
            {
                if !self.edit_snapshot_is_current() {
                    if let Some(edit) = self.account_edit.as_mut() {
                        let message = crate::l10n::tr(
                            "The account changed while authentication was settling.",
                        );
                        edit.phase = AccountEditPhase::Failed(message.clone());
                        self.settings.edit_account.error = Some(message);
                    }
                    return Task::none();
                }
                let Some(edit) = self.account_edit.as_mut() else {
                    return Task::none();
                };
                let Some(candidate_secret) = edit.candidate_secret.as_ref() else {
                    return Task::none();
                };
                let operation = self.credential_store.0.set(
                    edit.new_key.clone(),
                    Secret::new(candidate_secret.expose_secret().to_vec()),
                );
                edit.phase = AccountEditPhase::StoringAuthenticatedCandidate(generation);
                self.credential_task(
                    generation,
                    CredentialStage::StoreAuthenticatedEdit,
                    operation,
                )
            }
            Ok(backend::curl_backend::MailOperationOutput::Authenticated) => {
                self.publish_authenticated_edit(generation)
            }
            Err(error) if authenticate_before_store => {
                let message = crate::l10n::tr(error.user_message());
                self.account_edit = None;
                self.settings.edit_account.error = Some(message);
                self.finish_credential_transition(generation)
            }
            Err(error) => {
                self.delete_edit_candidate(generation, crate::l10n::tr(error.user_message()))
            }
            Ok(_) if authenticate_before_store => {
                let message = crate::l10n::tr(
                    "The mail backend returned an unexpected authentication result.",
                );
                self.account_edit = None;
                self.settings.edit_account.error = Some(message);
                self.finish_credential_transition(generation)
            }
            Ok(_) => self.delete_edit_candidate(
                generation,
                crate::l10n::tr("The mail backend returned an unexpected authentication result."),
            ),
        }
    }

    pub(crate) fn continue_account_edit_publication(&mut self) -> Task<Message> {
        let ready = self.account_edit.as_ref().is_some_and(|edit| {
            matches!(
                &edit.phase,
                AccountEditPhase::Publishing { local, preferences }
                    if local == &self.local_state_published
                        && preferences == &self.preferences_published
            )
        });
        if !ready {
            return Task::none();
        }
        let Some(edit) = self.account_edit.as_ref() else {
            return Task::none();
        };
        if edit.old_key == edit.new_key {
            let config = edit.candidate.clone();
            let current_matches = self
                .store
                .account(&config.id)
                .and_then(|account| account.config.as_ref())
                == Some(&config);
            self.account_edit = None;
            self.settings.edit_account.reset();
            if !current_matches {
                self.mail_notice = Some(MailNotice {
                    title: crate::l10n::tr("Account update completion ignored"),
                    body: crate::l10n::tr("The account changed while publication was settling."),
                    retryable: false,
                    can_cancel_outbox: false,
                });
                return Task::none();
            }
            self.mail_notice = Some(MailNotice {
                title: crate::l10n::tr("Account updated"),
                body: crate::l10n::tr("The updated account and credential are active."),
                retryable: false,
                can_cancel_outbox: false,
            });
            let Some(generation) = self
                .credential_mutations
                .journal()
                .active()
                .map(|record| record.sequence)
            else {
                return Task::none();
            };
            return self.finish_credential_transition(generation);
        }
        let old_key = edit.old_key.clone();
        let Some(generation) = self
            .credential_mutations
            .journal()
            .active()
            .map(|record| record.sequence)
        else {
            return Task::none();
        };
        if let Some(edit) = self.account_edit.as_mut() {
            edit.phase = AccountEditPhase::DeletingOld(generation);
        }
        let operation = self.credential_store.0.delete(old_key);
        self.credential_task(generation, CredentialStage::DeleteOldAfterEdit, operation)
    }

    pub(crate) fn retry_account_edit(&mut self) -> Task<Message> {
        let Some(edit) = self.account_edit.as_ref() else {
            return self.start_account_edit();
        };
        let phase = edit.phase.clone();
        let old_account = edit.old_account.clone();
        match phase {
            AccountEditPhase::Failed(_) => {
                if self.credential_inflight.is_some() || self.mail_cancel.is_some() {
                    return Task::none();
                }
                if self
                    .store
                    .account(&old_account.id)
                    .is_none_or(|current| current != &old_account)
                {
                    self.account_edit = None;
                    self.settings.edit_account.error =
                        Some(crate::l10n::tr("The account changed; reopen Edit Account."));
                    return self.continue_credential_mutation();
                }
                if self.credential_mutations.journal().active().is_some() {
                    self.account_edit = None;
                    self.settings.edit_account.error = Some(
                        "Credential state uncertain; reconciling the recorded keys before retry."
                            .to_owned(),
                    );
                    return self.continue_credential_mutation();
                }
                self.start_account_edit()
            }
            AccountEditPhase::Publishing { .. } => Task::none(),
            AccountEditPhase::OldCleanupFailed(_) => self.retry_old_credential_cleanup(),
            _ => Task::none(),
        }
    }

    fn retry_old_credential_cleanup(&mut self) -> Task<Message> {
        let Some(edit) = self.account_edit.as_ref() else {
            return Task::none();
        };
        if !matches!(&edit.phase, AccountEditPhase::OldCleanupFailed(_))
            || self.credential_inflight.is_some()
            || self.mail_cancel.is_some()
        {
            return Task::none();
        }
        let old_key = edit.old_key.clone();
        let Some(generation) = self
            .credential_mutations
            .journal()
            .active()
            .map(|record| record.sequence)
        else {
            return Task::none();
        };
        if let Some(edit) = self.account_edit.as_mut() {
            edit.phase = AccountEditPhase::DeletingOld(generation);
        }
        let operation = self.credential_store.0.delete(old_key);
        self.credential_task(generation, CredentialStage::DeleteOldAfterEdit, operation)
    }
}
