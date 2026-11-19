//! Bulk actions resolve conversation selections to explicit message IDs.
//! Empty Junk moves mail to Trash; emptying Trash permanently removes it.

use std::collections::HashSet;

use super::types::{MailboxKind, MailboxStore, MessageLocation, SidebarSelection};

// The destructive-action gate.
//
// The selection itself lives in
// `enzui_iced_core::list_selection::ListSelection` on `State::row_selection`;
// this holds only the pending destructive confirmation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct BulkState {
    pub(crate) pending_empty: Option<MailboxKind>,
    pub(crate) pending_empty_account: Option<String>,
}

pub(crate) fn expand_conversation_ids<S: std::hash::BuildHasher>(
    store: &MailboxStore,
    selection: &SidebarSelection,
    conv_ids: &std::collections::HashSet<String, S>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for convo in store.conversations_for(selection) {
        if !conv_ids.contains(&convo.id) {
            continue;
        }
        for mid in &convo.message_ids {
            if seen.insert(mid.clone()) {
                out.push(mid.clone());
            }
        }
    }
    out
}

pub(crate) fn bulk_move(store: &mut MailboxStore, ids: &[String], mailbox: MailboxKind) -> usize {
    store.mark_dirty();
    let mut n = 0;
    for id in ids {
        if let Some(m) = store.messages.iter_mut().find(|m| &m.id == id) {
            m.location = MessageLocation::Mailbox(mailbox);
            n += 1;
        }
    }
    n
}

pub(crate) fn bulk_set_unread(store: &mut MailboxStore, ids: &[String], unread: bool) -> usize {
    store.mark_dirty();
    let mut n = 0;
    for id in ids {
        if let Some(m) = store.messages.iter_mut().find(|m| &m.id == id)
            && m.unread != unread
        {
            m.unread = unread;
            n += 1;
        }
    }
    n
}

pub(crate) fn bulk_set_flagged(store: &mut MailboxStore, ids: &[String], flagged: bool) -> usize {
    store.mark_dirty();
    let mut n = 0;
    for id in ids {
        if let Some(m) = store.messages.iter_mut().find(|m| &m.id == id)
            && m.flagged != flagged
        {
            m.flagged = flagged;
            n += 1;
        }
    }
    n
}

pub(crate) fn mark_all_read(store: &mut MailboxStore, selection: &SidebarSelection) -> usize {
    let ids: Vec<String> = store
        .sorted_messages(selection)
        .into_iter()
        .filter(|m| m.unread)
        .map(|m| m.id.clone())
        .collect();
    bulk_set_unread(store, &ids, false)
}

pub(crate) fn unread_in_selection(store: &MailboxStore, selection: &SidebarSelection) -> usize {
    store
        .sorted_messages(selection)
        .into_iter()
        .filter(|m| m.unread)
        .count()
}

pub(crate) fn mailbox_count_for(
    store: &MailboxStore,
    kind: MailboxKind,
    account_id: &str,
) -> usize {
    store
        .messages
        .iter()
        .filter(|message| {
            message.account_id == account_id && message.location == MessageLocation::Mailbox(kind)
        })
        .count()
}

pub(crate) fn empty_action_label(kind: MailboxKind) -> Option<&'static str> {
    match kind {
        MailboxKind::Trash => Some("Empty Trash"),
        MailboxKind::Junk => Some("Empty Junk"),
        _ => None,
    }
}

// Confirmation-button label for the empty-mailbox dialog, distinct from the
// [`empty_mailbox_prompt`] title so one "Empty Trash" / "Empty Junk"
// affordance renders.
pub(crate) fn empty_confirm_button_label(kind: MailboxKind) -> &'static str {
    match kind {
        MailboxKind::Trash => "Erase",
        MailboxKind::Junk => "Move to Trash",
        _ => "Empty",
    }
}

// The emptiable mailbox the current sidebar row denotes, if any.
//
// Only `AccountMailbox`: a smart mailbox such as `Flagged` is a filter, not a
// location, and may contain trashed mail.
pub(crate) fn emptiable_mailbox(selection: &SidebarSelection) -> Option<MailboxKind> {
    match selection {
        SidebarSelection::AccountMailbox { mailbox, .. }
            if empty_action_label(*mailbox).is_some() =>
        {
            Some(*mailbox)
        }
        _ => None,
    }
}

// Title and body for the destructive confirmation, naming the count. Junk is
// a recoverable move to Trash; Trash is a purge.
pub(crate) fn empty_mailbox_prompt(kind: MailboxKind, count: usize) -> (String, String) {
    let (title, key) = match kind {
        MailboxKind::Trash => ("Empty Trash", "mail-empty-trash-body"),
        MailboxKind::Junk => ("Empty Junk", "mail-empty-junk-body"),
        _ => ("Empty Mailbox", "mail-empty-mailbox-body"),
    };
    (crate::l10n::tr(title), crate::l10n::count(key, count))
}

pub(crate) fn empty_mailbox(
    store: &mut MailboxStore,
    kind: MailboxKind,
    account_id: &str,
) -> usize {
    store.mark_dirty();
    match kind {
        MailboxKind::Trash => {
            let before = store.messages.len();
            store.messages.retain(|m| {
                m.account_id != account_id
                    || !matches!(m.location, MessageLocation::Mailbox(MailboxKind::Trash))
            });
            before - store.messages.len()
        }
        MailboxKind::Junk => {
            let mut n = 0;
            for m in &mut store.messages {
                if m.account_id == account_id
                    && matches!(m.location, MessageLocation::Mailbox(MailboxKind::Junk))
                {
                    m.location = MessageLocation::Mailbox(MailboxKind::Trash);
                    n += 1;
                }
            }
            n
        }
        _ => 0,
    }
}
