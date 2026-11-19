use super::*;

impl State {
    pub(crate) fn reduce_batch1(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::Theme(event) => {
                let before = self.themes.theme_generation();
                let task = self.themes.update(*event);
                debug_assert!(
                    self.themes.theme_generation() >= before,
                    "theme generation cannot regress"
                );
                return task.map(|event| Message::Theme(Box::new(event)));
            }
            Message::WindowResized(size) => {
                let was_narrow = layout_mode(self.window_size.width) == LayoutMode::Narrow;
                let now_narrow = layout_mode(size.width) == LayoutMode::Narrow;
                self.window_size = size;
                // Drop the detail push only on a real narrow-to-wider transition; the initial
                // resize to the compositor's size would otherwise clear a boot-time push.
                if was_narrow && !now_narrow {
                    self.narrow_nav.on_back();
                }
            }
            Message::SelectSidebar(sel) => {
                if self.selection != sel {
                    self.flush_deferred_read();
                    self.selection = sel;
                    self.selected_conversation = self
                        .store
                        .conversations_for(&self.selection)
                        .first()
                        .map(|c| c.id.clone());
                    self.narrow_nav.on_sidebar_change();
                }
                self.nav.dismiss_overlay();
            }
            Message::SelectConversation { id, toggle, range } => {
                use enzui_iced_core::drag::ItemId;
                use enzui_iced_core::list_selection::{ClickModel, ClickModifiers, ClickOutcome};

                let order: Vec<ItemId> = self
                    .store
                    .conversations_for(&self.selection)
                    .into_iter()
                    .map(|c| ItemId::new(c.id))
                    .collect();
                let model = ClickModel::new(order);
                let outcome = self.row_selection.on_pointer_click(
                    &model,
                    &ItemId::new(id.clone()),
                    ClickModifiers { toggle, range },
                );

                match outcome {
                    ClickOutcome::Ignored => {}
                    ClickOutcome::Activated(activated) => {
                        // A plain click opens and empties the set, the only exit from a
                        // multi-selection. The clicked row becomes the range anchor without
                        // joining.
                        use enzui_iced_core::drag::ItemId as AnchorId;
                        self.row_selection.replace(
                            std::iter::empty::<AnchorId>(),
                            Some(ItemId::new(activated.as_str().to_owned())),
                            Some(ItemId::new(activated.as_str().to_owned())),
                        );
                        self.open_conversation(activated.as_str().to_owned());
                        self.narrow_nav.on_select(true);
                    }
                    ClickOutcome::Selected { .. } => {
                        // A modifier click multi-selects in place; the set's verbs live on the
                        // right-click menu and mailbox kebab.

                        // With two or more selected, `view::reader_pane` shows an explicit count.
                        // A Ctrl+click that deselected the clicked row must not leave the reader
                        // anchored to it.
                        if self.row_selection.contains(&ItemId::new(id.clone())) {
                            self.selected_conversation = Some(id);
                        } else {
                            // Fall back to a surviving member in list order, so the reader is
                            // never blank while rows are selected.
                            let order: Vec<ItemId> = self
                                .store
                                .conversations_for(&self.selection)
                                .into_iter()
                                .map(|c| ItemId::new(c.id))
                                .collect();
                            self.selected_conversation = self
                                .row_selection
                                .selected_in_order(&order)
                                .first()
                                .map(|i| i.as_str().to_owned());
                        }
                        self.narrow_nav.on_select(true);
                    }
                }
            }
            Message::NarrowBack => {
                self.narrow_nav.on_back();
            }
            Message::SidebarState(next) => {
                self.nav = next;
            }
            Message::AllowImagesOnce(id) => {
                if let Some(message) = self.store.message(&id)
                    && !self.privacy.proxy_remote_content
                    && !crate::mail_actions::sender_is_blocked(
                        &self.privacy.block_list,
                        &message.sender_email,
                    )
                {
                    self.remote_images_once.insert(id);
                }
                return self.update_html_reader();
            }
            Message::AlwaysAllowSender(sender_email) => {
                self.privacy.get_mut().always_allow_sender(&sender_email);
                return self.save_preferences(preferences::SaveOrigin::CrossOrigin);
            }
            Message::OpenUrlDialogEvent(event) => {
                let Some(focus) = self.open_url_focus.as_ref() else {
                    return Task::none();
                };
                let outcome = focus.reduce::<Message>(event);
                let dismissed = outcome.dismissed();
                let task = outcome.into_task();
                if let Some(cause) = dismissed {
                    self.open_url_focus = None;
                    return Task::batch([
                        task,
                        self.update(Message::ConfirmOpenUrl(
                            enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent::Resolved(cause),
                        )),
                    ]);
                }
                return task;
            }
            Message::RequestOpenUrl(href) => {
                // HTML mail can say "your bank" while linking elsewhere, so the dialog shows
                // the full sanitised href and requires an affirmative action.
                self.link_open_notice = None;
                self.pending_open_url = Some(href);
                self.confirm_open_url = enzui_iced_core::confirm::ConfirmState::new(None);
                let (focus, operation) =
                    enzui_iced_widgets::modal_host::ModalFocusLifecycle::open_at::<Message>(
                        "mail-open-link",
                        None,
                        iced::advanced::widget::Id::new("mail-open-link-cancel"),
                    );
                self.open_url_focus = Some(focus);
                return iced::advanced::widget::operate(operation);
            }
            Message::ConfirmOpenUrl(event) => {
                use enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent;
                if matches!(
                    event,
                    ConfirmDialogEvent::FocusNext | ConfirmDialogEvent::FocusPrevious
                ) {
                    if let Some(op) = event.focus_operation::<Message>(true) {
                        return iced::advanced::widget::operate(op);
                    }
                    return Task::none();
                }
                let ConfirmDialogEvent::Resolved(cause) = event else {
                    return Task::none();
                };
                self.open_url_focus = None;
                let Some(href) = self.pending_open_url.take() else {
                    return Task::none();
                };
                if !enzui_iced_core::confirm::ConfirmDecision::from_cause(cause).is_confirmed() {
                    return Task::none();
                }
                // The framework OpenURI owns transport, `Request.Close`, correlation and
                // outcomes. A request that never starts reaches the same finish arm as a late
                // reply, so `OpenUrlFinished` is the single completion route.
                match self.link_opener.start(&href) {
                    Ok(()) => return Task::none(),
                    Err(outcome) => {
                        return Task::done(Message::OpenUrlFinished(outcome));
                    }
                }
            }
            Message::OpenUrlPortalTick(wake) => {
                // Only the live generation is pumped, so a superseded stream's queued wake
                // cannot consume the replacement's reply; the replacement primes itself.
                match self.link_opener.pump(wake) {
                    link_launcher::WakeVerdict::Finished(outcome) => {
                        return Task::done(Message::OpenUrlFinished(outcome));
                    }
                    link_launcher::WakeVerdict::Pending | link_launcher::WakeVerdict::Stale => {}
                }
            }
            Message::OpenUrlFinished(outcome) => {
                // Success creates no "opened" state; typed failures become a global notice,
                // and cancellation clears it.
                self.link_open_notice = link_launcher::failure_text(&outcome);
            }
            Message::DismissOpenUrlNotice => {
                self.link_open_notice = None;
            }
            Message::Window(command) => {
                if command == enzui_iced_widgets::title_bar_gesture::WindowCommand::Close {
                    return iced::window::latest().map(Message::WindowCloseResolved);
                }
                return iced::window::latest().then(move |maybe_id| match maybe_id {
                    Some(id) => command.into_task(id),
                    None => Task::none(),
                });
            }
            Message::WindowCloseResolved(Some(window)) => {
                return self.request_window_close(window, Instant::now());
            }
            Message::WindowCloseResolved(None) => {}
            Message::CloseSettlementTick(_, _) => {}
            Message::RetryCloseSettlement => {
                return self.retry_close_settlement(Instant::now());
            }
            Message::DiscardCloseSettlement => return self.discard_close_settlement(),
            Message::CancelCloseSettlement => self.cancel_close_settlement(),
            Message::RetryLocalTimestamp => return self.retry_local_timestamp(),
            Message::CredentialStoreReady(store) => {
                self.credential_store = store;
                self.credential_store_ready = true;
                let credential_recovery = self.continue_credential_mutation();
                let configs = self
                    .store
                    .accounts
                    .iter()
                    .filter_map(|account| account.config.clone())
                    .collect::<Vec<_>>();
                let mut tasks = vec![credential_recovery];
                if self.credential_mutations.journal().active().is_none() {
                    tasks.push(self.start_next_mail_operation());
                    for config in configs {
                        tasks.push(self.queue_inbox_sync(config));
                    }
                }
                return Task::batch(tasks);
            }
            Message::CredentialFinished(generation, stage, completion) => {
                let current =
                    self.credential_inflight
                        .as_ref()
                        .is_some_and(|(active, _, active_stage)| {
                            *active == generation && *active_stage == stage
                        });
                if !current {
                    return Task::none();
                }
                self.credential_inflight = None;
                let Some(outcome) = completion.take() else {
                    return Task::none();
                };
                if matches!(
                    stage,
                    CredentialStage::DeleteAccount | CredentialStage::VerifyAccountDeleted
                ) {
                    return self.complete_account_removal(generation, stage, outcome);
                }
                if matches!(
                    stage,
                    CredentialStage::RecoverConfirmCurrent
                        | CredentialStage::RecoverDeleteExact
                        | CredentialStage::RecoverConfirmAbsent
                ) {
                    return self.complete_credential_recovery(generation, stage, outcome);
                }
                if matches!(
                    stage,
                    CredentialStage::LoadOldForEdit
                        | CredentialStage::StoreEditCandidate
                        | CredentialStage::StoreAuthenticatedEdit
                        | CredentialStage::LoadEditCandidate
                        | CredentialStage::DeleteEditCandidate
                        | CredentialStage::RestoreOldAfterEdit
                        | CredentialStage::VerifyEditCandidateDeleted
                        | CredentialStage::VerifyRestoredOldAfterEdit
                        | CredentialStage::DeleteOldAfterEdit
                        | CredentialStage::VerifyOldDeletedAfterEdit
                ) {
                    return self.complete_edit_credential(generation, stage, outcome);
                }
                if stage == CredentialStage::DeleteAbandonedAccount {
                    self.mail_discard_generation = None;
                    if !matches!(outcome, CredentialOutcome::Deleted) {
                        self.mail_notice = Some(MailNotice {
                            title: crate::l10n::tr("Could not remove account credential"),
                            body: "The system credential service did not confirm deletion."
                                .to_owned(),
                            retryable: false,
                            can_cancel_outbox: false,
                        });
                    }
                    return self.start_next_mail_operation();
                }
                let Some(intent) = self.mail_intent.clone() else {
                    return self.start_next_mail_operation();
                };
                let discard = self.mail_discard_generation == Some(generation);
                match stage {
                    CredentialStage::StoreAccount if discard => {
                        let config = intent.config().clone();
                        self.mail_intent = None;
                        self.mail_discard_generation = None;
                        return self.delete_account_credential(generation, &config);
                    }
                    CredentialStage::StoreAccount => match outcome {
                        CredentialOutcome::Stored => {
                            let operation = self
                                .credential_store
                                .0
                                .get(Self::credential_key(intent.config()));
                            return self.credential_task(
                                generation,
                                CredentialStage::LoadForOperation,
                                operation,
                            );
                        }
                        outcome => {
                            self.mail_intent = None;
                            self.mail_discard_generation = None;
                            self.settings.add_account.error = Some(
                                "Credential state uncertain; reconciling the exact recorded key."
                                    .to_owned(),
                            );
                            self.mail_notice =
                                Some(Self::credential_failure_notice(&intent, outcome));
                            return self.continue_credential_mutation();
                        }
                    },
                    CredentialStage::LoadForOperation if discard => {
                        self.mail_discard_generation = None;
                        match &intent {
                            MailIntent::ProbeAccount(config) => {
                                let config = config.clone();
                                self.mail_intent = None;
                                return self.delete_account_credential(generation, &config);
                            }
                            MailIntent::SubmitOutbox { .. } => {
                                self.mail_intent = None;
                                self.mail_notice = None;
                                return self.restore_cancelled_outbox();
                            }
                            MailIntent::SyncMailbox { .. } => {
                                self.mail_intent = None;
                                return self.start_next_mail_operation();
                            }
                            MailIntent::SearchServer { .. } => {
                                self.mail_intent = None;
                                self.server_search = ServerSearch::Idle;
                                return self.start_next_mail_operation();
                            }
                            MailIntent::ReconcileServerMutation { .. } => {
                                self.mail_intent = None;
                                self.mail_retry = Some(intent.clone());
                                self.mail_notice = Some(MailNotice {
                                    title: crate::l10n::tr("Server mutation reconciliation pending"),
                                    body: "The durable mutation was kept without starting an IMAP command."
                                        .to_owned(),
                                    retryable: true,
                                    can_cancel_outbox: false,
                                });
                                return Task::none();
                            }
                            MailIntent::ProbeAccountEdit(_) => return Task::none(),
                        }
                    }
                    CredentialStage::LoadForOperation => match outcome {
                        CredentialOutcome::Retrieved(secret) => {
                            return self.start_backend_with_secret(secret);
                        }
                        CredentialOutcome::NotFound
                            if matches!(&intent, MailIntent::ProbeAccount(_))
                                && self.credential_mutations.journal().active().is_some() =>
                        {
                            self.mail_intent = None;
                            self.mail_discard_generation = None;
                            self.settings.add_account.error = Some(
                                "The stored credential could not be confirmed; the account was not added."
                                    .to_owned(),
                            );
                            let generation = self
                                .credential_mutations
                                .journal()
                                .active()
                                .map_or(generation, |record| record.sequence);
                            return self.finish_credential_transition(generation);
                        }
                        outcome => {
                            self.mail_intent = None;
                            self.mail_discard_generation = None;
                            let retry_sync_automatically = matches!(
                                &outcome,
                                CredentialOutcome::Unavailable | CredentialOutcome::Failed(_)
                            );
                            let notice = Self::credential_failure_notice(&intent, outcome);
                            if matches!(&intent, MailIntent::ProbeAccount(_)) {
                                self.settings.add_account.error = Some(notice.body.clone());
                            }
                            self.mail_notice = Some(notice);
                            if matches!(&intent, MailIntent::SyncMailbox { .. }) {
                                if retry_sync_automatically {
                                    self.retain_mailbox_sync_retry(intent, Instant::now());
                                } else {
                                    self.mailbox_sync_retry = Some(intent);
                                    self.mailbox_sync_retry_at = None;
                                }
                            } else {
                                self.mail_retry = Some(intent);
                            }
                        }
                    },
                    CredentialStage::DeleteAbandonedAccount => {
                        return self.start_next_mail_operation();
                    }
                    CredentialStage::DeleteAccount
                    | CredentialStage::VerifyAccountDeleted
                    | CredentialStage::RecoverConfirmCurrent
                    | CredentialStage::RecoverDeleteExact
                    | CredentialStage::RecoverConfirmAbsent
                    | CredentialStage::LoadOldForEdit
                    | CredentialStage::StoreEditCandidate
                    | CredentialStage::StoreAuthenticatedEdit
                    | CredentialStage::LoadEditCandidate
                    | CredentialStage::DeleteEditCandidate
                    | CredentialStage::RestoreOldAfterEdit
                    | CredentialStage::VerifyEditCandidateDeleted
                    | CredentialStage::VerifyRestoredOldAfterEdit
                    | CredentialStage::DeleteOldAfterEdit
                    | CredentialStage::VerifyOldDeletedAfterEdit => return Task::none(),
                }
            }
            Message::MailOperationFinished(generation, result) => {
                return self.complete_mail_operation(generation, result);
            }
            Message::RetryMailOperation => {
                if self.pending_sync_adoption.is_some() {
                    self.local_state_error = None;
                    self.mail_notice = None;
                    return self.save_sync_adoption();
                }
                if let Some(intent) = self.mail_retry.take() {
                    self.mail_busy = false;
                    self.mail_notice = None;
                    return self.start_credential_load(intent);
                }
                if let Some(intent) = self.mailbox_sync_retry.take() {
                    self.mailbox_sync_retry_at = None;
                    self.mailbox_sync_retry_attempt = 0;
                    self.mail_notice = None;
                    return self.start_credential_load(intent);
                }
                if self.credential_mutations.journal().active().is_some() {
                    self.mail_notice = None;
                    return self.continue_credential_mutation();
                }
            }
            Message::CancelMailOperation => {
                self.mail_busy = false;
                if let Some((generation, cancel)) = &self.mail_cancel {
                    cancel.cancel();
                    self.mail_discard_generation = Some(*generation);
                } else if let Some((generation, cancel, _)) = &self.credential_inflight {
                    cancel.cancel();
                    self.mail_discard_generation = Some(*generation);
                } else if let Some(intent) = self.mail_retry.take() {
                    self.mail_notice = None;
                    return match intent {
                        MailIntent::ProbeAccount(config) => {
                            let Some(generation) = self.advance_mail_generation() else {
                                self.mail_retry = Some(MailIntent::ProbeAccount(config));
                                return self.refuse_exhausted_mail_generation();
                            };
                            self.settings.add_account.reset();
                            self.delete_account_credential(generation, &config)
                        }
                        MailIntent::SubmitOutbox { .. } => {
                            self.outgoing_mime = None;
                            self.restore_cancelled_outbox()
                        }
                        MailIntent::SyncMailbox { .. } => self.start_next_mail_operation(),
                        MailIntent::SearchServer { .. } => {
                            self.server_search = ServerSearch::Idle;
                            self.start_next_mail_operation()
                        }
                        intent @ MailIntent::ReconcileServerMutation { .. } => {
                            self.mail_retry = Some(intent);
                            Task::none()
                        }
                        MailIntent::ProbeAccountEdit(_) => Task::none(),
                    };
                } else if self.mailbox_sync_retry.take().is_some() {
                    self.mailbox_sync_retry_at = None;
                    self.mailbox_sync_retry_attempt = 0;
                    self.mail_notice = None;
                    return self.start_next_mail_operation();
                } else if self.pending_send.is_some() {
                    self.mail_notice = None;
                    self.outgoing_mime = None;
                    return self.restore_cancelled_outbox();
                }
            }
            Message::DismissMailNotice => self.mail_notice = None,
            Message::HideNotice(key) => {
                if !self.hidden_notices.contains(&key) {
                    self.hidden_notices.push(key);
                }
            }
            Message::RefreshMailbox => return self.manual_mailbox_refresh(),
            Message::LoadOlderMailbox => return self.load_older_mailbox(),
            Message::PeriodicMailboxSync(generation, now) => {
                return self.periodic_mailbox_refresh(generation, now);
            }
            Message::MailboxSyncRetryTick(generation, now) => {
                return self.retry_mailbox_sync_if_due(generation, now);
            }

            Message::OpenCompose(mode) => {
                self.attachment_reminder_open = false;
                let previous_pending_targets = self
                    .fixture_mode
                    .then(|| self.pending_send_targets())
                    .flatten();
                self.compose_generation = self.compose_generation.wrapping_add(1);
                let seed_caret = !matches!(mode, ComposeMode::EditDraft(_));
                if !self.open_compose(mode) {
                    return Task::none();
                }
                if let Some(targets) = previous_pending_targets {
                    self.queue_server_mutation(local_state::ServerMutationKind::Send, targets);
                }
                self.compose.format = *self.compose_format.get();
                // A draft with a formatted body never opens in Plain, which would drop the
                // stored HTML on the next save; an Html draft keeps its own label.
                if self.compose.body_html.is_some()
                    && self.compose.format == model::compose::ComposeFormat::Plain
                {
                    self.compose.format = model::compose::ComposeFormat::Rich;
                }
                self.compose_overflow_open = false;
                self.compose_attach_notice = None;
                self.sync_body_editor();
                if seed_caret {
                    self.position_compose_caret(0);
                }
                self.reconcile_selection();
                return self.arm_compose_focus();
            }
            Message::CloseCompose => {
                if self.compose.can_save_draft()
                    && (self.compose.is_user_edited() || self.compose.editing_draft_id.is_some())
                {
                    return self.update(Message::SaveDraft);
                }
                if self.compose.open {
                    self.compose_ctl().close();
                    if self.local_timestamp_failure.is_some_and(|failure| {
                        matches!(
                            failure.retry,
                            LocalTimestampRetry::SaveDraft | LocalTimestampRetry::AutosaveDraft
                        )
                    }) {
                        self.local_timestamp_failure = None;
                    }
                    self.compose_overflow_open = false;
                    self.sync_body_editor();
                    return self.dismiss_compose_focus(DismissCause::CloseButton);
                }
            }
            Message::ShowCc => {
                self.compose.cc_shown = true;
                return iced::widget::operation::focus(crate::compose_view::recipient_field_id(
                    RecipientField::Cc,
                ));
            }
            Message::ShowBcc => {
                self.compose.bcc_shown = true;
                return iced::widget::operation::focus(crate::compose_view::recipient_field_id(
                    RecipientField::Bcc,
                ));
            }
            Message::ComposeRecipientMenuEvent(event) => {
                // Rebuilt on open so it offers only the lines still hidden.
                if matches!(
                    event,
                    enzui_iced_widgets::menu_button::MenuButtonEvent::Open(_)
                ) {
                    self.compose_recipient_model =
                        crate::compose_view::recipient_menu_model(&self.compose);
                }
                let restore = enzui_iced_widgets::menu_button::focus_restore_operation(
                    &event,
                    Some(&iced::advanced::widget::Id::new(
                        "mail-compose-recipient-menu",
                    )),
                )
                .map_or_else(Task::none, iced::advanced::widget::operate);
                let activated = enzui_iced_widgets::menu_button::apply_event(
                    &mut self.compose_recipient_model,
                    &mut self.compose_recipient_menu,
                    &event,
                );
                match activated
                    .as_ref()
                    .map(enzui_iced_core::menu_model::EntryId::as_str)
                {
                    Some(crate::compose_view::ADD_CC) => return self.update_impl(Message::ShowCc),
                    Some(crate::compose_view::ADD_BCC) => {
                        return self.update_impl(Message::ShowBcc);
                    }
                    _ => {}
                }
                return restore;
            }
            Message::ComposeToEdited(v) => self.compose.to = v,
            Message::ComposeCcEdited(v) => self.compose.cc = v,
            Message::ComposeBccEdited(v) => self.compose.bcc = v,
            Message::ComposeSubjectEdited(v) => self.compose.subject = v,
            _ => unreachable!("reduce_batch1 handles only its own variants"),
        }
        Task::none()
    }
}
