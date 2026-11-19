//! Reply references join messages despite subject changes or missing ancestors.
//! Subject fallback applies only to unlinked replies, never unrelated originals.

use std::collections::HashMap;

use super::{Message, MessageDate};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Conversation {
    // The newest message ID namespaces this projection; selection reanchors by member IDs.
    pub(crate) id: String,
    pub(crate) subject: String,
    pub(crate) preview: String,
    // Every message id in the conversation, oldest → newest.
    pub(crate) message_ids: Vec<String>,
    // Participants in order of first appearance, deduplicated on `sender_name`
    // (case-insensitive). The caller filters out the account owner.
    pub(crate) participants: Vec<String>,
    pub(crate) latest_date: MessageDate,
    pub(crate) unread_count: usize,
    pub(crate) flagged_any: bool,
    pub(crate) has_attachment_any: bool,
}

impl Conversation {
    pub(crate) fn message_count(&self) -> usize {
        self.message_ids.len()
    }
}

// Normalize a subject for threading: strip repeated leading `Re:` / `Fwd:` /
// `Fw:` prefixes (case-insensitive), trim, and lowercase.
//
// A subject of only prefixes ("Re: Re:") yields the empty string, which must
// not be threaded by subject (see `subject_key`).
pub(crate) fn normalize_subject(subject: &str) -> String {
    let mut s = subject.trim();
    loop {
        let lower = s.to_ascii_lowercase();
        // Strip recognised prefixes until none remain.
        let stripped = if let Some(rest) = lower.strip_prefix("re:") {
            s.get(s.len() - rest.len()..)
                .expect("ASCII reply prefix preserves byte boundaries")
        } else if let Some(rest) = lower.strip_prefix("fwd:") {
            s.get(s.len() - rest.len()..)
                .expect("ASCII reply prefix preserves byte boundaries")
        } else if let Some(rest) = lower.strip_prefix("fw:") {
            s.get(s.len() - rest.len()..)
                .expect("ASCII reply prefix preserves byte boundaries")
        } else {
            break;
        };
        s = stripped.trim_start();
    }
    s.trim().to_ascii_lowercase()
}

// The subject grouping key, or `None` for an empty normalized subject, which
// never threads by subject.
fn subject_key(subject: &str) -> Option<String> {
    let n = normalize_subject(subject);
    if n.is_empty() { None } else { Some(n) }
}

// Whether the subject carries a reply or forward prefix. Lexical; it only
// permits rule 3 repair, never groups two originals.
fn has_reply_prefix(subject: &str) -> bool {
    let s = subject.trim().to_ascii_lowercase();
    s.starts_with("re:") || s.starts_with("fwd:") || s.starts_with("fw:")
}

// Whether the message is a reply, by reply headers or subject prefix. A
// non-reply is a thread root and never merges into another root by subject.
pub(crate) fn looks_like_reply(m: &Message) -> bool {
    m.in_reply_to.is_some() || !m.references.is_empty() || has_reply_prefix(&m.subject)
}

fn ancestor_ids(m: &Message) -> Vec<&str> {
    let mut ids: Vec<&str> = Vec::with_capacity(m.references.len() + 1);
    if let Some(p) = m.in_reply_to.as_deref() {
        ids.push(p);
    }
    for r in &m.references {
        let r = r.as_str();
        if !ids.contains(&r) {
            ids.push(r);
        }
    }
    ids.retain(|id| !id.trim().is_empty());
    ids
}

pub(crate) fn thread_messages(messages: &[&Message]) -> Vec<Conversation> {
    if messages.is_empty() {
        return Vec::new();
    }

    let n = messages.len();
    let mut parent: Vec<usize> = (0..n).collect();

    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            // The smaller index stays root so results are stable.
            if ra < rb {
                parent[rb] = ra;
            } else {
                parent[ra] = rb;
            }
        }
    }

    let id_to_index: HashMap<(&str, &str), usize> = messages
        .iter()
        .enumerate()
        .map(|(i, m)| ((m.account_id.as_str(), m.id.as_str()), i))
        .collect();
    let mut linked = vec![false; n];
    let mut absent_anchor: HashMap<(&str, &str), usize> = HashMap::new();
    for (i, m) in messages.iter().enumerate() {
        for id in ancestor_ids(m) {
            let key = (m.account_id.as_str(), id);
            match id_to_index.get(&key) {
                Some(&j) if j != i => {
                    union(&mut parent, i, j);
                    linked[i] = true;
                    linked[j] = true;
                }
                Some(_) => {}
                None => match absent_anchor.get(&key) {
                    Some(&j) => {
                        union(&mut parent, i, j);
                        linked[i] = true;
                        linked[j] = true;
                    }
                    None => {
                        absent_anchor.insert(key, i);
                    }
                },
            }
        }
    }
    //
    // The first message with a key anchors it. A later one joins only if it is an
    // unlinked reply, or if the anchor is an unlinked reply and this is its
    // original.
    let mut first_for_subject: HashMap<(&str, String), usize> = HashMap::new();
    for (i, m) in messages.iter().enumerate() {
        let Some(key) = subject_key(&m.subject) else {
            continue;
        };
        let key = (m.account_id.as_str(), key);
        match first_for_subject.get(&key) {
            Some(&j) => {
                let i_repairable = !linked[i] && looks_like_reply(m);
                let j_repairable = !linked[j] && looks_like_reply(messages[j]);
                if i_repairable || j_repairable {
                    union(&mut parent, i, j);
                }
            }
            None => {
                first_for_subject.insert(key, i);
            }
        }
    }

    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }

    let mut conversations: Vec<Conversation> = groups
        .into_values()
        .map(|mut indices| {
            indices.sort_by(|&a, &b| {
                let ma = messages[a];
                let mb = messages[b];
                ma.date.cmp(&mb.date).then_with(|| ma.id.cmp(&mb.id))
            });

            let newest_idx = *indices.last().expect("non-empty group");
            let newest = messages[newest_idx];

            let mut participants: Vec<String> = Vec::new();
            let mut seen_lower: Vec<String> = Vec::new();
            for &i in &indices {
                let name = messages[i].sender_name.trim();
                if name.is_empty() {
                    continue;
                }
                let key = name.to_ascii_lowercase();
                if !seen_lower.iter().any(|s| s == &key) {
                    seen_lower.push(key);
                    participants.push(name.to_owned());
                }
            }

            let unread_count = indices.iter().filter(|&&i| messages[i].unread).count();
            let flagged_any = indices.iter().any(|&i| messages[i].flagged);
            let has_attachment_any = indices.iter().any(|&i| messages[i].has_attachment);

            Conversation {
                id: newest.id.clone(),
                subject: newest.subject.clone(),
                preview: newest.preview.clone(),
                message_ids: indices.iter().map(|&i| messages[i].id.clone()).collect(),
                participants,
                latest_date: newest.date,
                unread_count,
                flagged_any,
                has_attachment_any,
            }
        })
        .collect();

    conversations.sort_by(|a, b| {
        b.latest_date
            .cmp(&a.latest_date)
            .then_with(|| a.id.cmp(&b.id))
    });
    conversations
}
