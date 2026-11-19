use enzui_iced_core::activation::ApplicationIntent;
use enzui_iced_core::activation::dbus::search::SearchRequest;
use iced::{Subscription, Task};

use crate::app_instance::AppInstance;
use crate::desktop_search::SearchIndex;
use crate::{ComposeMode, Message, State};

const MAX_PENDING_SEARCHES: usize = 16;

#[derive(Default)]
pub(crate) struct Desktop {
    pub(crate) instance: Option<AppInstance>,
    pub(crate) pending: std::collections::VecDeque<String>,
    pub(crate) searches: std::collections::VecDeque<SearchRequest>,
    pub(crate) search: SearchIndex,
    broker: enzui_iced_core::activation::ActivationBroker,
    notification_pending: bool,
    notification_target: Option<String>,
    dispatching: bool,
    contexts: Vec<enzui_iced_core::activation::ActivationContext>,
    focus_pending: bool,
}

impl Desktop {
    pub(crate) fn queue_notification_target(&mut self, target: String) {
        self.notification_pending = true;
        self.notification_target = Some(target);
    }

    pub(crate) fn notification_context(
        &mut self,
        invocation: &mut enzui_iced_core::portal::notification::NotificationActionInvocation,
    ) -> Option<enzui_iced_core::activation::ActivationContext> {
        self.broker
            .admit_notification_invocation(invocation, std::time::Instant::now())
            .ok()
            .flatten()
    }

    pub(crate) fn enqueue(&mut self, intent: ApplicationIntent) {
        match intent {
            ApplicationIntent::Open(uris) => {
                for uri in uris {
                    if self.pending.len() < 32 && uri.len() <= 64 * 1024 {
                        self.pending.push_back(uri);
                    }
                }
            }
            ApplicationIntent::ActivateAction { name }
                if name == crate::new_mail_notify::ACTION_OPEN =>
            {
                self.notification_pending = true;
            }
            _ => {}
        }
    }

    pub(crate) fn enqueue_search(&mut self, request: SearchRequest) {
        if self.searches.len() < MAX_PENDING_SEARCHES {
            self.searches.push_back(request);
        }
    }

    #[cfg(feature = "center")]
    pub(crate) fn enqueue_search_activation(
        &mut self,
        activation: enzui_iced_core::activation::dbus::search::SearchActivation,
    ) {
        self.enqueue_search(activation.request);
        self.keep_context(activation.context);
    }

    #[cfg(feature = "center")]
    fn keep_context(&mut self, context: Option<enzui_iced_core::activation::ActivationContext>) {
        if let Some(context) = context {
            if self.contexts.len() < 32 {
                self.contexts.push(context);
            }
        } else {
            self.focus_pending = true;
        }
    }

    #[cfg(feature = "center")]
    pub(crate) fn enqueue_call(
        &mut self,
        call: enzui_iced_core::activation::dbus::ApplicationCall,
    ) {
        self.enqueue(call.intent);
        self.keep_context(call.context);
    }

    pub(crate) fn subscription(&self) -> Subscription<Message> {
        self.instance
            .as_ref()
            .map_or_else(Subscription::none, |instance| {
                instance.subscription(|_| Message::DesktopWake)
            })
    }
}

impl State {
    pub(crate) fn desktop_wake(&mut self) -> Task<Message> {
        let Some(drained) = self.desktop.instance.as_mut().map(AppInstance::drain) else {
            return self.desktop_pending();
        };
        for intent in drained.launches {
            self.desktop.enqueue(intent);
        }
        for request in drained.searches {
            self.desktop.enqueue_search(request);
        }
        Task::batch([drained.raise, self.desktop_pending()])
    }

    // Answers the desktop's Search from the mail this instance has loaded, on
    // whichever single-instance service this process owns.
    pub(crate) fn serve_desktop_search(&mut self) {
        self.refresh_desktop_search();
        let provider = self.desktop.search.provider();
        #[cfg(feature = "center")]
        if let Err(error) = self.agent.serve_search(provider.clone()) {
            eprintln!("mail-app: Search provider unavailable: {error}");
        }
        if let Some(instance) = self.desktop.instance.as_mut()
            && let Err(error) = instance.serve_search(provider)
        {
            eprintln!("mail-app: Search provider unavailable: {error}");
        }
    }

    pub(crate) fn refresh_desktop_search(&mut self) {
        self.desktop.search.refresh(&self.store);
    }

    pub(crate) fn desktop_pending(&mut self) -> Task<Message> {
        #[cfg(feature = "center")]
        if self.agent.is_windowless() {
            return Task::none();
        }
        let mut tasks = self
            .desktop
            .contexts
            .drain(..)
            .map(|context| {
                let mut context = Some(context);
                iced::window::latest().then(move |window| match (window, context.take()) {
                    (Some(window), Some(context)) => context
                        .activate(window, std::time::Instant::now())
                        .map(|_| Message::DesktopActivated),
                    _ => Task::none(),
                })
            })
            .collect::<Vec<_>>();
        if std::mem::take(&mut self.desktop.focus_pending) {
            tasks.push(iced::window::latest().and_then(iced::window::gain_focus));
        }
        tasks.push(self.desktop_pending_intent());
        Task::batch(tasks)
    }

    fn desktop_pending_intent(&mut self) -> Task<Message> {
        if self.startup_is_loading()
            || self.desktop.dispatching
            || self.pending_send.is_some()
            || self.compose.open
            || self.close_settlement.is_some()
            || self.close_committed
        {
            return Task::none();
        }
        if !self.desktop.searches.is_empty() {
            self.desktop.dispatching = true;
            let tasks: Vec<Task<Message>> = std::mem::take(&mut self.desktop.searches)
                .into_iter()
                .flat_map(crate::desktop_search::messages_for)
                .map(|message| self.update(message))
                .collect();
            self.desktop.dispatching = false;
            return Task::batch(tasks.into_iter().chain([self.desktop_pending_intent()]));
        }
        while let Some(uri) = self.desktop.pending.pop_front() {
            if let Some(route) = compose_route(&uri) {
                self.desktop.dispatching = true;
                let task = self.update(Message::OpenCompose(ComposeMode::New));
                self.desktop.dispatching = false;
                if self.compose.open {
                    self.compose.to = route.to;
                    self.compose.subject = route.subject;
                    self.compose.body = route.body;
                    self.sync_body_editor();
                } else {
                    self.desktop.pending.push_front(uri);
                }
                return task;
            }
        }
        if std::mem::take(&mut self.desktop.notification_pending) {
            if let Some(target) = self.desktop.notification_target.take() {
                self.open_notification_message(&target);
            } else if self.new_mail.newest().is_some() {
                self.open_newest_arrival();
            } else {
                return self.update(Message::SelectSidebar(
                    crate::model::types::SidebarSelection::Favorite(
                        crate::model::types::FavoriteKind::Inbox,
                    ),
                ));
            }
        }
        Task::none()
    }
}

fn compose_route(uri: &str) -> Option<crate::backend::unsubscribe::MailtoRoute> {
    if uri.len() > 64 * 1024 {
        return None;
    }
    let route = crate::backend::unsubscribe::parse_mailto(uri)?;
    if route.to.chars().any(char::is_control) || route.subject.chars().any(char::is_control) {
        return None;
    }
    Some(route)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_desktop_search_choice_opens_its_message_or_mails_own_search() {
        let (mut state, _) = State::boot_for_test();
        let _ = state.update(Message::DesktopActivated);
        let target = state
            .store
            .message("pets-1")
            .expect("fixture folder message")
            .clone();
        let word = target
            .subject
            .split_whitespace()
            .max_by_key(|word| word.len())
            .expect("fixture subject")
            .to_owned();
        assert!(state.desktop.search.ids(&word).contains(&target.id));

        let _ = state.update(Message::OpenCompose(ComposeMode::New));
        assert!(state.compose.open);
        state.desktop.enqueue_search(SearchRequest::Open {
            id: target.id.clone(),
            query: word,
        });
        let _ = state.desktop_pending();
        assert_eq!(state.desktop.searches.len(), 1);
        state.compose.open = false;
        let _ = state.desktop_pending();
        assert!(state.desktop.searches.is_empty());
        assert_eq!(
            state.selection,
            crate::model::types::SidebarSelection::Folder {
                account_id: target.account_id.clone(),
                folder_id: "pets".into(),
            }
        );
        let opened = state
            .selected_conversation
            .clone()
            .expect("the chosen message's conversation is open");
        assert!(state.message_ids_of(&opened).contains(&target.id));

        state.desktop.enqueue_search(SearchRequest::Search {
            query: "invoice".into(),
        });
        let _ = state.desktop_pending();
        assert!(state.search_open);
        assert_eq!(state.search_query, "invoice");
    }

    #[test]
    fn unsubscribe_mailto_waits_for_the_open_draft() {
        let (mut state, _) = State::boot_for_test();
        let message = state
            .store
            .messages
            .iter_mut()
            .find(|message| message.id == "m-nl-1")
            .expect("seeded mailing list message");
        message.unsubscribe = Some(crate::model::types::ListUnsubscribe {
            uris: vec!["mailto:list@example.invalid?subject=Leave%20list".into()],
            one_click: false,
        });
        let _ = state.update(Message::OpenCompose(ComposeMode::New));
        state.compose.subject = "Draft in progress".into();

        let _ = state.unsubscribe("m-nl-1".into());
        assert_eq!(state.compose.subject, "Draft in progress");
        assert_eq!(state.desktop.pending.len(), 1);

        state.compose.open = false;
        let _ = state.desktop_pending();
        assert_eq!(state.compose.to, "list@example.invalid");
        assert_eq!(state.compose.subject, "Leave list");
    }

    #[test]
    fn mailto_waits_behind_an_unresolved_outbox_without_losing_the_request() {
        for uri in [
            "mailto:qa@example.invalid%0D%0ABcc:bad@example.invalid",
            "mailto:qa@example.invalid?subject=x%0D%0ABcc:bad",
            "https://example.invalid",
        ] {
            assert!(compose_route(uri).is_none());
        }
        let (mut state, _) = State::boot_for_test();
        let _ = state.update(Message::OpenCompose(ComposeMode::New));
        state.compose.to = "qa@example.invalid".into();
        state.compose.subject = "Synthetic submission".into();
        state.compose.body = "Never delivered".into();
        assert!(state.send_compose());
        assert!(state.pending_send.is_some());
        state.desktop.enqueue(ApplicationIntent::Open(vec![
            "mailto:next@example.invalid?subject=Review".into(),
        ]));
        let _ = state.desktop_pending();
        assert_eq!(state.desktop.pending.len(), 1);
        assert!(!state.compose.open);
        state.pending_send = None;
        let _ = state.desktop_pending();
        assert_eq!(state.compose.to, "next@example.invalid");
        assert_eq!(state.compose.subject, "Review");
        assert!(state.pending_send.is_none());
    }
}
