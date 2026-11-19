use crate::notification_settings::Preview;
use enzui_iced_core::app_data::AppId;
use enzui_iced_core::portal::notification::freedesktop::FreedesktopNotificationBackend;
use enzui_iced_core::portal::notification::{
    DesktopNotification, DesktopNotificationClient, NotificationAction, NotificationActionId,
    NotificationActionTarget, NotificationError, NotificationEvent, NotificationIcon,
    NotificationId, NotificationOperationOutcome, NotificationWake,
};
use iced::Subscription;

use crate::model::types::Message;

// The identity the portal attributes the notification to, matched to the
// window's application id.
const NOTIFY_APP_ID: &str = "io.github.enzui.Mail";
const NEW_MAIL_ID: &str = "new-mail";
pub(crate) const ACTION_OPEN: &str = "open";
pub(crate) const ACTION_SETTINGS: &str = "open-notification-settings";
const NAMED_SENDERS: usize = 2;
// The longest sender or subject a notification shows, in bytes; the
// portal refuses a title over 256.
const MAX_SHOWN_BYTES: usize = 200;

pub(crate) fn new_mail_notification(
    arrivals: &[&Message],
    preview: Preview,
) -> Option<DesktopNotification> {
    let newest = arrivals.first()?;
    let id = NotificationId::new(NEW_MAIL_ID).expect("BUG: the new-mail id is a fixed literal");
    let (title, body) = if preview == Preview::Private {
        (
            crate::l10n::current().text("mail-notification-private-title"),
            crate::l10n::count("mail-new-messages", arrivals.len()),
        )
    } else if preview == Preview::Sender {
        (
            sender_of(newest),
            crate::l10n::count("mail-new-messages", arrivals.len()),
        )
    } else if let [only] = arrivals {
        (sender_of(only), subject_of(only))
    } else {
        let mut senders: Vec<String> = Vec::new();
        for message in arrivals {
            let sender = sender_of(message);
            if !senders.contains(&sender) {
                senders.push(sender);
            }
        }
        let others = senders.len().saturating_sub(NAMED_SENDERS);
        let named = senders[..senders.len().min(NAMED_SENDERS)].join(", ");
        let mut arguments = enzui_iced_core::localization::Arguments::new();
        arguments.set("senders", named);
        arguments.set("count", others);
        let from = crate::l10n::current().text_with("mail-notification-from", &arguments);
        (
            crate::l10n::count("mail-new-messages", arrivals.len()),
            format!("{from}\n{}", subject_of(newest)),
        )
    };
    Some(
        DesktopNotification::new(id, title)
            .icon(NotificationIcon::Themed(vec![
                NOTIFY_APP_ID.into(),
                "mail-unread".into(),
            ]))
            .category("email.arrived")
            .body(body)
            .default_action(
                NotificationAction::new(
                    NotificationActionId::new(ACTION_OPEN)
                        .expect("BUG: the open action is a fixed literal"),
                )
                .target(NotificationActionTarget::Text(newest.id.clone())),
            ),
    )
}

fn sender_of(message: &Message) -> String {
    let name = [&message.sender_name, &message.sender_email]
        .into_iter()
        .find(|text| !text.trim().is_empty());
    name.map_or_else(|| crate::l10n::tr("Unknown sender"), |name| shown(name))
}

fn subject_of(message: &Message) -> String {
    shown(&crate::model::types::shown_subject(&message.subject))
}

// One line of at most `MAX_SHOWN_BYTES`, without the control characters a
// crafted header could carry.
fn shown(text: &str) -> String {
    let mut line = String::new();
    for c in text.trim().chars() {
        let c = if c.is_control() { ' ' } else { c };
        if line.len() + c.len_utf8() > MAX_SHOWN_BYTES {
            line.push('…');
            break;
        }
        line.push(c);
    }
    line
}

// The owner of Mail's new-mail notification.
//
// Tests stay inert; the desktop client is created only after a windowed launch.
#[derive(Default)]
pub(crate) struct NewMailNotifier {
    client: Option<DesktopNotificationClient<FreedesktopNotificationBackend>>,
    enabled: bool,
    attempted: bool,
    pub(crate) status: Option<&'static str>,
    newest: Option<String>,
}

impl std::fmt::Debug for NewMailNotifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewMailNotifier")
            .field("enabled", &self.enabled)
            .field("newest", &self.newest)
            .finish_non_exhaustive()
    }
}

impl NewMailNotifier {
    pub(crate) fn enable(&mut self) {
        self.enabled = true;
    }

    pub(crate) fn announce(&mut self, arrivals: &[&Message], preview: Preview) {
        let Some(notification) = new_mail_notification(arrivals, preview) else {
            return;
        };
        self.newest = arrivals.first().map(|message| message.id.clone());
        self.post(notification);
    }

    fn post(&mut self, notification: DesktopNotification) {
        if !self.enabled {
            return;
        }
        let result = self.client_mut().map(|client| client.send(notification));
        self.status = Some(match result {
            Some(Ok(_) | Err(NotificationError::Deferred)) => "mail-notification-pending",
            _ => "mail-notification-unavailable",
        });
    }

    pub(crate) fn test(&mut self) {
        let id = NotificationId::new("notification-test").expect("fixed test notification id");
        self.post(
            DesktopNotification::new(
                id,
                crate::l10n::current().text("mail-notification-test-title"),
            )
            .body(crate::l10n::current().text("mail-notification-test-body"))
            .default_action(NotificationAction::new(
                NotificationActionId::new(ACTION_SETTINGS).expect("fixed settings action id"),
            ))
            .icon(NotificationIcon::Themed(vec![
                NOTIFY_APP_ID.into(),
                "mail-unread".into(),
            ])),
        );
    }

    pub(crate) fn newest(&self) -> Option<&str> {
        self.newest.as_deref()
    }

    pub(crate) fn withdraw(&mut self) {
        self.newest = None;
        if let Some(client) = self.client.as_mut() {
            for value in [NEW_MAIL_ID, "notification-test"] {
                let id = NotificationId::new(value).expect("fixed notification id");
                let _ = client.withdraw(&id);
            }
        }
    }

    pub(crate) fn wake_subscription(&self) -> Subscription<NotificationWake> {
        match &self.client {
            Some(client) => client.wake_subscription(),
            None => Subscription::none(),
        }
    }

    pub(crate) fn drain(&mut self) -> Vec<NotificationEvent> {
        let events: Vec<_> = self.client.as_mut().map_or_else(Vec::new, |client| {
            std::iter::from_fn(|| client.poll_event()).collect()
        });
        for event in &events {
            match event {
                NotificationEvent::Operation { outcome, .. } => {
                    self.status = Some(match outcome {
                        NotificationOperationOutcome::Sent
                        | NotificationOperationOutcome::Replaced => "mail-notification-delivered",
                        NotificationOperationOutcome::Unavailable => {
                            "mail-notification-unavailable"
                        }
                        NotificationOperationOutcome::Denied => "mail-notification-denied",
                        NotificationOperationOutcome::Unsupported(_)
                        | NotificationOperationOutcome::Failed => "mail-notification-failed",
                        _ => continue,
                    });
                }
                NotificationEvent::ServiceChanged {
                    available: false, ..
                }
                | NotificationEvent::ActionDeliveryUnavailable => {
                    self.status = Some("mail-notification-unavailable");
                }
                _ => {}
            }
        }
        events
    }

    fn client_mut(
        &mut self,
    ) -> Option<&mut DesktopNotificationClient<FreedesktopNotificationBackend>> {
        if !self.enabled {
            return None;
        }
        if self.client.is_none() && !self.attempted {
            self.attempted = true;
            let app_id = AppId::new(NOTIFY_APP_ID).expect("fixed Mail app id");
            if let Ok(backend) = FreedesktopNotificationBackend::new(app_id.clone()) {
                self.client = Some(DesktopNotificationClient::new(app_id, backend));
            }
        }
        self.client.as_mut()
    }
}
