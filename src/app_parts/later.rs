//! Snooze and send-later: hiding a conversation until a time, and
//! scheduling a draft to send at one.

use super::*;

impl State {
    pub(crate) fn active_smart_mailboxes(&self) -> Vec<model::sidebar_layout::SmartMailboxKind> {
        use model::sidebar_layout::SmartMailboxKind;
        let mut items = self.sidebar_layout.visible_smart().to_vec();
        for kind in [SmartMailboxKind::Scheduled, SmartMailboxKind::Snoozed] {
            if !items.contains(&kind)
                && self
                    .store
                    .messages
                    .iter()
                    .any(|m| kind.matches(m, self.store.today))
            {
                items.push(kind);
            }
        }
        items
    }

    // Snooze `ids` until `until`, or bring them back with `None`. The first
    // snooze puts the Snoozed mailbox in the sidebar, so the mail can be found.
    pub(crate) fn snooze(
        &mut self,
        ids: &[String],
        until: Option<model::types::MessageDate>,
    ) -> Task<Message> {
        let mut changed = 0;
        for message in self
            .store
            .messages
            .iter_mut()
            .filter(|message| ids.contains(&message.id))
        {
            message.snoozed_until = until;
            if until.is_none() {
                message.unread = true;
            }
            changed += 1;
        }
        if changed == 0 {
            return Task::none();
        }
        self.store.mark_dirty();
        self.reconcile_after_mutation();
        if until.is_none() {
            return Task::none();
        }
        self.edit_sidebar_layout(|layout| {
            layout.add_smart(model::sidebar_layout::SmartMailboxKind::Snoozed)
        })
    }

    pub(crate) fn wake_due_snoozes(&mut self) {
        let Ok(now) = self.local_clock.now() else {
            return;
        };
        let due: Vec<String> = self
            .store
            .messages
            .iter()
            .filter(|message| {
                !matches!(
                    message.location,
                    model::types::MessageLocation::Mailbox(MailboxKind::Trash | MailboxKind::Junk)
                ) && message
                    .snoozed_until
                    .is_some_and(|until| model::snooze::minutes_until(now, until) <= 0)
            })
            .map(|message| message.id.clone())
            .collect();
        if !due.is_empty() {
            let _ = self.snooze(&due, None);
        }
    }

    // Save the open message as a draft that sends itself at `preset`'s time,
    // waiting in the Scheduled mailbox until then.
    #[cfg(test)]
    pub(crate) fn schedule_send(&mut self, preset: model::snooze::SnoozePreset) -> Task<Message> {
        if !self.compose.open || !self.compose.can_send() {
            return Task::none();
        }
        let Ok(now) = self.local_clock.now() else {
            return Task::none();
        };
        self.schedule_send_at(preset.until(now))
    }

    pub(crate) fn schedule_send_at(&mut self, at: model::types::MessageDate) -> Task<Message> {
        let Some(id) = self.save_compose_draft() else {
            return Task::none();
        };
        self.queue_server_mutation(
            local_state::ServerMutationKind::DraftUpserted,
            vec![id.clone()],
        );
        if let Some(message) = self.store.messages.iter_mut().find(|m| m.id == id) {
            message.send_at = Some(at);
        }
        self.store.mark_dirty();
        self.compose_overflow_open = false;
        self.sync_body_editor();
        self.reconcile_selection();
        let layout = self.edit_sidebar_layout(|layout| {
            let changed = layout.add_smart(model::sidebar_layout::SmartMailboxKind::Scheduled)
                || !layout.smart_visible();
            layout.set_smart_visible(true);
            changed
        });
        self.collapsed_sections.remove(crate::view::SMART_SECTION);
        self.selection =
            SidebarSelection::SmartMailbox(model::sidebar_layout::SmartMailboxKind::Scheduled);
        self.selected_conversation = self
            .store
            .conversations_for(&self.selection)
            .iter()
            .find(|c| c.message_ids.contains(&id))
            .map(|c| c.id.clone());
        self.narrow_nav.on_sidebar_change();
        self.narrow_nav.on_select(true);
        let dismiss = self.dismiss_compose_focus(DismissCause::DefaultAction);
        Task::batch([layout, dismiss])
    }

    pub(crate) fn send_due_drafts(&mut self) -> Task<Message> {
        if self.compose.open || self.pending_send.is_some() || self.blocking_presentation_open() {
            return Task::none();
        }
        let Ok(now) = self.local_clock.now() else {
            return Task::none();
        };
        let Some(id) = self
            .store
            .messages
            .iter()
            .filter(|message| {
                matches!(
                    message.location,
                    model::types::MessageLocation::Mailbox(MailboxKind::Drafts)
                ) && message
                    .send_at
                    .is_some_and(|at| model::snooze::minutes_until(now, at) <= 0)
            })
            .min_by_key(|message| {
                message
                    .send_at
                    .map(|at| (at.year, at.month, at.day, at.minute))
            })
            .map(|message| message.id.clone())
        else {
            return Task::none();
        };
        let open = self.update(Message::OpenCompose(ComposeMode::EditDraft(id.clone())));
        if !self.compose.open {
            return open;
        }
        self.attachment_reminder_accepted = true;
        let send = self.update(Message::SendCompose);
        self.attachment_reminder_accepted = false;
        if self.pending_send.is_some() {
            if let Some(message) = self.store.messages.iter_mut().find(|m| m.id == id) {
                message.send_at = None;
            }
            self.store.mark_dirty();
        }
        Task::batch([open, send])
    }

    // How long until the earliest snooze or scheduled send comes due, from the
    // local clock.
    pub(crate) fn next_snooze_wait(&self) -> Option<std::time::Duration> {
        let snoozed: Vec<model::types::MessageDate> = self
            .store
            .messages
            .iter()
            .filter_map(|message| message.snoozed_until.or(message.send_at))
            .collect();
        if snoozed.is_empty() {
            return None;
        }
        let now = self.local_clock.now().ok()?;
        let minutes = snoozed
            .into_iter()
            .map(|until| model::snooze::minutes_until(now, until))
            .min()?;
        // Re-read the clock at least hourly, so a suspend or a time change
        // cannot hold mail back.
        let seconds = (minutes.max(0) as u64 * 60).clamp(30, 3600);
        Some(std::time::Duration::from_secs(seconds))
    }
}
