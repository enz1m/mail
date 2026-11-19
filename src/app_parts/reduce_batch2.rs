use super::*;

impl State {
    pub(crate) fn reduce_batch2(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::SetComposeFrom(account_id) => {
                if self.compose.open && self.store.account(&account_id).is_some() {
                    self.compose.from_account_id = account_id;
                }
            }
            Message::ComposeRecipientSelected(field, chosen) => {
                let slot = self.recipient_field_mut(field);
                *slot = crate::model::recipients::replace_active_token(slot, &chosen);
            }
            Message::ComposeRecipientCustom(field, typed) => {
                // A typed address matching no suggestion is kept verbatim and terminated. A
                // token matching exactly one suggestion commits that suggestion's full line
                // instead, as a desktop combobox would.
                let trimmed = typed.trim();
                if !trimmed.is_empty() {
                    let current = self.recipient_field_mut(field).clone();
                    let contacts = model::recipients::recent_contacts(&self.store);
                    let suggestions = model::recipients::suggest(&contacts, &current);
                    let replacement = if suggestions.len() == 1 {
                        suggestions[0].address_line()
                    } else {
                        trimmed.to_owned()
                    };
                    let slot = self.recipient_field_mut(field);
                    *slot = crate::model::recipients::replace_active_token(slot, &replacement);
                }
            }
            Message::ComposeBodyAction(action) => {
                let is_edit = action.is_edit();
                self.body_editor.perform(action);
                if is_edit {
                    self.compose.body = self.body_editor.text();
                }
            }
            Message::SetComposeFormat(fmt) => {
                use model::compose::ComposeFormat;
                if self.compose.open && self.compose.format != fmt {
                    let old = self.compose.format;
                    match (old, fmt) {
                        (ComposeFormat::Plain, ComposeFormat::Rich | ComposeFormat::Html) => {
                            // Re-seed only when the plain surface holds text the controller has
                            // not seen; rebuilding from its own text would drop the carried
                            // formatting.
                            if self.rich_controller.document().text() != self.compose.body {
                                let doc =
                                    enzui_iced_widgets::rich_edit::RichEditDocument::from_plain(
                                        &self.compose.body,
                                    );
                                self.rich_controller =
                                    enzui_iced_widgets::rich_edit::RichEditController::new(doc);
                            }
                            self.compose.body_html = Some(enzui_iced_widgets::rich_edit::to_html(
                                self.rich_controller.document(),
                            ));
                            self.rich_editor = text_editor::Content::with_text(&self.compose.body);
                        }
                        (ComposeFormat::Rich | ComposeFormat::Html, ComposeFormat::Plain) => {
                            self.compose.body = self.rich_controller.document().text().to_owned();
                            self.compose.body_html = None;
                            // Only the plain surface is rebuilt: the stored HTML is cleared, so
                            // `sync_body_editor` would re-seed the controller from flattened text.
                            self.body_editor = text_editor::Content::with_text(&self.compose.body);
                        }
                        _ => {
                            // Rich and Html share the editor; only the label changes.
                        }
                    }
                    self.compose.format = fmt;
                    if *self.compose_format.get() != fmt {
                        *self.compose_format.get_mut() = fmt;
                        return self.save_preferences(preferences::SaveOrigin::ComposeFormat);
                    }
                }
            }
            Message::ComposeRichEditAction(action) => {
                use enzui_iced_widgets::rich_edit::RichEditAction;
                let refocus = action.returns_to_editor();
                let mutated = match &action {
                    RichEditAction::Native(native) => {
                        self.rich_editor.perform(native.clone());
                        enzui_iced_widgets::rich_edit::sync_selection_from_content(
                            &mut self.rich_controller,
                            &self.rich_editor,
                        );
                        false
                    }
                    _ => action.apply(&mut self.rich_controller),
                };
                if mutated {
                    self.compose.body = self.rich_controller.document().text().to_owned();
                    self.compose.body_html = Some(enzui_iced_widgets::rich_edit::to_html(
                        self.rich_controller.document(),
                    ));
                    self.rich_editor = text_editor::Content::with_text(&self.compose.body);
                    for restore in
                        enzui_iced_widgets::rich_edit::content_cursor_actions(&self.rich_controller)
                    {
                        self.rich_editor.perform(restore);
                    }
                }
                if self.rich_controller.menu()
                    == Some(enzui_iced_widgets::rich_edit::ToolbarMenu::Link)
                {
                    return iced::widget::operation::focus(
                        enzui_iced_widgets::rich_edit::link_field_id(),
                    );
                }
                if refocus {
                    return iced::widget::operation::focus(crate::compose_view::compose_body_id());
                }
            }
            Message::ToggleComposeOverflow => {
                // Trigger press only; dismissals route through `CloseComposeOverflow`.
                if self.compose.open {
                    self.compose_overflow_open = !self.compose_overflow_open;
                }
            }
            Message::CloseComposeOverflow => {
                // Idempotent set-false, guarded like the toggle so a stray close cannot
                // mutate hidden chrome.
                if self.compose.open {
                    self.compose_overflow_open = false;
                }
            }
            Message::DroppedAttachment(path) => {
                if !self.accepts_attachment_drop() {
                    return Task::none();
                }
                self.attachment_stage_inflight = self.attachment_stage_inflight.saturating_add(1);
                return self.stage_attachments_task(AttachmentRetry::Stage {
                    generation: self.attachment_picker_generation,
                    paths: vec![path],
                    existing_count: self.compose.attachments.len(),
                    existing_bytes: self
                        .compose
                        .attachments
                        .iter()
                        .fold(0_u64, |total, attachment| {
                            total.saturating_add(attachment.size)
                        }),
                });
            }
            Message::AttachFile => {
                if self.compose.open {
                    if self.attachment_stage_inflight > 0 {
                        self.compose_attach_notice =
                            Some(crate::l10n::current().text("mail-attachment-staging-busy"));
                        return Task::none();
                    }
                    self.compose_attach_notice = None;
                    let Some(generation) = self.attachment_picker_generation.checked_add(1) else {
                        self.compose_attach_notice = Some(
                            crate::l10n::current().text("mail-attachment-generation-exhausted"),
                        );
                        return Task::none();
                    };
                    self.attachment_picker_generation = generation;
                    let target = picker::PickerTarget::ComposeAttachment(generation);
                    return self.request_attachment_picker(target);
                }
            }
            Message::AttachFallback(event) => return self.attach_chooser.handle(event),
            Message::AttachPickerOutcome(target, outcome) => {
                return self.finish_attach_pick(target, outcome);
            }
            Message::AttachPortalTick => {
                if let Some((target, outcome)) = self.attach_chooser.pump() {
                    return self.finish_attach_pick(target, outcome);
                }
            }
            Message::AttachmentsStaged(generation, result) => {
                let native = self.native_drop.take_pending(generation);
                self.attachment_stage_inflight = self.attachment_stage_inflight.saturating_sub(1);
                match result {
                    Ok(mut attachments)
                        if self.compose.open && generation == self.attachment_picker_generation =>
                    {
                        let count_exceeded = self
                            .compose
                            .attachments
                            .len()
                            .saturating_add(attachments.len())
                            > attachment_store::MAX_ATTACHMENT_COUNT;
                        let total_bytes = self
                            .compose
                            .attachments
                            .iter()
                            .chain(attachments.iter())
                            .fold(0_u64, |total, attachment| {
                                total.saturating_add(attachment.size)
                            });
                        if count_exceeded
                            || total_bytes > attachment_store::MAX_TOTAL_ATTACHMENT_BYTES
                        {
                            let error = if count_exceeded {
                                attachment_store::AttachmentFailure::TooMany
                            } else {
                                attachment_store::AttachmentFailure::TotalTooLarge
                            };
                            self.compose_attach_notice =
                                Some(crate::l10n::tr(error.user_message()));
                            let payloads = attachments
                                .drain(..)
                                .filter_map(|attachment| attachment.payload_ref())
                                .collect::<Vec<_>>();
                            return self.remove_payloads_task(payloads);
                        }
                        self.compose.attachments.append(&mut attachments);
                        if let Some(received) = native {
                            received.finish();
                        }
                        self.compose_attach_notice = None;
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
                    Ok(attachments) => {
                        let payloads = attachments
                            .into_iter()
                            .filter_map(|attachment| attachment.payload_ref())
                            .collect::<Vec<_>>();
                        if !payloads.is_empty() {
                            return self.remove_payloads_task(payloads);
                        }
                    }
                    Err(error)
                        if self.compose.open && generation == self.attachment_picker_generation =>
                    {
                        self.compose_attach_notice = Some(crate::l10n::tr(error.user_message()));
                        if self.attachment_store.has_cleanup_debt() {
                            self.attachment_reconcile_needed = true;
                            return self.reconcile_attachment_store_task();
                        }
                    }
                    Err(_) => {
                        if self.attachment_store.has_cleanup_debt() {
                            self.attachment_reconcile_needed = true;
                            return self.reconcile_attachment_store_task();
                        }
                    }
                }
            }
            Message::InboundContentStaged(
                generation,
                refresh_generation,
                config,
                mailbox,
                result,
            ) => {
                let config = *config;
                if self.content_sync_inflight != Some((generation, refresh_generation)) {
                    let stale_payloads =
                        result.as_ref().map(Self::sync_payloads).unwrap_or_default();
                    return if stale_payloads.is_empty() {
                        Task::none()
                    } else {
                        self.remove_payloads_task(stale_payloads)
                    };
                }
                self.content_sync_inflight = None;
                if refresh_generation != self.mailbox_sync_generation {
                    let stale_payloads =
                        result.as_ref().map(Self::sync_payloads).unwrap_or_default();
                    let cleanup = if stale_payloads.is_empty() {
                        Task::none()
                    } else {
                        self.remove_payloads_task(stale_payloads)
                    };
                    return Task::batch([cleanup, self.start_next_mail_operation()]);
                }
                let config_is_current = self
                    .store
                    .account(&config.id)
                    .and_then(|account| account.config.as_ref())
                    == Some(&config);
                if !config_is_current {
                    let stale_payloads =
                        result.as_ref().map(Self::sync_payloads).unwrap_or_default();
                    self.mail_notice = Some(MailNotice {
                        title: crate::l10n::tr("Mailbox content ignored"),
                        body: "The account was removed or reconfigured while message content was being staged. No fetched message was adopted."
                            .to_owned(),
                        retryable: false,
                        can_cancel_outbox: false,
                    });
                    let cleanup = if stale_payloads.is_empty() {
                        Task::none()
                    } else {
                        self.remove_payloads_task(stale_payloads)
                    };
                    return Task::batch([cleanup, self.start_next_mail_operation()]);
                }
                match result {
                    Ok(batch) => return self.stage_sync_adoption(generation, config, batch),
                    Err(error) => {
                        self.attachment_reconcile_needed = true;
                        let intent = MailIntent::SyncMailbox {
                            config,
                            mailbox,
                            refresh_generation: self.mailbox_sync_generation,
                        };
                        self.retain_mailbox_sync_retry(intent, Instant::now());
                        self.mail_notice = Some(MailNotice {
                            title: crate::l10n::tr("Mailbox content was not staged"),
                            body: crate::l10n::message(
                                "mail-notice-the-last-good-mailbox-remains-visible-a-bounded-retry-is-scheduled",
                                &[("value1", (error.user_message()).to_owned())],
                            ),
                            retryable: true,
                            can_cancel_outbox: false,
                        });
                        return self.reconcile_attachment_store_task();
                    }
                }
            }
            Message::AttachmentSaved(payload, result) => {
                if self.attachment_export_pending.as_ref() != Some(&payload) {
                    return Task::none();
                }
                self.attachment_export_pending = None;
                self.attachment_notice = Some(match result {
                    Ok(()) => crate::l10n::tr("Attachment saved."),
                    Err(error) => crate::l10n::tr(error.user_message()),
                });
            }
            Message::RemoveComposeAttachment(i) => {
                if self.compose.open && i < self.compose.attachments.len() {
                    self.compose.attachments.remove(i);
                    let autosaved = self.autosave_compose_draft();
                    let draft_id = if autosaved {
                        self.compose.editing_draft_id.clone()
                    } else {
                        self.compose_ctl().sync_existing_draft_attachments()
                    };
                    if let Some(id) = draft_id {
                        self.draft_save_pending = true;
                        self.queue_server_mutation(
                            local_state::ServerMutationKind::DraftUpserted,
                            vec![id],
                        );
                    }
                }
            }
            Message::SendWithoutAttachment => {
                self.attachment_reminder_open = false;
                self.attachment_reminder_accepted = true;
                let task = self.update(Message::SendCompose);
                self.attachment_reminder_accepted = false;
                return task;
            }
            Message::DismissAttachmentReminder => self.attachment_reminder_open = false,
            Message::SendCompose => {
                if self.blocking_presentation_open() {
                    return Task::none();
                }
                if self.compose.open
                    && self.compose.mentions_missing_attachment()
                    && !self.attachment_reminder_accepted
                {
                    self.attachment_reminder_open = true;
                    return Task::none();
                }
                self.attachment_reminder_open = false;
                if self.pending_send.is_some() && !self.fixture_mode {
                    self.mail_notice = Some(MailNotice {
                        title: crate::l10n::tr("Message remains in Outbox"),
                        body: "Retry or cancel the queued message before sending another."
                            .to_owned(),
                        retryable: self.mail_retry.is_some(),
                        can_cancel_outbox: true,
                    });
                    return Task::none();
                }
                if let Some(error) = self.compose_attachment_failure() {
                    self.compose_attach_notice = Some(crate::l10n::message(
                        "mail-notice-message-not-queued",
                        &[("value1", (error.user_message()).to_owned())],
                    ));
                    return Task::none();
                }
                let previous_pending_targets = self
                    .fixture_mode
                    .then(|| self.pending_send_targets())
                    .flatten();
                if self.send_compose() {
                    if let Some(targets) = previous_pending_targets {
                        self.queue_server_mutation(local_state::ServerMutationKind::Send, targets);
                    }
                    self.compose_overflow_open = false;
                    self.sync_body_editor();
                    self.send_generation = self.send_generation.wrapping_add(1);
                    self.arm_undo_send_toast(std::time::Instant::now());
                    return self.dismiss_compose_focus(DismissCause::DefaultAction);
                }
            }
            Message::Toast(event) => return self.update_undo_send_toast(event),
            Message::Unsubscribe(message_id) => return self.unsubscribe(message_id),
            Message::SearchServer => return self.search_server(),
            Message::SetSnoozeChoice(label) => {
                if let Some(preset) = model::snooze::SnoozePreset::ALL
                    .into_iter()
                    .find(|preset| preset.label() == label)
                {
                    self.snooze_choice = preset;
                }
            }
            Message::ConfirmSnooze => {
                let ids = std::mem::take(&mut self.pending_snooze_targets);
                let until = self
                    .local_clock
                    .now()
                    .ok()
                    .map(|now| self.snooze_choice.until(now));
                if let Some(until) = until {
                    let layout = self.snooze(&ids, Some(until));
                    let dismiss = self.dismiss_snooze_dialog_focus(DismissCause::DefaultAction);
                    return Task::batch([layout, dismiss]);
                }
                return self.dismiss_snooze_dialog_focus(DismissCause::DefaultAction);
            }
            Message::RemoveSnooze => {
                let ids = std::mem::take(&mut self.pending_snooze_targets);
                let _ = self.snooze(&ids, None);
                return self.dismiss_snooze_dialog_focus(DismissCause::DefaultAction);
            }
            Message::CancelSnooze => {
                self.pending_snooze_targets.clear();
                return self.dismiss_snooze_dialog_focus(DismissCause::CloseButton);
            }
            Message::SnoozeWake => {
                self.wake_due_snoozes();
                return self.send_due_drafts();
            }
            Message::InboxWatch(update) => return self.reduce_inbox_watch(update),
            Message::NewMailNotificationWake => return self.reduce_new_mail_wake(),
            Message::OpenSchedule => {
                if self.compose.open
                    && self.compose.can_send()
                    && let Ok(now) = self.local_clock.now()
                {
                    return self.open_schedule(
                        self.compose
                            .editing_draft_id
                            .as_ref()
                            .and_then(|id| self.store.message(id))
                            .and_then(|m| m.send_at)
                            .unwrap_or_else(|| model::snooze::SnoozePreset::Tomorrow.until(now)),
                        None,
                    );
                }
            }
            Message::ScheduleChoice(label) => {
                if let Some(preset) = model::snooze::SnoozePreset::ALL
                    .into_iter()
                    .find(|preset| preset.label() == label)
                    && let Ok(now) = self.local_clock.now()
                {
                    let at = preset.until(now);
                    if let Some(editor) = self.schedule_editor.as_mut() {
                        if let Some(date) = i32::try_from(at.year).ok().and_then(|y| {
                            enzui_iced_core::civil_date::Date::new(y, at.month, at.day)
                        }) {
                            editor.date = date;
                        }
                        if let Some(time) = enzui_iced_core::time_of_day::TimeOfDay::hm(
                            at.minute / 60,
                            at.minute % 60,
                        ) {
                            editor.time = time;
                            editor.draft = crate::schedule::time_format().format(time);
                        }
                        editor.error = None;
                    }
                }
            }
            Message::ScheduleDate(event) => return self.schedule_date(event),
            Message::ScheduleTime(event) => self.schedule_time(event),
            Message::ScheduleAllowMissingAttachment(value) => {
                if let Some(editor) = self.schedule_editor.as_mut() {
                    editor.allow_missing_attachment = value;
                    editor.error = None;
                }
            }
            Message::ConfirmSchedule => return self.confirm_schedule(),
            Message::ScheduleDialog(event) => return self.schedule_event(event),
            Message::ChangeSchedule(id) => {
                if let Some(at) = self.store.message(&id).and_then(|m| m.send_at) {
                    return self.open_schedule(at, Some(id));
                }
            }
            Message::CancelScheduledSend(id) => {
                if let Some(message) = self
                    .store
                    .messages
                    .iter_mut()
                    .find(|m| m.id == id && m.send_at.is_some())
                {
                    message.send_at = None;
                    self.store.mark_dirty();
                    self.selection = SidebarSelection::Favorite(model::types::FavoriteKind::Drafts);
                    self.reconcile_after_mutation();
                    self.selected_conversation = self
                        .store
                        .conversations_for(&self.selection)
                        .iter()
                        .find(|c| c.message_ids.contains(&id))
                        .map(|c| c.id.clone());
                    self.narrow_nav.on_select(true);
                }
            }
            Message::Unsnooze => {
                if let Some(conversation) = self.selected_conversation.clone() {
                    let ids = self.message_ids_of(&conversation);
                    let _ = self.snooze(&ids, None);
                }
            }
            Message::AttachmentPreviewLoaded(key, payload) => {
                if let Some(slot) = self.attachment_previews.get_mut(&key) {
                    *slot = match payload {
                        Some(PreviewPayload::Image(handle)) => AttachmentPreview::Image(handle),
                        Some(PreviewPayload::Text(text)) => AttachmentPreview::Text(text),
                        None => AttachmentPreview::Unavailable,
                    };
                }
            }
            Message::StartQuickReply(mode) => {
                if self.quick_reply_for != self.selected_conversation {
                    self.quick_reply = text_editor::Content::new();
                }
                self.quick_reply_for = self.selected_conversation.clone();
                self.quick_reply_mode = Some(mode);
                return Task::batch([
                    iced::widget::operation::snap_to_end(
                        iced::advanced::widget::Id::new("mail-reader-scroll"),
                        iced::widget::operation::Animation::Instant,
                    ),
                    iced::widget::operation::focus(iced::advanced::widget::Id::new(
                        "mail-inline-reply",
                    )),
                ]);
            }
            Message::CancelQuickReply => {
                self.quick_reply = text_editor::Content::new();
                self.quick_reply_for = None;
                self.quick_reply_mode = None;
            }
            Message::QuickReplyEdit(action) => {
                if self.quick_reply_for != self.selected_conversation {
                    self.quick_reply = iced::widget::text_editor::Content::new();
                    self.quick_reply_for = self.selected_conversation.clone();
                }
                self.quick_reply.perform(action);
            }
            Message::QuickReplySend => return self.quick_reply(true),
            Message::QuickReplyExpand => return self.quick_reply(false),
            Message::UnsubscribeFinished(sender, result) => {
                self.unsubscribes.insert(
                    sender,
                    match result {
                        Ok(()) => UnsubscribeProgress::Done,
                        Err(reason) => UnsubscribeProgress::Failed(reason),
                    },
                );
            }
            Message::SaveDraft => {
                let was_open = self.compose.open;
                if let Some(id) = self.save_compose_draft() {
                    self.queue_server_mutation(
                        local_state::ServerMutationKind::DraftUpserted,
                        vec![id],
                    );
                }
                if self.local_timestamp_failure.is_some() {
                    return Task::none();
                }
                self.compose_overflow_open = false;
                self.sync_body_editor();
                self.reconcile_selection();
                if was_open {
                    return self.dismiss_compose_focus(DismissCause::DefaultAction);
                }
            }
            Message::AutosaveDraftTick => {
                self.checkpoint_compose_draft();
            }
            Message::FocusNext => {
                return operate(traverse::<Message>(Traversal::Next));
            }
            Message::FocusPrevious => {
                return operate(traverse::<Message>(Traversal::Previous));
            }
            Message::SearchEdited(v) => {
                if v.trim() != self.search_query.trim() {
                    self.clear_server_search();
                }
                self.search_query = v;
                self.search_highlight = None;
                self.search_open = true;
            }
            Message::SearchCleared => {
                self.clear_server_search();
                self.search_query.clear();
                self.search_highlight = None;
            }
            Message::SearchActivated => {
                // Idempotent: a second click on an open field must not toggle
                // the dropdown shut under the user's cursor.
                self.search_open = true;
                // `enz_search_field` has no focus callback, so the field is focused by its
                // stable id; otherwise a keyboard user gets an open dropdown they cannot type
                // into.
                return iced::widget::operation::focus(crate::view::search_field_id());
            }
            Message::ToggleTitlebarOverflow => {
                self.titlebar_overflow_open = !self.titlebar_overflow_open;
            }
            Message::CloseTitlebarOverflow => {
                self.titlebar_overflow_open = false;
            }
            Message::SearchDismissed => {
                // The query is kept: dismissing the dropdown must not discard typing.
                self.search_open = false;
                self.search_highlight = None;
            }
            Message::SearchNavigate(movement) => {
                if self.search_open {
                    let next = crate::view::search_suggestion_list(self)
                        .nav_target(self.search_highlight, movement);
                    self.search_highlight = next;
                }
            }
            Message::SearchCommitHighlighted => {
                let highlighted = if self.search_open {
                    let list = crate::view::search_suggestion_list(self);
                    list.highlighted_payload().cloned()
                } else {
                    None
                };
                if let Some(id) = highlighted {
                    return self.update(Message::SelectSearchResult(id));
                }
            }
            Message::SelectSearchScope(index) => {
                if let Some(scope) = SearchScope::ALL.get(index).copied() {
                    self.search_scope = scope;
                    self.search_highlight = None;
                }
            }
            Message::ToggleSidebarSection(account_id) => {
                if !self.collapsed_sections.remove(&account_id) {
                    self.collapsed_sections.insert(account_id);
                }
            }
            Message::SelectSearchResult(message_id) => {
                if let Some(msg) = self.store.message(&message_id) {
                    let sel = match &msg.location {
                        model::types::MessageLocation::Mailbox(kind) => {
                            SidebarSelection::AccountMailbox {
                                account_id: msg.account_id.clone(),
                                mailbox: *kind,
                            }
                        }
                        model::types::MessageLocation::Folder(folder_id) => {
                            SidebarSelection::Folder {
                                account_id: msg.account_id.clone(),
                                folder_id: folder_id.clone(),
                            }
                        }
                    };
                    self.flush_deferred_read();
                    self.selection = sel;
                    let convo_id = self
                        .store
                        .conversations_for(&self.selection)
                        .into_iter()
                        .find(|c| c.message_ids.iter().any(|id| id == &message_id))
                        .map(|c| c.id);
                    match convo_id {
                        Some(id) => self.open_conversation(id),
                        None => self.selected_conversation = None,
                    }
                    self.narrow_nav.on_select(true);
                }
                self.search_query.clear();
                self.search_open = false;
                self.search_highlight = None;
                self.nav.dismiss_overlay();
            }
            Message::OpenSettings => {
                self.settings.open();
                return self.arm_settings_focus();
            }
            Message::WelcomeAddAccount => {
                let opened = self.update_impl(Message::OpenSettings);
                let begun = self.update_impl(Message::BeginAddAccount);
                return Task::batch([opened, begun]);
            }
            Message::CloseSettings => {
                if self.account_edit.is_none() {
                    self.settings.edit_account.reset();
                }
                return self.dismiss_settings_focus(DismissCause::CloseButton);
            }
            Message::SettingsExitSettled(key) => {
                if self.settings.finish_exit(key) {
                    // A sign-in still waiting must not keep its port and pool worker.
                    self.end_oauth_sign_in();
                }
            }
            Message::SelectSettingsTab(index) => {
                // Ignored rather than clamped, which would silently select General.
                if let Some(tab) = SettingsTab::from_index(index) {
                    self.settings.select_tab(tab);
                }
            }
            Message::SettingsStackedBack => {
                // Meaningful only at the Narrow rung; a no-op elsewhere.
                self.settings.stacked_back_to_list();
            }
            Message::SettingsFocusEvent(event) => {
                return self.reduce_settings_dialog_event(event);
            }
            Message::SetRemoteContentPolicy(policy) => {
                self.privacy.get_mut().policy = policy;
                return self.save_preferences(preferences::SaveOrigin::Privacy);
            }
            // Not persisted: no fetch path reads either yet, so a saved value would
            // promise protection the app does not deliver.
            Message::SetProxyRemoteContent(on) => self.privacy.get_mut().proxy_remote_content = on,
            _ => unreachable!("reduce_batch2 handles only its own variants"),
        }
        Task::none()
    }
}
