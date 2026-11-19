use super::*;

use backend::inbox_watch::{InboxWatchEvent, InboxWatchUpdate};

impl State {
    // Watch `config`'s Inbox for pushes with `secret`, the credential its
    // sync is about to use, unless a watch with these settings runs or the
    // account's watch gave up.
    pub(crate) fn start_inbox_watch(&mut self, config: &backend::AccountConfig, secret: &Secret) {
        if config.protocol != backend::MailProtocol::Imap
            || self.close_settlement.is_some()
            || self.inbox_watch_stopped.contains_key(&config.id)
            || self
                .inbox_watches
                .get(&config.id)
                .is_some_and(|watch| watch.config() == config)
        {
            return;
        }
        let secret = Secret::new(secret.expose_secret().to_vec());
        if let Some(watch) = self.mail_backend.watch_inbox(config, secret) {
            self.inbox_watches.insert(config.id.clone(), watch);
        }
    }

    // A change on the server syncs the Inbox; a watch that gave up is
    // forgotten until it is worth asking again.
    pub(crate) fn reduce_inbox_watch(&mut self, update: InboxWatchUpdate) -> Task<Message> {
        let current = self
            .inbox_watches
            .get(&update.account_id)
            .is_some_and(|watch| watch.id() == update.watch_id);
        if !current {
            return Task::none();
        }
        match update.event {
            InboxWatchEvent::Changed => self
                .store
                .account(&update.account_id)
                .and_then(|account| account.config.clone())
                .map_or_else(Task::none, |config| self.queue_inbox_sync(config)),
            InboxWatchEvent::Ended(failure) => {
                self.inbox_watches.remove(&update.account_id);
                self.inbox_watch_stopped
                    .insert(update.account_id, failure.kind);
                Task::none()
            }
        }
    }

    pub(crate) fn stop_inbox_watches(&mut self) {
        self.inbox_watches.clear();
    }

    pub(crate) fn forget_inbox_watch(&mut self, account_id: &str) {
        self.inbox_watches.remove(account_id);
        self.inbox_watch_stopped.remove(account_id);
        self.inbox_announce_ready.retain(|(id, _)| id != account_id);
    }

    pub(crate) fn account_message_ids(&self, account_id: &str) -> HashSet<String> {
        self.store
            .messages
            .iter()
            .filter(|message| message.account_id == account_id)
            .map(|message| message.id.clone())
            .collect()
    }

    // Initial downloads stay quiet; only newly durable unread arrivals are eligible.
    pub(crate) fn announce_new_mail(
        &mut self,
        account_id: &str,
        mailbox: MailboxKind,
        before: &HashSet<String>,
        had_cached: bool,
    ) {
        if self.inbox_watch_stopped.get(account_id)
            == Some(&backend::curl_backend::MailFailureKind::Authentication)
        {
            self.inbox_watch_stopped.remove(account_id);
        }
        if !self.startup.preferences.publishable() {
            return;
        }
        let first = self
            .inbox_announce_ready
            .insert((account_id.to_owned(), mailbox));
        if first && !had_cached {
            return;
        }
        let mut arrivals: Vec<&model::types::Message> = self
            .store
            .messages
            .iter()
            .rev()
            .filter(|message| {
                message.account_id == account_id
                    && self
                        .notifications
                        .get()
                        .allows(&notification_settings::MailboxTarget {
                            account_id: account_id.to_owned(),
                            location: message.location.clone(),
                        })
                    && message.unread
                    && !message.blocked
                    && !crate::mail_actions::sender_is_blocked(
                        &self.privacy.block_list,
                        &message.sender_email,
                    )
                    && message.snoozed_until.is_none()
                    && !before.contains(&message.id)
            })
            .collect();
        arrivals.sort_by_key(|message| std::cmp::Reverse(message.date));
        self.new_mail
            .announce(&arrivals, self.notifications.get().preview);
    }

    pub(crate) fn reduce_new_mail_wake(&mut self) -> Task<Message> {
        let mut target = None;
        let mut settings = false;
        let mut activation = None;
        for event in self.new_mail.drain() {
            if let enzui_iced_core::portal::notification::NotificationEvent::Action(
                mut invocation,
            ) = event
            {
                match invocation.action_id.as_str() {
                    new_mail_notify::ACTION_OPEN => {
                        if let Some(
                            enzui_iced_core::portal::notification::NotificationActionTarget::Text(
                                id,
                            ),
                        ) = &invocation.target
                        {
                            target = Some(id.clone());
                            settings = false;
                        } else {
                            continue;
                        }
                    }
                    new_mail_notify::ACTION_SETTINGS => {
                        target = None;
                        settings = true;
                    }
                    _ => continue,
                }
                activation = self
                    .desktop
                    .notification_context(&mut invocation)
                    .or(activation);
            }
        }
        let action = if settings {
            let task = self.update(Message::OpenSettings);
            self.settings.select_tab(SettingsTab::Notifications);
            task
        } else if let Some(target) = target {
            if self.compose.open || self.pending_send.is_some() || self.close_settlement.is_some() {
                self.desktop.queue_notification_target(target);
            } else {
                self.open_notification_message(&target);
            }
            Task::none()
        } else {
            return Task::none();
        };
        let activate = if let Some(context) = activation {
            let mut context = Some(context);
            iced::window::latest().then(move |window| match (window, context.take()) {
                (Some(window), Some(context)) => context
                    .activate(window, Instant::now())
                    .map(|_| Message::DesktopActivated),
                _ => Task::none(),
            })
        } else {
            iced::window::latest().and_then(iced::window::gain_focus)
        };
        Task::batch([action, activate])
    }

    pub(crate) fn open_newest_arrival(&mut self) {
        let Some(newest) = self.new_mail.newest().map(str::to_owned) else {
            return;
        };
        self.open_notification_message(&newest);
    }

    pub(crate) fn open_notification_message(&mut self, newest: &str) {
        let Some(message) = self.store.message(newest) else {
            self.new_mail.withdraw();
            return;
        };
        self.selection = match &message.location {
            model::types::MessageLocation::Mailbox(MailboxKind::Inbox) => {
                SidebarSelection::Favorite(model::types::FavoriteKind::Inbox)
            }
            model::types::MessageLocation::Mailbox(mailbox) => SidebarSelection::AccountMailbox {
                account_id: message.account_id.clone(),
                mailbox: *mailbox,
            },
            model::types::MessageLocation::Folder(folder_id) => SidebarSelection::Folder {
                account_id: message.account_id.clone(),
                folder_id: folder_id.clone(),
            },
        };
        let conversation = self
            .store
            .conversations_for(&self.selection)
            .into_iter()
            .find(|conversation| conversation.message_ids.iter().any(|id| id == newest));
        if let Some(conversation) = conversation {
            self.open_conversation(conversation.id);
        }
        self.reconcile_selection();
    }

    pub(crate) fn withdraw_seen_new_mail(&mut self, opened: &[String]) {
        if self
            .new_mail
            .newest()
            .is_some_and(|newest| opened.iter().any(|id| id == newest))
        {
            self.new_mail.withdraw();
        }
    }
}
