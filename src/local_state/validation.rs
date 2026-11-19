use super::{
    LOCAL_STATE_ACCOUNTS_MAX, LOCAL_STATE_FIELD_DECODED_BYTES_MAX,
    LOCAL_STATE_FOLDERS_PER_ACCOUNT_MAX, LOCAL_STATE_IDENTITY_BYTES_MAX, LOCAL_STATE_MESSAGES_MAX,
    LOCAL_STATE_MUTATIONS_MAX, LOCAL_STATE_RECIPIENTS_PER_MESSAGE_MAX, LOCAL_STATE_RECORDS_MAX,
    LOCAL_STATE_REFERENCES_PER_MESSAGE_MAX, LOCAL_STATE_RETAINED_BYTES_MAX,
    LOCAL_STATE_TARGETS_PER_MUTATION_MAX, LOCAL_STATE_UNSUBSCRIBE_URIS_MAX, LocalState,
    ServerMutationKind, escaped_field_bytes,
};
use crate::credential_mutation::MAX_CREDENTIAL_TRANSITIONS;
use crate::model::types::{
    Attachment, AttachmentPayloadId, AttachmentPayloadIdentity, MessageLocation,
};

#[derive(Default)]
struct LocalStateBudget {
    records: usize,
    retained_bytes: usize,
}

impl LocalStateBudget {
    fn records(&mut self, additional: usize) -> Result<(), String> {
        self.records = self
            .records
            .checked_add(additional)
            .ok_or_else(|| "local-state record count overflow".to_owned())?;
        if self.records > LOCAL_STATE_RECORDS_MAX {
            return Err(format!(
                "local-state has too many records: {} exceeds {LOCAL_STATE_RECORDS_MAX}",
                self.records
            ));
        }
        Ok(())
    }

    fn field(&mut self, value: &str, name: &str) -> Result<(), String> {
        self.field_with_limit(value, LOCAL_STATE_FIELD_DECODED_BYTES_MAX, name)
    }

    fn identity(&mut self, value: &str, name: &str) -> Result<(), String> {
        self.field_with_limit(value, LOCAL_STATE_IDENTITY_BYTES_MAX, name)
    }

    fn field_with_limit(&mut self, value: &str, maximum: usize, name: &str) -> Result<(), String> {
        if value.len() > maximum {
            return Err(format!("{name} is too large"));
        }
        let encoded_maximum = maximum
            .checked_mul(2)
            .ok_or_else(|| "local-state encoded field limit overflow".to_owned())?;
        let encoded = escaped_field_bytes(value)
            .ok_or_else(|| "local-state encoded field length overflow".to_owned())?;
        if encoded > encoded_maximum {
            return Err(format!("encoded {name} is too large"));
        }
        self.retained_bytes = self
            .retained_bytes
            .checked_add(value.len())
            .ok_or_else(|| "local-state retained byte count overflow".to_owned())?;
        if self.retained_bytes > LOCAL_STATE_RETAINED_BYTES_MAX {
            return Err("local-state retained strings exceed the aggregate limit".to_owned());
        }
        Ok(())
    }
}

pub(super) fn validate_count(actual: usize, maximum: usize, name: &str) -> Result<(), String> {
    if actual > maximum {
        Err(format!("too many {name}: {actual} exceeds {maximum}"))
    } else {
        Ok(())
    }
}

fn validate_attachment(
    budget: &mut LocalStateBudget,
    payloads: &mut std::collections::HashMap<AttachmentPayloadId, AttachmentPayloadIdentity>,
    attachment: &Attachment,
) -> Result<(), String> {
    budget.field_with_limit(
        &attachment.name,
        crate::attachment_store::MAX_ATTACHMENT_NAME_BYTES,
        "attachment name",
    )?;
    budget.field_with_limit(
        &attachment.mime,
        crate::attachment_store::MAX_ATTACHMENT_MIME_BYTES,
        "attachment MIME type",
    )?;
    if attachment.size > crate::attachment_store::MAX_ATTACHMENT_BYTES {
        return Err("attachment payload size exceeds the per-file limit".to_owned());
    }
    if let Some(payload_id) = &attachment.payload_id {
        budget.identity(payload_id.as_str(), "attachment payload identity")?;
    }
    if attachment.payload_identity.is_some() && attachment.payload_id.is_none() {
        return Err("attachment payload metadata has no payload id".to_owned());
    }
    if attachment
        .payload_identity
        .is_some_and(|identity| !identity.is_valid() || identity.size != attachment.size)
    {
        return Err("attachment payload identity is invalid or has the wrong size".to_owned());
    }
    if let Some(payload) = attachment.payload_ref()
        && let Some(previous) = payloads.insert(payload.id, payload.identity)
        && previous != payload.identity
    {
        return Err("attachment payload id has conflicting descriptor identities".to_owned());
    }
    Ok(())
}

pub(super) fn validate_local_state_domain(state: &LocalState) -> Result<(), String> {
    let mut budget = LocalStateBudget::default();
    let mut attachment_payloads = std::collections::HashMap::new();
    budget.records(1)?;
    budget.identity(&state.active_account_id, "active account identity")?;
    validate_count(state.accounts.len(), LOCAL_STATE_ACCOUNTS_MAX, "accounts")?;
    validate_count(state.messages.len(), LOCAL_STATE_MESSAGES_MAX, "messages")?;
    validate_count(
        state.pending_server_mutations.len(),
        LOCAL_STATE_MUTATIONS_MAX,
        "pending server mutations",
    )?;
    validate_count(
        state.credential_journal.records().len(),
        MAX_CREDENTIAL_TRANSITIONS,
        "credential transitions",
    )?;
    state.credential_journal.validate()?;
    for transition in state.credential_journal.records() {
        budget.records(1)?;
        budget.identity(
            &transition.account_id,
            "credential transition account identity",
        )?;
        for key in [transition.old_key.as_ref(), transition.new_key.as_ref()]
            .into_iter()
            .flatten()
        {
            budget.identity(key.app_id(), "credential application identity")?;
            budget.identity(key.owner(), "credential owner identity")?;
            budget.identity(key.label(), "credential label identity")?;
        }
    }

    for account in &state.accounts {
        budget.records(1)?;
        budget.identity(&account.id, "account identity")?;
        budget.field(&account.display_name, "account display name")?;
        budget.field(&account.email, "account email")?;
        budget.field(&account.signature, "account signature")?;
        validate_count(account.mailboxes.len(), 6, "account mailboxes")?;
        validate_count(
            account.folders.len(),
            LOCAL_STATE_FOLDERS_PER_ACCOUNT_MAX,
            "account folders",
        )?;
        if let Some(config) = &account.config {
            budget.identity(&config.id, "account configuration identity")?;
            budget.identity(&config.credential_owner, "credential owner identity")?;
            budget.field(&config.display_name, "configuration display name")?;
            budget.field(&config.email, "configuration email")?;
            budget.field(&config.incoming.server, "incoming server")?;
            budget.field(&config.incoming.username, "incoming username")?;
            budget.field(&config.smtp.server, "SMTP server")?;
            budget.field(&config.smtp.username, "SMTP username")?;
        }
        budget.records(account.folders.len())?;
        for folder in &account.folders {
            budget.identity(&folder.id, "folder identity")?;
            budget.field(&folder.name, "folder name")?;
        }
    }

    for message in &state.messages {
        budget.records(1)?;
        budget.identity(&message.id, "message identity")?;
        budget.identity(&message.account_id, "message account identity")?;
        if let MessageLocation::Folder(folder) = &message.location {
            budget.identity(folder, "message folder identity")?;
        }
        budget.field(&message.sender_name, "sender name")?;
        budget.field(&message.sender_email, "sender email")?;
        budget.field(&message.subject, "message subject")?;
        budget.field(&message.preview, "message preview")?;
        budget.field(&message.body, "message body")?;
        if let Some(html) = &message.body_html {
            budget.field(html, "message HTML body")?;
        }
        if !message.body_state.is_complete()
            && (!message.body.is_empty() || message.body_html.is_some())
        {
            return Err("incomplete message content retained displayable body bytes".to_owned());
        }
        if let Some(reply) = &message.in_reply_to {
            budget.identity(reply, "reply identity")?;
        }
        validate_count(
            message
                .to
                .len()
                .saturating_add(message.cc.len())
                .saturating_add(message.bcc.len()),
            LOCAL_STATE_RECIPIENTS_PER_MESSAGE_MAX,
            "message recipients",
        )?;
        validate_count(
            message.attachments.len(),
            crate::attachment_store::MAX_ATTACHMENT_COUNT,
            "message attachments",
        )?;
        validate_count(
            message.references.len(),
            LOCAL_STATE_REFERENCES_PER_MESSAGE_MAX,
            "message references",
        )?;
        budget.records(
            message
                .to
                .len()
                .saturating_add(message.cc.len())
                .saturating_add(message.bcc.len()),
        )?;
        budget.records(message.attachments.len())?;
        budget.records(message.references.len())?;
        if let Some(unsubscribe) = &message.unsubscribe {
            validate_count(
                unsubscribe.uris.len(),
                LOCAL_STATE_UNSUBSCRIBE_URIS_MAX,
                "unsubscribe routes",
            )?;
            budget.records(unsubscribe.uris.len())?;
            for uri in &unsubscribe.uris {
                budget.field(uri, "unsubscribe route")?;
            }
        }
        for recipient in message.to.iter().chain(&message.cc).chain(&message.bcc) {
            budget.field(recipient, "message recipient")?;
        }
        for attachment in &message.attachments {
            validate_attachment(&mut budget, &mut attachment_payloads, attachment)?;
        }
        for reference in &message.references {
            budget.identity(reference, "message reference")?;
        }
    }

    if let Some(pending) = &state.pending_send {
        budget.records(1)?;
        budget.identity(&pending.id, "pending-send identity")?;
        budget.identity(&pending.account_id, "pending-send account identity")?;
        budget.field(&pending.sender_name, "pending sender name")?;
        budget.field(&pending.sender_email, "pending sender email")?;
        budget.field(&pending.to_raw, "pending recipients")?;
        budget.field(&pending.cc_raw, "pending carbon-copy recipients")?;
        budget.field(&pending.bcc_raw, "pending blind-copy recipients")?;
        budget.field(&pending.subject, "pending subject")?;
        budget.field(&pending.body, "pending body")?;
        if let Some(html) = &pending.body_html {
            budget.field(html, "pending HTML body")?;
        }
        if let Some(reply) = &pending.in_reply_to {
            budget.identity(reply, "pending reply identity")?;
        }
        if let Some(source) = pending.mode.source_message_id() {
            budget.identity(source, "pending compose source identity")?;
        }
        if let Some(restored) = &pending.restored_draft_id {
            budget.identity(restored, "restored draft identity")?;
        }
        validate_count(
            pending.attachments.len(),
            crate::attachment_store::MAX_ATTACHMENT_COUNT,
            "pending attachments",
        )?;
        validate_count(
            pending.references.len(),
            LOCAL_STATE_REFERENCES_PER_MESSAGE_MAX,
            "pending references",
        )?;
        budget.records(pending.attachments.len())?;
        budget.records(pending.references.len())?;
        for attachment in &pending.attachments {
            validate_attachment(&mut budget, &mut attachment_payloads, attachment)?;
        }
        for reference in &pending.references {
            budget.identity(reference, "pending reference")?;
        }
    }

    for mutation in &state.pending_server_mutations {
        budget.records(1)?;
        validate_count(
            mutation.target_ids.len(),
            LOCAL_STATE_TARGETS_PER_MUTATION_MAX,
            "mutation targets",
        )?;
        validate_count(
            mutation.server_targets.len(),
            LOCAL_STATE_TARGETS_PER_MUTATION_MAX,
            "server mutation targets",
        )?;
        budget.records(
            mutation
                .target_ids
                .len()
                .checked_add(mutation.server_targets.len())
                .ok_or_else(|| "mutation record count overflow".to_owned())?,
        )?;
        for target in &mutation.target_ids {
            budget.identity(target, "mutation target identity")?;
        }
        for target in &mutation.server_targets {
            budget.identity(&target.message_id, "server mutation message identity")?;
            budget.identity(&target.account_id, "server mutation account identity")?;
            if target.uid == 0 || target.uid_validity == 0 {
                return Err("server mutation UID identity contains zero".to_owned());
            }
            if !mutation.target_ids.contains(&target.message_id) {
                return Err("server mutation identity is not a local target".to_owned());
            }
        }
        if !mutation.server_targets.is_empty()
            && !matches!(
                &mutation.kind,
                ServerMutationKind::Move(_)
                    | ServerMutationKind::SetUnread(_)
                    | ServerMutationKind::SetFlagged(_)
                    | ServerMutationKind::EmptyMailbox(_)
            )
        {
            return Err("non-IMAP mutation contains a server identity".to_owned());
        }
    }
    Ok(())
}
