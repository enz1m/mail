//! Attachment intake and retrieval: staging, publishing, validation,
//! reading, opening, export, and outgoing MIME.

use super::{
    AttachmentFailure, AttachmentStore, Backend, CleanupState, MAX_ATTACHMENT_BYTES,
    MAX_ATTACHMENT_COUNT, MAX_ATTACHMENT_MIME_BYTES, MAX_TOTAL_ATTACHMENT_BYTES, MemoryPayload,
    MemoryStore, OpenedAttachment, append_mime_bytes, attachment_payload_ref, bounded_display_name,
    clean_header, encode_mime_part, guess_mime, normalize_body, open_payload_file,
    payload_identity, pending_message_id, verify_file_identity,
};
use crate::model::compose::PendingSend;
use crate::model::types::{
    Attachment, AttachmentPayloadId, AttachmentPayloadIdentity, AttachmentPayloadRef,
};
use enzui_iced_core::io_class::IoFailureCategory;
#[cfg(not(test))]
use enzui_iced_core::persist::{PersistenceKind, XdgResolver};
use enzui_iced_core::random;
use enzui_iced_core::secure_dir::SecureRoot;
use enzui_iced_core::tempstage::RootTempStage;
use enzui_iced_core::user_file::{self, SavePolicy};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

impl AttachmentStore {
    #[cfg(not(test))]
    pub(crate) fn from_xdg() -> Self {
        let root = XdgResolver::from_env()
            .resolve(PersistenceKind::State, "mail-app", "attachments")
            .ok()
            .flatten();
        Self::over(root.and_then(|root| SecureRoot::open(&root).ok()))
    }

    // A store over `root`; without one every operation reports
    // `StoreUnavailable`.
    pub(super) fn over(root: Option<SecureRoot>) -> Self {
        match root {
            Some(root) => Self {
                backend: Backend::Files(Arc::new(root)),
                cleanup: Arc::new(Mutex::new(CleanupState::default())),
            },
            None => Self {
                backend: Backend::Unavailable,
                cleanup: Arc::new(Mutex::new(CleanupState::default())),
            },
        }
    }

    pub(crate) fn memory() -> Self {
        Self {
            backend: Backend::Memory(Arc::new(Mutex::new(MemoryStore::default()))),
            cleanup: Arc::new(Mutex::new(CleanupState::default())),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_root(root: &Path) -> Result<Self, AttachmentFailure> {
        let root = SecureRoot::open(root).map_err(|_| AttachmentFailure::StoreUnavailable)?;
        Ok(Self::over(Some(root)))
    }

    pub(crate) fn stage_paths(
        &self,
        paths: Vec<PathBuf>,
        existing_count: usize,
        existing_bytes: u64,
    ) -> Result<Vec<Attachment>, AttachmentFailure> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        if existing_count.saturating_add(paths.len()) > MAX_ATTACHMENT_COUNT {
            return Err(AttachmentFailure::TooMany);
        }
        let mut staged = Vec::with_capacity(paths.len());
        let mut total = existing_bytes;
        for path in paths {
            let attachment = match self.stage_one(&path, total) {
                Ok(attachment) => attachment,
                Err(error) => {
                    let payloads = staged
                        .iter()
                        .filter_map(Attachment::payload_ref)
                        .collect::<Vec<_>>();
                    self.remove_payloads(&payloads)?;
                    return Err(error);
                }
            };
            total = total
                .checked_add(attachment.size)
                .ok_or(AttachmentFailure::TotalTooLarge)?;
            staged.push(attachment);
        }
        Ok(staged)
    }

    fn stage_one(&self, path: &Path, existing_bytes: u64) -> Result<Attachment, AttachmentFailure> {
        let mut source = File::open(path)
            .map_err(|error| AttachmentFailure::Read(IoFailureCategory::classify(&error)))?;
        let source_identity = payload_identity(&source)?;
        if existing_bytes
            .checked_add(source_identity.size)
            .is_none_or(|total| total > MAX_TOTAL_ATTACHMENT_BYTES)
        {
            return Err(AttachmentFailure::TotalTooLarge);
        }
        let mut bytes = Vec::with_capacity(usize::try_from(source_identity.size).unwrap_or(0));
        source
            .by_ref()
            .take(MAX_ATTACHMENT_BYTES.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| AttachmentFailure::Read(IoFailureCategory::classify(&error)))?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_ATTACHMENT_BYTES {
            return Err(AttachmentFailure::FileTooLarge);
        }
        verify_file_identity(&source, source_identity)?;
        if existing_bytes
            .checked_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
            .is_none_or(|total| total > MAX_TOTAL_ATTACHMENT_BYTES)
        {
            return Err(AttachmentFailure::TotalTooLarge);
        }
        let payload = self.publish(&bytes)?;
        let name = bounded_display_name(
            &path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        Ok(Attachment {
            name: name.clone(),
            mime: guess_mime(&name),
            size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            payload_id: Some(payload.id),
            payload_identity: Some(payload.identity),
        })
    }

    pub(crate) fn stage_inbound(
        &self,
        name: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<Attachment, AttachmentFailure> {
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_ATTACHMENT_BYTES
            || mime.len() > MAX_ATTACHMENT_MIME_BYTES
        {
            return Err(AttachmentFailure::InvalidMetadata);
        }
        let name = bounded_display_name(name);
        let payload = self.publish(bytes)?;
        Ok(Attachment {
            name,
            mime: mime.to_owned(),
            size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            payload_id: Some(payload.id),
            payload_identity: Some(payload.identity),
        })
    }

    fn publish(&self, bytes: &[u8]) -> Result<AttachmentPayloadRef, AttachmentFailure> {
        self.ensure_writes_allowed()?;
        let id = random::mint_unique(
            || random::random_array::<16>().map(AttachmentPayloadId::from_bytes),
            |candidate| self.contains(candidate),
        )
        .map_err(|_| AttachmentFailure::IdentityUnavailable)?;
        let identity = match &self.backend {
            Backend::Files(root) => {
                let mut stage = RootTempStage::create(root, id.file_name()).map_err(|error| {
                    AttachmentFailure::Write(IoFailureCategory::classify(&error))
                })?;
                let staged_inode = stage.identity();
                stage.write_all(bytes).map_err(|error| {
                    AttachmentFailure::Write(IoFailureCategory::classify(&error))
                })?;
                stage.commit().map_err(|error| {
                    AttachmentFailure::Write(IoFailureCategory::classify(&error))
                })?;
                let file = match root.open_entry_read(id.file_name()) {
                    Ok(file) => file,
                    Err(error) => {
                        if root
                            .unlink_entry_if_identity(id.file_name(), staged_inode)
                            .ok()
                            != Some(true)
                        {
                            self.retain_inode_debt(id.clone(), staged_inode.0, staged_inode.1);
                        }
                        return Err(AttachmentFailure::Write(IoFailureCategory::classify(
                            &error,
                        )));
                    }
                };
                let identity = match payload_identity(&file) {
                    Ok(identity) => identity,
                    Err(error) => {
                        if root
                            .unlink_entry_if_identity(id.file_name(), staged_inode)
                            .ok()
                            != Some(true)
                        {
                            self.retain_inode_debt(id.clone(), staged_inode.0, staged_inode.1);
                        }
                        return Err(error);
                    }
                };
                if (identity.device, identity.inode) != staged_inode {
                    if root
                        .unlink_entry_if_identity(id.file_name(), staged_inode)
                        .ok()
                        != Some(true)
                    {
                        self.retain_inode_debt(id.clone(), staged_inode.0, staged_inode.1);
                    }
                    return Err(AttachmentFailure::InvalidPayload);
                }
                if root
                    .ordinary_directory()
                    .and_then(|directory| directory.sync_all())
                    .is_err()
                {
                    self.retain_cleanup_debt(&[AttachmentPayloadRef {
                        id: id.clone(),
                        identity,
                    }]);
                    return Err(AttachmentFailure::StoreUncertain);
                }
                if let Err(error) = verify_file_identity(&file, identity) {
                    self.retain_cleanup_debt(&[AttachmentPayloadRef {
                        id: id.clone(),
                        identity,
                    }]);
                    return Err(error);
                }
                identity
            }
            Backend::Memory(store) => {
                let mut store = store
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                store.next_inode = store
                    .next_inode
                    .checked_add(1)
                    .ok_or(AttachmentFailure::IdentityUnavailable)?;
                let identity = AttachmentPayloadIdentity {
                    device: 0,
                    inode: store.next_inode,
                    size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                    mtime_secs: 0,
                    mtime_nanos: 0,
                    ctime_secs: 0,
                    ctime_nanos: 0,
                };
                store.entries.insert(
                    id.clone(),
                    MemoryPayload {
                        bytes: bytes.to_vec(),
                        identity,
                    },
                );
                identity
            }
            Backend::Unavailable => return Err(AttachmentFailure::StoreUnavailable),
        };
        Ok(AttachmentPayloadRef { id, identity })
    }

    // Whether maintenance on this store performs filesystem work.
    //
    // Only a file-backed store must be surveyed off the caller's thread; the
    // in-memory and unavailable backends answer without I/O.
    pub(crate) fn needs_blocking_maintenance(&self) -> bool {
        matches!(self.backend, Backend::Files(_))
    }

    pub(crate) fn read(&self, attachment: &Attachment) -> Result<Vec<u8>, AttachmentFailure> {
        let payload = attachment_payload_ref(attachment)?;
        match &self.backend {
            Backend::Files(root) => {
                let mut file = open_payload_file(root, &payload)?;
                let mut bytes =
                    Vec::with_capacity(usize::try_from(payload.identity.size).unwrap_or(0));
                file.by_ref()
                    .take(MAX_ATTACHMENT_BYTES.saturating_add(1))
                    .read_to_end(&mut bytes)
                    .map_err(|error| {
                        AttachmentFailure::Read(IoFailureCategory::classify(&error))
                    })?;
                verify_file_identity(&file, payload.identity)?;
                if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != payload.identity.size {
                    return Err(AttachmentFailure::PayloadChanged);
                }
                Ok(bytes)
            }
            Backend::Memory(store) => {
                let store = store
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let stored = store
                    .entries
                    .get(&payload.id)
                    .ok_or(AttachmentFailure::MissingPayload)?;
                if stored.identity != payload.identity {
                    return Err(AttachmentFailure::PayloadChanged);
                }
                Ok(stored.bytes.clone())
            }
            Backend::Unavailable => Err(AttachmentFailure::StoreUnavailable),
        }
    }

    pub(crate) fn open(
        &self,
        attachment: &Attachment,
    ) -> Result<OpenedAttachment, AttachmentFailure> {
        let payload = attachment_payload_ref(attachment)?;
        match &self.backend {
            Backend::Files(root) => {
                let file = open_payload_file(root, &payload)?;
                Ok(OpenedAttachment {
                    root: Arc::clone(root),
                    file: Arc::new(file),
                    payload,
                })
            }
            Backend::Memory(store) => {
                let store = store
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match store.entries.get(&payload.id) {
                    Some(stored) if stored.identity == payload.identity => {
                        Err(AttachmentFailure::StoreUnavailable)
                    }
                    Some(_) => Err(AttachmentFailure::PayloadChanged),
                    None => Err(AttachmentFailure::MissingPayload),
                }
            }
            Backend::Unavailable => Err(AttachmentFailure::StoreUnavailable),
        }
    }

    pub(crate) fn export(
        &self,
        attachment: &Attachment,
        destination: &Path,
    ) -> Result<(), AttachmentFailure> {
        let bytes = self.read(attachment)?;
        user_file::save(destination, SavePolicy::CreateNew, &bytes).map_err(|error| {
            let category = IoFailureCategory::classify(error.io_error());
            if error.was_published() {
                AttachmentFailure::SaveUncertain(category)
            } else if error.io_error().kind() == std::io::ErrorKind::AlreadyExists {
                AttachmentFailure::DestinationExists
            } else {
                AttachmentFailure::Write(category)
            }
        })
    }

    pub(crate) fn outgoing_mime_bytes(
        &self,
        pending: &PendingSend,
    ) -> Result<Vec<u8>, AttachmentFailure> {
        if pending.attachments.len() > MAX_ATTACHMENT_COUNT {
            return Err(AttachmentFailure::TooMany);
        }
        let message_id = pending_message_id(pending)?;
        let boundary_id = random::random_array::<16>()
            .map(AttachmentPayloadId::from_bytes)
            .map_err(|_| AttachmentFailure::IdentityUnavailable)?;
        let boundary = format!("mail-app-{}", boundary_id.as_str());
        let from = if pending.sender_name.trim().is_empty() {
            clean_header(&pending.sender_email)
        } else {
            format!(
                "{} <{}>",
                clean_header(&pending.sender_name),
                clean_header(&pending.sender_email)
            )
        };
        let mut out = Vec::new();
        append_mime_bytes(
            &mut out,
            format!("From: {from}\r\nTo: {}\r\n", clean_header(&pending.to_raw)).as_bytes(),
        )?;
        if !pending.cc_raw.trim().is_empty() {
            append_mime_bytes(
                &mut out,
                format!("Cc: {}\r\n", clean_header(&pending.cc_raw)).as_bytes(),
            )?;
        }
        append_mime_bytes(&mut out, format!("Message-ID: {message_id}\r\n").as_bytes())?;
        append_mime_bytes(
            &mut out,
            format!(
                "Subject: {}\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"{boundary}\"\r\n\r\n",
                clean_header(&pending.subject)
            )
            .as_bytes(),
        )?;
        let (body_type, body) = pending
            .body_html
            .as_deref()
            .map_or(("text/plain", pending.body.as_str()), |html| {
                ("text/html", html)
            });
        append_mime_bytes(
            &mut out,
            format!(
                "--{boundary}\r\nContent-Type: {body_type}; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n"
            )
            .as_bytes(),
        )?;
        append_mime_bytes(&mut out, normalize_body(body).as_bytes())?;
        append_mime_bytes(&mut out, b"\r\n")?;

        let mut total_bytes = 0_u64;
        for attachment in &pending.attachments {
            append_mime_bytes(&mut out, format!("--{boundary}\r\n").as_bytes())?;
            let bytes = self.read(attachment)?;
            total_bytes = total_bytes
                .checked_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
                .ok_or(AttachmentFailure::TotalTooLarge)?;
            if total_bytes > MAX_TOTAL_ATTACHMENT_BYTES {
                return Err(AttachmentFailure::TotalTooLarge);
            }
            append_mime_bytes(&mut out, &encode_mime_part(attachment, &bytes))?;
        }
        append_mime_bytes(&mut out, format!("--{boundary}--\r\n").as_bytes())?;
        Ok(out)
    }
}
