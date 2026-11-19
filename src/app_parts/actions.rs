use super::*;

impl State {
    // A [`Message::MailAction`] running `action` on the highlighted rows.
    //
    // `None` when nothing is highlighted, so a triage key with no visible
    // target does nothing rather than choosing one.
    fn highlighted_verb(&self, action: mail_actions::MailAction) -> Option<Message> {
        let target = crate::view::action_target_for_breadth(self)?;
        Some(action.message(&target))
    }

    fn highlighted_toggle(
        &self,
        group: mail_actions::ActionGroup,
        is_pair: fn(mail_actions::MailAction) -> bool,
    ) -> Option<Message> {
        let target = crate::view::action_target_for_breadth(self)?;
        let state = crate::view::action_state_for_breadth(self)?;
        let action = mail_actions::actions_for(state, &target)
            .into_iter()
            .find(|a| a.group() == group && is_pair(*a))?;
        Some(action.message(&target))
    }

    // Resolve a decoded [`shortcuts::ShortcutAction`] into the reducer
    // [`Message`] it runs against live state, or `None` without a target.
    //
    // Triage verbs dispatch [`Message::MailAction`] against
    // [`crate::view::selection_breadth`], with toggle directions from
    // [`crate::mail_actions::actions_for`]. Reply and Reply-all target the newest
    // message of the reader-anchored thread.
    pub(crate) fn resolve_shortcut(&self, action: shortcuts::ShortcutAction) -> Option<Message> {
        use shortcuts::ShortcutAction::*;
        match action {
            ArchiveSelected => self.highlighted_verb(mail_actions::MailAction::Archive),
            DeleteSelected => self.highlighted_verb(mail_actions::MailAction::Delete),
            ToggleReadSelected => self.highlighted_toggle(mail_actions::ActionGroup::Triage, |a| {
                matches!(
                    a,
                    mail_actions::MailAction::MarkRead | mail_actions::MailAction::MarkUnread
                )
            }),
            ToggleFlaggedSelected => {
                self.highlighted_toggle(mail_actions::ActionGroup::Triage, |a| {
                    matches!(
                        a,
                        mail_actions::MailAction::Flag | mail_actions::MailAction::Unflag
                    )
                })
            }
            Compose => Some(Message::OpenCompose(ComposeMode::New)),
            Reply => self
                .selected_conversation
                .as_deref()
                .and_then(|c| self.newest_message_of(c))
                .filter(|id| {
                    self.store.message(id).is_some_and(|m| {
                        !matches!(
                            m.location,
                            model::types::MessageLocation::Mailbox(MailboxKind::Drafts)
                        )
                    })
                })
                .map(|id| Message::StartQuickReply(ComposeMode::Reply(id))),
            ReplyAll => self
                .selected_conversation
                .as_deref()
                .and_then(|c| self.newest_message_of(c))
                .filter(|id| {
                    self.store.message(id).is_some_and(|m| {
                        !matches!(
                            m.location,
                            model::types::MessageLocation::Mailbox(MailboxKind::Drafts)
                        )
                    })
                })
                .map(|id| Message::StartQuickReply(ComposeMode::ReplyAll(id))),
            Forward => self
                .selected_conversation
                .as_deref()
                .and_then(|c| self.newest_message_of(c))
                .map(|id| Message::OpenCompose(ComposeMode::Forward(id))),
            // `SendCompose` is a no-op with compose closed, so a stray chord never
            // commits a pending send.
            SendCompose => (!self.blocking_presentation_open()).then_some(Message::SendCompose),
            NextConversation => Some(Message::SelectNextConversation),
            PreviousConversation => Some(Message::SelectPreviousConversation),
            FocusSearch => Some(Message::FocusSearchField),
            ShowShortcuts => Some(Message::ToggleShortcuts),
        }
    }

    // Every message id in one conversation of the current selection; empty when
    // the conversation is gone, so a stale context verb touches nothing.
    pub(crate) fn message_ids_of(&self, conversation_id: &str) -> Vec<String> {
        self.store
            .conversations_shared(&self.selection)
            .iter()
            .find(|c| c.id == conversation_id)
            .map(|c| c.message_ids.clone())
            .unwrap_or_default()
    }

    // The newest message of one conversation, which Reply and Forward compose
    // against: `last()` of the oldest-first `Conversation::message_ids`.
    pub(crate) fn newest_message_of(&self, conversation_id: &str) -> Option<String> {
        self.store
            .conversations_shared(&self.selection)
            .iter()
            .find(|c| c.id == conversation_id)
            .and_then(|c| c.message_ids.last().cloned())
    }

    fn mail_action_ids(&self, target: &mail_actions::ActionTarget) -> Vec<String> {
        use mail_actions::ActionTarget;
        match target {
            ActionTarget::Message { message_id } => {
                if self.store.message(message_id).is_some() {
                    vec![message_id.clone()]
                } else {
                    Vec::new()
                }
            }
            ActionTarget::Conversation {
                conversation_id, ..
            } => self.message_ids_of(conversation_id),
            ActionTarget::Selection {
                conversation_ids, ..
            } => {
                let set: std::collections::HashSet<String> =
                    conversation_ids.iter().cloned().collect();
                model::bulk::expand_conversation_ids(&self.store, &self.selection, &set)
            }
        }
    }

    pub(crate) fn perform_move(&mut self, ids: &[String], mailbox: MailboxKind) {
        let changed = ids
            .iter()
            .filter(|id| {
                self.store.message(id).is_some_and(|message| {
                    message.location != model::types::MessageLocation::Mailbox(mailbox)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        let previous: Vec<_> = changed
            .iter()
            .filter_map(|id| {
                self.store
                    .message(id)
                    .map(|message| (id.clone(), message.location.clone()))
            })
            .collect();
        let journal_len = self.pending_server_mutations.len();
        if changed.is_empty()
            || !self.queue_server_mutations_for_messages(
                local_state::ServerMutationKind::Move(mailbox),
                &changed,
            )
        {
            return;
        }
        let queued: Vec<u64> = self.pending_server_mutations[journal_len..]
            .iter()
            .map(|mutation| mutation.id)
            .collect();
        let order: Vec<String> = self
            .store
            .conversations_for(&self.selection)
            .into_iter()
            .map(|c| c.id)
            .collect();
        let open_index = self
            .selected_conversation
            .as_ref()
            .and_then(|open| order.iter().position(|id| id == open));
        if model::bulk::bulk_move(&mut self.store, &changed, mailbox) == 0 {
            return;
        }
        let reopen = self.selected_conversation.clone();
        let open_moved = self
            .selected_conversation
            .as_ref()
            .is_some_and(|open| self.message_ids_of(open).is_empty());
        self.reconcile_after_mutation();
        let left = order
            .len()
            .saturating_sub(self.store.conversations_for(&self.selection).len());
        self.offer_move_undo(mailbox, left.max(1), previous, queued, reopen);
        if open_moved && let Some(index) = open_index {
            let remaining: std::collections::HashSet<String> = self
                .store
                .conversations_for(&self.selection)
                .into_iter()
                .map(|c| c.id)
                .collect();
            let next = order[index..]
                .iter()
                .chain(order[..index].iter().rev())
                .find(|id| remaining.contains(*id))
                .cloned();
            if let Some(next) = next {
                self.open_conversation(next);
            }
        }
    }

    fn offer_move_undo(
        &mut self,
        mailbox: MailboxKind,
        conversations: usize,
        previous: Vec<(String, model::types::MessageLocation)>,
        mutations: Vec<u64>,
        reopen: Option<String>,
    ) {
        let toast = self
            .undo_send_toasts
            .push(undo_move_toast_spec(mailbox, conversations));
        self.held_mutations.extend(mutations.iter().copied());
        self.undo_moves.push(UndoMove {
            toast,
            previous,
            mutations,
            reopen,
        });
    }

    pub(crate) fn release_settled_move_undos(&mut self) -> Task<Message> {
        let live = self.undo_send_toasts.ids();
        let before = self.undo_moves.len();
        let mut released = Vec::new();
        self.undo_moves.retain(|undo| {
            let keep = live.contains(&undo.toast);
            if !keep {
                released.extend(undo.mutations.iter().copied());
            }
            keep
        });
        for id in &released {
            self.held_mutations.remove(id);
        }
        if self.undo_moves.len() == before {
            return Task::none();
        }
        self.start_next_mail_operation()
    }

    pub(crate) fn undo_move_from_toast(&mut self, event: &ToastEvent) {
        if let ToastEvent::Action(id, label) = event
            && label == UNDO_MOVE_LABEL
            && let Some(index) = self.undo_moves.iter().position(|undo| undo.toast == *id)
        {
            let undo = self.undo_moves.remove(index);
            self.undo_move(undo);
        }
    }

    // Held mutations never reached the server, so dropping them and the local
    // move is the whole undo.
    fn undo_move(&mut self, undo: UndoMove) {
        for id in &undo.mutations {
            self.held_mutations.remove(id);
        }
        self.pending_server_mutations
            .retain(|mutation| !undo.mutations.contains(&mutation.id));
        let mut restored = 0;
        for (id, location) in undo.previous {
            if let Some(message) = self.store.messages.iter_mut().find(|m| m.id == id) {
                message.location = location;
                restored += 1;
            }
        }
        if restored == 0 {
            return;
        }
        self.store.mark_dirty();
        self.reconcile_after_mutation();
        if let Some(conversation) = undo.reopen
            && !self.message_ids_of(&conversation).is_empty()
        {
            self.open_conversation(conversation);
        }
    }

    // Opens `conversation_id` in the reader and marks it read. Where read mail
    // leaves the list, the mark waits until the reader moves on, so the open
    // row does not vanish under the pointer.
    pub(crate) fn open_conversation(&mut self, conversation_id: String) {
        if self
            .deferred_read
            .as_ref()
            .is_some_and(|(open, _)| *open == conversation_id)
        {
            self.selected_conversation = Some(conversation_id);
            return;
        }
        self.flush_deferred_read();
        let ids = self.message_ids_of(&conversation_id);
        self.withdraw_seen_new_mail(&ids);
        self.selected_conversation = Some(conversation_id.clone());
        if matches!(
            self.selection,
            SidebarSelection::SmartMailbox(model::sidebar_layout::SmartMailboxKind::Unread)
        ) {
            self.deferred_read = Some((conversation_id, ids));
        } else {
            self.perform_set_unread(&ids, false);
        }
    }

    pub(crate) fn flush_deferred_read(&mut self) {
        if let Some((_, ids)) = self.deferred_read.take() {
            self.perform_set_unread(&ids, false);
        }
    }

    fn perform_set_unread(&mut self, ids: &[String], unread: bool) {
        let changed = ids
            .iter()
            .filter(|id| {
                self.store
                    .message(id)
                    .is_some_and(|message| message.unread != unread)
            })
            .cloned()
            .collect::<Vec<_>>();
        if changed.is_empty()
            || !self.queue_server_mutations_for_messages(
                local_state::ServerMutationKind::SetUnread(unread),
                &changed,
            )
        {
            return;
        }
        if model::bulk::bulk_set_unread(&mut self.store, &changed, unread) > 0 {
            self.reconcile_after_mutation();
        }
    }

    fn perform_set_flagged(&mut self, ids: &[String], flagged: bool) {
        let changed = ids
            .iter()
            .filter(|id| {
                self.store
                    .message(id)
                    .is_some_and(|message| message.flagged != flagged)
            })
            .cloned()
            .collect::<Vec<_>>();
        if changed.is_empty()
            || !self.queue_server_mutations_for_messages(
                local_state::ServerMutationKind::SetFlagged(flagged),
                &changed,
            )
        {
            return;
        }
        if model::bulk::bulk_set_flagged(&mut self.store, &changed, flagged) > 0 {
            self.reconcile_after_mutation();
        }
    }

    pub(crate) fn apply_mail_action(
        &mut self,
        action: mail_actions::MailAction,
        target: mail_actions::ActionTarget,
    ) -> Task<Message> {
        use mail_actions::MailAction as A;
        let anchor = target.anchor_message_id().to_owned();
        if matches!(action, A::Reply | A::ReplyAll | A::Snooze)
            && self
                .mail_action_ids(&target)
                .iter()
                .filter_map(|id| self.store.message(id))
                .any(|m| {
                    matches!(
                        m.location,
                        model::types::MessageLocation::Mailbox(MailboxKind::Drafts)
                    )
                })
        {
            return Task::none();
        }
        match action {
            A::Reply => return self.update(Message::StartQuickReply(ComposeMode::Reply(anchor))),
            A::ReplyAll => {
                return self.update(Message::StartQuickReply(ComposeMode::ReplyAll(anchor)));
            }
            A::Forward => return self.update(Message::OpenCompose(ComposeMode::Forward(anchor))),
            A::BlockSender => return self.update(Message::BlockSenderOf(anchor)),
            A::UnblockSender => return self.update(Message::UnblockSenderOf(anchor)),
            A::MoveTo => {
                // Arms the picker only; `ConfirmMessageMove` is the sole mutation path.
                let ids = self.mail_action_ids(&target);
                if ids.is_empty() {
                    return Task::none();
                }
                self.pending_move_targets = ids;
                self.message_move_destination = MailboxKind::Archive;
                return self.arm_move_dialog_focus();
            }
            A::Snooze => {
                let ids = self.mail_action_ids(&target);
                if ids.is_empty() {
                    return Task::none();
                }
                self.pending_snooze_targets = ids;
                self.snooze_choice = model::snooze::SnoozePreset::Tomorrow;
                return self.arm_snooze_dialog_focus();
            }
            A::MarkRead | A::MarkUnread => {
                let ids = self.mail_action_ids(&target);
                self.perform_set_unread(&ids, action == A::MarkUnread);
            }
            A::Flag | A::Unflag => {
                let ids = self.mail_action_ids(&target);
                self.perform_set_flagged(&ids, action == A::Flag);
            }
            A::Archive | A::Junk | A::Delete => {
                let Some(mailbox) = mail_actions::move_destination(action) else {
                    return Task::none();
                };
                let ids = self.mail_action_ids(&target);
                self.perform_move(&ids, mailbox);
            }
        }
        Task::none()
    }

    pub(crate) fn perform_mark_all_read(&mut self, selection: &SidebarSelection) {
        let ids = self
            .store
            .sorted_messages(selection)
            .into_iter()
            .filter(|message| message.unread)
            .map(|message| message.id.clone())
            .collect::<Vec<_>>();
        if !ids.is_empty()
            && self.queue_server_mutations_for_messages(
                local_state::ServerMutationKind::SetUnread(false),
                &ids,
            )
        {
            let _ = model::bulk::mark_all_read(&mut self.store, selection);
        }
    }

    pub(crate) fn reconcile_after_mutation(&mut self) {
        self.reconcile_selection();
    }

    // `Tracked::get_mut` always advances the revision, so edits are
    // evaluated on a clone before committing.
    pub(crate) fn edit_sidebar_layout(
        &mut self,
        edit: impl FnOnce(&mut model::sidebar_layout::SidebarLayout) -> bool,
    ) -> Task<Message> {
        let mut next = self.sidebar_layout.get().clone();
        if !edit(&mut next) || &next == self.sidebar_layout.get() {
            return Task::none();
        }
        *self.sidebar_layout.get_mut() = next;
        self.after_sidebar_layout_change()
    }

    fn after_sidebar_layout_change(&mut self) -> Task<Message> {
        let reconcile = self.reconcile_sidebar_selection();
        let save = self.save_preferences(preferences::SaveOrigin::SidebarLayout);
        Task::batch([reconcile, save])
    }

    // An account-free store has no mountable fallback; retain the existing
    // selection until an account or configurable row is available.
    pub(crate) fn reconcile_sidebar_selection(&mut self) -> Task<Message> {
        if self.sidebar_selection_is_mounted(&self.selection) {
            return Task::none();
        }
        let replacement = self
            .sidebar_layout
            .visible_selections()
            .into_iter()
            .next()
            .or_else(|| {
                self.store.accounts.iter().find_map(|account| {
                    account
                        .mailboxes
                        .first()
                        .map(|mailbox| SidebarSelection::AccountMailbox {
                            account_id: account.id.clone(),
                            mailbox: *mailbox,
                        })
                })
            });
        let Some(replacement) = replacement else {
            return Task::none();
        };
        // Bypass the outer update wrapper so toast expiry and missing-host
        // reconciliation run once for the originating sidebar edit.
        self.update_impl(Message::SelectSidebar(replacement))
    }

    fn sidebar_selection_is_mounted(&self, sel: &SidebarSelection) -> bool {
        match sel {
            SidebarSelection::Favorite(_) | SidebarSelection::SmartMailbox(_) => {
                self.sidebar_layout.visible_selections().contains(sel)
                    || matches!(sel, SidebarSelection::SmartMailbox(kind) if self.active_smart_mailboxes().contains(kind))
            }
            SidebarSelection::AccountMailbox {
                account_id,
                mailbox,
            } => self
                .store
                .accounts
                .iter()
                .any(|a| &a.id == account_id && a.mailboxes.contains(mailbox)),
            SidebarSelection::Folder {
                account_id,
                folder_id,
            } => self
                .store
                .accounts
                .iter()
                .any(|a| &a.id == account_id && a.folders.iter().any(|f| &f.id == folder_id)),
        }
    }

    pub(crate) fn refresh_rules_count(&mut self) {
        self.rules_count_label = crate::settings_view::rules_count_label(self.rules.len());
    }

    // Arm the canonical Block-sender confirmation, the single path every Block
    // sender row or pill funnels through.
    //
    // `self.privacy` is untouched until `ConfirmBlockSender` resolves
    // affirmatively, so no surface may call `block_sender` directly.
    pub(crate) fn arm_block_sender_confirmation(&mut self, addr: String) {
        self.pending_block_sender = Some(addr);
        self.confirm_block_sender = enzui_iced_core::confirm::ConfirmState::new(None);
    }

    pub(crate) fn update(&mut self, msg: Message) -> Task<Message> {
        enzui_iced_core::crash_report::observe(&msg);
        self.localizer.clone().scope(|| {
            let task = self.update_localized(msg);
            self.forget_gone_notices();
            self.refresh_desktop_search();
            task
        })
    }

    // A closed corner notice comes back if its cause clears and recurs.
    fn forget_gone_notices(&mut self) {
        if self.hidden_notices.is_empty() {
            return;
        }
        let showing = crate::view::notice_keys(self);
        self.hidden_notices.retain(|key| showing.contains(key));
    }

    fn update_localized(&mut self, msg: Message) -> Task<Message> {
        if let Message::DraftPointerMoved(point) = msg {
            self.draft_pointer = Some(point);
            return enzui_iced_core::native_loop::quiet();
        }
        if let Message::AttachParentReady(ready) = msg {
            return self
                .attach_chooser
                .parent_ready(ready)
                .map_or_else(Task::none, |(target, outcome)| {
                    self.finish_attach_pick(target, outcome)
                });
        }
        if let Message::NativeDrop(event) = msg {
            return self.native_drop_event(event);
        }
        if matches!(msg, Message::DesktopWake) {
            return self.desktop_wake();
        }
        if matches!(msg, Message::DesktopActivated) {
            return Task::none();
        }
        if let Message::HtmlViewports(reports) = msg {
            for (id, visible, width) in reports {
                self.html_reader.report_visibility(id, visible, width);
            }
            return self.update_html_reader();
        }
        if let Message::HtmlNativeReady(ready) = msg {
            if ready == enzui_iced_core::native_loop::NativeReady::Unavailable {
                self.html_reader.stop_watcher();
                return self.update_html_reader();
            }
            let revision = self.html_reader.revision;
            let task = self.update_html_reader();
            return if revision == self.html_reader.revision {
                Task::batch([task, enzui_iced_core::native_loop::quiet()])
            } else {
                task
            };
        }
        if matches!(msg, Message::HtmlTick) {
            return self.update_html_reader();
        }
        if let Message::HtmlAllocated(request, result) = msg {
            self.html_reader.allocated(request, result);
            return self.update_html_reader();
        }
        if let Message::HtmlInput(view, input) = msg {
            self.html_reader.input(view, input);
            return Task::none();
        }
        // Connector events bypass the close and startup filters; the
        // host must answer every call, so it refuses them itself.
        #[cfg(feature = "center")]
        match msg {
            Message::Center(event) => return self.center_event(event),
            // Lifecycle wakes are not user actions; startup and close
            // filters do not apply to them.
            Message::ApplicationWake => return self.application_wake(),
            Message::BackgroundTaskRefused(error) => {
                eprintln!("mail-app: background task refused: {error}");
                return Task::none();
            }
            Message::CenterDeadline => return self.center_deadline(),
            _ => {}
        }
        if !self.close_message_allowed(&msg) {
            return Task::none();
        }
        // Refusal precedes every snapshot and the reducer itself, so a refused
        // action can neither mutate the loading model nor schedule a save of it.
        if !self.startup_message_allowed(&msg) {
            self.notify_startup_loading();
            return Task::none();
        }
        let close_progress_time = self.close_progress_time(&msg);
        let html_before = (
            self.store.generation(),
            self.selected_conversation.clone(),
            self.effective_html_dark_palette(),
            self.privacy.revision(),
        );
        let payloads_before = self.attachment_payloads();
        let local_state_before = self.local_state_token();
        let schedule_local_save = !matches!(&msg, Message::LocalStateSaved(..));
        let schedule_payload_cleanup = !matches!(&msg, Message::AttachmentCleanupFinished(..));

        let task = self.update_impl(msg);
        let html_after = (
            self.store.generation(),
            self.selected_conversation.clone(),
            self.effective_html_dark_palette(),
            self.privacy.revision(),
        );
        let html = if html_before != html_after {
            self.update_html_reader()
        } else {
            Task::none()
        };
        let task = Task::batch([task, html]);
        let send_reconciliation = self.reconcile_undo_send_toast_removals();
        #[cfg(feature = "center")]
        self.reconcile_review();
        let _ = self
            .undo_send_toasts
            .sync_expiry(&mut self.undo_send_expiry, std::time::Instant::now());
        let released = self.release_settled_move_undos();
        let html_ids = if self.compose.open {
            Vec::new()
        } else {
            self.selected_conversation
                .as_deref()
                .map(|id| self.message_ids_of(id))
                .unwrap_or_default()
        };
        self.html_reader.retain(&html_ids);
        let store = &self.store;
        self.remote_images_once
            .retain(|id| store.message(id).is_some());
        self.html_reader
            .remove_deleted(|id| store.message(id).is_some());
        let previews = self.request_attachment_previews();
        let task = Task::batch([task, released, previews]);
        let task = match self.close_missing_modal_hosts() {
            Some(close) => Task::batch([task, close]),
            None => task,
        };
        let payloads_after = self.attachment_payloads();
        let mut retired = payloads_before
            .into_iter()
            .filter(|(id, identity)| payloads_after.get(id) != Some(identity))
            .collect::<Vec<_>>();
        retired.sort_unstable_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
        for (id, identity) in retired {
            let retired = model::types::AttachmentPayloadRef { id, identity };
            if !self.pending_payload_cleanup.contains(&retired) {
                self.pending_payload_cleanup.push(retired);
            }
        }
        let local_state_changed = local_state_before != self.local_state_token();
        let local_save = if schedule_local_save && local_state_changed {
            self.save_local_state()
        } else {
            Task::none()
        };
        let payload_cleanup =
            if local_state_changed || !schedule_payload_cleanup || !self.local_state_is_published()
            {
                Task::none()
            } else {
                self.cleanup_retired_payloads()
            };
        let close_progress =
            close_progress_time.map_or_else(Task::none, |now| self.advance_close_settlement(now));
        #[cfg(feature = "center")]
        let waiting_calls = self.answer_waiting_center_calls();
        #[cfg(not(feature = "center"))]
        let waiting_calls = Task::none();
        Task::batch([
            task,
            send_reconciliation,
            local_save,
            payload_cleanup,
            close_progress,
            waiting_calls,
            self.desktop_pending(),
        ])
    }

    pub(crate) fn update_impl(&mut self, msg: Message) -> Task<Message> {
        if !self.compose.open
            && self.quick_reply_mode.is_some()
            && matches!(
                msg,
                Message::SelectSidebar(_)
                    | Message::SelectConversation { .. }
                    | Message::NarrowBack
                    | Message::SelectSearchResult(_)
                    | Message::SelectNextConversation
                    | Message::SelectPreviousConversation
                    | Message::OpenCompose(_)
                    | Message::OpenSettings
            )
        {
            if self.quick_reply_text().is_some() {
                let saved = self.save_inline_reply();
                if self.compose.open || self.quick_reply_mode.is_some() {
                    return saved;
                }
                return Task::batch([saved, self.update(msg)]);
            }
            self.quick_reply_mode = None;
            self.quick_reply_for = None;
        }
        if self.compose.open && matches!(msg, Message::OpenSettings) {
            self.checkpoint_compose_draft();
        }
        if self.compose.open
            && matches!(
                msg,
                Message::SelectSidebar(_)
                    | Message::SelectConversation { .. }
                    | Message::NarrowBack
                    | Message::SelectSearchResult(_)
                    | Message::SelectNextConversation
                    | Message::SelectPreviousConversation
                    | Message::OpenCompose(_)
            )
        {
            let leave = if self.compose.can_save_draft()
                && (self.compose.is_user_edited() || self.compose.editing_draft_id.is_some())
            {
                Message::SaveDraft
            } else {
                Message::CloseCompose
            };
            let saved = self.update(leave);
            if self.compose.open {
                return saved;
            }
            return Task::batch([saved, self.update(msg)]);
        }
        match msg {
            Message::DraftClickAway | Message::DraftWindowBlur => {
                let away = matches!(msg, Message::DraftWindowBlur)
                    || self
                        .draft_pointer
                        .zip(self.draft_bounds.get())
                        .is_some_and(|(point, bounds)| !bounds.contains(point));
                if !away {
                    return Task::none();
                }
                if self.compose.open {
                    return self.update(Message::AutosaveDraftTick);
                }
                if self.quick_reply_mode.is_some() && self.quick_reply_text().is_some() {
                    return self.save_inline_reply();
                }
                Task::none()
            }
            Message::DraftPointerMoved(..) => Task::none(),
            Message::LeaveCompose => {
                if self.compose.open {
                    self.update(
                        if self.compose.can_save_draft()
                            && (self.compose.is_user_edited()
                                || self.compose.editing_draft_id.is_some())
                        {
                            Message::SaveDraft
                        } else {
                            Message::CloseCompose
                        },
                    )
                } else if self.quick_reply_mode.is_some() {
                    self.save_inline_reply()
                } else {
                    Task::none()
                }
            }
            Message::HtmlViewports(..)
            | Message::HtmlTick
            | Message::HtmlInput(..)
            | Message::HtmlAllocated(..)
            | Message::HtmlNativeReady(..) => Task::none(),
            #[cfg(feature = "center")]
            Message::Center(event) => self.center_event(event),
            #[cfg(feature = "center")]
            Message::ApplicationWake => self.application_wake(),
            #[cfg(feature = "center")]
            Message::BackgroundTaskRefused(_) => Task::none(),
            #[cfg(feature = "center")]
            Message::CenterDeadline => self.center_deadline(),
            Message::ApplyRules
            | Message::RuleNameEdited(..)
            | Message::RuleConditionFieldSelected(..)
            | Message::RuleConditionMatchSelected(..)
            | Message::RuleConditionValueEdited(..)
            | Message::RuleAddCondition
            | Message::RuleRemoveCondition(..)
            | Message::RuleCombinatorSelected(..)
            | Message::RuleActionKindSelected(..)
            | Message::RuleActionMailboxSelected(..)
            | Message::RuleAddAction
            | Message::RuleRemoveAction(..) => self.reduce_batch6(msg),
            Message::SaveAttachment(..)
            | Message::AttachmentBusy(..)
            | Message::AttachmentOpenPortalTick(..)
            | Message::AttachmentOpenFinished(..)
            | Message::OpenAttachmentMenu { .. }
            | Message::DismissAttachmentNotice
            | Message::OpenMessageSelectionMenu(..)
            | Message::ClearMessageSelection
            | Message::ClipboardWritten
            | Message::SetMessageMoveDestination(..)
            | Message::ConfirmMessageMove
            | Message::CancelMessageMove
            | Message::PickerDialogEvent(..)
            | Message::ConfirmBlockSender(..)
            | Message::OpenConversationMenu(..)
            | Message::OpenSidebarMenu { .. }
            | Message::ContextMenuEvent(..)
            | Message::MailAction(..)
            | Message::ContextMarkAllRead(..)
            | Message::AddFavorite(..)
            | Message::RemoveFavorite(..)
            | Message::RemoveSmartMailbox(..)
            | Message::AddSmartMailbox(..)
            | Message::SetFavoritesVisible(..)
            | Message::SetSmartMailboxesVisible(..)
            | Message::ReorderFavorite(..)
            | Message::ReorderSmartMailbox(..)
            | Message::OpenRules
            | Message::CloseRules
            | Message::RulesHostEvent(..)
            | Message::NewRule
            | Message::EditRule(..)
            | Message::CancelRuleEdit
            | Message::SaveRule
            | Message::RequestDeleteRule(..)
            | Message::ConfirmDeleteRuleEvent(..)
            | Message::MoveRuleUp(..)
            | Message::MoveRuleDown(..)
            | Message::RuleDropped(..) => self.reduce_batch5(msg),
            Message::SelectAllConversations
            | Message::ClearSelection
            | Message::SelectNextConversation
            | Message::SelectPreviousConversation
            | Message::FocusSearchField
            | Message::Shortcut(..)
            | Message::KeyPressed { .. }
            | Message::Shortcuts(..)
            | Message::PersistenceBusy(..)
            | Message::BlockingRetryTick
            | Message::ShortcutsSaved(..)
            | Message::PreferencesSaved(..)
            | Message::SyncAdoptionSaved(..)
            | Message::LocalStateSaved(..)
            | Message::AttachmentCleanupFinished(..)
            | Message::AttachmentReconcileFinished(..)
            | Message::AttachmentStartupSurveyFinished(..)
            | Message::FrameLoadSettled(..)
            | Message::FrameLoadFailed(..)
            | Message::FrameCustodySettled(..)
            | Message::OutgoingMimePrepared(..)
            | Message::ToggleShortcuts
            | Message::ShortcutsDialogEvent(..)
            | Message::MarkAllRead
            | Message::RequestEmptyMailbox(..)
            | Message::ConfirmEmptyEvent(..)
            | Message::ToggleMailboxOverflow
            | Message::CloseMailboxOverflow
            | Message::SetPreferPlainText(..)
            | Message::SetDarkHtml(..)
            | Message::SetNotificationSettings(..)
            | Message::TestNotification
            | Message::SetMessagePlainText(..)
            | Message::ToggleQuotedText(..)
            | Message::ToggleReaderOverflow(..)
            | Message::CloseReaderOverflow(..)
            | Message::BlockSenderOf(..)
            | Message::UnblockSenderOf(..)
            | Message::ComposeFormatMenuEvent(..)
            | Message::ComposeColorPicked(..)
            | Message::SenderMenuEvent(..)
            | Message::CopyMessageSelection(..)
            | Message::ShowAttachmentDetails { .. }
            | Message::OpenAttachment(..)
            | Message::AttachmentPathResolved(..) => self.reduce_batch4(msg),
            Message::SetAutoDeclineReadReceipts(..)
            | Message::TogglePrivacyAdvanced(..)
            | Message::ToggleAllowListExpanded(..)
            | Message::ToggleBlockListExpanded(..)
            | Message::AllowInputEdited(..)
            | Message::BlockInputEdited(..)
            | Message::AddAllowedSender
            | Message::AddBlockedSender
            | Message::RemoveAllowedSender(..)
            | Message::RemoveBlockedSender(..)
            | Message::ToggleAccountExpanded(..)
            | Message::AccountSignatureAction(..)
            | Message::SetAccountAppendSignature(..)
            | Message::BeginAddAccount
            | Message::CancelAddAccount
            | Message::AddAccountEmailEdited(..)
            | Message::AddAccountNameEdited(..)
            | Message::AddAccountServersFound(..)
            | Message::AddAccountContinue
            | Message::AddAccountBack
            | Message::AddAccountFieldEdited(..)
            | Message::AddAccountPasswordEdited(..)
            | Message::ToggleAddAccountPasswordRevealed
            | Message::AddAccountOAuthSignIn
            | Message::AddAccountOAuthCancel
            | Message::AddAccountOAuthFinished(..)
            | Message::SetAddAccountIncomingEncryption(..)
            | Message::SetAddAccountSmtpEncryption(..)
            | Message::SetAddAccountAuth(..)
            | Message::ToggleAddAccountAdvanced(..)
            | Message::CommitAddAccount
            | Message::RequestRemoveAccount(..)
            | Message::ConfirmRemoveAccount(..)
            | Message::RetryRemoveAccount
            | Message::CancelRemoveAccount
            | Message::BeginEditAccount(..)
            | Message::EditAccountFieldEdited(..)
            | Message::EditAccountPasswordEdited(..)
            | Message::ToggleEditAccountPasswordRevealed
            | Message::SetEditAccountIncomingEncryption(..)
            | Message::SetEditAccountSmtpEncryption(..)
            | Message::CommitEditAccount
            | Message::RetryEditAccount
            | Message::CancelEditAccount => self.reduce_batch3(msg),
            Message::SetComposeFrom(..)
            | Message::ComposeRecipientSelected(..)
            | Message::ComposeRecipientCustom(..)
            | Message::ComposeBodyAction(..)
            | Message::SetComposeFormat(..)
            | Message::ComposeRichEditAction(..)
            | Message::ToggleComposeOverflow
            | Message::CloseComposeOverflow
            | Message::AttachParentReady(..)
            | Message::NativeDrop(..)
            | Message::DroppedAttachment(..)
            | Message::AttachFile
            | Message::AttachFallback(..)
            | Message::AttachPickerOutcome(..)
            | Message::DesktopWake
            | Message::DesktopActivated
            | Message::AttachPortalTick
            | Message::AttachmentsStaged(..)
            | Message::InboundContentStaged(..)
            | Message::AttachmentSaved(..)
            | Message::RemoveComposeAttachment(..)
            | Message::SendCompose
            | Message::Toast(..)
            | Message::Unsubscribe(..)
            | Message::SearchServer
            | Message::AttachmentPreviewLoaded(..)
            | Message::SetSnoozeChoice(..)
            | Message::ConfirmSnooze
            | Message::RemoveSnooze
            | Message::CancelSnooze
            | Message::SnoozeWake
            | Message::InboxWatch(..)
            | Message::NewMailNotificationWake
            | Message::Unsnooze
            | Message::OpenSchedule
            | Message::ScheduleChoice(..)
            | Message::ScheduleDate(..)
            | Message::ScheduleTime(..)
            | Message::ConfirmSchedule
            | Message::ScheduleDialog(..)
            | Message::ChangeSchedule(..)
            | Message::ScheduleAllowMissingAttachment(..)
            | Message::CancelScheduledSend(..)
            | Message::StartQuickReply(..)
            | Message::CancelQuickReply
            | Message::QuickReplyEdit(..)
            | Message::SendWithoutAttachment
            | Message::DismissAttachmentReminder
            | Message::QuickReplySend
            | Message::QuickReplyExpand
            | Message::UnsubscribeFinished(..)
            | Message::SaveDraft
            | Message::AutosaveDraftTick
            | Message::FocusNext
            | Message::FocusPrevious
            | Message::SearchEdited(..)
            | Message::SearchCleared
            | Message::SearchActivated
            | Message::ToggleTitlebarOverflow
            | Message::CloseTitlebarOverflow
            | Message::SearchDismissed
            | Message::SearchNavigate(..)
            | Message::SearchCommitHighlighted
            | Message::SelectSearchScope(..)
            | Message::ToggleSidebarSection(..)
            | Message::SelectSearchResult(..)
            | Message::OpenSettings
            | Message::WelcomeAddAccount
            | Message::CloseSettings
            | Message::SettingsExitSettled(..)
            | Message::SelectSettingsTab(..)
            | Message::SettingsStackedBack
            | Message::SettingsFocusEvent(..)
            | Message::SetRemoteContentPolicy(..)
            | Message::SetProxyRemoteContent(..) => self.reduce_batch2(msg),
            Message::Theme(..)
            | Message::WindowResized(..)
            | Message::SelectSidebar(..)
            | Message::SelectConversation { .. }
            | Message::NarrowBack
            | Message::SidebarState(..)
            | Message::AlwaysAllowSender(..)
            | Message::AllowImagesOnce(..)
            | Message::OpenUrlDialogEvent(..)
            | Message::RequestOpenUrl(..)
            | Message::ConfirmOpenUrl(..)
            | Message::OpenUrlPortalTick(..)
            | Message::OpenUrlFinished(..)
            | Message::DismissOpenUrlNotice
            | Message::Window(..)
            | Message::WindowCloseResolved(Some(..) | None)
            | Message::CloseSettlementTick(_, _)
            | Message::RetryCloseSettlement
            | Message::DiscardCloseSettlement
            | Message::CancelCloseSettlement
            | Message::RetryLocalTimestamp
            | Message::CredentialStoreReady(..)
            | Message::CredentialFinished(..)
            | Message::MailOperationFinished(..)
            | Message::RetryMailOperation
            | Message::CancelMailOperation
            | Message::DismissMailNotice
            | Message::HideNotice(..)
            | Message::LoadOlderMailbox
            | Message::RefreshMailbox
            | Message::PeriodicMailboxSync(..)
            | Message::MailboxSyncRetryTick(..)
            | Message::OpenCompose(..)
            | Message::CloseCompose
            | Message::ShowCc
            | Message::ShowBcc
            | Message::ComposeRecipientMenuEvent(..)
            | Message::ComposeToEdited(..)
            | Message::ComposeCcEdited(..)
            | Message::ComposeBccEdited(..)
            | Message::ComposeSubjectEdited(..) => self.reduce_batch1(msg),
        }
    }
}
