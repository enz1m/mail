use crate::model::types::MailboxStore;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Contact {
    // The bare address, lower-cased for identity; the autocomplete payload and
    // dedup key.
    pub(crate) email: String,
    // The display name, empty when only the bare address was ever seen.
    pub(crate) name: String,
}

impl Contact {
    #[must_use]
    pub(crate) fn address_line(&self) -> String {
        crate::model::compose::format_address(&self.name, &self.email)
    }
}

// Parse one raw recipient token (`"Name <a@b.com>"` or `"a@b.com"`) into a
// name and lower-cased email, or `None` without a usable email.
pub(crate) fn parse_recipient(raw: &str) -> Option<(String, String)> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some((phrase, tail)) = raw.rsplit_once('<')
        && let Some((email, _)) = tail.split_once('>')
    {
        let email = email.trim();
        if email.is_empty() {
            return None;
        }
        let name = phrase.trim().trim_matches('"').trim();
        return Some((name.to_owned(), email.to_ascii_lowercase()));
    }
    Some((String::new(), raw.to_ascii_lowercase()))
}

#[must_use]
pub(crate) fn recent_contacts(store: &MailboxStore) -> Vec<Contact> {
    let mut order: Vec<Contact> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    let mut absorb = |name: &str, email_raw: &str| {
        let email = email_raw.trim();
        if email.bytes().any(|byte| byte.is_ascii_control())
            || !email
                .rsplit_once('@')
                .is_some_and(|(local, domain)| !local.is_empty() && !domain.is_empty())
        {
            return;
        }
        let key = email.to_ascii_lowercase();
        let name = name.trim().to_owned();
        match index.get(&key).copied() {
            Some(i) => {
                if order[i].name.is_empty() && !name.is_empty() {
                    order[i].name = name;
                }
            }
            None => {
                index.insert(key.clone(), order.len());
                order.push(Contact { email: key, name });
            }
        }
    };

    for m in &store.messages {
        absorb(&m.sender_name, &m.sender_email);
        for raw in &m.to {
            if let Some((name, email)) = parse_recipient(raw) {
                absorb(&name, &email);
            }
        }
    }

    order.sort_by(|a, b| {
        let ka = if a.name.is_empty() { &a.email } else { &a.name };
        let kb = if b.name.is_empty() { &b.email } else { &b.name };
        ka.to_lowercase()
            .cmp(&kb.to_lowercase())
            .then_with(|| a.email.cmp(&b.email))
    });
    order
}

// The committed prefix includes its comma and following whitespace; completion replaces only the active token.
#[must_use]
pub(crate) fn active_token(field: &str) -> (&str, &str) {
    let start = match field.rfind(',') {
        Some(i) => {
            let tail = field.get(i + 1..).expect("comma is an ASCII boundary");
            field.len() - tail.trim_start_matches([' ', '\t']).len()
        }
        None => field.len() - field.trim_start().len(),
    };
    field.split_at(start)
}

#[must_use]
pub(crate) fn contact_matches(c: &Contact, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    c.name.to_lowercase().contains(&q) || c.email.to_lowercase().contains(&q)
}

// The suggestions for the current active token, filtered and ranked, without
// contacts already committed earlier in the field.
#[must_use]
pub(crate) fn suggest(contacts: &[Contact], field: &str) -> Vec<Contact> {
    let (prefix, token) = active_token(field);
    let already: std::collections::HashSet<String> = crate::model::compose::split_addresses(prefix)
        .iter()
        .map(|a| {
            crate::model::compose::email_token(a)
                .trim()
                .to_ascii_lowercase()
        })
        .collect();
    contacts
        .iter()
        .filter(|c| !already.contains(&c.email))
        .filter(|c| contact_matches(c, token))
        .cloned()
        .collect()
}

#[must_use]
pub(crate) fn replace_active_token(field: &str, chosen: &str) -> String {
    let (prefix, _token) = active_token(field);
    let prefix = prefix.trim_end();
    if prefix.is_empty() {
        format!("{chosen}, ")
    } else {
        let prefix = prefix.trim_end_matches(',').trim_end();
        format!("{prefix}, {chosen}, ")
    }
}
