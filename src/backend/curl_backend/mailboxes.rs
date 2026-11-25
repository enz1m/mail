//! Special-use mailbox discovery and IMAP mailbox naming.

use super::transport::ImapSession;
use super::types::{MailFailure, MailFailureKind, MailResult};
use crate::backend::AccountConfig;
use crate::model::types::MailboxKind;
use enzui_services::credential_store::Secret;
use imap_proto::{MailboxDatum, NameAttribute, Response};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{LazyLock, Mutex};

const MAX_MAILBOX_LIST_BYTES: usize = 4 * 1024 * 1024;
const MAX_MAILBOX_NAME_BYTES: usize = 1024;

const SPECIAL_KINDS: [MailboxKind; 5] = [
    MailboxKind::Drafts,
    MailboxKind::Sent,
    MailboxKind::Junk,
    MailboxKind::Trash,
    MailboxKind::Archive,
];

// Server names of an account's special-use mailboxes; a kind without a name
// has no matching mailbox on the server.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct ImapMailboxNames {
    pub(super) drafts: Option<String>,
    pub(super) sent: Option<String>,
    pub(super) junk: Option<String>,
    pub(super) trash: Option<String>,
    pub(super) archive: Option<String>,
}

impl ImapMailboxNames {
    fn get(&self, kind: MailboxKind) -> Option<&str> {
        match kind {
            MailboxKind::Inbox => Some("INBOX"),
            MailboxKind::Drafts => self.drafts.as_deref(),
            MailboxKind::Sent => self.sent.as_deref(),
            MailboxKind::Junk => self.junk.as_deref(),
            MailboxKind::Trash => self.trash.as_deref(),
            MailboxKind::Archive => self.archive.as_deref(),
        }
    }

    fn slot_mut(&mut self, kind: MailboxKind) -> Option<&mut Option<String>> {
        match kind {
            MailboxKind::Inbox => None,
            MailboxKind::Drafts => Some(&mut self.drafts),
            MailboxKind::Sent => Some(&mut self.sent),
            MailboxKind::Junk => Some(&mut self.junk),
            MailboxKind::Trash => Some(&mut self.trash),
            MailboxKind::Archive => Some(&mut self.archive),
        }
    }
}

// Discovered names per account endpoint for the life of the process.
static DISCOVERED: LazyLock<Mutex<HashMap<String, ImapMailboxNames>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn account_key(config: &AccountConfig) -> String {
    format!(
        "{}\n{}\n{}\n{}",
        config.id, config.incoming.server, config.incoming.port, config.incoming.username
    )
}

fn cached_names(config: &AccountConfig) -> Option<ImapMailboxNames> {
    DISCOVERED
        .lock()
        .ok()
        .and_then(|cache| cache.get(&account_key(config)).cloned())
}

// Drops the cached names so the next operation lists the server again, as
// after a refused `STATUS` on a renamed or deleted mailbox.
pub(super) fn forget_mailbox_names(config: &AccountConfig) {
    if let Ok(mut cache) = DISCOVERED.lock() {
        cache.remove(&account_key(config));
    }
}

pub(super) fn session_mailbox_names(
    session: &mut ImapSession<'_>,
    config: &AccountConfig,
) -> MailResult<Option<ImapMailboxNames>> {
    if let Some(names) = cached_names(config) {
        return Ok(Some(names));
    }
    let names = discover_mailbox_names(session)?;
    if let (Some(names), Ok(mut cache)) = (&names, DISCOVERED.lock()) {
        cache.insert(account_key(config), names.clone());
    }
    Ok(names)
}

pub(super) fn account_mailbox_names(
    config: &AccountConfig,
    secret: &Secret,
    cancel: &AtomicBool,
    root_url: &str,
) -> MailResult<Option<ImapMailboxNames>> {
    if let Some(names) = cached_names(config) {
        return Ok(Some(names));
    }
    let mut session = ImapSession::open(config, secret, cancel, root_url)?;
    session_mailbox_names(&mut session, config)
}

// `Unavailable` when a listing showed that the server has no such mailbox.
pub(super) fn resolved_mailbox_name(
    names: Option<&ImapMailboxNames>,
    mailbox: MailboxKind,
) -> MailResult<String> {
    match names {
        None => Ok(conventional_name(mailbox).to_owned()),
        Some(names) => names.get(mailbox).map(str::to_owned).ok_or_else(|| {
            MailFailure::new(
                MailFailureKind::Unavailable,
                format!("the server has no {} mailbox", conventional_name(mailbox)),
            )
        }),
    }
}

fn conventional_name(mailbox: MailboxKind) -> &'static str {
    match mailbox {
        MailboxKind::Inbox => "INBOX",
        MailboxKind::Drafts => "Drafts",
        MailboxKind::Sent => "Sent",
        MailboxKind::Junk => "Junk",
        MailboxKind::Trash => "Trash",
        MailboxKind::Archive => "Archive",
    }
}

pub(super) fn imap_quoted(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 2);
    out.push('"');
    for character in name.chars() {
        if matches!(character, '"' | '\\') {
            out.push('\\');
        }
        out.push(character);
    }
    out.push('"');
    out
}

// libcurl percent-decodes the URL path into the mailbox it selects.
pub(super) fn imap_mailbox_path(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

// `None` when the server refuses every listing form; callers then keep the
// conventional names and discover again on a later operation.
pub(super) fn discover_mailbox_names(
    session: &mut ImapSession<'_>,
) -> MailResult<Option<ImapMailboxNames>> {
    let mut listed = None;
    for command in [
        "LIST \"\" \"*\" RETURN (SPECIAL-USE)",
        "XLIST \"\" \"*\"",
        "LIST \"\" \"*\"",
    ] {
        let listing = match session.run(command, MAX_MAILBOX_LIST_BYTES) {
            Ok(response) => parse_mailbox_list(&response),
            Err(error) if error.kind == MailFailureKind::Protocol => continue,
            Err(error) => return Err(error),
        };
        if listing
            .entries
            .iter()
            .any(|entry| entry.special_use.is_some())
        {
            return Ok(Some(listing.names()));
        }
        if !listing.entries.is_empty() {
            listed.get_or_insert(listing);
        }
    }
    Ok(listed.map(|listing| listing.names()))
}

#[derive(Debug)]
pub(super) struct ListedMailbox {
    name: String,
    delimiter: Option<String>,
    special_use: Option<MailboxKind>,
}

#[derive(Debug)]
pub(super) struct MailboxListing {
    entries: Vec<ListedMailbox>,
    // False when a listed mailbox could not be parsed, so an unmatched kind
    // may still exist under its conventional name.
    complete: bool,
}

impl MailboxListing {
    pub(super) fn names(&self) -> ImapMailboxNames {
        let mut names = resolve_mailbox_names(&self.entries);
        if !self.complete {
            for kind in SPECIAL_KINDS {
                if let Some(slot @ None) = names.slot_mut(kind) {
                    *slot = Some(conventional_name(kind).to_owned());
                }
            }
        }
        names
    }
}

pub(super) fn parse_mailbox_list(response: &[u8]) -> MailboxListing {
    let response = xlist_as_list(response);
    let mut listing = MailboxListing {
        entries: Vec::new(),
        complete: true,
    };
    let mut rest = response.as_slice();
    while !rest.is_empty() {
        let line = match Response::parse(rest) {
            Ok((remaining, response)) => {
                rest = remaining;
                response
            }
            Err(_) => {
                if rest
                    .get(..7)
                    .is_some_and(|start| start.eq_ignore_ascii_case(b"* LIST "))
                {
                    listing.complete = false;
                }
                let skip = rest
                    .windows(2)
                    .position(|pair| pair == b"\r\n")
                    .map_or(rest.len(), |at| at + 2);
                rest = &rest[skip..];
                continue;
            }
        };
        let Response::MailboxData(MailboxDatum::List(list)) = line else {
            continue;
        };
        if !valid_mailbox_name(&list.name) {
            continue;
        }
        let mut special_use = None;
        let mut selectable = true;
        for attribute in &list.name_attributes {
            match attribute {
                NameAttribute::NoSelect => selectable = false,
                NameAttribute::Drafts => special_use = Some(MailboxKind::Drafts),
                NameAttribute::Sent => special_use = Some(MailboxKind::Sent),
                NameAttribute::Junk => special_use = Some(MailboxKind::Junk),
                NameAttribute::Trash => special_use = Some(MailboxKind::Trash),
                NameAttribute::Archive => special_use = Some(MailboxKind::Archive),
                NameAttribute::Extension(name) => {
                    if name.eq_ignore_ascii_case("\\Spam") {
                        special_use = Some(MailboxKind::Junk);
                    } else if name.eq_ignore_ascii_case("\\NonExistent") {
                        selectable = false;
                    }
                }
                _ => {}
            }
        }
        if selectable {
            listing.entries.push(ListedMailbox {
                name: list.name.into_owned(),
                delimiter: list.delimiter.map(std::borrow::Cow::into_owned),
                special_use,
            });
        }
    }
    listing
}

// XLIST answers carry the same grammar as LIST under another verb.
fn xlist_as_list(response: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(response.len());
    let mut line_start = true;
    let mut index = 0;
    while index < response.len() {
        if line_start && response[index..].starts_with(b"* XLIST ") {
            out.extend_from_slice(b"* LIST ");
            index += b"* XLIST ".len();
        }
        let byte = response[index];
        out.push(byte);
        line_start = byte == b'\n';
        index += 1;
    }
    out
}

// Names stay in their modified UTF-7 wire form; quoted specials are refused
// because the parser returns quoted names still escaped.
fn valid_mailbox_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_MAILBOX_NAME_BYTES
        && !name.eq_ignore_ascii_case("INBOX")
        && name
            .bytes()
            .all(|byte| (0x20..0x7f).contains(&byte) && !matches!(byte, b'"' | b'\\'))
}

// Special-use attributes win; otherwise a top-level name, then a leaf name
// under a personal namespace such as `INBOX.`, matches the common names. No
// server mailbox serves two kinds.
pub(super) fn resolve_mailbox_names(entries: &[ListedMailbox]) -> ImapMailboxNames {
    let mut names = ImapMailboxNames::default();
    let mut assigned = Vec::<&str>::new();
    for kind in SPECIAL_KINDS {
        let candidates = entries
            .iter()
            .filter(|entry| !assigned.contains(&entry.name.as_str()))
            .filter(|entry| entry.special_use.is_none_or(|special| special == kind))
            .collect::<Vec<_>>();
        let named = |name: &str| {
            common_names(kind)
                .iter()
                .any(|common| common.eq_ignore_ascii_case(name))
        };
        let found = candidates
            .iter()
            .find(|entry| entry.special_use == Some(kind))
            .or_else(|| candidates.iter().find(|entry| named(&entry.name)))
            .or_else(|| candidates.iter().find(|entry| named(leaf_name(entry))));
        if let (Some(slot), Some(entry)) = (names.slot_mut(kind), found) {
            *slot = Some(entry.name.clone());
            assigned.push(&entry.name);
        }
    }
    names
}

fn leaf_name(entry: &ListedMailbox) -> &str {
    entry
        .delimiter
        .as_deref()
        .filter(|delimiter| !delimiter.is_empty())
        .and_then(|delimiter| entry.name.rsplit_once(delimiter))
        .map_or(entry.name.as_str(), |(_, leaf)| leaf)
}

fn common_names(kind: MailboxKind) -> &'static [&'static str] {
    match kind {
        MailboxKind::Inbox => &["INBOX"],
        MailboxKind::Drafts => &["Drafts", "Draft"],
        MailboxKind::Sent => &["Sent", "Sent Items", "Sent Messages", "Sent Mail"],
        MailboxKind::Junk => &["Junk", "Spam", "Junk E-mail", "Junk Email", "Bulk Mail"],
        MailboxKind::Trash => &["Trash", "Deleted Items", "Deleted Messages", "Bin"],
        MailboxKind::Archive => &["Archive", "Archives"],
    }
}
