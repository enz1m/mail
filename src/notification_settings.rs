//! Persisted notification policy keyed by canonical account and mailbox identity.

use crate::model::types::{MailboxKind, MessageLocation};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Preview {
    #[default]
    Full,
    Sender,
    Private,
}

impl Preview {
    pub(crate) fn token(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Sender => "sender",
            Self::Private => "private",
        }
    }
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "full" => Some(Self::Full),
            "sender" => Some(Self::Sender),
            "private" => Some(Self::Private),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub(crate) struct MailboxTarget {
    pub(crate) account_id: String,
    pub(crate) location: MessageLocation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NotificationSettings {
    pub(crate) enabled: bool,
    pub(crate) all_mailboxes: bool,
    pub(crate) preview: Preview,
    pub(crate) overrides: BTreeMap<MailboxTarget, bool>,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            all_mailboxes: false,
            preview: Preview::Full,
            overrides: BTreeMap::new(),
        }
    }
}

impl NotificationSettings {
    pub(crate) fn allows(&self, target: &MailboxTarget) -> bool {
        self.enabled
            && self.overrides.get(target).copied().unwrap_or_else(|| {
                self.all_mailboxes
                    || target.location == MessageLocation::Mailbox(MailboxKind::Inbox)
            })
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.overrides.len() > 512 {
            return Err("too many notification mailbox overrides".into());
        }
        for target in self.overrides.keys() {
            for value in std::iter::once(&target.account_id).chain(match &target.location {
                MessageLocation::Folder(id) => Some(id),
                _ => None,
            }) {
                if value.is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
                    return Err("invalid notification mailbox identity".into());
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Change {
    Enabled(bool),
    Scope(bool),
    Preview(Preview),
    Mailbox(MailboxTarget, Option<bool>),
}
