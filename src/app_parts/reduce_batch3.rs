use super::*;

impl State {
    pub(crate) fn reduce_batch3(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::SetAutoDeclineReadReceipts(on) => {
                self.privacy.get_mut().auto_decline_read_receipts = on;
            }
            Message::TogglePrivacyAdvanced(on) => self.settings.privacy_advanced_expanded = on,
            Message::ToggleAllowListExpanded(on) => self.settings.allow_list_expanded = on,
            Message::ToggleBlockListExpanded(on) => self.settings.block_list_expanded = on,
            Message::AllowInputEdited(v) => self.settings.allow_input = v,
            Message::BlockInputEdited(v) => self.settings.block_input = v,
            Message::AddAllowedSender => {
                if self
                    .privacy
                    .get_mut()
                    .allow_sender(&self.settings.allow_input)
                {
                    self.settings.allow_input.clear();
                }
                return self.save_preferences(preferences::SaveOrigin::Privacy);
            }
            Message::AddBlockedSender => {
                if self
                    .privacy
                    .get_mut()
                    .block_sender(&self.settings.block_input)
                {
                    self.settings.block_input.clear();
                }
                return self.save_preferences(preferences::SaveOrigin::Privacy);
            }
            Message::RemoveAllowedSender(addr) => {
                self.privacy.get_mut().remove_allowed(&addr);
                return self.save_preferences(preferences::SaveOrigin::Privacy);
            }
            Message::RemoveBlockedSender(addr) => {
                self.privacy.get_mut().remove_blocked(&addr);
                return self.save_preferences(preferences::SaveOrigin::Privacy);
            }
            Message::ToggleAccountExpanded(id) => {
                if self.settings.edit_account.is_open_for(&id) {
                    if self.account_edit.is_some() {
                        return Task::none();
                    }
                    self.settings.edit_account.reset();
                }
                let now_open = self.settings.toggle_account(&id);
                // Re-seed the signature buffer for the account now open, or clear it, so a
                // keystroke cannot write the previous account's signature onto this one.
                self.sync_signature_editor(now_open.as_deref());
            }
            Message::AccountSignatureAction(id, action) => {
                if self
                    .account_removal
                    .as_ref()
                    .is_some_and(|removal| removal.account.id == id)
                    || self
                        .settings
                        .edit_account
                        .account_id
                        .as_deref()
                        .is_some_and(|editing| editing == id)
                {
                    return Task::none();
                }
                let is_edit = action.is_edit();
                self.signature_editor.perform(action);
                if is_edit {
                    let text = self.signature_editor.text();
                    if let Some(acct) = self.store.accounts.iter_mut().find(|a| a.id == id)
                        && acct.signature != text
                    {
                        acct.signature = text;
                        self.store.mark_dirty();
                    }
                }
            }
            Message::SetAccountAppendSignature(id, on) => {
                if self
                    .account_removal
                    .as_ref()
                    .is_some_and(|removal| removal.account.id == id)
                    || self
                        .settings
                        .edit_account
                        .account_id
                        .as_deref()
                        .is_some_and(|editing| editing == id)
                {
                    return Task::none();
                }
                if let Some(acct) = self.store.accounts.iter_mut().find(|a| a.id == id)
                    && acct.append_signature != on
                {
                    acct.append_signature = on;
                    self.store.mark_dirty();
                }
            }
            Message::BeginAddAccount => {
                self.settings.select_tab(SettingsTab::Accounts);
                self.settings.add_account.begin();
                self.end_oauth_sign_in();
            }
            Message::AddAccountOAuthSignIn => return self.start_oauth_sign_in(),
            Message::AddAccountOAuthCancel => {
                if let Some((_, cancel, _)) = &self.oauth_sign_in {
                    cancel.cancel();
                }
            }
            Message::AddAccountOAuthFinished(generation, handoff) => {
                if self
                    .oauth_sign_in
                    .as_ref()
                    .is_none_or(|(live, _, _)| *live != generation)
                {
                    // A superseded sign-in; its grant (if any) is dropped.
                    return Task::none();
                }
                let Some((_, _, binding)) = self.oauth_sign_in.take() else {
                    return Task::none();
                };
                let provider = self
                    .settings
                    .add_account
                    .oauth_provider()
                    .map_or("the provider", |p| p.label());
                self.settings.add_account.oauth = match handoff.take() {
                    Some(Ok(grant)) => {
                        self.oauth_grant = Some((binding, grant));
                        OAuthSignInStatus::SignedIn { provider }
                    }
                    Some(Err(backend::curl_backend::oauth::SignInError::Cancelled)) | None => {
                        OAuthSignInStatus::NotStarted
                    }
                    Some(Err(error)) => OAuthSignInStatus::Failed(error.user_message()),
                };
            }
            Message::CancelAddAccount => {
                self.settings.add_account.reset();
                self.end_oauth_sign_in();
                if self.credential_secret.take().is_some()
                    && matches!(self.mail_intent, Some(MailIntent::ProbeAccount(_)))
                {
                    self.mail_intent = None;
                    if let Some(generation) = self
                        .credential_mutations
                        .journal()
                        .active()
                        .map(|record| record.sequence)
                    {
                        return self.finish_credential_transition(generation);
                    }
                }
                if matches!(self.mail_intent, Some(MailIntent::ProbeAccount(_))) {
                    if let Some((generation, cancel, _)) = &self.credential_inflight {
                        cancel.cancel();
                        self.mail_discard_generation = Some(*generation);
                    }
                    if let Some((generation, cancel)) = &self.mail_cancel {
                        cancel.cancel();
                        self.mail_discard_generation = Some(*generation);
                    }
                } else if matches!(self.mail_retry, Some(MailIntent::ProbeAccount(_))) {
                    let Some(generation) = self.advance_mail_generation() else {
                        return self.refuse_exhausted_mail_generation();
                    };
                    if let Some(MailIntent::ProbeAccount(config)) = self.mail_retry.take() {
                        self.mail_notice = None;
                        return self.delete_account_credential(generation, &config);
                    }
                }
            }
            Message::AddAccountEmailEdited(v) => {
                self.settings.add_account.email = v;
                self.settings.add_account.error = None;
            }
            // A sign-in belongs to the address, server and authentication it
            // started with; any step that can change them ends it.
            Message::AddAccountNameEdited(v) => self.settings.add_account.name = v,
            Message::AddAccountContinue => {
                self.end_oauth_sign_in();
                let add = &mut self.settings.add_account;
                if add.continue_to_configure() && add.detected_preset().is_none() {
                    return self.look_up_servers();
                }
            }
            Message::AddAccountServersFound(generation, found) => {
                self.settings.add_account.finish_lookup(generation, found);
            }
            Message::AddAccountBack => {
                self.end_oauth_sign_in();
                self.settings.add_account.back_to_email();
            }
            Message::AddAccountFieldEdited(field, v) => {
                if matches!(
                    field,
                    DraftField::IncomingServer | DraftField::IncomingUsername
                ) {
                    self.end_oauth_sign_in();
                }
                self.settings.add_account.set_field(field, v);
            }
            Message::AddAccountPasswordEdited(v) => self.settings.add_account.password = v,
            Message::ToggleAddAccountPasswordRevealed => {
                let a = &mut self.settings.add_account;
                a.password_revealed = !a.password_revealed;
            }
            Message::SetAddAccountIncomingEncryption(e) => {
                self.settings.add_account.draft.set_imap_encryption(e);
            }
            Message::SetAddAccountSmtpEncryption(e) => {
                self.settings.add_account.draft.set_smtp_encryption(e);
            }
            Message::SetAddAccountAuth(a) => {
                if a != self.settings.add_account.draft.auth {
                    self.end_oauth_sign_in();
                }
                self.settings.add_account.draft.auth = a;
            }
            Message::ToggleAddAccountAdvanced(on) => {
                self.settings.add_account.advanced_expanded = on;
            }
            Message::CommitAddAccount => {
                if self.fixture_mode {
                    if let Ok(id) = commit_add_account(
                        &mut self.store,
                        &mut self.settings.add_account,
                        &mut self.next_account_ordinal,
                    ) {
                        self.store.mark_dirty();
                        if self.active_account_id.is_empty() {
                            self.active_account_id.clone_from(&id);
                        }
                        self.queue_server_mutation(
                            local_state::ServerMutationKind::AccountAdded,
                            vec![id],
                        );
                    }
                    return Task::none();
                }
                if self.credential_inflight.is_some()
                    || self.mail_cancel.is_some()
                    || self.mail_retry.is_some()
                    || self.account_edit.is_some()
                    || self.account_removal.is_some()
                    || self.credential_mutations.journal().active().is_some()
                {
                    self.settings.add_account.error = Some(crate::l10n::tr(
                        "Retry or cancel the current mail operation first.",
                    ));
                    return Task::none();
                }
                if !self.credential_store_ready {
                    self.settings.add_account.error = Some(crate::l10n::tr(
                        "The system credential service is still starting.",
                    ));
                    return Task::none();
                }
                let admission =
                    match admit_account_id(&self.store.accounts, self.next_account_ordinal) {
                        Ok(admission) => admission,
                        Err(error) => {
                            self.settings.add_account.error =
                                Some(crate::l10n::tr(error.user_message()));
                            return Task::none();
                        }
                    };
                let config = match self.settings.add_account.draft.validate(admission.id) {
                    Ok(config) => config,
                    Err(error) => {
                        self.settings.add_account.error = Some(error.message.to_owned());
                        return Task::none();
                    }
                };
                if let Err(error) = self.mail_backend.validate_configuration(&config) {
                    self.settings.add_account.error = Some(crate::l10n::tr(error.user_message()));
                    return Task::none();
                }
                let oauth = config.auth == backend::AuthMethod::OAuth2;
                let binding = model::settings::OAuthBinding::of(&self.settings.add_account);
                if oauth
                    && self
                        .oauth_grant
                        .as_ref()
                        .is_some_and(|(signed_in_for, _)| *signed_in_for != binding)
                {
                    // Never store one sign-in as another account's credential.
                    self.end_oauth_sign_in();
                }
                if oauth && self.oauth_grant.is_none() {
                    let provider = self
                        .settings
                        .add_account
                        .oauth_provider()
                        .map_or("your provider", |p| p.label());
                    self.settings.add_account.error = Some(crate::l10n::message(
                        "mail-sign-in-first",
                        &[("provider", crate::l10n::tr(provider))],
                    ));
                    return Task::none();
                }
                if !oauth && self.settings.add_account.password.is_empty() {
                    self.settings.add_account.error =
                        Some(crate::l10n::tr("Enter an account password."));
                    return Task::none();
                }
                if !oauth
                    && self.settings.add_account.password.len()
                        > backend::curl_backend::MAX_CREDENTIAL_BYTES
                {
                    self.settings.add_account.error = Some(crate::l10n::tr(
                        "The account password exceeds the supported size.",
                    ));
                    return Task::none();
                }
                let key = Self::credential_key(&config);
                let referenced = self.credential_keys_in_public_state();
                let _reservation = match self.credential_mutations.reserve(
                    credential_mutation::CredentialMutationKind::Add,
                    config.id.clone(),
                    true,
                    &referenced,
                    |_| credential_mutation::TransitionKeys {
                        old_key: None,
                        new_key: Some(key),
                    },
                ) {
                    Ok(reservation) => reservation,
                    Err(message) => {
                        self.settings.add_account.error = Some(message.to_owned());
                        return Task::none();
                    }
                };
                let password = std::mem::take(&mut self.settings.add_account.password);
                self.settings.add_account.password_revealed = false;
                self.settings.add_account.error = Some(
                    "Saving the credential transition before contacting the system keyring…"
                        .to_owned(),
                );
                // An OAuth account stores its grant where a password would go;
                // a password typed for it is discarded.
                let secret = match self.oauth_grant.take().filter(|_| oauth) {
                    Some((_, grant)) => grant,
                    None => Secret::new(password.into_bytes()),
                };
                self.mail_intent = Some(MailIntent::ProbeAccount(config));
                self.mail_retry = None;
                self.credential_secret = Some(secret);
                return Task::none();
            }
            Message::RequestRemoveAccount(account_id) => {
                let _ = self.invalidate_mailbox_syncs(true);
                return self.request_account_removal(&account_id);
            }
            Message::ConfirmRemoveAccount(event) => {
                use enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent;
                if matches!(
                    event,
                    ConfirmDialogEvent::FocusNext | ConfirmDialogEvent::FocusPrevious
                ) {
                    if let Some(operation) = event.focus_operation::<Message>(true) {
                        return iced::advanced::widget::operate(operation);
                    }
                    return Task::none();
                }
                let ConfirmDialogEvent::Resolved(cause) = event else {
                    return Task::none();
                };
                if enzui_iced_core::confirm::ConfirmDecision::from_cause(cause).is_confirmed() {
                    return self.start_account_removal();
                }
                self.account_removal = None;
            }
            Message::RetryRemoveAccount => return self.start_account_removal(),
            Message::CancelRemoveAccount => {
                let deleting_generation = self.account_removal.as_ref().and_then(|removal| {
                    if let AccountRemovalPhase::Deleting(generation) = &removal.phase {
                        Some(*generation)
                    } else {
                        None
                    }
                });
                if let Some(generation) = deleting_generation {
                    if let Some((active, cancel, CredentialStage::DeleteAccount)) =
                        &self.credential_inflight
                        && *active == generation
                    {
                        cancel.cancel();
                    }
                } else if let Some(generation) =
                    self.account_removal
                        .as_ref()
                        .and_then(|removal| match &removal.phase {
                            AccountRemovalPhase::AwaitingJournal(generation) => Some(*generation),
                            _ => None,
                        })
                {
                    self.account_removal = None;
                    return self.finish_credential_transition(generation);
                } else {
                    self.account_removal = None;
                }
            }
            Message::BeginEditAccount(account_id) => {
                let _ = self.invalidate_mailbox_syncs(true);
                return self.begin_account_edit(&account_id);
            }
            Message::EditAccountFieldEdited(field, value) => {
                if self.account_edit.is_none() {
                    self.settings.edit_account.set_field(field, value);
                }
            }
            Message::EditAccountPasswordEdited(value) => {
                if self.account_edit.is_none() {
                    self.settings.edit_account.password = value;
                    self.settings.edit_account.error = None;
                }
            }
            Message::ToggleEditAccountPasswordRevealed => {
                if self.account_edit.is_none() {
                    self.settings.edit_account.password_revealed =
                        !self.settings.edit_account.password_revealed;
                }
            }
            Message::SetEditAccountIncomingEncryption(encryption) => {
                if self.account_edit.is_none() {
                    self.settings
                        .edit_account
                        .draft
                        .set_imap_encryption(encryption);
                }
            }
            Message::SetEditAccountSmtpEncryption(encryption) => {
                if self.account_edit.is_none() {
                    self.settings
                        .edit_account
                        .draft
                        .set_smtp_encryption(encryption);
                }
            }
            Message::CommitEditAccount => return self.start_account_edit(),
            Message::RetryEditAccount => return self.retry_account_edit(),
            Message::CancelEditAccount => {
                let cleanup_warning =
                    self.account_edit
                        .as_ref()
                        .and_then(|edit| match &edit.phase {
                            AccountEditPhase::OldCleanupFailed(message)
                            | AccountEditPhase::Failed(message)
                                if message.contains("cleanup was not confirmed")
                                    || message
                                        .contains("rollback or cleanup was not confirmed") =>
                            {
                                Some(message.clone())
                            }
                            _ => None,
                        });
                if self.credential_inflight.is_none() && self.mail_cancel.is_none() {
                    let generation =
                        self.account_edit
                            .as_ref()
                            .and_then(|edit| match &edit.phase {
                                AccountEditPhase::AwaitingJournal(generation) => Some(*generation),
                                _ => None,
                            });
                    self.account_edit = None;
                    self.settings.edit_account.reset();
                    if let Some(generation) = generation {
                        return self.finish_credential_transition(generation);
                    }
                    if let Some(body) = cleanup_warning {
                        self.mail_notice = Some(MailNotice {
                            title: crate::l10n::tr("Credential cleanup incomplete"),
                            body,
                            retryable: true,
                            can_cancel_outbox: false,
                        });
                    }
                    if self.credential_mutations.journal().active().is_some() {
                        return self.continue_credential_mutation();
                    }
                }
            }

            _ => unreachable!("reduce_batch3 handles only its own variants"),
        }
        Task::none()
    }
}
