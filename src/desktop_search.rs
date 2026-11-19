//! The desktop's Search: messages by subject and sender.
//!
//! The provider answers from a snapshot of the loaded mailboxes that the
//! reducer refreshes when the store changes, so a query never touches the
//! model, the network or the window. A result id is the message's store id,
//! which names its account, mailbox and UID; opening it takes the same route
//! as a choice in Mail's own search.

use std::sync::{Arc, Mutex, MutexGuard};

use enzui_iced_core::activation::dbus::search::{
    SearchProvider, SearchQuery, SearchRequest, SearchResult,
};

use crate::model::types::{MailboxKind, MailboxStore, MessageDate, MessageLocation};

const RESULT_ICON: &str = "mail-message";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    id: String,
    title: String,
    subtitle: String,
    folded: String,
}

// The store's message buffer, its length, its durable-mutation generation and
// the day labels are relative to: any loaded-mail change moves one of them.
type Fingerprint = (usize, usize, u64, MessageDate);

#[derive(Debug, Clone, Default)]
pub(crate) struct SearchIndex {
    entries: Arc<Mutex<Vec<Entry>>>,
    fingerprint: Option<Fingerprint>,
}

impl SearchIndex {
    pub(crate) fn refresh(&mut self, store: &MailboxStore) {
        let fingerprint = (
            store.messages.as_ptr() as usize,
            store.messages.len(),
            store.generation(),
            store.today,
        );
        if self.fingerprint == Some(fingerprint) {
            return;
        }
        self.fingerprint = Some(fingerprint);
        *lock(&self.entries) = entries(store);
    }

    pub(crate) fn provider(&self) -> SearchProvider {
        let entries = Arc::clone(&self.entries);
        SearchProvider::blocking(move |query| search(&lock(&entries), query))
    }

    #[cfg(test)]
    pub(crate) fn ids(&self, text: &str) -> Vec<String> {
        let query = SearchQuery {
            text: text.to_owned(),
            limit: enzui_iced_core::activation::dbus::search::MAX_SEARCH_RESULTS,
            locale: None,
        };
        search(&lock(&self.entries), &query)
            .iter()
            .map(|result| result.id().to_owned())
            .collect()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

// Newest first; deleted and junk mail stays out of the desktop's Search.
fn entries(store: &MailboxStore) -> Vec<Entry> {
    let mut messages: Vec<_> = store
        .messages
        .iter()
        .filter(|message| {
            !matches!(
                message.location,
                MessageLocation::Mailbox(MailboxKind::Trash | MailboxKind::Junk)
            )
        })
        .collect();
    messages.sort_by_key(|message| std::cmp::Reverse(message.date));
    messages
        .into_iter()
        .filter_map(|message| {
            let title = crate::model::types::shown_subject(&message.subject).into_owned();
            SearchResult::new(message.id.clone(), title.clone()).ok()?;
            let sender = if message.sender_name.trim().is_empty() {
                message.sender_email.trim()
            } else {
                message.sender_name.trim()
            };
            Some(Entry {
                id: message.id.clone(),
                folded: enzui_iced_core::text_fold::fold(&format!(
                    "{}\n{}\n{}",
                    message.subject, message.sender_name, message.sender_email
                )),
                subtitle: plain_line(&format!(
                    "{sender} · {}",
                    message.date.relative_label(store.today)
                )),
                title,
            })
        })
        .collect()
}

// The desktop shows a subtitle as plain text, so the isolates Fluent wraps
// around formatted dates would show as stray glyphs.
fn plain_line(text: &str) -> String {
    text.chars()
        .filter(|c| !matches!(c, '\u{2066}'..='\u{2069}'))
        .collect()
}

fn search(entries: &[Entry], query: &SearchQuery) -> Vec<SearchResult> {
    let wanted = enzui_iced_core::text_fold::fold(query.text.trim());
    if wanted.is_empty() {
        return Vec::new();
    }
    entries
        .iter()
        .filter(|entry| entry.folded.contains(&wanted))
        .filter_map(|entry| {
            SearchResult::new(entry.id.clone(), entry.title.clone())
                .ok()
                .map(|result| result.subtitle(entry.subtitle.clone()).icon(RESULT_ICON))
        })
        .take(query.limit)
        .collect()
}

// A chosen message opens like a choice in Mail's own search; "Search in Mail"
// opens that search with the query.
pub(crate) fn messages_for(request: SearchRequest) -> Vec<crate::Message> {
    match request {
        SearchRequest::Open { id, .. } => vec![crate::Message::SelectSearchResult(id)],
        SearchRequest::Search { query } => vec![
            crate::Message::SearchActivated,
            crate::Message::SearchEdited(query),
        ],
    }
}

#[cfg(test)]
mod tests {}
