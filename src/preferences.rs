//! Durable rules, privacy decisions, and sidebar layout.
//!
//! The tab-separated format ignores unknown record types but rejects malformed
//! known records. Absent documents receive generic product defaults; stored
//! empty rule or sidebar lists remain empty. Inert policy fields are not persisted.

use enzui_iced_core::persist::{
    FileBackend, InMemoryBackend, PersistenceKind, RawByteLimit, SaveGate, Store,
};

use crate::model::compose::ComposeFormat;
use crate::model::privacy::RemoteContentPolicy;
use crate::model::rules::{ConditionCombinator, ConditionField, ConditionMatch, RuleSet};
use crate::model::sidebar_layout::{FavoriteTarget, SidebarLayout, SmartMailboxKind};
use crate::model::types::FavoriteKind;
use crate::model::types::{MailboxKind, PrivacyState};

// The persistence identity of the preferences document. Bumping the version
// makes an older file refused rather than misread.
pub(crate) const FORMAT_ID: &str = "mail-app-preferences";
pub(crate) const VERSION: u32 = 1;

const PREFERENCES_RECORDS_MAX: usize = 2_048;
const PREFERENCES_FIELDS_PER_RECORD_MAX: usize = 5;
pub(crate) const PREFERENCES_FIELD_DECODED_BYTES_MAX: usize = 64 * 1024;
const PREFERENCES_FIELD_ENCODED_BYTES_MAX: usize = 2 * PREFERENCES_FIELD_DECODED_BYTES_MAX;
const PREFERENCES_RECORD_ENCODED_BYTES_MAX: usize = 256 * 1024 + 256;
const PREFERENCES_RETAINED_BYTES_MAX: usize = 2 * 1024 * 1024;
const PREFERENCES_IDENTITY_BYTES_MAX: usize = 1024;
const PREFERENCES_DECODED_INPUT_BYTES_MAX: usize = 3 * 1024 * 1024;
pub(crate) const PREFERENCES_ALLOW_LIST_MAX: usize = 512;
const PREFERENCES_BLOCK_LIST_MAX: usize = 512;
const PREFERENCES_RULES_MAX: usize = 256;
pub(crate) const PREFERENCES_CONDITIONS_PER_RULE_MAX: usize = 16;
pub(crate) const PREFERENCES_ACTIONS_PER_RULE_MAX: usize = 16;
pub(crate) const PREFERENCES_FAVORITES_MAX: usize = 256;
const PREFERENCES_SMART_MAILBOXES_MAX: usize = SmartMailboxKind::ALL.len();
const PREFERENCES_STRUCTURAL_BYTES_PER_RECORD_MAX: u64 = 256;
pub(crate) const PREFERENCES_BODY_MAX: u64 = 2 * PREFERENCES_RETAINED_BYTES_MAX as u64
    + PREFERENCES_RECORDS_MAX as u64 * PREFERENCES_STRUCTURAL_BYTES_PER_RECORD_MAX;

// The body ceiling is 2× retained text for worst-case escaping plus 256 syntax
// bytes for every admitted record.
pub(crate) const PREFERENCES_RAW_BYTE_LIMIT: RawByteLimit =
    match RawByteLimit::for_body(FORMAT_ID, VERSION, PREFERENCES_BODY_MAX) {
        Ok(limit) => limit,
        Err(_) => panic!("preferences body ceiling exceeds framework authority"),
    };

// Revision tuple used to detect changes in durable preferences.
pub(crate) type PreferencesToken = (u64, u64, u64, u64, u64, u64, u64);

pub(crate) type PreferencesGate = SaveGate<Preferences, PreferencesToken>;

// Which user decision armed a preferences write.
//
// Rules and privacy share one document and one `save_preferences` call. The
// origin travels with the write in [`crate::Message::PreferencesSaved`], so a
// failure names the decision the user made and is reported where it was made.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SaveOrigin {
    Rules,
    Privacy,
    SidebarLayout,
    ReaderFormat,
    Notifications,
    ComposeFormat,
    // A privacy decision armed outside Settings, such as the reader's "Always
    // allow this sender"; reported in both owning panes.
    CrossOrigin,
    CloseSettlement,
}

impl SaveOrigin {
    // The notice title for a failure with this origin, naming only the decision
    // that did not reach disk.
    pub(crate) fn failure_title(self) -> &'static str {
        match self {
            Self::Rules => "Could not save rules",
            Self::SidebarLayout => "Could not save sidebar settings",
            Self::ReaderFormat => "Could not save reading settings",
            Self::Notifications => "Could not save notification settings",
            Self::Privacy => "Could not save privacy settings",
            Self::ComposeFormat => "Could not save the message format",
            Self::CrossOrigin => "Could not save sender permissions",
            Self::CloseSettlement => "Could not finish saving before close",
        }
    }

    pub(crate) fn failure_body(self) -> &'static str {
        match self {
            Self::Rules => {
                "These rules apply to this session, but may not come back the \
                 next time mail-app starts."
            }
            Self::SidebarLayout | Self::ReaderFormat | Self::Notifications => {
                "These settings apply for now, but may not \
                 come back the next time mail-app starts."
            }
            Self::ComposeFormat => {
                "New messages use this format for now, but it may not come \
                 back the next time mail-app starts."
            }
            Self::Privacy | Self::CrossOrigin => {
                "These rules and sender decisions apply to this session, \
                 but may not come back the next time mail-app starts."
            }
            Self::CloseSettlement => {
                "The window remains open. Retry saving, keep working, or close without saving."
            }
        }
    }
}

// A preferences write that failed, with the decision that armed it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SaveFailure {
    origin: SaveOrigin,
    error: enzui_iced_core::persist::PersistError,
}

impl SaveFailure {
    // Pair a refused write with the decision that armed it.
    pub(crate) fn new(origin: SaveOrigin, error: enzui_iced_core::persist::PersistError) -> Self {
        Self { origin, error }
    }
    // The decision that armed the refused write.
    pub(crate) fn origin(&self) -> SaveOrigin {
        self.origin
    }
    // Stable failure category used by bounded close retry policy.
    pub(crate) fn kind(&self) -> enzui_iced_core::persist::PersistErrorKind {
        self.error.kind()
    }
}

impl std::fmt::Display for SaveFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.origin.failure_title(), self.error)
    }
}

// The persisted document: everything from `State` that must outlive the
// process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Preferences {
    // The user's ordered rule set. Order is semantic, so the file preserves
    // it verbatim.
    pub(crate) rules: RuleSet,
    pub(crate) policy: RemoteContentPolicy,
    pub(crate) allow_list: Vec<String>,
    pub(crate) block_list: Vec<String>,
    pub(crate) sidebar_layout: SidebarLayout,
    pub(crate) compose_format: ComposeFormat,
    // Prefer the MIME plain-text alternative when reading messages.
    pub(crate) prefer_plain_text: bool,
    pub(crate) dark_html: bool,
    pub(crate) notifications: crate::notification_settings::NotificationSettings,
}

impl Preferences {
    pub(crate) fn defaults() -> Self {
        Preferences {
            rules: RuleSet::default(),
            policy: PrivacyState::default().policy,
            allow_list: Vec::new(),
            block_list: Vec::new(),
            sidebar_layout: SidebarLayout::default(),
            compose_format: ComposeFormat::default(),
            prefer_plain_text: false,
            dark_html: false,
            notifications: Default::default(),
        }
    }

    pub(crate) fn demo_fixture() -> Self {
        Self {
            rules: crate::demo_rules(),
            policy: PrivacyState::default().policy,
            allow_list: Vec::new(),
            block_list: Vec::new(),
            sidebar_layout: SidebarLayout::demo_fixture(),
            compose_format: ComposeFormat::default(),
            prefer_plain_text: false,
            dark_html: false,
            notifications: Default::default(),
        }
    }

    // Apply a loaded document onto a fresh [`PrivacyState`], leaving the
    // session-only and inert fields at their defaults.
    pub(crate) fn privacy_state(&self) -> PrivacyState {
        PrivacyState {
            policy: self.policy,
            allow_list: self.allow_list.clone(),
            block_list: self.block_list.clone(),
            ..PrivacyState::default()
        }
    }
}

// The production store: `$XDG_CONFIG_HOME/mail-app/preferences`.
//
// Falls back to an in-memory backend when XDG cannot be resolved, as
// [`crate::shortcuts::file_store`] does.
pub(crate) fn file_store() -> Store<Preferences> {
    match FileBackend::xdg(
        PersistenceKind::Config,
        "mail-app",
        "preferences",
        PREFERENCES_RAW_BYTE_LIMIT,
    )
    .expect("the static preferences XDG descendants are valid")
    {
        Some(backend) => Store::new(backend, FORMAT_ID, VERSION),
        None => memory_store(),
    }
}

// The production preferences path, so a refused frame can be taken into
// custody instead of being published over.
pub(crate) fn file_store_path() -> Option<std::path::PathBuf> {
    enzui_iced_core::persist::xdg_path(PersistenceKind::Config, "mail-app", "preferences")
        .expect("the static preferences XDG descendants are valid")
}

// A store that never touches disk. `boot_for_test` uses this so the suite can
// neither read nor write the developer's real configuration.
pub(crate) fn memory_store() -> Store<Preferences> {
    Store::new(InMemoryBackend::new(), FORMAT_ID, VERSION)
}

const SEP: char = '\t';

// Escape a field value so it stays one line and one field; user text can
// contain a TAB or newline that would otherwise split the record.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

// Inverse of [`escape`]. An unknown escape (`\q`) keeps the character
// verbatim, so a stray backslash cannot cost the whole rule set.
fn escaped_field_bytes(value: &str) -> Option<usize> {
    value.as_bytes().iter().try_fold(0_usize, |bytes, byte| {
        bytes.checked_add(if matches!(*byte, b'\\' | b'\t' | b'\n' | b'\r') {
            2
        } else {
            1
        })
    })
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn decoded_field_bytes(value: &str) -> Option<usize> {
    let mut bytes = 0_usize;
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        let added = if ch != '\\' {
            ch.len_utf8()
        } else {
            match chars.next() {
                Some('\\' | 't' | 'n' | 'r') | None => 1,
                Some(other) => other.len_utf8(),
            }
        };
        bytes = bytes.checked_add(added)?;
    }
    Some(bytes)
}

fn charge_preferences_retained(
    fields: &[&str],
    retained_bytes: &mut usize,
    record: usize,
) -> Result<(), String> {
    let Some(kind) = fields.first().copied() else {
        return Ok(());
    };
    let indices: &[usize] = match kind {
        "allow" | "block" => &[1],
        "rule" => &[1, 4],
        "cond" => &[3],
        "favorite" if fields.get(1).copied() == Some("mailbox") => &[2],
        "favorite" if fields.get(1).copied() == Some("folder") => &[2, 3],
        _ => &[],
    };
    for index in indices {
        let Some(value) = fields.get(*index) else {
            continue;
        };
        let decoded = decoded_field_bytes(value)
            .ok_or_else(|| format!("line {record}: retained field length overflow"))?;
        *retained_bytes = retained_bytes
            .checked_add(decoded)
            .ok_or_else(|| "preferences retained byte count overflow".to_owned())?;
        if *retained_bytes > PREFERENCES_RETAINED_BYTES_MAX {
            return Err("preferences retained strings exceed the aggregate limit".to_owned());
        }
    }
    Ok(())
}

fn validate_preferences_identities(fields: &[&str], record: usize) -> Result<(), String> {
    let Some(kind) = fields.first().copied() else {
        return Ok(());
    };
    let indices: &[usize] = match kind {
        "rule" => &[1],
        "favorite" if fields.get(1).copied() == Some("mailbox") => &[2],
        "favorite" if fields.get(1).copied() == Some("folder") => &[2, 3],
        _ => &[],
    };
    for index in indices {
        let Some(value) = fields.get(*index) else {
            continue;
        };
        if value.len() > 2 * PREFERENCES_IDENTITY_BYTES_MAX {
            return Err(format!(
                "line {record}: encoded identity field is too large"
            ));
        }
        let decoded = decoded_field_bytes(value)
            .ok_or_else(|| format!("line {record}: identity length overflow"))?;
        if decoded > PREFERENCES_IDENTITY_BYTES_MAX {
            return Err(format!("line {record}: identity field is too large"));
        }
    }
    Ok(())
}

fn validate_encoded_preferences(body: &str) -> Result<(), String> {
    let body_bytes = u64::try_from(body.len()).map_err(|_| "preferences body length overflow")?;
    if body_bytes > PREFERENCES_BODY_MAX {
        return Err(format!(
            "preferences body is too large: {body_bytes} bytes exceeds {PREFERENCES_BODY_MAX}"
        ));
    }
    let mut decoded_bytes = 0_usize;
    let mut retained_bytes = 0_usize;
    for (index, line) in body.lines().enumerate() {
        let record = index
            .checked_add(1)
            .ok_or_else(|| "preferences record count overflow".to_owned())?;
        if record > PREFERENCES_RECORDS_MAX {
            return Err(format!(
                "preferences has too many records: {record} exceeds {PREFERENCES_RECORDS_MAX}"
            ));
        }
        if line.len() > PREFERENCES_RECORD_ENCODED_BYTES_MAX {
            return Err(format!("line {record}: encoded record is too large"));
        }
        let field_count = line.split(SEP).count();
        if field_count > PREFERENCES_FIELDS_PER_RECORD_MAX {
            return Err(format!("line {record}: too many fields"));
        }
        let fields = line.split(SEP).collect::<Vec<_>>();
        validate_preferences_identities(&fields, record)?;
        charge_preferences_retained(&fields, &mut retained_bytes, record)?;
        for value in fields {
            if value.len() > PREFERENCES_FIELD_ENCODED_BYTES_MAX {
                return Err(format!("line {record}: encoded field is too large"));
            }
            let decoded = decoded_field_bytes(value)
                .ok_or_else(|| format!("line {record}: decoded field length overflow"))?;
            if decoded > PREFERENCES_FIELD_DECODED_BYTES_MAX {
                return Err(format!("line {record}: decoded field is too large"));
            }
            decoded_bytes = decoded_bytes
                .checked_add(decoded)
                .ok_or_else(|| "preferences decoded byte count overflow".to_owned())?;
            if decoded_bytes > PREFERENCES_DECODED_INPUT_BYTES_MAX {
                return Err("preferences decoded fields exceed the aggregate limit".to_owned());
            }
        }
    }
    Ok(())
}

fn admit_next(len: usize, maximum: usize, name: &str, line: usize) -> Result<(), String> {
    if len >= maximum {
        Err(format!("line {line}: too many {name}"))
    } else {
        Ok(())
    }
}

fn policy_token(p: RemoteContentPolicy) -> &'static str {
    match p {
        RemoteContentPolicy::AlwaysBlock => "always-block",
        RemoteContentPolicy::NeverBlock => "never-block",
        RemoteContentPolicy::AskPerSender => "ask-per-sender",
    }
}

fn policy_from_token(t: &str) -> Option<RemoteContentPolicy> {
    match t {
        "always-block" => Some(RemoteContentPolicy::AlwaysBlock),
        "never-block" => Some(RemoteContentPolicy::NeverBlock),
        "ask-per-sender" => Some(RemoteContentPolicy::AskPerSender),
        _ => None,
    }
}

fn combinator_token(c: ConditionCombinator) -> &'static str {
    match c {
        ConditionCombinator::All => "all",
        ConditionCombinator::Any => "any",
    }
}

fn combinator_from_token(t: &str) -> Option<ConditionCombinator> {
    match t {
        "all" => Some(ConditionCombinator::All),
        "any" => Some(ConditionCombinator::Any),
        _ => None,
    }
}

fn field_token(f: ConditionField) -> &'static str {
    match f {
        ConditionField::From => "from",
        ConditionField::Subject => "subject",
        ConditionField::Recipient => "recipient",
    }
}

fn field_from_token(t: &str) -> Option<ConditionField> {
    match t {
        "from" => Some(ConditionField::From),
        "subject" => Some(ConditionField::Subject),
        "recipient" => Some(ConditionField::Recipient),
        _ => None,
    }
}

fn match_token(m: ConditionMatch) -> &'static str {
    match m {
        ConditionMatch::Contains => "contains",
        ConditionMatch::Is => "is",
        ConditionMatch::BeginsWith => "begins-with",
        ConditionMatch::EndsWith => "ends-with",
    }
}

fn match_from_token(t: &str) -> Option<ConditionMatch> {
    match t {
        "contains" => Some(ConditionMatch::Contains),
        "is" => Some(ConditionMatch::Is),
        "begins-with" => Some(ConditionMatch::BeginsWith),
        "ends-with" => Some(ConditionMatch::EndsWith),
        _ => None,
    }
}

// Mailbox tokens for `MoveTo`, spelled out so a UI-label change cannot
// invalidate files on disk.
fn mailbox_token(k: MailboxKind) -> &'static str {
    match k {
        MailboxKind::Inbox => "inbox",
        MailboxKind::Drafts => "drafts",
        MailboxKind::Sent => "sent",
        MailboxKind::Junk => "junk",
        MailboxKind::Trash => "trash",
        MailboxKind::Archive => "archive",
    }
}

fn mailbox_from_token(t: &str) -> Option<MailboxKind> {
    match t {
        "inbox" => Some(MailboxKind::Inbox),
        "drafts" => Some(MailboxKind::Drafts),
        "sent" => Some(MailboxKind::Sent),
        "junk" => Some(MailboxKind::Junk),
        "trash" => Some(MailboxKind::Trash),
        "archive" => Some(MailboxKind::Archive),
        _ => None,
    }
}

// Persisted tokens do not depend on UI labels.
fn favorite_kind_token(k: FavoriteKind) -> &'static str {
    match k {
        FavoriteKind::Inbox => "inbox",
        FavoriteKind::Flagged => "flagged",
        FavoriteKind::Drafts => "drafts",
        FavoriteKind::Sent => "sent",
    }
}

fn favorite_kind_from_token(t: &str) -> Option<FavoriteKind> {
    match t {
        "inbox" => Some(FavoriteKind::Inbox),
        "flagged" => Some(FavoriteKind::Flagged),
        "drafts" => Some(FavoriteKind::Drafts),
        "sent" => Some(FavoriteKind::Sent),
        _ => None,
    }
}

fn favorite_fields(target: &FavoriteTarget) -> String {
    match target {
        FavoriteTarget::Unified(kind) => {
            format!("unified{SEP}{}", favorite_kind_token(*kind))
        }
        FavoriteTarget::Mailbox {
            account_id,
            mailbox,
        } => format!(
            "mailbox{SEP}{}{SEP}{}",
            escape(account_id),
            mailbox_token(*mailbox)
        ),
        FavoriteTarget::Folder {
            account_id,
            folder_id,
        } => format!(
            "folder{SEP}{}{SEP}{}",
            escape(account_id),
            escape(folder_id)
        ),
    }
}

// One unreadable favorite record is skipped without rejecting other settings.
fn favorite_from_fields(rest: &[&str]) -> Option<FavoriteTarget> {
    match (rest.first().copied(), rest.get(1), rest.get(2)) {
        (Some("unified"), Some(kind), _) => {
            favorite_kind_from_token(kind).map(FavoriteTarget::Unified)
        }
        (Some("mailbox"), Some(account), Some(mailbox)) => {
            mailbox_from_token(mailbox).map(|mailbox| FavoriteTarget::Mailbox {
                account_id: unescape(account),
                mailbox,
            })
        }
        (Some("folder"), Some(account), Some(folder)) => Some(FavoriteTarget::Folder {
            account_id: unescape(account),
            folder_id: unescape(folder),
        }),
        _ => None,
    }
}

// Persisted tokens do not depend on UI labels.
fn compose_format_token(format: ComposeFormat) -> &'static str {
    match format {
        ComposeFormat::Plain => "plain",
        ComposeFormat::Rich => "rich",
        ComposeFormat::Html => "html",
    }
}

fn compose_format_from_token(t: &str) -> Option<ComposeFormat> {
    match t {
        "plain" => Some(ComposeFormat::Plain),
        "rich" => Some(ComposeFormat::Rich),
        "html" => Some(ComposeFormat::Html),
        _ => None,
    }
}

fn bool_token(b: bool) -> &'static str {
    if b { "1" } else { "0" }
}

fn bool_from_token(t: &str) -> Option<bool> {
    match t {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests;

#[path = "preferences/validation.rs"]
mod validation;
