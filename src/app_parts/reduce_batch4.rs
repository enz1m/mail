use super::*;

impl State {
    pub(crate) fn reduce_batch4(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::SelectAllConversations => {
                use enzui_iced_core::drag::ItemId;
                let ids: Vec<ItemId> = crate::view::searched_conversations(
                    &self.store,
                    &self.selection,
                    self.search_scope,
                    &self.search_query,
                )
                .into_iter()
                .map(|conversation| ItemId::new(conversation.id))
                .collect();
                self.selected_conversation = ids.first().map(|id| id.as_str().to_owned());
                self.row_selection.replace(ids, None, None);
            }
            Message::ClearSelection => {
                self.row_selection.clear();
            }
            Message::SelectNextConversation | Message::SelectPreviousConversation => {
                let next = matches!(msg, Message::SelectNextConversation);
                let convos = crate::view::searched_conversations(
                    &self.store,
                    &self.selection,
                    self.search_scope,
                    &self.search_query,
                );
                if !convos.is_empty() {
                    let current = self
                        .selected_conversation
                        .as_ref()
                        .and_then(|id| convos.iter().position(|c| &c.id == id));
                    let index = match (current, next) {
                        (Some(i), true) => (i + 1).min(convos.len() - 1),
                        (Some(i), false) => i.saturating_sub(1),
                        (None, _) => 0,
                    };
                    let convo = &convos[index];
                    let focus = crate::view::conversation_row_id(convo);
                    let id = convo.id.clone();
                    let item = enzui_iced_core::drag::ItemId::new(id.clone());
                    self.row_selection
                        .replace([item.clone()], Some(item.clone()), Some(item));
                    self.open_conversation(id);
                    self.narrow_nav.on_select(true);
                    let height = (self.window_size.height
                        - 120.0
                        - if self.older_mailbox_cursor().is_some() {
                            36.0
                        } else {
                            0.0
                        })
                    .max(72.0);
                    let current = self.conversation_scroll.get().y;
                    let top = index as f32 * 72.0;
                    let offset = current.min(top).max(top + 72.0 - height).max(0.0);
                    self.conversation_scroll.set(iced::Vector::new(0.0, offset));
                    return iced::widget::operation::scroll_to(
                        iced::advanced::widget::Id::from(format!(
                            "mail-conversations-{:?}-{}",
                            self.selection, self.search_query
                        )),
                        iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: offset },
                        iced::widget::operation::Animation::Instant,
                    )
                    .chain(iced::widget::operation::focus(focus));
                }
            }
            Message::FocusSearchField => {
                if self.mode() == crate::model::layout::LayoutMode::Narrow {
                    if self.compose.open {
                        return Task::none();
                    }
                    self.narrow_nav.on_back();
                }
                self.search_open = true;
                return iced::widget::operation::focus(crate::view::search_field_id());
            }
            Message::Shortcut(action) => {
                if let Some(msg) = self.resolve_shortcut(action) {
                    return self.update(msg);
                }
            }
            Message::KeyPressed { event, captured } => {
                // Decoded here because the table is the user's resolved one, not the
                // defaults.
                if let Some(action) = self.resolve_key(&event, captured) {
                    return self.update(Message::Shortcut(action));
                }
            }
            Message::Shortcuts(event) => return self.update_shortcut_settings(event),
            Message::PersistenceBusy(debt) => {
                if let PersistenceDebt::Shortcuts(revision) = &debt {
                    self.shortcut_saves_inflight.remove(revision);
                }
                if matches!(&debt, PersistenceDebt::SyncAdoption(_)) {
                    self.mail_notice = Some(MailNotice {
                        title: crate::l10n::tr("Mailbox synchronization waiting to save"),
                        body: "Persistence is busy. The last-good mailbox remains visible and the synchronized snapshot is queued for retry."
                            .to_owned(),
                        retryable: true,
                        can_cancel_outbox: false,
                    });
                }
                self.retain_persistence_retry(debt);
            }
            Message::BlockingRetryTick => return self.retry_blocking_work(),
            Message::ShortcutsSaved(revision, result) => {
                self.shortcut_saves_inflight.remove(&revision);
                if revision == self.shortcut_revision {
                    match result {
                        Ok(()) => {
                            self.shortcut_published = revision;
                            self.shortcut_error_kind = None;
                            self.shortcuts.finish_save(Ok(()));
                        }
                        Err(error) => {
                            self.shortcut_error_kind = Some(error.kind());
                            self.shortcuts.finish_save(Err(error));
                        }
                    }
                }
            }
            Message::PreferencesSaved(origin, token, result) => {
                if token == self.preferences_token() {
                    match result {
                        Ok(()) => {
                            self.preferences_published = token;
                            self.preferences_error = None;
                        }
                        Err(error) => {
                            self.preferences_error =
                                Some(preferences::SaveFailure::new(origin, error));
                        }
                    }
                }
                return self.continue_account_edit_publication();
            }
            Message::SyncAdoptionSaved(generation, token, result) => {
                let matching = self.pending_sync_adoption.as_ref().is_some_and(|pending| {
                    pending.generation == generation && pending.candidate_token == token
                });
                if !matching {
                    return Task::none();
                }
                let base_is_current = self
                    .pending_sync_adoption
                    .as_ref()
                    .is_some_and(|pending| pending.base_token == self.local_state_token());
                if !base_is_current {
                    let Some(pending) = self.pending_sync_adoption.take() else {
                        return Task::none();
                    };
                    if let Some(config) = self
                        .store
                        .account(&pending.account_id)
                        .and_then(|account| account.config.clone())
                    {
                        self.retain_mailbox_sync_retry(
                            MailIntent::SyncMailbox {
                                config,
                                mailbox: pending.mailbox,
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
                    let cleanup = if pending.fetched_payloads.is_empty() {
                        Task::none()
                    } else {
                        self.remove_payloads_task(pending.fetched_payloads)
                    };
                    return Task::batch([self.save_local_state(), cleanup]);
                }
                match result {
                    Ok(()) => {
                        let Some(pending) = self.pending_sync_adoption.take() else {
                            return Task::none();
                        };
                        let before = self.account_message_ids(&pending.account_id);
                        let mailbox = pending.mailbox;
                        let had_cached = self.store.messages.iter().any(|message| {
                            message.account_id == pending.account_id
                                && message.location
                                    == model::types::MessageLocation::Mailbox(mailbox)
                        });
                        let account_id = pending.account_id.clone();
                        self.store = pending.candidate_store;
                        self.sync_adoption_revision = pending.candidate_revision;
                        self.local_state_published = token;
                        self.local_state_error = None;
                        self.mailbox_sync_retry = None;
                        self.mailbox_sync_retry_at = None;
                        self.mailbox_sync_retry_attempt = 0;
                        let _ = self.apply_rules_with_server_journal();
                        if pending.announce {
                            self.announce_new_mail(&account_id, mailbox, &before, had_cached);
                        }
                        self.reconcile_selection();
                        // A finished sync says nothing: the refresh control
                        // stops spinning, and an earlier failure notice goes.
                        self.mail_notice = None;
                        self.attachment_reconcile_needed = true;
                        let cleanup = self.reconcile_attachment_store_task();
                        let next = self.start_next_mail_operation();
                        return Task::batch([cleanup, next]);
                    }
                    Err(error) => {
                        self.local_state_error = Some(error);
                        self.mail_notice = Some(MailNotice {
                            title: crate::l10n::tr("Mailbox synchronization not saved"),
                            body: "The last-good mailbox remains visible. Retry to publish the synchronized snapshot."
                                .to_owned(),
                            retryable: true,
                            can_cancel_outbox: false,
                        });
                    }
                }
            }
            Message::LocalStateSaved(token, result) => match result {
                Ok(()) if token == self.local_state_token() => {
                    self.local_state_published = token;
                    self.local_state_error = None;
                    self.draft_save_pending = false;
                    let cleanup = self.cleanup_retired_payloads();
                    let edit = self.continue_account_edit_publication();
                    let credentials = self.continue_credential_mutation();
                    let server_mutations = self.start_next_mail_operation();
                    let send = if self.pending_send.is_some()
                        && self.pending_undo_toast.is_none()
                        && self.mail_intent.is_none()
                        && self.mail_retry.is_none()
                        && !self.account_edit_has_active_work()
                    {
                        self.start_pending_send()
                    } else {
                        Task::none()
                    };
                    return Task::batch([cleanup, edit, credentials, server_mutations, send]);
                }
                Ok(()) => {}
                Err(error) if token == self.local_state_token() => {
                    self.local_state_error = Some(error);
                }
                Err(_) => {}
            },
            Message::AttachmentCleanupFinished(payloads, result) => {
                self.attachment_cleanup_inflight =
                    self.attachment_cleanup_inflight.saturating_sub(1);
                match result {
                    Ok(()) => {
                        if self.pending_payload_cleanup.is_empty()
                            && self.attachment_cleanup_inflight == 0
                        {
                            self.attachment_cleanup_error = None;
                        }
                    }
                    Err(error) => {
                        self.attachment_reconcile_needed = true;
                        for payload in payloads {
                            if !self.pending_payload_cleanup.contains(&payload) {
                                self.pending_payload_cleanup.push(payload);
                            }
                        }
                        self.attachment_cleanup_error = Some(error);
                        self.attachment_notice = Some(crate::l10n::tr(error.user_message()));
                        if error == attachment_store::AttachmentFailure::MaintenanceDeferred {
                            self.retain_attachment_retry(AttachmentRetry::Cleanup);
                        } else if error == attachment_store::AttachmentFailure::PayloadChanged {
                            self.retain_attachment_retry(AttachmentRetry::Reconcile);
                        }
                    }
                }
            }
            Message::AttachmentReconcileFinished(result) => {
                self.attachment_cleanup_inflight =
                    self.attachment_cleanup_inflight.saturating_sub(1);
                match result {
                    Ok(()) => {
                        self.attachment_reconcile_needed = false;
                        if self.pending_payload_cleanup.is_empty()
                            && self.attachment_cleanup_inflight == 0
                        {
                            self.attachment_cleanup_error = None;
                        }
                    }
                    Err(error) => {
                        self.attachment_reconcile_needed = true;
                        self.attachment_cleanup_error = Some(error);
                        self.attachment_notice = Some(crate::l10n::tr(error.user_message()));
                        if error == attachment_store::AttachmentFailure::MaintenanceDeferred {
                            self.retain_attachment_retry(AttachmentRetry::Reconcile);
                        }
                    }
                }
            }
            Message::AttachmentStartupSurveyFinished(cleanup) => {
                self.attachment_cleanup_inflight =
                    self.attachment_cleanup_inflight.saturating_sub(1);
                let retry =
                    cleanup == Err(attachment_store::AttachmentFailure::MaintenanceDeferred);
                self.apply_attachment_startup_survey(cleanup);
                if retry {
                    self.retain_attachment_retry(AttachmentRetry::StartupSurvey);
                }
            }
            Message::FrameLoadSettled(handoff) => {
                let Some(completion) = handoff.take() else {
                    return Task::none();
                };
                return self.finish_frame_load(completion);
            }
            Message::FrameLoadFailed(kind, error) => {
                match kind {
                    FrameKind::LocalState => {
                        self.local_state_load_error =
                            Some(self.startup.local_state.refuse_worker(error));
                    }
                    FrameKind::Preferences => {
                        self.preferences_load_error =
                            Some(self.startup.preferences.refuse_worker(error));
                    }
                    FrameKind::Shortcuts => {
                        self.shortcut_load_error =
                            Some(self.startup.shortcuts.refuse_worker(error));
                    }
                }
                return self.install_settled_frames();
            }
            Message::FrameCustodySettled(kind, retained) => {
                return self.finish_frame_custody(kind, retained);
            }
            Message::OutgoingMimePrepared(pending_id, result) => {
                let account_id = self
                    .pending_send
                    .as_ref()
                    .filter(|pending| pending.id == pending_id)
                    .map(|pending| pending.account_id.clone());
                let Some(account_id) = account_id else {
                    self.outgoing_mime = None;
                    return self.start_next_mail_operation();
                };
                match result {
                    Ok(mime) => {
                        self.outgoing_mime = Some((pending_id, mime));
                        return self.start_pending_send();
                    }
                    Err(error) => {
                        self.outgoing_mime = None;
                        self.mail_retry = self
                            .store
                            .account(&account_id)
                            .and_then(|account| account.config.clone())
                            .map(|config| MailIntent::SubmitOutbox { config, pending_id });
                        let retry_when_owner_drains = error
                            == attachment_store::AttachmentFailure::MaintenanceDeferred
                            && self.mail_retry.is_some();
                        self.mail_notice = Some(MailNotice {
                            title: crate::l10n::tr("Message remains in Outbox"),
                            body: crate::l10n::tr(error.user_message()),
                            retryable: self.mail_retry.is_some(),
                            can_cancel_outbox: true,
                        });
                        self.mail_busy = retry_when_owner_drains;
                    }
                }
            }
            Message::ToggleShortcuts => {
                self.shortcuts_open = !self.shortcuts_open;
                return self.toggle_shortcuts_focus();
            }
            Message::ShortcutsDialogEvent(event) => {
                return self.reduce_shortcuts_host(event);
            }
            Message::MarkAllRead => {
                let selection = self.selection.clone();
                self.perform_mark_all_read(&selection);
                self.reconcile_after_mutation();
            }
            Message::RequestEmptyMailbox(kind, account_id) => {
                if self.store.account(&account_id).is_none()
                    || !matches!(kind, MailboxKind::Trash | MailboxKind::Junk)
                {
                    return Task::none();
                }
                self.mailbox_overflow_open = false;
                self.context_menu.close();
                self.bulk.pending_empty = Some(kind);
                self.bulk.pending_empty_account = Some(account_id);
                self.confirm_empty = enzui_iced_core::confirm::ConfirmState::new(None);
                return self.arm_empty_confirm_focus();
            }
            Message::ConfirmEmptyEvent(event) => {
                use enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent;
                use enzui_iced_widgets::modal_host::ModalHostEvent;
                let host_event = match event {
                    ConfirmDialogEvent::FocusNext => ModalHostEvent::FocusNext,
                    ConfirmDialogEvent::FocusPrevious => ModalHostEvent::FocusPrevious,
                    ConfirmDialogEvent::Resolved(cause) => ModalHostEvent::Dismissed(cause),
                    ConfirmDialogEvent::CheckboxToggled(_) => return Task::none(),
                };
                let focus_task = self.reduce_empty_confirm_host(host_event);
                let ConfirmDialogEvent::Resolved(cause) = event else {
                    return focus_task;
                };
                let Some(kind) = self.bulk.pending_empty.take() else {
                    return focus_task;
                };
                let Some(account_id) = self.bulk.pending_empty_account.take() else {
                    return focus_task;
                };
                // The confirmation retains its originating account across navigation.
                if !enzui_iced_core::confirm::ConfirmDecision::from_cause(cause).is_confirmed() {
                    return focus_task;
                }
                let ids = self
                    .store
                    .messages
                    .iter()
                    .filter(|message| {
                        message.account_id == account_id && matches!(&message.location, model::types::MessageLocation::Mailbox(location) if *location == kind)
                    })
                    .map(|message| message.id.clone())
                    .collect::<Vec<_>>();
                let emptied = !ids.is_empty()
                    && self.queue_server_mutations_for_messages(
                        local_state::ServerMutationKind::EmptyMailbox(kind),
                        &ids,
                    )
                    && model::bulk::empty_mailbox(&mut self.store, kind, &account_id) > 0;
                if emptied {
                    self.reconcile_after_mutation();
                }
                return focus_task;
            }
            Message::ToggleMailboxOverflow => {
                let was_open = self.mailbox_overflow_open;
                self.mailbox_overflow_open = !was_open;
                if let Some(op) = enzui_iced_widgets::action_bar::overflow_focus_restore_operation(
                    was_open,
                    Some(&bulk_view::mailbox_actions_invoker_id()),
                ) {
                    return iced::advanced::widget::operate(op);
                }
            }
            Message::CloseMailboxOverflow => {
                // The sole close-and-restore seam for the mailbox kebab. Row commands do not
                // clear the flag themselves, or this arm would skip the focus restore. Only a
                // real `true → false` transition restores.
                if self.mailbox_overflow_open {
                    self.mailbox_overflow_open = false;
                    if let Some(op) =
                        enzui_iced_widgets::action_bar::overflow_focus_restore_operation(
                            true,
                            Some(&bulk_view::mailbox_actions_invoker_id()),
                        )
                    {
                        return iced::advanced::widget::operate(op);
                    }
                }
            }
            Message::SetNotificationSettings(change) => {
                let mut settings = self.notifications.get().clone();
                match change {
                    notification_settings::Change::Enabled(value) => settings.enabled = value,
                    notification_settings::Change::Scope(value) => settings.all_mailboxes = value,
                    notification_settings::Change::Preview(value) => settings.preview = value,
                    notification_settings::Change::Mailbox(target, value) => {
                        let valid = self
                            .store
                            .account(&target.account_id)
                            .is_some_and(|account| match &target.location {
                                model::types::MessageLocation::Mailbox(mailbox) => {
                                    account.mailboxes.contains(mailbox)
                                }
                                model::types::MessageLocation::Folder(id) => {
                                    account.folders.iter().any(|folder| &folder.id == id)
                                }
                            });
                        if !valid {
                            return Task::none();
                        }
                        match value {
                            Some(value) => {
                                settings.overrides.insert(target, value);
                            }
                            None => {
                                settings.overrides.remove(&target);
                            }
                        }
                    }
                }
                if settings.validate().is_ok() && *self.notifications.get() != settings {
                    *self.notifications.get_mut() = settings;
                    self.new_mail.withdraw();
                    return self.save_preferences(preferences::SaveOrigin::Notifications);
                }
            }
            Message::TestNotification => {
                if self.notifications.get().enabled {
                    self.new_mail.test();
                }
            }
            Message::SetDarkHtml(enabled) => {
                if *self.dark_html.get() != enabled {
                    *self.dark_html.get_mut() = enabled;
                    return Task::batch([
                        self.save_preferences(preferences::SaveOrigin::ReaderFormat),
                        self.update_html_reader(),
                    ]);
                }
            }
            Message::SetPreferPlainText(plain) => {
                if *self.prefer_plain_text.get() != plain {
                    *self.prefer_plain_text.get_mut() = plain;
                    return Task::batch([
                        self.save_preferences(preferences::SaveOrigin::ReaderFormat),
                        self.update_html_reader(),
                    ]);
                }
            }
            Message::SetMessagePlainText(mid, plain) => {
                if self
                    .store
                    .message(&mid)
                    .is_some_and(|m| m.body_html.is_some())
                {
                    self.message_plain_text.insert(mid, plain);
                    self.reader_overflow_target = None;
                    return self.update_html_reader();
                }
            }
            Message::ToggleQuotedText(mid) => {
                if self
                    .selected_conversation
                    .as_deref()
                    .is_some_and(|conversation| self.message_ids_of(conversation).contains(&mid))
                {
                    if !self.quoted_text_expanded.remove(&mid) {
                        self.quoted_text_expanded.insert(mid);
                    }
                    return self.update_html_reader();
                }
            }
            Message::ToggleReaderOverflow(mid) => {
                if self.reader_overflow_target.as_deref() == Some(mid.as_str()) {
                    self.reader_overflow_target = None;
                } else {
                    self.reader_overflow_target = Some(mid);
                }
            }
            Message::CloseReaderOverflow(mid) => {
                // Target-matched so a dismissal from a card that already
                // closed cannot shut a menu opened since.
                if self.reader_overflow_target.as_deref() == Some(mid.as_str()) {
                    self.reader_overflow_target = None;
                }
            }
            //
            // Unblock is not destructive, so it calls `remove_blocked` directly. Block
            // arms the shared confirmation; only `ConfirmBlockSender` calls
            // `block_sender`, which keeps the blocklist-beats-allowlist invariant.
            Message::BlockSenderOf(message_id) => {
                // An unresolved message id is inert (`missing_sender_block_request_is_inert`);
                // never arm with an empty address.
                if let Some(addr) = self
                    .store
                    .message(&message_id)
                    .map(|m| m.sender_email.clone())
                {
                    self.arm_block_sender_confirmation(addr);
                }
            }
            Message::UnblockSenderOf(message_id) => {
                if let Some(addr) = self
                    .store
                    .message(&message_id)
                    .map(|m| m.sender_email.clone())
                {
                    self.privacy.get_mut().remove_blocked(&addr);
                    return self.save_preferences(preferences::SaveOrigin::Privacy);
                }
            }

            Message::ComposeFormatMenuEvent(event) => {
                let restore = enzui_iced_widgets::menu_button::focus_restore_operation(
                    &event,
                    Some(&iced::advanced::widget::Id::new("mail-compose-format")),
                )
                .map_or_else(Task::none, iced::advanced::widget::operate);
                let activated = enzui_iced_widgets::menu_button::apply_event(
                    &mut self.compose_format_model,
                    &mut self.compose_format_menu,
                    &event,
                );
                if let Some(id) = activated
                    && let Some(format) = compose_view::format_for_menu_entry(id.as_str())
                {
                    return Task::batch([
                        self.update_impl(Message::SetComposeFormat(format)),
                        restore,
                    ]);
                }
                return restore;
            }
            Message::ComposeColorPicked(color) => {
                if self.compose.open {
                    self.rich_controller.set_color(color);
                    // A colour applied to a selection edits the document, so the outgoing HTML is
                    // rebuilt; a colour armed at the caret changes nothing yet.
                    self.compose.body_html = Some(enzui_iced_widgets::rich_edit::to_html(
                        self.rich_controller.document(),
                    ));
                }
            }
            Message::SenderMenuEvent(message_id, recipient, event) => {
                let address = self.store.message(&message_id).and_then(|message| {
                    recipient.map_or_else(
                        || Some((message.sender_name.clone(), message.sender_email.clone())),
                        |index| {
                            message
                                .to
                                .get(index)
                                .and_then(|address| model::recipients::parse_recipient(address))
                        },
                    )
                });
                let Some((name, email)) = address else {
                    return Task::none();
                };
                let target = AddressMenuTarget {
                    message_id,
                    recipient,
                    name: name.clone(),
                    email: email.clone(),
                };
                use enzui_iced_widgets::menu_button::MenuButtonEvent;
                if matches!(event, MenuButtonEvent::Open(_)) {
                    self.sender_menu_model = crate::view::address_action_menu_model(
                        !name.trim().is_empty(),
                        recipient.is_some(),
                    );
                    self.sender_menu_target = Some(target.clone());
                }
                if self.sender_menu_target.as_ref() != Some(&target) {
                    if self.sender_menu_target.as_ref().is_some_and(|previous| {
                        previous.message_id == target.message_id && previous.recipient == recipient
                    }) {
                        self.sender_menu_state
                            .close(enzui_iced_core::menu_model::CloseCause::Host);
                        self.sender_menu_target = None;
                    }
                    return Task::none();
                }
                let activated = enzui_iced_widgets::menu_button::apply_event(
                    &mut self.sender_menu_model,
                    &mut self.sender_menu_state,
                    &event,
                );
                if !self.sender_menu_state.is_open() {
                    self.sender_menu_target = None;
                }
                if let Some(id) = activated {
                    match id.as_str() {
                        "copy-address" => {
                            return iced::clipboard::write(email)
                                .map(|_| Message::ClipboardWritten);
                        }
                        "copy-name-and-email" => {
                            let text = model::compose::format_address(&name, &email);
                            return iced::clipboard::write(text).map(|_| Message::ClipboardWritten);
                        }
                        "compose-to" => {
                            let previous_pending_targets = self
                                .fixture_mode
                                .then(|| self.pending_send_targets())
                                .flatten();
                            self.compose_generation = self.compose_generation.wrapping_add(1);
                            if !self.open_compose(ComposeMode::New) {
                                return Task::none();
                            }
                            if let Some(targets) = previous_pending_targets {
                                self.queue_server_mutation(
                                    local_state::ServerMutationKind::Send,
                                    targets,
                                );
                            }
                            self.compose.to = model::compose::format_address(&name, &email);
                            self.compose_overflow_open = false;
                            self.compose_attach_notice = None;
                            self.sync_body_editor();
                            self.reconcile_selection();
                            return self.arm_compose_focus();
                        }
                        "filter-from" => {
                            self.search_query = email;
                            self.search_scope = SearchScope::AllMailboxes;
                            self.search_open = true;
                            self.search_highlight = None;
                        }
                        _ => {}
                    }
                }
            }
            Message::CopyMessageSelection(text) => {
                return iced::clipboard::write(text).map(|_| Message::ClipboardWritten);
            }
            Message::ShowAttachmentDetails { message_id, name } => {
                if let Some(attachment) = self.store.message(&message_id).and_then(|message| {
                    message
                        .attachments
                        .iter()
                        .find(|attachment| attachment.name == name)
                }) {
                    self.attachment_notice = Some(match attachment.payload_ref() {
                        Some(_) => crate::l10n::message(
                            "mail-notice-open-or-save-checks-the-local-payload-before-use",
                            &[
                                ("value1", (attachment.name).clone()),
                                ("value2", (attachment.mime).clone()),
                                ("value3", (attachment.size_label())),
                            ],
                        ),
                        None => crate::l10n::message(
                            "mail-notice-file-data-is-unavailable-this-record-contains-metadata-only",
                            &[
                                ("value1", (attachment.name).clone()),
                                ("value2", (attachment.mime).clone()),
                                ("value3", (attachment.size_label())),
                            ],
                        ),
                    });
                }
            }
            Message::OpenAttachment(attachment) => {
                self.attachment_notice = None;
                let Some(generation) = self.attachment_open_generation.checked_add(1) else {
                    self.attachment_notice = Some(
                        "Attachment opening is unavailable until the application restarts."
                            .to_owned(),
                    );
                    return Task::none();
                };
                let Some(_) = attachment.payload_ref() else {
                    self.attachment_notice = Some(
                        attachment_store::AttachmentFailure::MissingPayload
                            .user_message()
                            .to_owned(),
                    );
                    return Task::none();
                };
                self.attachment_opener
                    .cancel(enzui_iced_core::portal::CancelReason::Application);
                if let Some(opened) = self.attachment_open_held.take()
                    && let Err(error) = opened.verify_path()
                {
                    self.attachment_store
                        .retain_cleanup_authority(&opened.payload_ref());
                    self.attachment_reconcile_needed = true;
                    self.attachment_notice = Some(crate::l10n::tr(error.user_message()));
                    return Task::none();
                }
                self.attachment_open_generation = generation;
                self.attachment_open_pending = Some(attachment.clone());
                self.attachment_open_busy = false;
                return self.open_attachment_task(generation, attachment);
            }
            Message::AttachmentPathResolved(generation, payload, result) => {
                if generation != self.attachment_open_generation
                    || self
                        .attachment_open_pending
                        .as_ref()
                        .and_then(model::types::Attachment::payload_ref)
                        .as_ref()
                        != Some(&payload)
                {
                    return Task::none();
                }
                match result {
                    Ok(opened) => {
                        self.attachment_open_pending = None;
                        self.attachment_open_busy = false;
                        let retained_file = match opened.retained_file() {
                            Ok(file) => file,
                            Err(error) => {
                                self.attachment_notice =
                                    Some(crate::l10n::tr(error.user_message()));
                                return Task::none();
                            }
                        };
                        if let Err(outcome) = self.attachment_opener.start_file(retained_file) {
                            self.attachment_notice = attachment_store::open_failure_text(&outcome);
                        } else {
                            self.attachment_open_held = Some(opened);
                        }
                    }
                    Err(attachment_store::AttachmentFailure::MaintenanceDeferred) => {
                        self.attachment_open_busy = true;
                    }
                    Err(error) => {
                        self.attachment_open_pending = None;
                        self.attachment_open_busy = false;
                        self.attachment_notice = Some(crate::l10n::tr(error.user_message()));
                    }
                }
            }
            _ => unreachable!("reduce_batch4 handles only its own variants"),
        }
        Task::none()
    }
}
