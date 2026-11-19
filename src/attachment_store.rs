//! Durable attachment payload staging and transfer.
//!
//! Payload files live under one retained private-root descriptor. Random names
//! locate entries, while descriptor metadata remains the durable byte authority.

use std::collections::HashMap;
use std::fs::File;
use std::os::unix::fs::MetadataExt;
use std::sync::{Arc, Mutex};

#[cfg(test)]
use std::path::PathBuf;

use enzui_iced_core::io_class::IoFailureCategory;
use enzui_iced_core::portal::PortalOutcome;
use enzui_iced_core::portal::open_uri::RetainedFile;
use enzui_iced_core::secure_dir::{EnumerationLimits, RetainedEntry, SecureRoot};

use crate::model::compose::PendingSend;
use crate::model::types::{
    Attachment, AttachmentPayloadId, AttachmentPayloadIdentity, AttachmentPayloadRef,
};

pub(crate) const MAX_ATTACHMENT_COUNT: usize = 10;
pub(crate) const MAX_ATTACHMENT_NAME_BYTES: usize = 255;
pub(crate) const MAX_ATTACHMENT_MIME_BYTES: usize = 255;
pub(crate) const MAX_ATTACHMENT_BYTES: u64 = 25 * 1024 * 1024;
pub(crate) const MAX_TOTAL_ATTACHMENT_BYTES: u64 = 50 * 1024 * 1024;
pub(crate) const MAX_OUTGOING_MIME_BYTES: usize = 64 * 1024 * 1024;
const STALE_STAGE_AGE_NANOS: i128 = 24 * 60 * 60 * 1_000_000_000;
// Cleanup is fail-closed unless one descriptor-backed census fits both budgets.
pub(crate) const MAX_ATTACHMENT_STORE_ENTRIES: usize = 512;
pub(crate) const MAX_ATTACHMENT_STORE_NAME_BYTES: usize = 32 * 1024;
pub(crate) const MAX_ATTACHMENT_CLEANUP_DEBTS: usize = MAX_ATTACHMENT_STORE_ENTRIES;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AttachmentFailure {
    StoreUnavailable,
    StoreUncertain,
    IdentityUnavailable,
    // The pending send's durable identifiers cannot form a stable Message-ID.
    MalformedIdentity,
    TooMany,
    FileTooLarge,
    TotalTooLarge,
    OutgoingMessageTooLarge,
    MaintenanceDeferred,
    MaintenanceIncomplete,
    WorkerCancelled,
    WorkerShutdown,
    WorkerPanicked,
    NotRegular,
    InvalidPayload,
    PayloadChanged,
    InvalidMetadata,
    InvalidDestination,
    DestinationExists,
    MissingPayload,
    #[cfg(test)]
    NoPendingSend,
    Read(IoFailureCategory),
    Write(IoFailureCategory),
    SaveUncertain(IoFailureCategory),
    OpenUncertain,
}

impl AttachmentFailure {
    pub(crate) fn user_message(self) -> &'static str {
        match self {
            Self::StoreUnavailable => "The private attachment store is unavailable.",
            Self::StoreUncertain => {
                "Attachment bytes were published, but private-store durability is uncertain."
            }
            Self::IdentityUnavailable => "A safe attachment identity could not be created.",
            Self::MalformedIdentity => {
                "The message identifier is not safe to send. Restart the message."
            }
            Self::TooMany => "No attachment was added because the 10-file limit would be exceeded.",
            Self::FileTooLarge => "No attachment was added because one file exceeds 25 MiB.",
            Self::TotalTooLarge => {
                "No attachment was added because the 50 MiB message limit would be exceeded."
            }
            Self::OutgoingMessageTooLarge => {
                "The encoded message exceeds the 64 MiB sending limit."
            }
            Self::MaintenanceDeferred => "Attachment work could not start yet and will be retried.",
            Self::MaintenanceIncomplete => {
                "Attachment maintenance is incomplete. No new attachment bytes can be stored yet."
            }
            Self::WorkerCancelled => "Attachment work was cancelled before it completed.",
            Self::WorkerShutdown => "The attachment worker is unavailable.",
            Self::WorkerPanicked => "The attachment worker stopped unexpectedly.",
            Self::NotRegular => "Only regular local files can be attached.",
            Self::InvalidPayload => "The attachment payload identity is invalid.",
            Self::PayloadChanged => "The attachment payload changed and was not used.",
            Self::InvalidMetadata => "The attachment name or media type is invalid.",
            Self::InvalidDestination => "The selected save location is not a local file.",
            Self::DestinationExists => {
                "A file already exists at that location. Choose a new destination."
            }
            Self::MissingPayload => "The attachment payload is no longer available locally.",
            #[cfg(test)]
            Self::NoPendingSend => "There is no pending message to prepare.",
            Self::Read(IoFailureCategory::Permission) => {
                "The attachment could not be read because access was denied."
            }
            Self::Read(IoFailureCategory::NotFound) => {
                "The attachment could not be read because the file is missing."
            }
            Self::Read(_) => "The attachment could not be read.",
            Self::Write(IoFailureCategory::Permission) => {
                "The attachment could not be saved because access was denied."
            }
            Self::Write(IoFailureCategory::StorageExhausted) => {
                "The attachment could not be saved because storage is full."
            }
            Self::Write(_) => "The attachment could not be saved.",
            Self::SaveUncertain(IoFailureCategory::StorageExhausted) => {
                "The attachment may have been saved, but storage durability could not be confirmed."
            }
            Self::SaveUncertain(_) => {
                "The attachment may have been saved, but durable publication could not be confirmed."
            }
            Self::OpenUncertain => {
                "The exact attachment descriptor could not be handed off, or its stored name changed."
            }
        }
    }
}

// Describes an attachment-open failure; cancellation has no error text.
pub(crate) fn open_failure_text(outcome: &PortalOutcome<()>) -> Option<String> {
    let text = match outcome {
        PortalOutcome::Completed(()) | PortalOutcome::Cancelled(_) => return None,
        PortalOutcome::Unavailable => "No desktop file opener is available on this system.",
        PortalOutcome::Unsupported => {
            "This desktop does not support opening files through its portal."
        }
        PortalOutcome::NotDeclared => "This build is not permitted to open local files.",
        PortalOutcome::Denied => "The portal refused permission to open the attachment.",
        PortalOutcome::ParentUnavailable => {
            "The attachment opener could not be attached to this window."
        }
        PortalOutcome::Failed => "The attachment could not be opened.",
    };
    Some(text.to_owned())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CleanupDebt {
    id: AttachmentPayloadId,
    device: u64,
    inode: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct CleanupState {
    exact: Vec<CleanupDebt>,
    cleanup_census_required: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CensusDisposition {
    Retain,
    Remove,
}

#[derive(Debug)]
struct CensusEntry {
    retained: RetainedEntry,
    payload_id: Option<AttachmentPayloadId>,
    identity: AttachmentPayloadIdentity,
    disposition: CensusDisposition,
    file: File,
}

#[derive(Clone, Debug)]
struct MemoryPayload {
    bytes: Vec<u8>,
    identity: AttachmentPayloadIdentity,
}

#[derive(Debug, Default)]
struct MemoryStore {
    entries: HashMap<AttachmentPayloadId, MemoryPayload>,
    next_inode: u64,
}

#[derive(Debug)]
pub(crate) struct OpenedAttachment {
    root: Arc<SecureRoot>,
    file: Arc<File>,
    payload: AttachmentPayloadRef,
}

impl OpenedAttachment {
    pub(crate) fn retained_file(&self) -> Result<RetainedFile, AttachmentFailure> {
        self.verify_path()?;
        RetainedFile::from_arc(Arc::clone(&self.file)).map_err(|_| AttachmentFailure::OpenUncertain)
    }

    pub(crate) fn payload_ref(&self) -> AttachmentPayloadRef {
        self.payload.clone()
    }

    pub(crate) fn verify_path(&self) -> Result<(), AttachmentFailure> {
        self.root
            .revalidate()
            .map_err(|_| AttachmentFailure::OpenUncertain)?;
        verify_file_identity(&self.file, self.payload.identity)?;
        let named = self
            .root
            .entry_identity(self.payload.id.file_name())
            .map_err(|_| AttachmentFailure::OpenUncertain)?;
        if named != (self.payload.identity.device, self.payload.identity.inode) {
            return Err(AttachmentFailure::OpenUncertain);
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
enum Backend {
    Files(Arc<SecureRoot>),
    Memory(Arc<Mutex<MemoryStore>>),
    Unavailable,
}

#[derive(Clone, Debug)]
pub(crate) struct AttachmentStore {
    backend: Backend,
    cleanup: Arc<Mutex<CleanupState>>,
}

impl AttachmentStore {
    pub(crate) fn remove_payloads(
        &self,
        payloads: &[AttachmentPayloadRef],
    ) -> Result<(), AttachmentFailure> {
        let result = self.remove_payloads_inner(payloads);
        if result.is_err() {
            self.retain_cleanup_debt(payloads);
        }
        result
    }

    fn remove_payloads_inner(
        &self,
        payloads: &[AttachmentPayloadRef],
    ) -> Result<(), AttachmentFailure> {
        match &self.backend {
            Backend::Files(root) => {
                for payload in payloads {
                    if !payload.id.is_valid() {
                        return Err(AttachmentFailure::InvalidPayload);
                    }
                    let file = match root.open_entry_read(payload.id.file_name()) {
                        Ok(file) => file,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(error) => {
                            return Err(AttachmentFailure::Write(IoFailureCategory::classify(
                                &error,
                            )));
                        }
                    };
                    verify_file_identity(&file, payload.identity)?;
                    let removed = root
                        .unlink_entry_if_identity(
                            payload.id.file_name(),
                            (payload.identity.device, payload.identity.inode),
                        )
                        .map_err(|error| {
                            AttachmentFailure::Write(IoFailureCategory::classify(&error))
                        })?;
                    if !removed {
                        return Err(AttachmentFailure::PayloadChanged);
                    }
                }
                root.ordinary_directory()
                    .and_then(|directory| directory.sync_all())
                    .map_err(|error| AttachmentFailure::Write(IoFailureCategory::classify(&error)))
            }
            Backend::Memory(store) => {
                let mut store = store
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                for payload in payloads {
                    match store.entries.get(&payload.id) {
                        Some(stored) if stored.identity == payload.identity => {
                            store.entries.remove(&payload.id);
                        }
                        Some(_) => return Err(AttachmentFailure::PayloadChanged),
                        None => {}
                    }
                }
                Ok(())
            }
            Backend::Unavailable => Err(AttachmentFailure::StoreUnavailable),
        }
    }

    pub(crate) fn retain_cleanup_authority(&self, payload: &AttachmentPayloadRef) {
        self.retain_inode_debt(
            payload.id.clone(),
            payload.identity.device,
            payload.identity.inode,
        );
    }

    fn retain_cleanup_debt(&self, payloads: &[AttachmentPayloadRef]) {
        for payload in payloads {
            self.retain_inode_debt(
                payload.id.clone(),
                payload.identity.device,
                payload.identity.inode,
            );
        }
    }

    fn retain_inode_debt(&self, id: AttachmentPayloadId, device: u64, inode: u64) {
        let mut cleanup = self
            .cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pending = CleanupDebt { id, device, inode };
        if cleanup.exact.contains(&pending) {
            return;
        }
        if cleanup.exact.len() < MAX_ATTACHMENT_CLEANUP_DEBTS {
            cleanup.exact.push(pending);
        } else {
            cleanup.cleanup_census_required = true;
        }
    }

    fn mark_cleanup_census_required(&self) {
        self.cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cleanup_census_required = true;
    }

    fn cleanup_snapshot(&self) -> CleanupState {
        self.cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn clear_cleanup_state_if_unchanged(&self, snapshot: &CleanupState) -> bool {
        let mut cleanup = self
            .cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if &*cleanup != snapshot {
            return false;
        }
        cleanup.exact.clear();
        cleanup.cleanup_census_required = false;
        true
    }

    fn ensure_writes_allowed(&self) -> Result<(), AttachmentFailure> {
        if self
            .cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cleanup_census_required
        {
            Err(AttachmentFailure::MaintenanceIncomplete)
        } else {
            Ok(())
        }
    }

    pub(crate) fn has_cleanup_debt(&self) -> bool {
        let cleanup = self
            .cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cleanup.cleanup_census_required || !cleanup.exact.is_empty()
    }

    pub(crate) fn cleanup_unreferenced(
        &self,
        referenced: &HashMap<AttachmentPayloadId, AttachmentPayloadIdentity>,
    ) -> Result<(), AttachmentFailure> {
        let result = self.cleanup_unreferenced_inner(referenced);
        if result.is_err() {
            self.mark_cleanup_census_required();
        }
        result
    }

    fn cleanup_unreferenced_inner(
        &self,
        referenced: &HashMap<AttachmentPayloadId, AttachmentPayloadIdentity>,
    ) -> Result<(), AttachmentFailure> {
        let cleanup_snapshot = self.cleanup_snapshot();
        let debts = &cleanup_snapshot.exact;
        match &self.backend {
            Backend::Files(root) => {
                let census = bounded_cleanup_census(root, referenced)?;
                classify_cleanup_debts(debts, &census)?;
                for entry in &census {
                    verify_descriptor_identity(&entry.file, entry.identity)
                        .map_err(|_| AttachmentFailure::MaintenanceIncomplete)?;
                    entry
                        .retained
                        .verify()
                        .map_err(|_| AttachmentFailure::MaintenanceIncomplete)?;
                    if (entry.identity.device, entry.identity.inode) != entry.retained.identity() {
                        return Err(AttachmentFailure::MaintenanceIncomplete);
                    }
                }
                let mut removed_any = false;
                for entry in census
                    .iter()
                    .filter(|entry| entry.disposition == CensusDisposition::Remove)
                {
                    verify_descriptor_identity(&entry.file, entry.identity)
                        .map_err(|_| AttachmentFailure::MaintenanceIncomplete)?;
                    entry
                        .retained
                        .verify()
                        .map_err(|_| AttachmentFailure::MaintenanceIncomplete)?;
                    let removed = entry
                        .retained
                        .remove_if_identity()
                        .map_err(|_| AttachmentFailure::MaintenanceIncomplete)?;
                    if !removed {
                        return Err(AttachmentFailure::PayloadChanged);
                    }
                    removed_any = true;
                }
                if removed_any {
                    root.ordinary_directory()
                        .and_then(|directory| directory.sync_all())
                        .map_err(|_| AttachmentFailure::MaintenanceIncomplete)?;
                }
                if !self.clear_cleanup_state_if_unchanged(&cleanup_snapshot) {
                    return Err(AttachmentFailure::MaintenanceIncomplete);
                }
                Ok(())
            }
            Backend::Memory(store) => {
                let mut store = store
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if store.entries.len() > MAX_ATTACHMENT_STORE_ENTRIES
                    || store
                        .entries
                        .keys()
                        .try_fold(0_usize, |total, id| total.checked_add(id.as_str().len()))
                        .is_none_or(|total| total > MAX_ATTACHMENT_STORE_NAME_BYTES)
                {
                    return Err(AttachmentFailure::MaintenanceIncomplete);
                }
                for debt in debts {
                    if let Some(payload) = store.entries.get(&debt.id)
                        && (payload.identity.device, payload.identity.inode)
                            != (debt.device, debt.inode)
                    {
                        return Err(AttachmentFailure::PayloadChanged);
                    }
                }
                store.entries.retain(|id, payload| {
                    referenced
                        .get(id)
                        .is_some_and(|identity| *identity == payload.identity)
                });
                drop(store);
                if !self.clear_cleanup_state_if_unchanged(&cleanup_snapshot) {
                    return Err(AttachmentFailure::MaintenanceIncomplete);
                }
                Ok(())
            }
            Backend::Unavailable
                if debts.is_empty() && !cleanup_snapshot.cleanup_census_required =>
            {
                Ok(())
            }
            Backend::Unavailable => Err(AttachmentFailure::StoreUnavailable),
        }
    }

    fn contains(&self, id: &AttachmentPayloadId) -> bool {
        match &self.backend {
            Backend::Files(root) => match root.entry_identity(id.file_name()) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Ok(_) | Err(_) => true,
            },
            Backend::Memory(store) => store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entries
                .contains_key(id),
            Backend::Unavailable => false,
        }
    }
}

fn classify_cleanup_debts(
    debts: &[CleanupDebt],
    census: &[CensusEntry],
) -> Result<(), AttachmentFailure> {
    for debt in debts {
        let Some(entry) = census
            .iter()
            .find(|entry| entry.payload_id.as_ref() == Some(&debt.id))
        else {
            continue;
        };
        if (entry.identity.device, entry.identity.inode) != (debt.device, debt.inode) {
            return Err(AttachmentFailure::PayloadChanged);
        }
    }
    Ok(())
}

fn bounded_cleanup_census(
    root: &SecureRoot,
    referenced: &HashMap<AttachmentPayloadId, AttachmentPayloadIdentity>,
) -> Result<Vec<CensusEntry>, AttachmentFailure> {
    let entries = root
        .ordinary_directory()
        .and_then(|directory| {
            directory.retain_entries_bounded(&EnumerationLimits {
                max_entries: MAX_ATTACHMENT_STORE_ENTRIES,
                max_name_bytes: MAX_ATTACHMENT_STORE_NAME_BYTES,
            })
        })
        .map_err(|_| AttachmentFailure::MaintenanceIncomplete)?;
    let mut census = Vec::with_capacity(entries.len());
    for retained in entries {
        let file = root
            .open_entry_read(retained.name())
            .map_err(|_| AttachmentFailure::MaintenanceIncomplete)?;
        let identity =
            descriptor_identity(&file).map_err(|_| AttachmentFailure::MaintenanceIncomplete)?;
        if (identity.device, identity.inode) != retained.identity() {
            return Err(AttachmentFailure::MaintenanceIncomplete);
        }
        let name_utf8 = retained.name().to_str();
        let payload_id = name_utf8.and_then(AttachmentPayloadId::from_file_name);
        let staging_name = name_utf8.is_some_and(is_staging_payload_name);
        let disposition = if payload_id.as_ref().is_some_and(|id| {
            referenced
                .get(id)
                .is_some_and(|expected| *expected == identity)
        }) {
            CensusDisposition::Retain
        } else if payload_id.is_some()
            || (staging_name && identity_mtime_nanos(identity).is_some_and(staging_entry_is_old))
        {
            CensusDisposition::Remove
        } else {
            CensusDisposition::Retain
        };
        census.push(CensusEntry {
            retained,
            payload_id,
            identity,
            disposition,
            file,
        });
    }
    Ok(census)
}

fn identity_mtime_nanos(identity: AttachmentPayloadIdentity) -> Option<i128> {
    i128::from(identity.mtime_secs)
        .checked_mul(1_000_000_000)
        .and_then(|seconds| seconds.checked_add(i128::from(identity.mtime_nanos)))
}

fn attachment_payload_ref(
    attachment: &Attachment,
) -> Result<AttachmentPayloadRef, AttachmentFailure> {
    let payload = attachment
        .payload_ref()
        .ok_or(AttachmentFailure::MissingPayload)?;
    if !payload.id.is_valid() || payload.identity.size != attachment.size {
        return Err(AttachmentFailure::InvalidPayload);
    }
    Ok(payload)
}

fn descriptor_identity(file: &File) -> Result<AttachmentPayloadIdentity, AttachmentFailure> {
    let metadata = file
        .metadata()
        .map_err(|error| AttachmentFailure::Read(IoFailureCategory::classify(&error)))?;
    if !metadata.is_file() {
        return Err(AttachmentFailure::NotRegular);
    }
    Ok(AttachmentPayloadIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        size: metadata.len(),
        mtime_secs: metadata.mtime(),
        mtime_nanos: metadata.mtime_nsec(),
        ctime_secs: metadata.ctime(),
        ctime_nanos: metadata.ctime_nsec(),
    })
}

fn payload_identity(file: &File) -> Result<AttachmentPayloadIdentity, AttachmentFailure> {
    let identity = descriptor_identity(file)?;
    if identity.size > MAX_ATTACHMENT_BYTES {
        return Err(AttachmentFailure::FileTooLarge);
    }
    Ok(identity)
}

fn verify_descriptor_identity(
    file: &File,
    expected: AttachmentPayloadIdentity,
) -> Result<(), AttachmentFailure> {
    if descriptor_identity(file)? == expected {
        Ok(())
    } else {
        Err(AttachmentFailure::PayloadChanged)
    }
}

fn verify_file_identity(
    file: &File,
    expected: AttachmentPayloadIdentity,
) -> Result<(), AttachmentFailure> {
    if expected.size > MAX_ATTACHMENT_BYTES {
        return Err(AttachmentFailure::FileTooLarge);
    }
    verify_descriptor_identity(file, expected)
}

fn open_payload_file(
    root: &SecureRoot,
    payload: &AttachmentPayloadRef,
) -> Result<File, AttachmentFailure> {
    let file = root
        .open_entry_read(payload.id.file_name())
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                AttachmentFailure::MissingPayload
            } else {
                AttachmentFailure::Read(IoFailureCategory::classify(&error))
            }
        })?;
    verify_file_identity(&file, payload.identity)?;
    Ok(file)
}

fn is_staging_payload_name(name: &str) -> bool {
    let Some((payload, suffix)) = name.split_once(".tmp-") else {
        return false;
    };
    AttachmentPayloadId::parse(payload).is_some()
        && suffix.split('-').count() == 3
        && suffix
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn staging_entry_is_old(mtime_nanos: i128) -> bool {
    let Some(now_nanos) = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i128::try_from(duration.as_nanos()).ok())
    else {
        return false;
    };
    now_nanos
        .checked_sub(mtime_nanos)
        .is_some_and(|age| age >= STALE_STAGE_AGE_NANOS)
}

pub(crate) fn bounded_display_name(name: &str) -> String {
    let cleaned = name
        .chars()
        .map(|character| {
            if character.is_control() || matches!(character, '/' | '\\') {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    let cleaned = cleaned.trim();
    let cleaned = if cleaned.is_empty() || matches!(cleaned, "." | "..") {
        "attachment"
    } else {
        cleaned
    };
    let end = cleaned
        .char_indices()
        .map(|(index, character)| index + character.len_utf8())
        .take_while(|end| *end <= MAX_ATTACHMENT_NAME_BYTES)
        .last()
        .unwrap_or(0);
    if end == 0 {
        "attachment".to_owned()
    } else {
        cleaned
            .get(..end)
            .expect("filename limit ends on a character boundary")
            .to_owned()
    }
}

fn guess_mime(name: &str) -> String {
    let extension = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match extension.as_str() {
        "pdf" => "application/pdf",
        "csv" => "text/csv",
        "txt" | "md" | "log" => "text/plain",
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        "gz" | "gzip" => "application/x-gzip",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" | "mov" | "mkv" => "video/mp4",
        "mp3" | "m4a" | "wav" | "flac" => "audio/mpeg",
        _ => "application/octet-stream",
    }
    .to_owned()
}

fn append_mime_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), AttachmentFailure> {
    let next = out
        .len()
        .checked_add(bytes.len())
        .ok_or(AttachmentFailure::OutgoingMessageTooLarge)?;
    if next > MAX_OUTGOING_MIME_BYTES {
        return Err(AttachmentFailure::OutgoingMessageTooLarge);
    }
    out.extend_from_slice(bytes);
    Ok(())
}

const MESSAGE_ID_LOCAL_MAX: usize = 257;

fn is_atom_safe_component(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn pending_message_id(pending: &PendingSend) -> Result<String, AttachmentFailure> {
    if !is_atom_safe_component(&pending.account_id) || !is_atom_safe_component(&pending.id) {
        return Err(AttachmentFailure::MalformedIdentity);
    }
    let local = format!("{}.{}", pending.account_id, pending.id);
    if local.len() > MESSAGE_ID_LOCAL_MAX {
        return Err(AttachmentFailure::MalformedIdentity);
    }
    Ok(format!("<{local}@enzui.invalid>"))
}

fn clean_header(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn normalize_body(body: &str) -> String {
    body.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n")
}

fn encode_mime_part(attachment: &Attachment, bytes: &[u8]) -> Vec<u8> {
    let mime = attachment
        .mime
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'.' | b'+'))
        .map(char::from)
        .collect::<String>();
    let mime = if mime.is_empty() {
        "application/octet-stream".to_owned()
    } else {
        mime
    };
    let name = attachment
        .name
        .chars()
        .map(|ch| match ch {
            '"' | '/' | '\\' => '_',
            other if other.is_control() => '_',
            other => other,
        })
        .collect::<String>();
    let encoded = base64(bytes);
    let mut out = format!(
        "Content-Type: {mime}; name=\"{name}\"\r\nContent-Disposition: attachment; filename=\"{name}\"\r\nContent-Transfer-Encoding: base64\r\n\r\n"
    )
    .into_bytes();
    for chunk in encoded.as_bytes().chunks(76) {
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\r\n");
    }
    out
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        out.push(char::from(TABLE[usize::from(a >> 2)]));
        out.push(char::from(TABLE[usize::from(((a & 0x03) << 4) | (b >> 4))]));
        if chunk.len() > 1 {
            out.push(char::from(TABLE[usize::from(((b & 0x0f) << 2) | (c >> 6))]));
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(char::from(TABLE[usize::from(c & 0x3f)]));
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

    fn pending_with(attachments: Vec<Attachment>) -> PendingSend {
        PendingSend {
            id: "sent-test".into(),
            account_id: "account-test".into(),
            sender_name: "Sender".into(),
            sender_email: "sender@example.com".into(),
            to_raw: "recipient@example.com".into(),
            cc_raw: String::new(),
            bcc_raw: String::new(),
            bcc_shown: false,
            subject: "Payload test".into(),
            body: "Body text".into(),
            body_html: None,
            attachments,
            in_reply_to: None,
            references: Vec::new(),
            format: crate::model::compose::ComposeFormat::Plain,
            mode: crate::model::compose::ComposeMode::New,
            restored_draft_id: None,
            review_key: None,
        }
    }

    struct FixtureDir(PathBuf);

    impl FixtureDir {
        fn new(label: &str) -> Self {
            let ordinal = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "mail-app-attachment-{label}-{}-{ordinal}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create isolated attachment fixture");
            Self(path)
        }
    }

    impl Drop for FixtureDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn staged_payload_is_private_restart_readable_and_mime_ready() {
        let fixture = FixtureDir::new("restart");
        let source = fixture.0.join("source name.txt");
        std::fs::write(&source, b"payload bytes").expect("write source fixture");
        let root = fixture.0.join("store");
        let store = AttachmentStore::for_root(&root).expect("private store");
        let attachments = store
            .stage_paths(vec![source.clone()], 0, 0)
            .expect("stage attachment");
        let attachment = attachments[0].clone();
        let payload = attachment.payload_ref().expect("payload identity");
        let id = payload.id.clone();
        assert_eq!(id.as_str().len(), 32);
        assert!(!id.as_str().contains('/'));
        let root_mode = std::fs::metadata(&root)
            .expect("private payload root")
            .permissions()
            .mode()
            & 0o777;
        let mode = std::fs::metadata(root.join(id.file_name()))
            .expect("published payload")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(root_mode, 0o700);
        assert_eq!(mode, 0o600);

        std::fs::remove_file(source).expect("remove ambient source");
        let reopened = AttachmentStore::for_root(&root).expect("reopen store");
        assert_eq!(reopened.read(&attachment).unwrap(), b"payload bytes");
        let mime = reopened
            .outgoing_mime_bytes(&pending_with(attachments))
            .expect("MIME handoff bytes");
        let mime = String::from_utf8(mime).expect("ASCII MIME envelope");
        assert!(mime.contains("Subject: Payload test\r\n"));
        assert!(mime.contains("Body text\r\n"));
        assert!(mime.contains("filename=\"source name.txt\""));
        assert!(mime.contains("cGF5bG9hZCBieXRlcw=="));

        let stale_stage = root.join(format!("{}.tmp-1-2-3", id.as_str()));
        let stale_file = File::create(&stale_stage).expect("create stale stage fixture");
        stale_file
            .set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
            .expect("age stale stage fixture");
        let fresh_stage = root.join(format!("{}.tmp-1-2-4", id.as_str()));
        std::fs::write(&fresh_stage, b"in flight").expect("write fresh stage fixture");
        reopened
            .cleanup_unreferenced(&HashMap::from([(id, payload.identity)]))
            .expect("keep referenced payload and remove abandoned stage");
        assert_eq!(reopened.read(&attachment).unwrap(), b"payload bytes");
        assert!(!stale_stage.exists());
        assert!(fresh_stage.exists());
        reopened
            .cleanup_unreferenced(&HashMap::new())
            .expect("remove orphan");
        assert_eq!(
            reopened.read(&attachment),
            Err(AttachmentFailure::MissingPayload)
        );
    }

    #[test]
    fn staging_quota_failure_is_all_or_nothing() {
        {
            let fixture = FixtureDir::new("no-root");
            let source = fixture.0.join("source.txt");
            std::fs::write(&source, b"payload bytes").expect("write source fixture");
            let store = AttachmentStore::over(None);
            assert!(matches!(
                store.stage_paths(vec![source], 0, 0),
                Err(AttachmentFailure::StoreUnavailable)
            ));
        }

        let fixture = FixtureDir::new("quota");
        let small = fixture.0.join("small.txt");
        let oversized = fixture.0.join("oversized.bin");
        std::fs::write(&small, b"small").expect("write small fixture");
        File::create(&oversized)
            .and_then(|file| file.set_len(MAX_ATTACHMENT_BYTES + 1))
            .expect("write oversized fixture");
        let root = fixture.0.join("store");
        let store = AttachmentStore::for_root(&root).expect("private store");

        assert_eq!(
            store.stage_paths(vec![small.clone(); MAX_ATTACHMENT_COUNT + 1], 0, 0),
            Err(AttachmentFailure::TooMany)
        );
        assert_eq!(
            store.stage_paths(vec![small, oversized], 0, 0),
            Err(AttachmentFailure::FileTooLarge)
        );
        assert!(std::fs::read_dir(root).unwrap().next().is_none());
        assert_eq!(
            store.stage_paths(
                vec![fixture.0.join("small.txt")],
                1,
                MAX_TOTAL_ATTACHMENT_BYTES
            ),
            Err(AttachmentFailure::TotalTooLarge)
        );
    }
}

#[path = "attachment_store/intake.rs"]
mod intake;
