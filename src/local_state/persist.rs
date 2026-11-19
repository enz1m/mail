use super::validation::{validate_count, validate_local_state_domain};
use super::{
    LOCAL_STATE_ACCOUNTS_MAX, LOCAL_STATE_BODY_MAX, LOCAL_STATE_FOLDERS_PER_ACCOUNT_MAX,
    LOCAL_STATE_MESSAGES_MAX, LOCAL_STATE_MUTATIONS_MAX, LOCAL_STATE_RECIPIENTS_PER_MESSAGE_MAX,
    LOCAL_STATE_REFERENCES_PER_MESSAGE_MAX, LOCAL_STATE_TARGETS_PER_MUTATION_MAX,
    LOCAL_STATE_UNSUBSCRIBE_URIS_MAX, LocalState, PendingServerMutation, ServerMutationTarget,
    admit_next, auth_token, body_state_fields, bool_token, encryption_token, field, format_token,
    mailbox_token, mode_fields, mutation_fields, parse_auth, parse_body_state, parse_bool,
    parse_encryption, parse_format, parse_mailbox, parse_mode, parse_mutation_kind,
    parse_payload_id, parse_payload_identity, parse_protocol, parse_u16, parse_u32, parse_u64,
    payload_identity_fields, protocol_token, unescape, validate_encoded_local_state, write_record,
};
use crate::backend::{AccountConfig, ImapConfig, IncomingConfig, SmtpConfig};
use crate::credential_mutation::{
    CredentialMutationJournal, CredentialMutationKind, CredentialTransition, DurableCredentialKey,
    MAX_CREDENTIAL_TRANSITIONS,
};
use crate::model::compose::PendingSend;
use crate::model::types::{Account, Folder};
use crate::model::types::{Attachment, AttachmentPayloadId, Message, MessageDate, MessageLocation};
use enzui_iced_core::persist::Persistable;

impl Persistable for LocalState {
    fn persist_serialize(&self) -> Result<String, String> {
        validate_local_state_domain(self)?;
        crate::model::settings::validate_account_allocator(
            &self.accounts,
            self.next_account_ordinal,
        )
        .map_err(|error| format!("invalid account allocator: {error:?}"))?;
        let mut out = String::new();
        write_record(
            &mut out,
            &[
                "meta".to_owned(),
                self.today.year.to_string(),
                self.today.month.to_string(),
                self.today.day.to_string(),
                self.today.minute.to_string(),
                self.next_message_ordinal.to_string(),
                self.next_account_ordinal.to_string(),
                self.active_account_id.clone(),
                self.send_generation.to_string(),
                self.next_mutation_ordinal.to_string(),
                self.credential_journal.public_generation().to_string(),
                self.credential_journal.sequence_high_water().to_string(),
            ],
        );
        for transition in self.credential_journal.records() {
            let (old_present, old_app, old_owner, old_label) =
                transition.old_key.as_ref().map_or_else(
                    || ("0".to_owned(), String::new(), String::new(), String::new()),
                    |key| {
                        (
                            "1".to_owned(),
                            key.app_id().to_owned(),
                            key.owner().to_owned(),
                            key.label().to_owned(),
                        )
                    },
                );
            let (new_present, new_app, new_owner, new_label) =
                transition.new_key.as_ref().map_or_else(
                    || ("0".to_owned(), String::new(), String::new(), String::new()),
                    |key| {
                        (
                            "1".to_owned(),
                            key.app_id().to_owned(),
                            key.owner().to_owned(),
                            key.label().to_owned(),
                        )
                    },
                );
            write_record(
                &mut out,
                &[
                    "credential-transition".to_owned(),
                    "2".to_owned(),
                    transition.kind.token().to_owned(),
                    transition.sequence.to_string(),
                    transition.target_public_generation.to_string(),
                    bool_token(transition.config_changed),
                    transition.account_id.clone(),
                    old_present,
                    old_app,
                    old_owner,
                    old_label,
                    new_present,
                    new_app,
                    new_owner,
                    new_label,
                ],
            );
        }
        for account in &self.accounts {
            let mailboxes = account
                .mailboxes
                .iter()
                .map(|mailbox| mailbox_token(*mailbox))
                .collect::<Vec<_>>()
                .join(",");
            let mut fields = vec![
                "account".to_owned(),
                account.id.clone(),
                account.display_name.clone(),
                account.email.clone(),
                account.signature.clone(),
                bool_token(account.append_signature),
                mailboxes,
                bool_token(account.config.is_some()),
            ];
            if let Some(config) = &account.config {
                fields.extend([
                    protocol_token(config.protocol).to_owned(),
                    config.incoming.server.clone(),
                    config.incoming.port.to_string(),
                    config.incoming.username.clone(),
                    encryption_token(config.incoming.encryption).to_owned(),
                    config.smtp.server.clone(),
                    config.smtp.port.to_string(),
                    config.smtp.username.clone(),
                    encryption_token(config.smtp.encryption).to_owned(),
                    auth_token(config.auth).to_owned(),
                    config.credential_owner.clone(),
                ]);
            }
            write_record(&mut out, &fields);
            for folder in &account.folders {
                write_record(
                    &mut out,
                    &[
                        "folder".to_owned(),
                        account.id.clone(),
                        folder.id.clone(),
                        folder.name.clone(),
                    ],
                );
            }
        }
        for message in &self.messages {
            let (location_kind, location_value) = match &message.location {
                MessageLocation::Mailbox(mailbox) => {
                    ("mailbox", mailbox_token(*mailbox).to_owned())
                }
                MessageLocation::Folder(folder) => ("folder", folder.clone()),
            };
            let body_state = body_state_fields(message.body_state);
            write_record(
                &mut out,
                &[
                    "message".to_owned(),
                    message.id.clone(),
                    message.account_id.clone(),
                    location_kind.to_owned(),
                    location_value,
                    message.sender_name.clone(),
                    message.sender_email.clone(),
                    message.subject.clone(),
                    message.preview.clone(),
                    message.body.clone(),
                    bool_token(message.body_html.is_some()),
                    message.body_html.clone().unwrap_or_default(),
                    message.date.year.to_string(),
                    message.date.month.to_string(),
                    message.date.day.to_string(),
                    message.date.minute.to_string(),
                    bool_token(message.unread),
                    bool_token(message.flagged),
                    bool_token(message.has_attachment),
                    bool_token(message.has_remote_content),
                    bool_token(message.has_inline_image),
                    bool_token(message.blocked),
                    bool_token(message.in_reply_to.is_some()),
                    message.in_reply_to.clone().unwrap_or_default(),
                    body_state[0].clone(),
                    body_state[1].clone(),
                    body_state[2].clone(),
                ],
            );
            for (kind, recipients) in [
                ("to", &message.to),
                ("cc", &message.cc),
                ("bcc", &message.bcc),
            ] {
                for recipient in recipients {
                    write_record(
                        &mut out,
                        &[kind.to_owned(), message.id.clone(), recipient.clone()],
                    );
                }
            }
            for attachment in &message.attachments {
                let identity = payload_identity_fields(attachment.payload_identity);
                write_record(
                    &mut out,
                    &[
                        "attachment".to_owned(),
                        message.id.clone(),
                        attachment.name.clone(),
                        attachment.mime.clone(),
                        attachment.size.to_string(),
                        attachment
                            .payload_id
                            .as_ref()
                            .map(AttachmentPayloadId::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        identity[0].clone(),
                        identity[1].clone(),
                        identity[2].clone(),
                        identity[3].clone(),
                        identity[4].clone(),
                        identity[5].clone(),
                        identity[6].clone(),
                    ],
                );
            }
            for reference in &message.references {
                write_record(
                    &mut out,
                    &[
                        "reference".to_owned(),
                        message.id.clone(),
                        reference.clone(),
                    ],
                );
            }
            if let Some(at) = message.send_at {
                write_record(
                    &mut out,
                    &[
                        "send-at".to_owned(),
                        message.id.clone(),
                        at.year.to_string(),
                        at.month.to_string(),
                        at.day.to_string(),
                        at.minute.to_string(),
                    ],
                );
            }
            if let Some(until) = message.snoozed_until {
                write_record(
                    &mut out,
                    &[
                        "snooze".to_owned(),
                        message.id.clone(),
                        until.year.to_string(),
                        until.month.to_string(),
                        until.day.to_string(),
                        until.minute.to_string(),
                    ],
                );
            }
            if let Some(unsubscribe) = &message.unsubscribe {
                for uri in &unsubscribe.uris {
                    write_record(
                        &mut out,
                        &[
                            "unsubscribe".to_owned(),
                            message.id.clone(),
                            bool_token(unsubscribe.one_click),
                            uri.clone(),
                        ],
                    );
                }
            }
        }
        if let Some(pending) = &self.pending_send {
            let (mode, mode_id) = mode_fields(&pending.mode);
            write_record(
                &mut out,
                &[
                    "pending".to_owned(),
                    pending.id.clone(),
                    pending.account_id.clone(),
                    pending.sender_name.clone(),
                    pending.sender_email.clone(),
                    pending.to_raw.clone(),
                    pending.cc_raw.clone(),
                    pending.bcc_raw.clone(),
                    bool_token(pending.bcc_shown),
                    pending.subject.clone(),
                    pending.body.clone(),
                    bool_token(pending.body_html.is_some()),
                    pending.body_html.clone().unwrap_or_default(),
                    bool_token(pending.in_reply_to.is_some()),
                    pending.in_reply_to.clone().unwrap_or_default(),
                    format_token(pending.format).to_owned(),
                    mode.to_owned(),
                    mode_id,
                    bool_token(pending.restored_draft_id.is_some()),
                    pending.restored_draft_id.clone().unwrap_or_default(),
                ],
            );
            for attachment in &pending.attachments {
                let identity = payload_identity_fields(attachment.payload_identity);
                write_record(
                    &mut out,
                    &[
                        "pending-attachment".to_owned(),
                        pending.id.clone(),
                        attachment.name.clone(),
                        attachment.mime.clone(),
                        attachment.size.to_string(),
                        attachment
                            .payload_id
                            .as_ref()
                            .map(AttachmentPayloadId::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        identity[0].clone(),
                        identity[1].clone(),
                        identity[2].clone(),
                        identity[3].clone(),
                        identity[4].clone(),
                        identity[5].clone(),
                        identity[6].clone(),
                    ],
                );
            }
            for reference in &pending.references {
                write_record(
                    &mut out,
                    &[
                        "pending-reference".to_owned(),
                        pending.id.clone(),
                        reference.clone(),
                    ],
                );
            }
        }
        for mutation in &self.pending_server_mutations {
            let (kind, value) = mutation_fields(&mutation.kind);
            write_record(
                &mut out,
                &[
                    "mutation".to_owned(),
                    mutation.id.to_string(),
                    kind.to_owned(),
                    value,
                ],
            );
            for target in &mutation.target_ids {
                write_record(
                    &mut out,
                    &[
                        "mutation-target".to_owned(),
                        mutation.id.to_string(),
                        target.clone(),
                    ],
                );
            }
            for target in &mutation.server_targets {
                write_record(
                    &mut out,
                    &[
                        "mutation-server-target".to_owned(),
                        mutation.id.to_string(),
                        target.message_id.clone(),
                        target.account_id.clone(),
                        mailbox_token(target.mailbox).to_owned(),
                        target.uid_validity.to_string(),
                        target.uid.to_string(),
                    ],
                );
            }
        }
        let body_bytes = u64::try_from(out.len())
            .map_err(|_| "serialized local-state body length overflow".to_owned())?;
        if body_bytes > LOCAL_STATE_BODY_MAX {
            return Err("serialized local state exceeds its derived body ceiling".to_owned());
        }
        Ok(out)
    }

    fn persist_deserialize(body: &str) -> Result<Self, String> {
        validate_encoded_local_state(body)?;
        let mut state = Self::empty();
        let mut credential_records = Vec::new();
        let mut credential_public_generation = 0_u64;
        let mut credential_sequence_high_water = 0_u64;
        for (index, raw) in body.lines().enumerate() {
            let line = index + 1;
            if raw.trim().is_empty() || raw.starts_with('#') {
                continue;
            }
            let fields = raw.split('\t').map(unescape).collect::<Vec<_>>();
            match field(&fields, 0, line, "record type")? {
                "meta" => {
                    state.today = MessageDate {
                        year: parse_u32(&fields, 1, line, "year")?,
                        month: parse_u32(&fields, 2, line, "month")?,
                        day: parse_u32(&fields, 3, line, "day")?,
                        minute: parse_u32(&fields, 4, line, "minute")?,
                    };
                    state.next_message_ordinal =
                        parse_u64(&fields, 5, line, "next message ordinal")?;
                    state.next_account_ordinal =
                        parse_u64(&fields, 6, line, "next account ordinal")?;
                    field(&fields, 7, line, "active account")?
                        .clone_into(&mut state.active_account_id);
                    state.send_generation = parse_u64(&fields, 8, line, "send generation")?;
                    state.next_mutation_ordinal =
                        parse_u64(&fields, 9, line, "next mutation ordinal")?;
                    credential_public_generation = fields
                        .get(10)
                        .filter(|value| !value.is_empty())
                        .map(|_| parse_u64(&fields, 10, line, "credential public generation"))
                        .transpose()?
                        .unwrap_or(0);
                    credential_sequence_high_water = fields
                        .get(11)
                        .filter(|value| !value.is_empty())
                        .map(|_| parse_u64(&fields, 11, line, "credential sequence high-water"))
                        .transpose()?
                        .unwrap_or(credential_public_generation);
                }
                "credential-transition" => {
                    if fields.len() != 15 {
                        return Err(format!(
                            "line {line}: credential transition field count is invalid"
                        ));
                    }
                    admit_next(
                        credential_records.len(),
                        MAX_CREDENTIAL_TRANSITIONS,
                        "credential transitions",
                        line,
                    )?;
                    if field(&fields, 1, line, "credential journal version")? != "2" {
                        return Err(format!(
                            "line {line}: unsupported credential journal version"
                        ));
                    }
                    let parse_key =
                        |marker: usize,
                         first: usize|
                         -> Result<Option<DurableCredentialKey>, String> {
                            if !parse_bool(&fields, marker, line, "credential key marker")? {
                                if (first..first + 3).any(|index| {
                                    fields.get(index).is_some_and(|value| !value.is_empty())
                                }) {
                                    return Err(format!(
                                        "line {line}: absent credential key has components"
                                    ));
                                }
                                return Ok(None);
                            }
                            DurableCredentialKey::from_parts(
                                field(&fields, first, line, "credential application id")?
                                    .to_owned(),
                                field(&fields, first + 1, line, "credential owner")?.to_owned(),
                                field(&fields, first + 2, line, "credential label")?.to_owned(),
                            )
                            .map(Some)
                            .map_err(|error| format!("line {line}: {error}"))
                        };
                    credential_records.push(CredentialTransition {
                        kind: CredentialMutationKind::parse(field(
                            &fields,
                            2,
                            line,
                            "credential transition kind",
                        )?)
                        .ok_or_else(|| {
                            format!("line {line}: unsupported credential transition kind")
                        })?,
                        sequence: parse_u64(&fields, 3, line, "credential transition sequence")?,
                        target_public_generation: parse_u64(
                            &fields,
                            4,
                            line,
                            "credential target public generation",
                        )?,
                        config_changed: parse_bool(
                            &fields,
                            5,
                            line,
                            "credential config-changed flag",
                        )?,
                        account_id: field(&fields, 6, line, "credential transition account")?
                            .to_owned(),
                        old_key: parse_key(7, 8)?,
                        new_key: parse_key(11, 12)?,
                    });
                }
                "account" => {
                    admit_next(
                        state.accounts.len(),
                        LOCAL_STATE_ACCOUNTS_MAX,
                        "accounts",
                        line,
                    )?;
                    let id = field(&fields, 1, line, "account id")?.to_owned();
                    if state.accounts.iter().any(|account| account.id == id) {
                        return Err(format!("line {line}: duplicate account id"));
                    }
                    let config = if parse_bool(&fields, 7, line, "config marker")? {
                        let protocol = parse_protocol(field(&fields, 8, line, "protocol")?)
                            .ok_or_else(|| format!("line {line}: invalid protocol"))?;
                        let incoming = IncomingConfig {
                            server: field(&fields, 9, line, "incoming server")?.to_owned(),
                            port: parse_u16(&fields, 10, line, "incoming port")?,
                            username: field(&fields, 11, line, "incoming username")?.to_owned(),
                            encryption: parse_encryption(field(
                                &fields,
                                12,
                                line,
                                "incoming encryption",
                            )?)
                            .ok_or_else(|| format!("line {line}: invalid incoming encryption"))?,
                        };
                        let smtp = SmtpConfig {
                            server: field(&fields, 13, line, "smtp server")?.to_owned(),
                            port: parse_u16(&fields, 14, line, "smtp port")?,
                            username: field(&fields, 15, line, "smtp username")?.to_owned(),
                            encryption: parse_encryption(field(
                                &fields,
                                16,
                                line,
                                "smtp encryption",
                            )?)
                            .ok_or_else(|| format!("line {line}: invalid smtp encryption"))?,
                        };
                        Some(AccountConfig {
                            id: id.clone(),
                            credential_owner: fields
                                .get(18)
                                .filter(|owner| !owner.is_empty())
                                .cloned()
                                .unwrap_or_else(|| id.clone()),
                            display_name: field(&fields, 2, line, "display name")?.to_owned(),
                            email: field(&fields, 3, line, "email")?.to_owned(),
                            protocol,
                            imap: ImapConfig::from(&incoming),
                            incoming,
                            smtp,
                            auth: parse_auth(field(&fields, 17, line, "authentication")?)
                                .ok_or_else(|| format!("line {line}: invalid authentication"))?,
                        })
                    } else {
                        None
                    };
                    let mailbox_field = field(&fields, 6, line, "mailboxes")?;
                    let mailbox_count = mailbox_field
                        .split(',')
                        .filter(|token| !token.is_empty())
                        .count();
                    validate_count(mailbox_count, 6, "account mailboxes")?;
                    let mailboxes = mailbox_field
                        .split(',')
                        .filter(|token| !token.is_empty())
                        .map(|token| {
                            parse_mailbox(token)
                                .ok_or_else(|| format!("line {line}: invalid mailbox"))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    state.accounts.push(Account {
                        id,
                        display_name: field(&fields, 2, line, "display name")?.to_owned(),
                        email: field(&fields, 3, line, "email")?.to_owned(),
                        mailboxes,
                        folders: Vec::new(),
                        config,
                        signature: field(&fields, 4, line, "signature")?.to_owned(),
                        append_signature: parse_bool(&fields, 5, line, "append signature")?,
                    });
                }
                "folder" => {
                    let account_id = field(&fields, 1, line, "folder account")?;
                    let account = state
                        .accounts
                        .iter_mut()
                        .find(|account| account.id == account_id)
                        .ok_or_else(|| format!("line {line}: folder references unknown account"))?;
                    admit_next(
                        account.folders.len(),
                        LOCAL_STATE_FOLDERS_PER_ACCOUNT_MAX,
                        "account folders",
                        line,
                    )?;
                    account.folders.push(Folder {
                        id: field(&fields, 2, line, "folder id")?.to_owned(),
                        name: field(&fields, 3, line, "folder name")?.to_owned(),
                    });
                }
                "message" => {
                    admit_next(
                        state.messages.len(),
                        LOCAL_STATE_MESSAGES_MAX,
                        "messages",
                        line,
                    )?;
                    let id = field(&fields, 1, line, "message id")?.to_owned();
                    if state.messages.iter().any(|message| message.id == id) {
                        return Err(format!("line {line}: duplicate message id"));
                    }
                    let location = match field(&fields, 3, line, "location kind")? {
                        "mailbox" => MessageLocation::Mailbox(
                            parse_mailbox(field(&fields, 4, line, "mailbox")?)
                                .ok_or_else(|| format!("line {line}: invalid mailbox"))?,
                        ),
                        "folder" => MessageLocation::Folder(
                            field(&fields, 4, line, "folder id")?.to_owned(),
                        ),
                        _ => return Err(format!("line {line}: invalid location kind")),
                    };
                    state.messages.push(Message {
                        id,
                        account_id: field(&fields, 2, line, "message account")?.to_owned(),
                        location,
                        // Older syncs stored encoded words raw; decoding is idempotent.
                        sender_name: crate::model::header_text::decode_header_text(field(
                            &fields,
                            5,
                            line,
                            "sender name",
                        )?),
                        sender_email: field(&fields, 6, line, "sender email")?.to_owned(),
                        to: Vec::new(),
                        subject: crate::model::header_text::decode_header_text(field(
                            &fields, 7, line, "subject",
                        )?),
                        preview: field(&fields, 8, line, "preview")?.to_owned(),
                        body: field(&fields, 9, line, "body")?.to_owned(),
                        body_html: parse_bool(&fields, 10, line, "html marker")?
                            .then(|| field(&fields, 11, line, "html body").map(str::to_owned))
                            .transpose()?,
                        date: MessageDate {
                            year: parse_u32(&fields, 12, line, "message year")?,
                            month: parse_u32(&fields, 13, line, "message month")?,
                            day: parse_u32(&fields, 14, line, "message day")?,
                            minute: parse_u32(&fields, 15, line, "message minute")?,
                        },
                        unread: parse_bool(&fields, 16, line, "unread")?,
                        flagged: parse_bool(&fields, 17, line, "flagged")?,
                        has_attachment: parse_bool(&fields, 18, line, "has attachment")?,
                        attachments: Vec::new(),
                        has_remote_content: parse_bool(&fields, 19, line, "remote content")?,
                        has_inline_image: parse_bool(&fields, 20, line, "inline image")?,
                        blocked: parse_bool(&fields, 21, line, "blocked")?,
                        in_reply_to: parse_bool(&fields, 22, line, "reply marker")?
                            .then(|| field(&fields, 23, line, "reply id").map(str::to_owned))
                            .transpose()?,
                        body_state: parse_body_state(&fields, line)?,
                        references: Vec::new(),
                        snoozed_until: None,
                        send_at: None,
                        cc: Vec::new(),
                        bcc: Vec::new(),
                        unsubscribe: None,
                    });
                }
                "to" | "cc" | "bcc" | "reference" => {
                    let message_id = field(&fields, 1, line, "message id")?;
                    let value = field(&fields, 2, line, "message child value")?;
                    let message = state
                        .messages
                        .iter_mut()
                        .find(|message| message.id == message_id)
                        .ok_or_else(|| format!("line {line}: child references unknown message"))?;
                    if fields[0] != "reference" {
                        let recipients = match fields[0].as_str() {
                            "cc" => &mut message.cc,
                            "bcc" => &mut message.bcc,
                            _ => &mut message.to,
                        };
                        admit_next(
                            recipients.len(),
                            LOCAL_STATE_RECIPIENTS_PER_MESSAGE_MAX,
                            "message recipients",
                            line,
                        )?;
                        recipients.push(value.to_owned());
                    } else {
                        admit_next(
                            message.references.len(),
                            LOCAL_STATE_REFERENCES_PER_MESSAGE_MAX,
                            "message references",
                            line,
                        )?;
                        message.references.push(value.to_owned());
                    }
                }
                "send-at" => {
                    let message_id = field(&fields, 1, line, "message id")?;
                    let at = MessageDate {
                        year: parse_u32(&fields, 2, line, "send year")?,
                        month: parse_u32(&fields, 3, line, "send month")?,
                        day: parse_u32(&fields, 4, line, "send day")?,
                        minute: parse_u32(&fields, 5, line, "send minute")?,
                    };
                    state
                        .messages
                        .iter_mut()
                        .find(|message| message.id == message_id)
                        .ok_or_else(|| {
                            format!("line {line}: send time references unknown message")
                        })?
                        .send_at = Some(at);
                }
                "snooze" => {
                    let message_id = field(&fields, 1, line, "message id")?;
                    let until = MessageDate {
                        year: parse_u32(&fields, 2, line, "snooze year")?,
                        month: parse_u32(&fields, 3, line, "snooze month")?,
                        day: parse_u32(&fields, 4, line, "snooze day")?,
                        minute: parse_u32(&fields, 5, line, "snooze minute")?,
                    };
                    state
                        .messages
                        .iter_mut()
                        .find(|message| message.id == message_id)
                        .ok_or_else(|| format!("line {line}: snooze references unknown message"))?
                        .snoozed_until = Some(until);
                }
                "unsubscribe" => {
                    let message_id = field(&fields, 1, line, "message id")?;
                    let one_click = parse_bool(&fields, 2, line, "one-click unsubscribe")?;
                    let uri = field(&fields, 3, line, "unsubscribe uri")?;
                    let message = state
                        .messages
                        .iter_mut()
                        .find(|message| message.id == message_id)
                        .ok_or_else(|| {
                            format!("line {line}: unsubscribe references unknown message")
                        })?;
                    let unsubscribe = message.unsubscribe.get_or_insert_with(Default::default);
                    admit_next(
                        unsubscribe.uris.len(),
                        LOCAL_STATE_UNSUBSCRIBE_URIS_MAX,
                        "unsubscribe routes",
                        line,
                    )?;
                    unsubscribe.one_click |= one_click;
                    unsubscribe.uris.push(uri.to_owned());
                }
                "attachment" => {
                    let message_id = field(&fields, 1, line, "message id")?;
                    let message = state
                        .messages
                        .iter_mut()
                        .find(|message| message.id == message_id)
                        .ok_or_else(|| {
                            format!("line {line}: attachment references unknown message")
                        })?;
                    admit_next(
                        message.attachments.len(),
                        crate::attachment_store::MAX_ATTACHMENT_COUNT,
                        "message attachments",
                        line,
                    )?;
                    message.attachments.push(Attachment {
                        name: field(&fields, 2, line, "attachment name")?.to_owned(),
                        mime: field(&fields, 3, line, "attachment mime")?.to_owned(),
                        size: parse_u64(&fields, 4, line, "attachment size")?,
                        payload_id: parse_payload_id(&fields, 5, line)?,
                        payload_identity: parse_payload_identity(&fields, 6, line)?,
                    });
                }
                "pending" => {
                    if state.pending_send.is_some() {
                        return Err(format!("line {line}: duplicate pending send"));
                    }
                    let id = field(&fields, 1, line, "pending id")?.to_owned();
                    state.pending_send = Some(PendingSend {
                        id,
                        account_id: field(&fields, 2, line, "pending account")?.to_owned(),
                        sender_name: field(&fields, 3, line, "pending sender name")?.to_owned(),
                        sender_email: field(&fields, 4, line, "pending sender email")?.to_owned(),
                        to_raw: field(&fields, 5, line, "pending to")?.to_owned(),
                        cc_raw: field(&fields, 6, line, "pending cc")?.to_owned(),
                        bcc_raw: field(&fields, 7, line, "pending bcc")?.to_owned(),
                        bcc_shown: parse_bool(&fields, 8, line, "pending bcc shown")?,
                        subject: field(&fields, 9, line, "pending subject")?.to_owned(),
                        body: field(&fields, 10, line, "pending body")?.to_owned(),
                        body_html: parse_bool(&fields, 11, line, "pending html marker")?
                            .then(|| field(&fields, 12, line, "pending html").map(str::to_owned))
                            .transpose()?,
                        attachments: Vec::new(),
                        in_reply_to: parse_bool(&fields, 13, line, "pending reply marker")?
                            .then(|| field(&fields, 14, line, "pending reply").map(str::to_owned))
                            .transpose()?,
                        references: Vec::new(),
                        format: parse_format(field(&fields, 15, line, "pending format")?)
                            .ok_or_else(|| format!("line {line}: invalid pending format"))?,
                        mode: parse_mode(
                            field(&fields, 16, line, "pending mode")?,
                            field(&fields, 17, line, "pending mode id")?.to_owned(),
                        )
                        .ok_or_else(|| format!("line {line}: invalid pending mode"))?,
                        restored_draft_id: parse_bool(
                            &fields,
                            18,
                            line,
                            "pending restored marker",
                        )?
                        .then(|| {
                            field(&fields, 19, line, "pending restored draft").map(str::to_owned)
                        })
                        .transpose()?,
                        // Reviews are not persisted; a restored outbox
                        // entry belongs to no review.
                        review_key: None,
                    });
                }
                "pending-attachment" => {
                    let pending_id = field(&fields, 1, line, "pending id")?;
                    let pending = state
                        .pending_send
                        .as_mut()
                        .filter(|pending| pending.id == pending_id)
                        .ok_or_else(|| {
                            format!("line {line}: attachment references unknown pending send")
                        })?;
                    admit_next(
                        pending.attachments.len(),
                        crate::attachment_store::MAX_ATTACHMENT_COUNT,
                        "pending attachments",
                        line,
                    )?;
                    pending.attachments.push(Attachment {
                        name: field(&fields, 2, line, "attachment name")?.to_owned(),
                        mime: field(&fields, 3, line, "attachment mime")?.to_owned(),
                        size: parse_u64(&fields, 4, line, "attachment size")?,
                        payload_id: parse_payload_id(&fields, 5, line)?,
                        payload_identity: parse_payload_identity(&fields, 6, line)?,
                    });
                }
                "pending-reference" => {
                    let pending_id = field(&fields, 1, line, "pending id")?;
                    let pending = state
                        .pending_send
                        .as_mut()
                        .filter(|pending| pending.id == pending_id)
                        .ok_or_else(|| {
                            format!("line {line}: reference targets unknown pending send")
                        })?;
                    admit_next(
                        pending.references.len(),
                        LOCAL_STATE_REFERENCES_PER_MESSAGE_MAX,
                        "pending references",
                        line,
                    )?;
                    pending
                        .references
                        .push(field(&fields, 2, line, "pending reference")?.to_owned());
                }
                "mutation" => {
                    admit_next(
                        state.pending_server_mutations.len(),
                        LOCAL_STATE_MUTATIONS_MAX,
                        "pending server mutations",
                        line,
                    )?;
                    let id = parse_u64(&fields, 1, line, "mutation id")?;
                    if state
                        .pending_server_mutations
                        .iter()
                        .any(|mutation| mutation.id == id)
                    {
                        return Err(format!("line {line}: duplicate mutation id"));
                    }
                    state.pending_server_mutations.push(PendingServerMutation {
                        id,
                        kind: parse_mutation_kind(
                            field(&fields, 2, line, "mutation kind")?,
                            field(&fields, 3, line, "mutation value")?,
                            line,
                        )?,
                        target_ids: Vec::new(),
                        server_targets: Vec::new(),
                    });
                }
                "mutation-target" => {
                    let id = parse_u64(&fields, 1, line, "mutation id")?;
                    let mutation = state
                        .pending_server_mutations
                        .iter_mut()
                        .find(|mutation| mutation.id == id)
                        .ok_or_else(|| {
                            format!("line {line}: target references unknown mutation")
                        })?;
                    admit_next(
                        mutation.target_ids.len(),
                        LOCAL_STATE_TARGETS_PER_MUTATION_MAX,
                        "mutation targets",
                        line,
                    )?;
                    mutation
                        .target_ids
                        .push(field(&fields, 2, line, "mutation target")?.to_owned());
                }
                "mutation-server-target" => {
                    let id = parse_u64(&fields, 1, line, "mutation id")?;
                    let mutation = state
                        .pending_server_mutations
                        .iter_mut()
                        .find(|mutation| mutation.id == id)
                        .ok_or_else(|| {
                            format!("line {line}: server target references unknown mutation")
                        })?;
                    admit_next(
                        mutation.server_targets.len(),
                        LOCAL_STATE_TARGETS_PER_MUTATION_MAX,
                        "server mutation targets",
                        line,
                    )?;
                    let mailbox =
                        parse_mailbox(field(&fields, 4, line, "server mutation mailbox")?)
                            .ok_or_else(|| {
                                format!("line {line}: invalid server mutation mailbox")
                            })?;
                    mutation.server_targets.push(ServerMutationTarget {
                        message_id: field(&fields, 2, line, "server mutation message")?.to_owned(),
                        account_id: field(&fields, 3, line, "server mutation account")?.to_owned(),
                        mailbox,
                        uid_validity: parse_u32(&fields, 5, line, "server mutation UIDVALIDITY")?,
                        uid: parse_u32(&fields, 6, line, "server mutation UID")?,
                    });
                }
                _ => {}
            }
        }
        state.credential_journal = CredentialMutationJournal::from_parts(
            credential_sequence_high_water,
            credential_public_generation,
            credential_records,
        )?;
        state.next_account_ordinal = crate::model::settings::recover_account_allocator(
            &state.accounts,
            state.next_account_ordinal,
        )
        .map_err(|error| format!("invalid account allocator: {error:?}"))?;
        crate::model::settings::validate_account_allocator(
            &state.accounts,
            state.next_account_ordinal,
        )
        .map_err(|error| format!("invalid account allocator: {error:?}"))?;
        state.recover_ordinals();
        validate_local_state_domain(&state)?;
        Ok(state)
    }

    fn persist_default() -> Self {
        Self::empty()
    }
}
