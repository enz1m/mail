//! Atomic local mail state uses additive records; malformed known records refuse the document.
//! Remote changes remain pending until their recorded mailbox generation and UID outcome are verified.

use enzui_iced_core::persist::{
    FileBackend, InMemoryBackend, Persistable, PersistenceKind, RawByteLimit, SaveGate, Store,
};

use crate::backend::{AuthMethod, Encryption, MailProtocol};
use crate::credential_mutation::{CredentialMutationJournal, MAX_CREDENTIAL_TRANSITIONS};
use crate::model::compose::{ComposeFormat, ComposeMode, PendingSend};
use crate::model::types::{
    Account, AttachmentPayloadId, AttachmentPayloadIdentity, MailboxKind, MailboxStore, Message,
    MessageBodyState, MessageDate,
};

mod metadata;
pub(crate) use metadata::LocalStateMetadata;

pub(crate) const FORMAT_ID: &str = "mail-app-local-state";
pub(crate) const VERSION: u32 = 1;

const LOCAL_STATE_RECORDS_MAX: usize = 8_192;
const LOCAL_STATE_FIELDS_PER_RECORD_MAX: usize = 27;
pub(crate) const LOCAL_STATE_FIELD_DECODED_BYTES_MAX: usize = 1024 * 1024;
const LOCAL_STATE_FIELD_ENCODED_BYTES_MAX: usize = 2 * LOCAL_STATE_FIELD_DECODED_BYTES_MAX;
const LOCAL_STATE_RECORD_ENCODED_BYTES_MAX: usize = 32 * 1024 * 1024 + 512;
const LOCAL_STATE_RETAINED_BYTES_MAX: usize = 16 * 1024 * 1024;
const LOCAL_STATE_DECODED_INPUT_BYTES_MAX: usize = 32 * 1024 * 1024;
const LOCAL_STATE_IDENTITY_BYTES_MAX: usize = 1024;
const LOCAL_STATE_ACCOUNTS_MAX: usize = MAX_CREDENTIAL_TRANSITIONS;
const LOCAL_STATE_MESSAGES_MAX: usize = 4_096;
const LOCAL_STATE_FOLDERS_PER_ACCOUNT_MAX: usize = 256;
const LOCAL_STATE_RECIPIENTS_PER_MESSAGE_MAX: usize = 256;
const LOCAL_STATE_REFERENCES_PER_MESSAGE_MAX: usize = 256;
const LOCAL_STATE_UNSUBSCRIBE_URIS_MAX: usize = 4;
pub(crate) const LOCAL_STATE_MUTATIONS_MAX: usize = 4_096;
pub(crate) const LOCAL_STATE_TARGETS_PER_MUTATION_MAX: usize = 256;
const LOCAL_STATE_STRUCTURAL_BYTES_PER_RECORD_MAX: u64 = 512;
const LOCAL_STATE_REPEATED_IDENTITY_BYTES_PER_RECORD_MAX: u64 =
    2 * LOCAL_STATE_IDENTITY_BYTES_MAX as u64;
pub(crate) const LOCAL_STATE_BODY_MAX: u64 = 2 * LOCAL_STATE_RETAINED_BYTES_MAX as u64
    + LOCAL_STATE_RECORDS_MAX as u64
        * (LOCAL_STATE_STRUCTURAL_BYTES_PER_RECORD_MAX
            + LOCAL_STATE_REPEATED_IDENTITY_BYTES_PER_RECORD_MAX);

// Payload bytes are external. The body ceiling is 2× retained text plus one
// escaped repeated identity and 512 syntax bytes for every admitted record.
pub(crate) const LOCAL_STATE_RAW_BYTE_LIMIT: RawByteLimit =
    match RawByteLimit::for_body(FORMAT_ID, VERSION, LOCAL_STATE_BODY_MAX) {
        Ok(limit) => limit,
        Err(_) => panic!("local-state body ceiling exceeds framework authority"),
    };

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalStateToken {
    store_generation: u64,
    sync_adoption_revision: u64,
    pending_send_id: Option<String>,
    next_message_ordinal: u64,
    next_account_ordinal: u64,
    active_account_id: String,
    send_generation: u64,
    next_mutation_ordinal: u64,
    queued_mutations: usize,
    credential_sequence_high_water: u64,
    credential_public_generation: u64,
    credential_transitions: usize,
}

pub(crate) type LocalStateGate = SaveGate<LocalState, LocalStateToken>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ServerMutationKind {
    AccountAdded,
    DraftUpserted,
    Send,
    Move(MailboxKind),
    SetUnread(bool),
    SetFlagged(bool),
    EmptyMailbox(MailboxKind),
    RulesApplied,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ServerMutationTarget {
    pub(crate) message_id: String,
    pub(crate) account_id: String,
    pub(crate) mailbox: MailboxKind,
    pub(crate) uid_validity: u32,
    pub(crate) uid: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingServerMutation {
    pub(crate) id: u64,
    pub(crate) kind: ServerMutationKind,
    pub(crate) target_ids: Vec<String>,
    pub(crate) server_targets: Vec<ServerMutationTarget>,
}

#[derive(Clone, Debug)]
pub(crate) struct LocalState {
    pub(crate) accounts: Vec<Account>,
    pub(crate) messages: Vec<Message>,
    pub(crate) today: MessageDate,
    pub(crate) pending_send: Option<PendingSend>,
    pub(crate) next_message_ordinal: u64,
    pub(crate) next_account_ordinal: u64,
    pub(crate) active_account_id: String,
    pub(crate) send_generation: u64,
    pub(crate) pending_server_mutations: Vec<PendingServerMutation>,
    pub(crate) next_mutation_ordinal: u64,
    pub(crate) credential_journal: CredentialMutationJournal,
}

impl LocalState {
    pub(crate) fn refresh_previews(&mut self) {
        for message in &mut self.messages {
            if message.body_state.is_complete()
                && (!message.body.trim().is_empty()
                    || message.preview.chars().count() < crate::model::compose::PREVIEW_MAX_CHARS
                    || message.preview.contains("[remote image"))
            {
                let text = if message.body.trim().is_empty() {
                    message
                        .body_html
                        .as_deref()
                        .map(|html| {
                            let rendered = crate::model::html::sanitize_and_render_html(
                                html,
                                crate::model::html::SanitizeOptions::SAFE,
                            );
                            crate::model::html::blocks_to_preview(&rendered.blocks)
                        })
                        .unwrap_or_default()
                } else {
                    message.body.clone()
                };
                message.preview = crate::model::compose::preview_from_body(&text);
            }
        }
    }

    pub(crate) fn empty() -> Self {
        Self {
            accounts: Vec::new(),
            messages: Vec::new(),
            today: current_utc_date(),
            pending_send: None,
            next_message_ordinal: 1,
            next_account_ordinal: 1,
            active_account_id: String::new(),
            send_generation: 0,
            pending_server_mutations: Vec::new(),
            next_mutation_ordinal: 1,
            credential_journal: CredentialMutationJournal::default(),
        }
    }

    pub(crate) fn from_fixture(store: MailboxStore) -> Self {
        let active_account_id = store
            .accounts
            .first()
            .map(|account| account.id.clone())
            .unwrap_or_default();
        let mut state = Self {
            accounts: store.accounts,
            messages: store.messages,
            today: store.today,
            pending_send: None,
            next_message_ordinal: 1,
            next_account_ordinal: 1,
            active_account_id,
            send_generation: 0,
            pending_server_mutations: Vec::new(),
            next_mutation_ordinal: 1,
            credential_journal: CredentialMutationJournal::default(),
        };
        state.recover_ordinals();
        state
    }

    pub(crate) fn snapshot(
        store: &MailboxStore,
        pending_send: Option<&PendingSend>,
        pending_server_mutations: &[PendingServerMutation],
        metadata: LocalStateMetadata<'_>,
    ) -> Self {
        let LocalStateMetadata {
            next_message_ordinal,
            next_account_ordinal,
            active_account_id,
            send_generation,
            next_mutation_ordinal,
            credential_journal,
        } = metadata;
        Self {
            accounts: store.accounts.clone(),
            messages: store.messages.clone(),
            today: store.today,
            pending_send: pending_send.cloned(),
            next_message_ordinal,
            next_account_ordinal,
            active_account_id: active_account_id.to_owned(),
            send_generation,
            pending_server_mutations: pending_server_mutations.to_vec(),
            next_mutation_ordinal,
            credential_journal: credential_journal.clone(),
        }
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "Mail operation inputs retain their distinct identities at this boundary."
    )]
    pub(crate) fn from_owned(
        accounts: Vec<Account>,
        messages: Vec<Message>,
        today: MessageDate,
        pending_send: Option<PendingSend>,
        next_message_ordinal: u64,
        next_account_ordinal: u64,
        active_account_id: String,
        send_generation: u64,
        pending_server_mutations: Vec<PendingServerMutation>,
        next_mutation_ordinal: u64,
        credential_journal: CredentialMutationJournal,
    ) -> Self {
        Self {
            accounts,
            messages,
            today,
            pending_send,
            next_message_ordinal,
            next_account_ordinal,
            active_account_id,
            send_generation,
            pending_server_mutations,
            next_mutation_ordinal,
            credential_journal,
        }
    }

    pub(crate) fn token(
        store: &MailboxStore,
        sync_adoption_revision: u64,
        pending_send: Option<&PendingSend>,
        queued_mutations: usize,
        metadata: LocalStateMetadata<'_>,
    ) -> LocalStateToken {
        let LocalStateMetadata {
            next_message_ordinal,
            next_account_ordinal,
            active_account_id,
            send_generation,
            next_mutation_ordinal,
            credential_journal,
        } = metadata;
        LocalStateToken {
            store_generation: store.generation(),
            sync_adoption_revision,
            pending_send_id: pending_send.map(|pending| pending.id.clone()),
            next_message_ordinal,
            next_account_ordinal,
            active_account_id: active_account_id.to_owned(),
            send_generation,
            next_mutation_ordinal,
            queued_mutations,
            credential_sequence_high_water: credential_journal.sequence_high_water(),
            credential_public_generation: credential_journal.public_generation(),
            credential_transitions: credential_journal.records().len(),
        }
    }

    pub(crate) fn recover_ordinals(&mut self) {
        let message_max = self
            .messages
            .iter()
            .filter_map(|message| {
                numeric_suffix(&message.id, "sent-")
                    .or_else(|| numeric_suffix(&message.id, "draft-"))
            })
            .chain(
                self.pending_send
                    .iter()
                    .filter_map(|pending| numeric_suffix(&pending.id, "sent-")),
            )
            .max()
            .unwrap_or(0);
        self.next_message_ordinal = self.next_message_ordinal.max(message_max.saturating_add(1));
        if let Ok(next) = crate::model::settings::recover_account_allocator(
            &self.accounts,
            self.next_account_ordinal,
        ) {
            self.next_account_ordinal = next;
        }
        self.next_mutation_ordinal = self.next_mutation_ordinal.max(
            self.pending_server_mutations
                .iter()
                .map(|mutation| mutation.id)
                .max()
                .unwrap_or(0)
                .saturating_add(1),
        );
        if !self
            .accounts
            .iter()
            .any(|account| account.id == self.active_account_id)
        {
            self.active_account_id = self
                .accounts
                .first()
                .map(|account| account.id.clone())
                .unwrap_or_default();
        }
    }
}

fn numeric_suffix(id: &str, prefix: &str) -> Option<u64> {
    id.strip_prefix(prefix)?.parse().ok()
}

fn current_utc_date() -> MessageDate {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = i64::try_from(seconds / 86_400).unwrap_or_default();
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    MessageDate::at(
        u32::try_from(year).unwrap_or_default(),
        u32::try_from(month).unwrap_or_default(),
        u32::try_from(day).unwrap_or_default(),
        u32::try_from((seconds % 86_400) / 3_600).unwrap_or_default(),
        u32::try_from((seconds % 3_600) / 60).unwrap_or_default(),
    )
}

pub(crate) fn load_with_previews(
    store: &Store<LocalState>,
) -> enzui_iced_core::persist::LoadOutcome<LocalState> {
    let mut outcome = store.load();
    if let enzui_iced_core::persist::LoadOutcome::Loaded(state) = &mut outcome {
        state.refresh_previews();
    }
    outcome
}

// The production store together with the path its frame occupies.
//
// The path is what lets a refused load take identity-bound custody of the
// unreadable bytes instead of publishing over them.
pub(crate) fn file_store_with_path() -> (Store<LocalState>, Option<std::path::PathBuf>) {
    match FileBackend::xdg(
        PersistenceKind::State,
        "mail-app",
        "local-mail",
        LOCAL_STATE_RAW_BYTE_LIMIT,
    )
    .expect("the static local-state XDG descendants are valid")
    {
        Some(backend) => {
            let path = backend.path().to_path_buf();
            (Store::new(backend, FORMAT_ID, VERSION), Some(path))
        }
        None => (memory_store(), None),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UnreadableStateCustody {
    Retained,
    // Custody was not proven, so the unreadable file stays exactly where it is.
    Declined,
}

// The device and inode of the local-state file before a load attempt.
//
// Captured first so custody can refuse to move a file that another writer
// replaced after this process read it.
#[cfg(unix)]
pub(crate) fn source_identity(
    path: &std::path::Path,
) -> Option<enzui_iced_core::quarantine::ExpectedSourceIdentity> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::symlink_metadata(path).ok()?;
    metadata.is_file().then(|| {
        enzui_iced_core::quarantine::ExpectedSourceIdentity::from_dev_ino(
            metadata.dev(),
            metadata.ino(),
        )
    })
}

#[cfg(not(unix))]
pub(crate) fn source_identity(
    _path: &std::path::Path,
) -> Option<enzui_iced_core::quarantine::ExpectedSourceIdentity> {
    None
}

// Retains an unreadable persisted frame under sibling recovery names.
//
// Only a completed move reports [`UnreadableStateCustody::Retained`]; uncertain
// durability, partial custody, a substituted source, and every refusal keep the
// original bytes in place so nothing can publish over them.
pub(crate) fn retain_unreadable_state(
    path: &std::path::Path,
    expected: enzui_iced_core::quarantine::ExpectedSourceIdentity,
) -> UnreadableStateCustody {
    match enzui_iced_core::quarantine::quarantine_sibling_expected(path, expected) {
        enzui_iced_core::quarantine::QuarantineOutcome::Moved { .. } => {
            UnreadableStateCustody::Retained
        }
        _ => UnreadableStateCustody::Declined,
    }
}

// Retains a refused frame and authorizes replacement only on proven custody,
// so this session cannot overwrite state it could not read.
pub(crate) fn authorize_after_custody<T: Persistable>(
    store: &Store<T>,
    path: Option<&std::path::Path>,
    identity: Option<enzui_iced_core::quarantine::ExpectedSourceIdentity>,
) -> bool {
    let (Some(path), Some(identity)) = (path, identity) else {
        return false;
    };
    if retain_unreadable_state(path, identity) != UnreadableStateCustody::Retained {
        return false;
    }
    store.authorize_replacement();
    true
}

pub(crate) fn memory_store() -> Store<LocalState> {
    Store::new(InMemoryBackend::new(), FORMAT_ID, VERSION)
}

#[cfg(test)]
pub(crate) fn store_on(backend: &InMemoryBackend) -> Store<LocalState> {
    Store::new(backend.clone(), FORMAT_ID, VERSION)
}

fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn escaped_field_bytes(value: &str) -> Option<usize> {
    value.as_bytes().iter().try_fold(0_usize, |bytes, byte| {
        bytes.checked_add(if matches!(*byte, b'\\' | b'\t' | b'\n' | b'\r') {
            2
        } else {
            1
        })
    })
}

fn unescape(value: &str) -> String {
    let mut unescaped = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            unescaped.push(ch);
            continue;
        }
        match chars.next() {
            Some('t') => unescaped.push('\t'),
            Some('n') => unescaped.push('\n'),
            Some('r') => unescaped.push('\r'),
            Some('\\') => unescaped.push('\\'),
            Some(other) => {
                unescaped.push('\\');
                unescaped.push(other);
            }
            None => unescaped.push('\\'),
        }
    }
    unescaped
}

fn decoded_field_bytes(value: &str) -> Option<usize> {
    let mut bytes = 0_usize;
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        let added = if ch != '\\' {
            ch.len_utf8()
        } else {
            match chars.next() {
                Some('t' | 'n' | 'r' | '\\') | None => 1,
                Some(other) => 1_usize.checked_add(other.len_utf8())?,
            }
        };
        bytes = bytes.checked_add(added)?;
    }
    Some(bytes)
}

fn validate_encoded_identity(fields: &[&str], index: usize, record: usize) -> Result<(), String> {
    let Some(value) = fields.get(index) else {
        return Ok(());
    };
    if value.len() > 2 * LOCAL_STATE_IDENTITY_BYTES_MAX {
        return Err(format!(
            "line {record}: encoded identity field is too large"
        ));
    }
    let decoded = decoded_field_bytes(value)
        .ok_or_else(|| format!("line {record}: identity length overflow"))?;
    if decoded > LOCAL_STATE_IDENTITY_BYTES_MAX {
        return Err(format!("line {record}: identity field is too large"));
    }
    Ok(())
}

fn validate_encoded_identities(fields: &[&str], record: usize) -> Result<(), String> {
    let Some(kind) = fields.first().copied() else {
        return Ok(());
    };
    let indices: &[usize] = match kind {
        "meta" => &[7],
        "account" => &[1, 18],
        "folder" => &[1, 2],
        "message" => &[1, 2, 4, 23],
        "to" | "cc" | "bcc" => &[1],
        "reference" => &[1, 2],
        "attachment" => &[1, 5],
        "pending" => &[1, 2, 14, 17, 19],
        "pending-attachment" => &[1, 5],
        "pending-reference" => &[1, 2],
        "mutation-target" => &[2],
        "credential-transition" => &[6, 8, 9, 10, 12, 13, 14],
        _ => &[],
    };
    for index in indices {
        validate_encoded_identity(fields, *index, record)?;
    }
    Ok(())
}

fn charge_retained_field(
    fields: &[&str],
    index: usize,
    copies: usize,
    retained_bytes: &mut usize,
    record: usize,
) -> Result<(), String> {
    let Some(value) = fields.get(index) else {
        return Ok(());
    };
    let decoded = decoded_field_bytes(value)
        .ok_or_else(|| format!("line {record}: retained field length overflow"))?;
    let added = decoded
        .checked_mul(copies)
        .ok_or_else(|| "local-state retained byte count overflow".to_owned())?;
    *retained_bytes = retained_bytes
        .checked_add(added)
        .ok_or_else(|| "local-state retained byte count overflow".to_owned())?;
    if *retained_bytes > LOCAL_STATE_RETAINED_BYTES_MAX {
        return Err("local-state retained strings exceed the aggregate limit".to_owned());
    }
    Ok(())
}

fn charge_encoded_local_state_retained(
    fields: &[&str],
    retained_bytes: &mut usize,
    record: usize,
) -> Result<(), String> {
    let Some(kind) = fields.first().copied() else {
        return Ok(());
    };
    let indices: &[usize] = match kind {
        "meta" => &[7],
        "folder" => &[2, 3],
        "message" => &[1, 2, 4, 5, 6, 7, 8, 9, 11, 23],
        "to" | "cc" | "bcc" | "reference" => &[2],
        "attachment" => &[2, 3, 5],
        "pending" => &[1, 2, 3, 4, 5, 6, 7, 9, 10, 12, 14, 17, 19],
        "pending-attachment" => &[2, 3, 5],
        "pending-reference" | "mutation-target" => &[2],
        "credential-transition" => &[6, 8, 9, 10, 12, 13, 14],
        _ => &[],
    };
    if kind == "account" {
        let has_config = fields.get(7).copied() == Some("1");
        let id_copies = if has_config && fields.get(18).is_none_or(|owner| owner.is_empty()) {
            3
        } else if has_config {
            2
        } else {
            1
        };
        charge_retained_field(fields, 1, id_copies, retained_bytes, record)?;
        for index in [2, 3] {
            charge_retained_field(
                fields,
                index,
                if has_config { 2 } else { 1 },
                retained_bytes,
                record,
            )?;
        }
        for index in [4, 9, 11, 13, 15, 18] {
            charge_retained_field(fields, index, 1, retained_bytes, record)?;
        }
        return Ok(());
    }
    for index in indices {
        charge_retained_field(fields, *index, 1, retained_bytes, record)?;
    }
    Ok(())
}

fn validate_encoded_local_state(body: &str) -> Result<(), String> {
    let body_bytes = u64::try_from(body.len()).map_err(|_| "local-state body length overflow")?;
    if body_bytes > LOCAL_STATE_BODY_MAX {
        return Err(format!(
            "local-state body is too large: {body_bytes} bytes exceeds {LOCAL_STATE_BODY_MAX}"
        ));
    }
    let mut decoded_bytes = 0_usize;
    let mut retained_bytes = 0_usize;
    for (index, line) in body.lines().enumerate() {
        let record = index
            .checked_add(1)
            .ok_or_else(|| "local-state record count overflow".to_owned())?;
        if record > LOCAL_STATE_RECORDS_MAX {
            return Err(format!(
                "local-state has too many records: {record} exceeds {LOCAL_STATE_RECORDS_MAX}"
            ));
        }
        if line.len() > LOCAL_STATE_RECORD_ENCODED_BYTES_MAX {
            return Err(format!("line {record}: encoded record is too large"));
        }
        let field_count = line.split('\t').count();
        if field_count > LOCAL_STATE_FIELDS_PER_RECORD_MAX {
            return Err(format!("line {record}: too many fields"));
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        validate_encoded_identities(&fields, record)?;
        charge_encoded_local_state_retained(&fields, &mut retained_bytes, record)?;
        for value in fields {
            if value.len() > LOCAL_STATE_FIELD_ENCODED_BYTES_MAX {
                return Err(format!("line {record}: encoded field is too large"));
            }
            let decoded = decoded_field_bytes(value)
                .ok_or_else(|| format!("line {record}: decoded field length overflow"))?;
            if decoded > LOCAL_STATE_FIELD_DECODED_BYTES_MAX {
                return Err(format!("line {record}: decoded field is too large"));
            }
            decoded_bytes = decoded_bytes
                .checked_add(decoded)
                .ok_or_else(|| "local-state decoded byte count overflow".to_owned())?;
            if decoded_bytes > LOCAL_STATE_DECODED_INPUT_BYTES_MAX {
                return Err("local-state decoded fields exceed the aggregate limit".to_owned());
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

fn write_record(out: &mut String, fields: &[String]) {
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            out.push('\t');
        }
        out.push_str(&escape(field));
    }
    out.push('\n');
}

fn field<'a>(
    fields: &'a [String],
    index: usize,
    line: usize,
    name: &str,
) -> Result<&'a str, String> {
    fields
        .get(index)
        .map(String::as_str)
        .ok_or_else(|| format!("line {line}: missing {name}"))
}

fn parse_u64(fields: &[String], index: usize, line: usize, name: &str) -> Result<u64, String> {
    field(fields, index, line, name)?
        .parse()
        .map_err(|_| format!("line {line}: invalid {name}"))
}

fn parse_u32(fields: &[String], index: usize, line: usize, name: &str) -> Result<u32, String> {
    field(fields, index, line, name)?
        .parse()
        .map_err(|_| format!("line {line}: invalid {name}"))
}

fn parse_i64(fields: &[String], index: usize, line: usize, name: &str) -> Result<i64, String> {
    field(fields, index, line, name)?
        .parse()
        .map_err(|_| format!("line {line}: invalid {name}"))
}

fn parse_u16(fields: &[String], index: usize, line: usize, name: &str) -> Result<u16, String> {
    field(fields, index, line, name)?
        .parse()
        .map_err(|_| format!("line {line}: invalid {name}"))
}

fn body_state_fields(state: MessageBodyState) -> [String; 3] {
    match state {
        MessageBodyState::Complete => ["complete".to_owned(), String::new(), String::new()],
        MessageBodyState::Incomplete {
            retained_bytes,
            limit_bytes,
        } => [
            "incomplete".to_owned(),
            retained_bytes.to_string(),
            limit_bytes.to_string(),
        ],
        MessageBodyState::Malformed => ["malformed".to_owned(), String::new(), String::new()],
    }
}

fn parse_body_state(fields: &[String], line: usize) -> Result<MessageBodyState, String> {
    let Some(kind) = fields.get(24).map(String::as_str) else {
        return Ok(MessageBodyState::Complete);
    };
    match kind {
        "complete" => {
            if fields.get(25).is_some_and(|value| !value.is_empty())
                || fields.get(26).is_some_and(|value| !value.is_empty())
            {
                return Err(format!("line {line}: complete body carries bounds"));
            }
            Ok(MessageBodyState::Complete)
        }
        "incomplete" => {
            let retained_bytes = parse_u32(fields, 25, line, "retained body bytes")?;
            let limit_bytes = parse_u32(fields, 26, line, "body byte limit")?;
            if retained_bytes > limit_bytes || limit_bytes == 0 {
                return Err(format!("line {line}: invalid incomplete body bounds"));
            }
            Ok(MessageBodyState::Incomplete {
                retained_bytes,
                limit_bytes,
            })
        }
        "malformed" => {
            if fields.get(25).is_some_and(|value| !value.is_empty())
                || fields.get(26).is_some_and(|value| !value.is_empty())
            {
                return Err(format!("line {line}: malformed body carries bounds"));
            }
            Ok(MessageBodyState::Malformed)
        }
        _ => Err(format!("line {line}: invalid message body state")),
    }
}

fn payload_identity_fields(identity: Option<AttachmentPayloadIdentity>) -> [String; 7] {
    identity.map_or_else(
        || std::array::from_fn(|_| String::new()),
        |identity| {
            [
                identity.device.to_string(),
                identity.inode.to_string(),
                identity.size.to_string(),
                identity.mtime_secs.to_string(),
                identity.mtime_nanos.to_string(),
                identity.ctime_secs.to_string(),
                identity.ctime_nanos.to_string(),
            ]
        },
    )
}

fn parse_payload_identity(
    fields: &[String],
    first: usize,
    line: usize,
) -> Result<Option<AttachmentPayloadIdentity>, String> {
    let Some(device) = fields.get(first) else {
        return Ok(None);
    };
    if device.is_empty() {
        if fields
            .get(first + 1..first + 7)
            .is_some_and(|rest| rest.iter().any(|value| !value.is_empty()))
        {
            return Err(format!("line {line}: partial attachment payload identity"));
        }
        return Ok(None);
    }
    Ok(Some(AttachmentPayloadIdentity {
        device: parse_u64(fields, first, line, "attachment payload device")?,
        inode: parse_u64(fields, first + 1, line, "attachment payload inode")?,
        size: parse_u64(fields, first + 2, line, "attachment payload identity size")?,
        mtime_secs: parse_i64(fields, first + 3, line, "attachment payload mtime")?,
        mtime_nanos: parse_i64(fields, first + 4, line, "attachment payload mtime nanos")?,
        ctime_secs: parse_i64(fields, first + 5, line, "attachment payload ctime")?,
        ctime_nanos: parse_i64(fields, first + 6, line, "attachment payload ctime nanos")?,
    }))
}

fn bool_token(value: bool) -> String {
    if value { "1" } else { "0" }.to_owned()
}

fn parse_bool(fields: &[String], index: usize, line: usize, name: &str) -> Result<bool, String> {
    match field(fields, index, line, name)? {
        "1" => Ok(true),
        "0" => Ok(false),
        _ => Err(format!("line {line}: invalid {name}")),
    }
}

fn parse_payload_id(
    fields: &[String],
    index: usize,
    line: usize,
) -> Result<Option<AttachmentPayloadId>, String> {
    let Some(value) = fields.get(index).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    AttachmentPayloadId::parse(value.clone())
        .map(Some)
        .ok_or_else(|| format!("line {line}: invalid attachment payload identity"))
}

fn mailbox_token(mailbox: MailboxKind) -> &'static str {
    match mailbox {
        MailboxKind::Inbox => "inbox",
        MailboxKind::Drafts => "drafts",
        MailboxKind::Sent => "sent",
        MailboxKind::Junk => "junk",
        MailboxKind::Trash => "trash",
        MailboxKind::Archive => "archive",
    }
}

fn parse_mailbox(token: &str) -> Option<MailboxKind> {
    match token {
        "inbox" => Some(MailboxKind::Inbox),
        "drafts" => Some(MailboxKind::Drafts),
        "sent" => Some(MailboxKind::Sent),
        "junk" => Some(MailboxKind::Junk),
        "trash" => Some(MailboxKind::Trash),
        "archive" => Some(MailboxKind::Archive),
        _ => None,
    }
}

fn protocol_token(protocol: MailProtocol) -> &'static str {
    match protocol {
        MailProtocol::Imap => "imap",
        MailProtocol::Pop3 => "pop3",
        MailProtocol::Jmap => "jmap",
        MailProtocol::Exchange => "exchange",
    }
}

fn parse_protocol(token: &str) -> Option<MailProtocol> {
    match token {
        "imap" => Some(MailProtocol::Imap),
        "pop3" => Some(MailProtocol::Pop3),
        "jmap" => Some(MailProtocol::Jmap),
        "exchange" => Some(MailProtocol::Exchange),
        _ => None,
    }
}

fn encryption_token(encryption: Encryption) -> &'static str {
    match encryption {
        Encryption::None => "none",
        Encryption::StartTls => "starttls",
        Encryption::Tls => "tls",
    }
}

fn parse_encryption(token: &str) -> Option<Encryption> {
    match token {
        "none" => Some(Encryption::None),
        "starttls" => Some(Encryption::StartTls),
        "tls" => Some(Encryption::Tls),
        _ => None,
    }
}

fn auth_token(auth: AuthMethod) -> &'static str {
    match auth {
        AuthMethod::Plain => "plain",
        AuthMethod::Login => "login",
        AuthMethod::OAuth2 => "oauth2",
    }
}

fn parse_auth(token: &str) -> Option<AuthMethod> {
    match token {
        "plain" => Some(AuthMethod::Plain),
        "login" => Some(AuthMethod::Login),
        "oauth2" => Some(AuthMethod::OAuth2),
        _ => None,
    }
}

fn format_token(format: ComposeFormat) -> &'static str {
    match format {
        ComposeFormat::Plain => "plain",
        ComposeFormat::Rich => "rich",
        ComposeFormat::Html => "html",
    }
}

fn parse_format(token: &str) -> Option<ComposeFormat> {
    match token {
        "plain" => Some(ComposeFormat::Plain),
        "rich" => Some(ComposeFormat::Rich),
        "html" => Some(ComposeFormat::Html),
        _ => None,
    }
}

fn mode_fields(mode: &ComposeMode) -> (&'static str, String) {
    match mode {
        ComposeMode::New => ("new", String::new()),
        ComposeMode::Reply(id) => ("reply", id.clone()),
        ComposeMode::ReplyAll(id) => ("reply-all", id.clone()),
        ComposeMode::Forward(id) => ("forward", id.clone()),
        ComposeMode::EditDraft(id) => ("edit-draft", id.clone()),
    }
}

fn parse_mode(kind: &str, id: String) -> Option<ComposeMode> {
    match kind {
        "new" => Some(ComposeMode::New),
        "reply" => Some(ComposeMode::Reply(id)),
        "reply-all" => Some(ComposeMode::ReplyAll(id)),
        "forward" => Some(ComposeMode::Forward(id)),
        "edit-draft" => Some(ComposeMode::EditDraft(id)),
        _ => None,
    }
}

fn mutation_fields(kind: &ServerMutationKind) -> (&'static str, String) {
    match kind {
        ServerMutationKind::AccountAdded => ("account", String::new()),
        ServerMutationKind::DraftUpserted => ("draft", String::new()),
        ServerMutationKind::Send => ("send", String::new()),
        ServerMutationKind::Move(mailbox) => ("move", mailbox_token(*mailbox).to_owned()),
        ServerMutationKind::SetUnread(value) => ("unread", bool_token(*value)),
        ServerMutationKind::SetFlagged(value) => ("flagged", bool_token(*value)),
        ServerMutationKind::EmptyMailbox(mailbox) => ("empty", mailbox_token(*mailbox).to_owned()),
        ServerMutationKind::RulesApplied => ("rules", String::new()),
    }
}

fn parse_mutation_kind(kind: &str, value: &str, line: usize) -> Result<ServerMutationKind, String> {
    match kind {
        "account" => Ok(ServerMutationKind::AccountAdded),
        "draft" => Ok(ServerMutationKind::DraftUpserted),
        "send" => Ok(ServerMutationKind::Send),
        "move" => parse_mailbox(value)
            .map(ServerMutationKind::Move)
            .ok_or_else(|| format!("line {line}: invalid move mailbox")),
        "unread" => match value {
            "1" => Ok(ServerMutationKind::SetUnread(true)),
            "0" => Ok(ServerMutationKind::SetUnread(false)),
            _ => Err(format!("line {line}: invalid unread value")),
        },
        "flagged" => match value {
            "1" => Ok(ServerMutationKind::SetFlagged(true)),
            "0" => Ok(ServerMutationKind::SetFlagged(false)),
            _ => Err(format!("line {line}: invalid flagged value")),
        },
        "empty" => parse_mailbox(value)
            .map(ServerMutationKind::EmptyMailbox)
            .ok_or_else(|| format!("line {line}: invalid empty mailbox")),
        "rules" => Ok(ServerMutationKind::RulesApplied),
        _ => Err(format!("line {line}: invalid mutation kind")),
    }
}

#[cfg(test)]
mod tests;

#[path = "local_state/persist.rs"]
mod persist;
#[path = "local_state/validation.rs"]
mod validation;
