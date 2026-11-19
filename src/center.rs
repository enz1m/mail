//! Approved-agent mailbox navigation and bounded header queries.
//! Header access requires consent. Prepared mail opens an unsent composer and
//! remains bound to its source account and review identity. No API sends mail.

use enzui_iced_agent::center::{
    AppFunction, AppFunctionRegistry, AppMetadata, CenterFailure, CenterHost, CenterOutcome,
    CenterProvision, CenterTurn, ConnectorEvent, ConsentScope, FunctionRisk, FunctionSpec,
    object_schema,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::*;
use crate::model::{FavoriteKind, MailboxKind, MessageLocation, SidebarSelection};

// The app id the center credential is enrolled under.
pub(crate) const CENTER_APP_ID: &str = "io.github.enzui.Mail";
pub(crate) const LIST_LIMIT_MAX: usize = 50;

const MAILBOXES: [(&str, FavoriteKind); 4] = [
    ("inbox", FavoriteKind::Inbox),
    ("flagged", FavoriteKind::Flagged),
    ("drafts", FavoriteKind::Drafts),
    ("sent", FavoriteKind::Sent),
];

pub(crate) const PREPARE_RECIPIENTS_MAX: usize = 20;
pub(crate) const PREPARE_BODY_MAX: usize = 64 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CenterCall {
    ListMailboxes,
    ListMessages {
        mailbox: FavoriteKind,
        limit: usize,
        offset: usize,
    },
    SelectMailbox {
        mailbox: FavoriteKind,
    },
    PrepareSend(PreparedMessage),
    ReviewStatus {
        review_id: String,
        account: String,
    },
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreparedMessage {
    #[serde(default)]
    account: Option<String>,
    to: Vec<String>,
    #[serde(default)]
    cc: Vec<String>,
    subject: String,
    body: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusArgs {
    review_id: String,
    account: String,
}

#[derive(Debug)]
pub(crate) struct AgentReview {
    pub(crate) id: enzui_iced_agent::center::review::ReviewId,
    pub(crate) account: String,
    pub(crate) key: String,
    pub(crate) prepared: u64,
    pub(crate) sent_id: Option<String>,
    pub(crate) sent_account: Option<String>,
    pub(crate) sent_revision: Option<u64>,
}

#[derive(Debug, Default)]
pub(crate) struct ReviewOutcomes(std::collections::VecDeque<(String, bool)>);

impl ReviewOutcomes {
    const CAPACITY: usize = 64;

    fn record(&mut self, id: &str, sent_as_prepared: bool) {
        if self.0.len() == Self::CAPACITY {
            self.0.pop_front();
        }
        self.0.push_back((id.to_owned(), sent_as_prepared));
    }

    fn sent_as_prepared(&self, id: &str) -> Option<bool> {
        self.0
            .iter()
            .find(|(known, _)| known == id)
            .map(|(_, same)| *same)
    }
}

// The content a review is bound to: account, recipients, subject and body, as
// meaning rather than formatting, so field normalisation (a trailing newline,
// recipient separators) is not mistaken for the person's edits.
pub(crate) fn content_revision(
    account: &str,
    to: &str,
    cc: &str,
    bcc: &str,
    subject: &str,
    body: &str,
) -> u64 {
    use std::hash::{Hash, Hasher};
    fn recipients(line: &str) -> Vec<&str> {
        line.split([',', ';'])
            .map(str::trim)
            .filter(|address| !address.is_empty())
            .collect()
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    account.hash(&mut hasher);
    for line in [to, cc, bcc] {
        recipients(line).hash(&mut hasher);
    }
    subject.trim().hash(&mut hasher);
    body.trim_end().hash(&mut hasher);
    hasher.finish()
}

fn valid_address(address: &str) -> bool {
    let address = address.trim();
    !address.is_empty()
        && address.len() <= 254
        && address.contains('@')
        && !address
            .chars()
            .any(|c| c.is_control() || c == ',' || c == ';')
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum MailboxName {
    Inbox,
    Flagged,
    Drafts,
    Sent,
}

impl MailboxName {
    fn kind(&self) -> FavoriteKind {
        match self {
            Self::Inbox => FavoriteKind::Inbox,
            Self::Flagged => FavoriteKind::Flagged,
            Self::Drafts => FavoriteKind::Drafts,
            Self::Sent => FavoriteKind::Sent,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    mailbox: MailboxName,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailboxArgs {
    mailbox: MailboxName,
}

fn mailbox_schema() -> Value {
    json!({"type": "string", "enum": ["inbox", "flagged", "drafts", "sent"]})
}

pub(crate) fn registry() -> AppFunctionRegistry<CenterCall> {
    let mailboxes = FunctionSpec::new(
        "list_mailboxes",
        "List mailboxes",
        "Lists the favourite mailboxes across accounts with their message and unread counts.",
        object_schema(json!({}), &[]),
        object_schema(
            json!({"mailboxes": {"type": "array", "items": object_schema(json!({
                "mailbox": mailbox_schema(),
                "messages": {"type": "integer"},
                "unread": {"type": "integer"}
            }), &["mailbox", "messages", "unread"])}}),
            &["mailboxes"],
        ),
        FunctionRisk::SensitiveRead,
        vec![ConsentScope::Once, ConsentScope::Session],
    );
    let messages = FunctionSpec::new(
        "list_messages",
        "List message headers",
        "Lists the newest message headers in a mailbox: sender, subject, date and unread. \
         Bodies are not included. Optional offset and limit page through the mailbox \
         newest first; next_offset is null at the end.",
        object_schema(
            json!({
                "mailbox": mailbox_schema(),
                "limit": {"type": "integer", "minimum": 1, "maximum": LIST_LIMIT_MAX},
                "offset": {"type": "integer", "minimum": 0, "maximum": 4294967295_u64}
            }),
            &["mailbox"],
        ),
        object_schema(
            json!({
                "messages": {"type": "array", "items": object_schema(json!({
                    "message_id": {"type": "string"},
                    "sender": {"type": "string"},
                    "subject": {"type": "string"},
                    "date": {"type": "string"},
                    "unread": {"type": "boolean"}
                }), &["message_id", "sender", "subject", "date", "unread"])},
                "total": {"type": "integer"},
                "next_offset": {"type": ["integer", "null"]}
            }),
            &["messages", "total", "next_offset"],
        ),
        FunctionRisk::SensitiveRead,
        vec![ConsentScope::Once],
    );
    let select = FunctionSpec::new(
        "select_mailbox",
        "Show a mailbox",
        "Shows this favourite mailbox in the window.",
        object_schema(json!({"mailbox": mailbox_schema()}), &["mailbox"]),
        object_schema(json!({"mailbox": mailbox_schema()}), &["mailbox"]),
        FunctionRisk::Mutating,
        vec![ConsentScope::Once, ConsentScope::Session],
    );
    let address = json!({"type": "string", "minLength": 3, "maxLength": 254});
    let prepare = FunctionSpec::new(
        "prepare_send",
        "Prepare a message for the person to send",
        "Opens Mail's compose window with this message for the person to review. It \
         does not send: only the person's own Send does, and they may edit or discard \
         it. Returns a review id with state awaiting_user; read review_status for the \
         outcome instead of waiting. Plain text only. The account defaults to the \
         active one. Refused while the person is already writing a message.",
        object_schema(
            json!({
                "account": {"type": "string", "minLength": 1, "maxLength": 128},
                "to": {"type": "array", "items": address, "minItems": 1,
                       "maxItems": PREPARE_RECIPIENTS_MAX},
                "cc": {"type": "array", "items": address, "maxItems": PREPARE_RECIPIENTS_MAX},
                "subject": {"type": "string", "maxLength": 998},
                "body": {"type": "string", "maxLength": PREPARE_BODY_MAX}
            }),
            &["to", "subject", "body"],
        ),
        object_schema(
            json!({
                "review_id": {"type": "string"},
                "state": {"type": "string", "enum": ["awaiting_user"]},
                "account": {"type": "string"},
                "from": {"type": "string"}
            }),
            &["review_id", "state", "account", "from"],
        ),
        FunctionRisk::ExternalSideEffect,
        vec![ConsentScope::Once],
    )
    .required_capabilities(vec![
        enzui_iced_agent::center::review::USER_REVIEW_CAPABILITY.to_owned(),
    ]);
    let status = FunctionSpec::new(
        "review_status",
        "Status of a prepared message",
        "Reports what the person did with a message prepared by prepare_send: \
         awaiting_user, sending, committed (sent), discarded or superseded. \
         sent_as_prepared says whether the sent text was the prepared text.",
        object_schema(
            json!({
                "review_id": {"type": "string", "minLength": 1, "maxLength": 64},
                "account": {"type": "string", "minLength": 1, "maxLength": 128}
            }),
            &["review_id", "account"],
        ),
        object_schema(
            json!({
                "review_id": {"type": "string"},
                "state": {"type": "string", "enum": [
                    "awaiting_user", "sending", "committed", "discarded", "superseded"
                ]},
                "sent_as_prepared": {"type": ["boolean", "null"]}
            }),
            &["review_id", "state", "sent_as_prepared"],
        ),
        FunctionRisk::SensitiveRead,
        vec![ConsentScope::Once, ConsentScope::Session],
    );
    AppFunctionRegistry::from_functions(vec![
        AppFunction::new(mailboxes, |_| CenterCall::ListMailboxes).expect("list_mailboxes spec"),
        AppFunction::typed(messages, |args: ListArgs| CenterCall::ListMessages {
            mailbox: args.mailbox.kind(),
            limit: args.limit.unwrap_or(20).clamp(1, LIST_LIMIT_MAX),
            offset: args.offset,
        })
        .expect("list_messages spec"),
        AppFunction::typed(prepare, CenterCall::PrepareSend).expect("prepare_send spec"),
        AppFunction::typed(status, |args: StatusArgs| CenterCall::ReviewStatus {
            review_id: args.review_id,
            account: args.account,
        })
        .expect("review_status spec"),
        AppFunction::typed(select, |args: MailboxArgs| CenterCall::SelectMailbox {
            mailbox: args.mailbox.kind(),
        })
        .expect("select_mailbox spec"),
    ])
    .expect("the mail registry is valid")
}

pub(crate) fn metadata() -> AppMetadata {
    AppMetadata {
        id: CENTER_APP_ID.into(),
        name: "Mail".into(),
        description: "IMAP mail reader".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    }
}

fn mailbox_name(kind: FavoriteKind) -> &'static str {
    MAILBOXES
        .iter()
        .find(|(_, candidate)| *candidate == kind)
        .map_or("inbox", |(name, _)| name)
}

impl State {
    // Connect to enzui-center when this build opted in and a credential is
    // enrolled for [`CENTER_APP_ID`]; otherwise stay unexposed.
    pub(crate) fn start_center(&mut self) {
        let Some(provision) = CenterProvision::discover(CENTER_APP_ID) else {
            return;
        };
        match CenterHost::spawn(provision, metadata(), registry()) {
            Ok(host) => self.center = Some(host),
            Err(error) => eprintln!("mail-app: center connector disabled: {error}"),
        }
    }

    pub(crate) fn center_subscription(&self) -> Subscription<Message> {
        let Some(host) = self.center.as_ref() else {
            return Subscription::none();
        };
        let deadline = host
            .next_deadline()
            .map_or_else(Subscription::none, |deadline| {
                Subscription::run_with(deadline, |deadline| {
                    let deadline = *deadline;
                    iced::futures::stream::once(async move {
                        let wait = deadline
                            .duration_since(std::time::SystemTime::now())
                            .unwrap_or_default();
                        tokio::time::sleep(wait).await;
                        Message::CenterDeadline
                    })
                })
            });
        Subscription::batch([host.subscription(Message::Center), deadline])
    }

    // Final shutdown: refuse new calls, cancel live ones, join the connector.
    pub(crate) fn shutdown_center(&mut self) {
        if let Some(host) = self.center.take() {
            host.shutdown();
        }
    }

    pub(crate) fn center_event(&mut self, event: ConnectorEvent) -> Task<Message> {
        let Some(host) = self.center.as_mut() else {
            return Task::none();
        };
        let update = host.receive_with_ends(event);
        self.waiting_center_calls.retain(|(call, _)| {
            !update
                .ended
                .iter()
                .any(|ended| ended.call == call.reference())
        });
        let CenterTurn::Call(call, request) = update.turn else {
            return Task::none();
        };
        // A cold-started worker registers before mail loads, so the first call waits,
        // bounded by its deadline, instead of failing.
        if self.startup.pending() {
            if self.waiting_center_calls.len() >= WAITING_CALLS_MAX {
                host.fail(call, "busy", "too many calls are waiting for mail to load");
            } else {
                self.waiting_center_calls.push((call, request));
            }
            return Task::none();
        }
        if self.close_committed || self.close_settlement.is_some() {
            host.fail(call, "unavailable", "mail is closing");
            return Task::none();
        }
        let (task, outcome) = self.apply_center_call(request);
        if let Some(host) = self.center.as_mut() {
            host.answer(call, outcome);
        }
        task
    }

    fn apply_center_call(&mut self, request: CenterCall) -> (Task<Message>, CenterOutcome) {
        match request {
            CenterCall::ListMailboxes => {
                let mailboxes: Vec<Value> = MAILBOXES
                    .iter()
                    .map(|(name, kind)| {
                        let selection = SidebarSelection::Favorite(*kind);
                        json!({
                            "mailbox": name,
                            "messages": self.store.message_count(&selection),
                            "unread": self.store.unread_count(&selection),
                        })
                    })
                    .collect();
                (Task::none(), Ok(json!({ "mailboxes": mailboxes })))
            }
            CenterCall::ListMessages {
                mailbox,
                limit,
                offset,
            } => {
                if offset > u32::MAX as usize {
                    return (
                        Task::none(),
                        Err(CenterFailure::new(
                            "invalid_arguments",
                            "offset is out of bounds",
                        )),
                    );
                }
                let selection = SidebarSelection::Favorite(mailbox);
                let sorted = self.store.sorted_messages(&selection);
                let messages: Vec<Value> = sorted
                    .iter()
                    .skip(offset)
                    .take(limit)
                    .map(|message| {
                        json!({
                            "message_id": message.id,
                            "sender": if message.sender_name.is_empty() {
                                message.sender_email.clone()
                            } else {
                                message.sender_name.clone()
                            },
                            "subject": message.subject,
                            "date": format!(
                                "{:04}-{:02}-{:02}",
                                message.date.year, message.date.month, message.date.day
                            ),
                            "unread": message.unread,
                        })
                    })
                    .collect();
                let next = offset.saturating_add(messages.len());
                let next_offset = if next < sorted.len() {
                    json!(next)
                } else {
                    Value::Null
                };
                (
                    Task::none(),
                    Ok(json!({
                        "messages": messages,
                        "total": sorted.len(),
                        "next_offset": next_offset,
                    })),
                )
            }
            CenterCall::PrepareSend(message) => self.prepare_send(message),
            CenterCall::ReviewStatus { review_id, account } => {
                (Task::none(), self.review_status(&review_id, &account))
            }
            CenterCall::SelectMailbox { mailbox } => {
                let selection = SidebarSelection::Favorite(mailbox);
                let task = self.update(Message::SelectSidebar(selection.clone()));
                let outcome = if self.selection == selection {
                    Ok(json!({ "mailbox": mailbox_name(mailbox) }))
                } else {
                    Err(CenterFailure::new(
                        "unavailable",
                        "the mailbox could not be shown",
                    ))
                };
                (task, outcome)
            }
        }
    }
}

pub(crate) const WAITING_CALLS_MAX: usize = 16;

impl State {
    pub(crate) fn answer_waiting_center_calls(&mut self) -> Task<Message> {
        if self.startup.pending() || self.waiting_center_calls.is_empty() {
            return Task::none();
        }
        let waiting = std::mem::take(&mut self.waiting_center_calls);
        let mut tasks = Vec::new();
        for (call, request) in waiting {
            let Some(host) = self.center.as_ref() else {
                break;
            };
            // A call whose deadline passed, or whose connection went away,
            // while it waited is already answered by the host.
            if !host.is_live(&call) {
                continue;
            }
            if self.close_committed || self.close_settlement.is_some() {
                if let Some(host) = self.center.as_mut() {
                    host.fail(call, "unavailable", "mail is closing");
                }
                continue;
            }
            let (task, outcome) = self.apply_center_call(request);
            if let Some(host) = self.center.as_mut() {
                host.answer(call, outcome);
            }
            tasks.push(task);
        }
        Task::batch(tasks)
    }

    fn prepare_send(&mut self, message: PreparedMessage) -> (Task<Message>, CenterOutcome) {
        let refuse = |code: &'static str, text: &str| {
            (Task::none(), Err(CenterFailure::new(code, text.to_owned())))
        };
        if message.to.is_empty()
            || message.to.len() > PREPARE_RECIPIENTS_MAX
            || message.cc.len() > PREPARE_RECIPIENTS_MAX
            || !message
                .to
                .iter()
                .chain(&message.cc)
                .all(|a| valid_address(a))
        {
            return refuse(
                "invalid_arguments",
                "every recipient must be one email address",
            );
        }
        if message.subject.len() > 998
            || message.subject.contains(['\r', '\n'])
            || message.body.len() > PREPARE_BODY_MAX
        {
            return refuse("invalid_arguments", "subject or body is out of bounds");
        }
        let account = message
            .account
            .clone()
            .unwrap_or_else(|| self.active_account_id.clone());
        let Some(from) = self.store.account(&account).map(|a| a.email.clone()) else {
            return refuse("not_found", "no such account");
        };
        if self.pending_send.is_some() {
            return refuse("send_pending", "a message is already waiting to send");
        }
        // The person's own writing is never replaced; an untouched earlier agent
        // message may be, superseding its review.
        if self.compose.open {
            let untouched_agent_draft = self.agent_review.as_ref().is_some_and(|review| {
                self.compose.review_key.as_deref() == Some(review.key.as_str())
                    && self.compose_revision() == review.prepared
            });
            if !untouched_agent_draft {
                return refuse("compose_busy", "the person is writing a message");
            }
        }

        if let Some(previous) = self.agent_review.take() {
            self.reviews.supersede(&previous.account, &previous.key);
        }
        let mut task = self.update(Message::OpenCompose(model::compose::ComposeMode::New));
        if !self.compose.open {
            return (
                task,
                Err(CenterFailure::new("unavailable", "compose could not open")),
            );
        }
        self.compose.from_account_id.clone_from(&account);
        self.compose.to = message.to.join(", ");
        self.compose.cc = message.cc.join(", ");
        self.compose.subject = message.subject;
        self.compose.body = message.body;
        self.compose.format = model::compose::ComposeFormat::Plain;
        self.compose.body_html = None;
        self.sync_body_editor();

        self.review_serial = self.review_serial.wrapping_add(1);
        let key = format!("agent-compose-{}", self.review_serial);
        self.compose.review_key = Some(key.clone());
        let prepared = self.compose_revision();
        let id = self.reviews.open(account.clone(), key.clone(), prepared);
        self.agent_review = Some(AgentReview {
            id: id.clone(),
            account: account.clone(),
            key,
            prepared,
            sent_id: None,
            sent_account: None,
            sent_revision: None,
        });
        task = Task::batch([task, self.present_for_review()]);
        (
            task,
            Ok(json!({
                "review_id": id.as_str(),
                "state": "awaiting_user",
                "account": account,
                "from": from,
            })),
        )
    }

    fn compose_revision(&self) -> u64 {
        content_revision(
            &self.compose.from_account_id,
            &self.compose.to,
            &self.compose.cc,
            &self.compose.bcc,
            &self.compose.subject,
            &self.compose.body,
        )
    }

    fn review_status(&self, review_id: &str, account: &str) -> CenterOutcome {
        let Some(state) = self.reviews.status(&account.to_owned(), review_id) else {
            return Err(CenterFailure::new(
                "not_found",
                "no such review for this account",
            ));
        };
        let sending = state.is_open()
            && self
                .agent_review
                .as_ref()
                .is_some_and(|review| review.id.as_str() == review_id && review.sent_id.is_some());
        let label = if sending { "sending" } else { state.as_str() };
        Ok(json!({
            "review_id": review_id,
            "state": label,
            "sent_as_prepared": self.review_outcomes.sent_as_prepared(review_id),
        }))
    }

    // Review tracking must never initiate submission.
    pub(crate) fn reconcile_review(&mut self) {
        let Some(review) = self.agent_review.as_mut() else {
            return;
        };
        if self.compose.open && self.compose.review_key.as_deref() == Some(review.key.as_str()) {
            review.sent_id = None;
            review.sent_account = None;
            review.sent_revision = None;
            return;
        }
        if let Some(pending) = self.pending_send.as_ref()
            && pending.review_key.as_deref() == Some(review.key.as_str())
        {
            review.sent_id = Some(pending.id.clone());
            review.sent_account = Some(pending.account_id.clone());
            review.sent_revision = Some(content_revision(
                &pending.account_id,
                &pending.to_raw,
                &pending.cc_raw,
                &pending.bcc_raw,
                &pending.subject,
                &pending.body,
            ));
            return;
        }
        let review = self.agent_review.take().expect("tracked review");
        let landed = review.sent_id.as_deref().is_some_and(|id| {
            self.store.message(id).is_some_and(|message| {
                message.location == MessageLocation::Mailbox(MailboxKind::Sent)
            })
        });
        let same_account = review.sent_account.as_deref() == Some(review.account.as_str());
        if landed && same_account {
            if self
                .reviews
                .commit(&review.account, &review.key, review.prepared)
                .is_ok()
            {
                self.review_outcomes.record(
                    review.id.as_str(),
                    review.sent_revision == Some(review.prepared),
                );
            }
        } else {
            self.reviews.discard(&review.account, &review.key);
        }
    }
}

#[cfg(test)]
#[path = "center_tests.rs"]
mod tests;
