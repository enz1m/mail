use super::mime::{
    ParsedMessageContent, address_parts, list_unsubscribe, parse_internal_date, parse_mime_message,
};
use super::transport::mailbox_identity_token;
use super::types::{
    AttachmentCompletion, FetchedMessage, ImapContentIdentity, MailFailure, MailFailureKind,
    MailResult, SyncCoverage,
};
use super::{
    MAX_FETCH_RESPONSE_BYTES, MAX_MESSAGE_BODY_BYTES, MAX_MESSAGES_PER_SYNC,
    MAX_SYNC_AGGREGATE_FETCH_BYTES, MAX_SYNC_RETAINED_MESSAGE_BYTES, MAX_UIDS_PER_MAILBOX_SNAPSHOT,
};
use crate::model::types::{Attachment, MailboxKind, Message, MessageBodyState, MessageLocation};
use imap_proto::{AttributeValue, MailboxDatum, Outcome, Response, ResponseCode, StatusAttribute};

pub(super) fn parse_responses(input: &[u8]) -> MailResult<Vec<Response<'_>>> {
    if input.len() > MAX_FETCH_RESPONSE_BYTES {
        return Err(MailFailure::new(
            MailFailureKind::ResponseTooLarge,
            "IMAP parser input exceeded configured limit",
        ));
    }
    let mut remaining = input;
    let mut responses = Vec::new();
    while !remaining.is_empty() {
        if responses.len() > MAX_MESSAGES_PER_SYNC.saturating_mul(4).saturating_add(16) {
            return Err(MailFailure::new(
                MailFailureKind::ResponseTooLarge,
                "IMAP response count exceeded configured limit",
            ));
        }
        match Response::parse(remaining) {
            Ok((next, response)) if next.len() < remaining.len() => {
                responses.push(response);
                remaining = next;
            }
            _ => {
                return Err(MailFailure::new(
                    MailFailureKind::Protocol,
                    "IMAP response parse failed",
                ));
            }
        }
    }
    Ok(responses)
}

pub(super) fn parse_uid_validity(input: &[u8]) -> MailResult<u32> {
    for response in parse_responses(input)? {
        match response {
            Response::MailboxData(MailboxDatum::Status { status, .. }) => {
                for item in status {
                    if let StatusAttribute::UidValidity(value) = item {
                        return Ok(value);
                    }
                }
            }
            Response::Data {
                outcome:
                    Outcome {
                        code: Some(ResponseCode::UidValidity(value)),
                        ..
                    },
                ..
            } => return Ok(value),
            _ => {}
        }
    }
    Err(MailFailure::new(
        MailFailureKind::Protocol,
        "IMAP UIDVALIDITY missing",
    ))
}

pub(super) fn parse_message_count(input: &[u8]) -> MailResult<u32> {
    for response in parse_responses(input)? {
        if let Response::MailboxData(MailboxDatum::Status { status, .. }) = response {
            for item in status {
                if let StatusAttribute::Messages(count) = item {
                    return Ok(count);
                }
            }
        }
    }
    Err(MailFailure::new(
        MailFailureKind::Protocol,
        "IMAP STATUS MESSAGES missing",
    ))
}

pub(super) fn window_search_command(count: u32, window: u32) -> Option<String> {
    (count > 0).then(|| {
        let first = count.saturating_sub(window.max(1)).saturating_add(1);
        format!("UID SEARCH {first}:*")
    })
}

pub(super) fn parse_search_uids(input: &[u8]) -> MailResult<Vec<u32>> {
    let mut found = None;
    for response in parse_responses(input)? {
        if let Response::MailboxData(MailboxDatum::Search(ids)) = response
            && found.replace(ids).is_some()
        {
            return Err(MailFailure::new(
                MailFailureKind::Protocol,
                "IMAP SEARCH returned multiple populations",
            ));
        }
    }
    found.ok_or_else(|| MailFailure::new(MailFailureKind::Protocol, "IMAP SEARCH result missing"))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct UidPopulation {
    pub(super) uids: Vec<u32>,
}

impl UidPopulation {
    fn pages(&self) -> impl Iterator<Item = &[u32]> {
        self.uids.chunks(MAX_MESSAGES_PER_SYNC)
    }
}

#[derive(Default)]
pub(super) struct SyncAggregateBudget {
    pub(super) raw_bytes: usize,
    pub(super) retained_bytes: usize,
}

impl SyncAggregateBudget {
    pub(super) fn charge_raw(
        &mut self,
        bytes: usize,
        completed: usize,
        total: usize,
    ) -> MailResult<()> {
        if bytes > MAX_FETCH_RESPONSE_BYTES {
            return Err(MailFailure::new(
                MailFailureKind::ResponseTooLarge,
                "IMAP page response exceeded configured limit",
            )
            .with_sync_progress(completed, total));
        }
        self.raw_bytes = self
            .raw_bytes
            .checked_add(bytes)
            .filter(|total| *total <= MAX_SYNC_AGGREGATE_FETCH_BYTES)
            .ok_or_else(|| {
                MailFailure::new(
                    MailFailureKind::ResponseTooLarge,
                    "IMAP aggregate response exceeded configured limit",
                )
                .with_sync_progress(completed, total)
            })?;
        Ok(())
    }

    pub(super) fn charge_retained(
        &mut self,
        bytes: usize,
        completed: usize,
        total: usize,
    ) -> MailResult<()> {
        self.retained_bytes = self
            .retained_bytes
            .checked_add(bytes)
            .filter(|total| *total <= MAX_SYNC_RETAINED_MESSAGE_BYTES)
            .ok_or_else(|| {
                MailFailure::new(
                    MailFailureKind::ResponseTooLarge,
                    "IMAP retained message candidate exceeded configured limit",
                )
                .with_sync_progress(completed, total)
            })?;
        Ok(())
    }

    pub(super) fn release_retained(
        &mut self,
        bytes: usize,
        completed: usize,
        total: usize,
    ) -> MailResult<()> {
        self.retained_bytes = self.retained_bytes.checked_sub(bytes).ok_or_else(|| {
            MailFailure::new(
                MailFailureKind::ResponseTooLarge,
                "IMAP retained message accounting underflow",
            )
            .with_sync_progress(completed, total)
        })?;
        Ok(())
    }
}

fn checked_add_retained(total: &mut usize, bytes: usize) -> MailResult<()> {
    *total = (*total).checked_add(bytes).ok_or_else(|| {
        MailFailure::new(
            MailFailureKind::ResponseTooLarge,
            "IMAP retained message size overflow",
        )
    })?;
    Ok(())
}

pub(super) fn retained_fetched_message_bytes(fetched: &FetchedMessage) -> MailResult<usize> {
    let message = &fetched.message;
    let mut bytes = 0usize;
    for value in [
        &message.id,
        &message.account_id,
        &message.sender_name,
        &message.sender_email,
        &message.subject,
        &message.preview,
        &message.body,
    ] {
        checked_add_retained(&mut bytes, value.capacity())?;
    }
    if let Some(value) = &message.body_html {
        checked_add_retained(&mut bytes, value.capacity())?;
    }
    if let Some(value) = &message.in_reply_to {
        checked_add_retained(&mut bytes, value.capacity())?;
    }
    if let MessageLocation::Folder(value) = &message.location {
        checked_add_retained(&mut bytes, value.capacity())?;
    }
    for recipients in [&message.to, &message.cc, &message.bcc] {
        checked_add_retained(
            &mut bytes,
            recipients
                .capacity()
                .checked_mul(std::mem::size_of::<String>())
                .ok_or_else(|| {
                    MailFailure::new(
                        MailFailureKind::ResponseTooLarge,
                        "IMAP recipient allocation size overflow",
                    )
                })?,
        )?;
        for value in recipients {
            checked_add_retained(&mut bytes, value.capacity())?;
        }
        checked_add_retained(
            &mut bytes,
            message
                .references
                .capacity()
                .checked_mul(std::mem::size_of::<String>())
                .ok_or_else(|| {
                    MailFailure::new(
                        MailFailureKind::ResponseTooLarge,
                        "IMAP reference allocation size overflow",
                    )
                })?,
        )?;
        for value in &message.references {
            checked_add_retained(&mut bytes, value.capacity())?;
        }
        checked_add_retained(
            &mut bytes,
            message
                .attachments
                .capacity()
                .checked_mul(std::mem::size_of::<Attachment>())
                .ok_or_else(|| {
                    MailFailure::new(
                        MailFailureKind::ResponseTooLarge,
                        "IMAP attachment allocation size overflow",
                    )
                })?,
        )?;
    }
    for attachment in &message.attachments {
        checked_add_retained(&mut bytes, attachment.name.capacity())?;
        checked_add_retained(&mut bytes, attachment.mime.capacity())?;
        if let Some(payload_id) = &attachment.payload_id {
            checked_add_retained(&mut bytes, payload_id.as_str().len())?;
        }
    }
    checked_add_retained(&mut bytes, fetched.body_completion.account_id.capacity())?;
    checked_add_retained(&mut bytes, fetched.body_completion.part.capacity())?;
    checked_add_retained(
        &mut bytes,
        fetched
            .attachment_completions
            .capacity()
            .checked_mul(std::mem::size_of::<AttachmentCompletion>())
            .ok_or_else(|| {
                MailFailure::new(
                    MailFailureKind::ResponseTooLarge,
                    "IMAP attachment completion allocation size overflow",
                )
            })?,
    )?;
    for completion in &fetched.attachment_completions {
        checked_add_retained(&mut bytes, completion.identity.account_id.capacity())?;
        checked_add_retained(&mut bytes, completion.identity.part.capacity())?;
        checked_add_retained(&mut bytes, completion.decoded_bytes.capacity())?;
        if let Some(payload_id) = &completion.payload_id {
            checked_add_retained(&mut bytes, payload_id.as_str().len())?;
        }
    }
    Ok(bytes)
}

pub(super) fn fetch_uid_pages(
    sync_generation: u64,
    account_id: &str,
    mailbox: MailboxKind,
    uid_validity: u32,
    population: &UidPopulation,
    mut fetch: impl FnMut(&[u32]) -> MailResult<Vec<u8>>,
) -> MailResult<Vec<FetchedMessage>> {
    let total = population.uids.len();
    let mut budget = SyncAggregateBudget::default();
    let mut messages = Vec::new();
    for page in population.pages() {
        let completed = messages.len();
        let next_count = messages
            .len()
            .checked_add(page.len())
            .filter(|count| *count <= total && *count <= MAX_UIDS_PER_MAILBOX_SNAPSHOT)
            .ok_or_else(|| {
                MailFailure::new(
                    MailFailureKind::ResponseTooLarge,
                    "IMAP retained message count exceeded configured limit",
                )
                .with_sync_progress(completed, total)
            })?;
        let previous_capacity = messages.capacity();
        messages.try_reserve(page.len()).map_err(|_| {
            MailFailure::new(
                MailFailureKind::ResponseTooLarge,
                "IMAP retained message allocation was refused",
            )
            .with_sync_progress(completed, total)
        })?;
        let added_capacity = messages
            .capacity()
            .checked_sub(previous_capacity)
            .ok_or_else(|| {
                MailFailure::new(
                    MailFailureKind::ResponseTooLarge,
                    "IMAP retained message allocation accounting failed",
                )
                .with_sync_progress(completed, total)
            })?;
        let allocation_bytes = added_capacity
            .checked_mul(std::mem::size_of::<FetchedMessage>())
            .ok_or_else(|| {
                MailFailure::new(
                    MailFailureKind::ResponseTooLarge,
                    "IMAP retained message allocation size overflow",
                )
                .with_sync_progress(completed, total)
            })?;
        budget.charge_retained(allocation_bytes, completed, total)?;
        let response = fetch(page).map_err(|error| error.with_sync_progress(completed, total))?;
        budget.charge_raw(response.len(), completed, total)?;
        let mut fetched = Vec::new();
        fetched.try_reserve(page.len()).map_err(|_| {
            MailFailure::new(
                MailFailureKind::ResponseTooLarge,
                "IMAP page candidate allocation was refused",
            )
            .with_sync_progress(completed, total)
        })?;
        let page_allocation_bytes = fetched
            .capacity()
            .checked_mul(std::mem::size_of::<FetchedMessage>())
            .ok_or_else(|| {
                MailFailure::new(
                    MailFailureKind::ResponseTooLarge,
                    "IMAP page candidate allocation size overflow",
                )
                .with_sync_progress(completed, total)
            })?;
        budget.charge_retained(page_allocation_bytes, completed, total)?;
        parse_fetch_messages_with_admission(
            sync_generation,
            account_id,
            mailbox,
            uid_validity,
            page,
            &response,
            |message| {
                budget.charge_retained(
                    retained_fetched_message_bytes(&message)
                        .map_err(|error| error.with_sync_progress(completed, total))?,
                    completed,
                    total,
                )?;
                fetched.push(message);
                Ok(())
            },
        )
        .map_err(|error| error.with_sync_progress(completed, total))?;
        drop(response);
        messages.append(&mut fetched);
        drop(fetched);
        budget.release_retained(page_allocation_bytes, completed, total)?;
        debug_assert_eq!(
            messages.len(),
            next_count,
            "fetch page must match its checked allocation count"
        );
    }
    Ok(messages)
}

pub(super) fn bounded_sync_coverage(
    coverage: SyncCoverage,
    total: usize,
    messages: &[FetchedMessage],
) -> SyncCoverage {
    if !coverage.is_complete() {
        return coverage;
    }
    let completed = messages
        .iter()
        .filter(|fetched| {
            !matches!(
                fetched.message.body_state,
                MessageBodyState::Incomplete { .. }
            )
        })
        .count();
    if completed == messages.len() {
        SyncCoverage::Complete
    } else {
        SyncCoverage::Capped { completed, total }
    }
}

pub(super) fn bounded_uid_population(uids: Vec<u32>) -> MailResult<UidPopulation> {
    let mut ordered = Vec::with_capacity(uids.len().min(MAX_UIDS_PER_MAILBOX_SNAPSHOT));
    for uid in uids {
        if uid == 0 {
            return Err(MailFailure::new(
                MailFailureKind::Protocol,
                "IMAP SEARCH returned UID zero",
            ));
        }
        match ordered.last().copied() {
            Some(previous) if uid < previous => {
                return Err(MailFailure::new(
                    MailFailureKind::Protocol,
                    "IMAP SEARCH returned a changing UID order",
                ));
            }
            Some(previous) if uid == previous => continue,
            _ => ordered.push(uid),
        }
        if ordered.len() > MAX_UIDS_PER_MAILBOX_SNAPSHOT {
            return Err(MailFailure::new(
                MailFailureKind::ResponseTooLarge,
                "IMAP mailbox UID population exceeded configured limit",
            ));
        }
    }
    Ok(UidPopulation { uids: ordered })
}

pub(super) fn parse_fetch_messages_with_admission(
    sync_generation: u64,
    account_id: &str,
    mailbox: MailboxKind,
    uid_validity: u32,
    expected_uids: &[u32],
    input: &[u8],
    mut admit: impl FnMut(FetchedMessage) -> MailResult<()>,
) -> MailResult<()> {
    let mut fetched_count = 0usize;
    for response in parse_responses(input)? {
        let Response::Fetch(_, attributes) = response else {
            continue;
        };
        let mut uid = None;
        let mut flags = Vec::new();
        let mut date = None;
        let mut envelope = None;
        let mut raw_message = None;
        let mut date_seen = false;
        let mut body_seen = false;
        let mut duplicate_structure = false;
        for attribute in attributes {
            match attribute {
                AttributeValue::Uid(value) => {
                    if uid.is_some() {
                        duplicate_structure = true;
                    } else {
                        uid = Some(value);
                    }
                }
                AttributeValue::Flags(value) => flags = value,
                AttributeValue::InternalDate(value) => {
                    if date_seen {
                        duplicate_structure = true;
                    } else {
                        date_seen = true;
                        date = parse_internal_date(&value);
                    }
                }
                AttributeValue::Envelope(value) => {
                    if envelope.is_some() {
                        duplicate_structure = true;
                    } else {
                        envelope = Some(value);
                    }
                }
                AttributeValue::BodySection { data, .. } => {
                    if body_seen {
                        duplicate_structure = true;
                    } else {
                        body_seen = true;
                        raw_message = data;
                    }
                }
                AttributeValue::Rfc822Text(data) => {
                    if body_seen {
                        duplicate_structure = true;
                    } else {
                        body_seen = true;
                        raw_message = data;
                    }
                }
                _ => {}
            }
        }
        let uid = uid
            .ok_or_else(|| MailFailure::new(MailFailureKind::Protocol, "IMAP FETCH UID missing"))?;
        let envelope = envelope.ok_or_else(|| {
            MailFailure::new(MailFailureKind::Protocol, "IMAP FETCH ENVELOPE missing")
        })?;
        let date = date.ok_or_else(|| {
            MailFailure::new(MailFailureKind::Protocol, "IMAP FETCH INTERNALDATE invalid")
        })?;
        if expected_uids.get(fetched_count).copied() != Some(uid) {
            return Err(MailFailure::new(
                MailFailureKind::Protocol,
                "IMAP FETCH did not preserve the exact requested UID order",
            ));
        }
        let raw_message = match raw_message.as_deref() {
            Some(raw_message) => raw_message,
            None if duplicate_structure && body_seen => &[],
            None => {
                return Err(MailFailure::new(
                    MailFailureKind::Protocol,
                    "IMAP FETCH message body missing",
                ));
            }
        };
        let (sender_name, sender_email) = envelope
            .from
            .as_deref()
            .and_then(|addresses| addresses.first())
            .map_or_else(
                || ("Unknown sender".to_owned(), String::new()),
                address_parts,
            );
        let recipients = envelope
            .to
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|address| address_parts(address).1)
            .filter(|address| !address.is_empty())
            .collect();
        let subject = envelope
            .subject
            .as_deref()
            .map(|value| {
                crate::model::header_text::decode_header_text(&String::from_utf8_lossy(value))
            })
            .unwrap_or_default();
        let id = if mailbox == MailboxKind::Inbox {
            format!("imap-{account_id}-{uid_validity}-{uid}")
        } else {
            format!(
                "imap-{account_id}-{}-{uid_validity}-{uid}",
                mailbox_identity_token(mailbox)
            )
        };
        let root_identity = ImapContentIdentity {
            account_id: account_id.to_owned(),
            mailbox,
            uid_validity,
            uid,
            part: "RFC822".to_owned(),
            sync_generation,
        };
        let parsed = if duplicate_structure {
            ParsedMessageContent {
                body: String::new(),
                preview: String::new(),
                body_html: None,
                body_state: MessageBodyState::Malformed,
                selected_part: "RFC822".to_owned(),
                attachments: Vec::new(),
                has_remote_content: false,
                has_inline_image: false,
            }
        } else if raw_message.len() > MAX_MESSAGE_BODY_BYTES as usize {
            ParsedMessageContent {
                body: String::new(),
                preview: String::new(),
                body_html: None,
                body_state: MessageBodyState::Incomplete {
                    retained_bytes: 0,
                    limit_bytes: MAX_MESSAGE_BODY_BYTES,
                },
                selected_part: "RFC822".to_owned(),
                attachments: Vec::new(),
                has_remote_content: false,
                has_inline_image: false,
            }
        } else {
            parse_mime_message(raw_message).unwrap_or_else(|_| ParsedMessageContent {
                body: String::new(),
                preview: String::new(),
                body_html: None,
                body_state: MessageBodyState::Malformed,
                selected_part: "RFC822".to_owned(),
                attachments: Vec::new(),
                has_remote_content: false,
                has_inline_image: false,
            })
        };
        let body_completion = ImapContentIdentity {
            part: parsed.selected_part.clone(),
            ..root_identity.clone()
        };
        let mut attachments = Vec::with_capacity(parsed.attachments.len());
        let mut attachment_completions = Vec::with_capacity(parsed.attachments.len());
        for decoded in parsed.attachments {
            let attachment = Attachment {
                name: decoded.name,
                mime: decoded.mime,
                size: u64::try_from(decoded.bytes.len()).unwrap_or(u64::MAX),
                payload_id: None,
                payload_identity: None,
            };
            attachment_completions.push(AttachmentCompletion {
                identity: ImapContentIdentity {
                    part: decoded.part,
                    ..root_identity.clone()
                },
                payload_id: None,
                payload_identity: None,
                decoded_bytes: decoded.bytes,
            });
            attachments.push(attachment);
        }
        let message = Message {
            id,
            account_id: account_id.to_owned(),
            location: MessageLocation::Mailbox(mailbox),
            sender_name,
            sender_email,
            to: recipients,
            subject,
            preview: parsed.preview,
            body: parsed.body,
            body_html: parsed.body_html,
            date,
            unread: !flags.iter().any(|flag| flag.eq_ignore_ascii_case("\\Seen")),
            flagged: flags
                .iter()
                .any(|flag| flag.eq_ignore_ascii_case("\\Flagged")),
            has_attachment: !attachments.is_empty(),
            attachments,
            has_remote_content: parsed.has_remote_content,
            has_inline_image: parsed.has_inline_image,
            blocked: false,
            in_reply_to: envelope
                .in_reply_to
                .as_deref()
                .map(|value| String::from_utf8_lossy(value).into_owned()),
            body_state: parsed.body_state,
            references: Vec::new(),
            snoozed_until: None,
            send_at: None,
            cc: envelope
                .cc
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|address| address_parts(address).1)
                .filter(|address| !address.is_empty())
                .collect(),
            bcc: envelope
                .bcc
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|address| address_parts(address).1)
                .filter(|address| !address.is_empty())
                .collect(),
            unsubscribe: list_unsubscribe(raw_message),
        };
        let fetched = FetchedMessage {
            message,
            body_completion,
            attachment_completions,
        };
        admit(fetched)?;
        fetched_count = fetched_count.checked_add(1).ok_or_else(|| {
            MailFailure::new(
                MailFailureKind::ResponseTooLarge,
                "IMAP parsed message count overflow",
            )
        })?;
    }
    if fetched_count != expected_uids.len() {
        return Err(MailFailure::new(
            MailFailureKind::Protocol,
            "IMAP FETCH did not return the exact requested UID set",
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn parse_fetch_messages(
    sync_generation: u64,
    account_id: &str,
    mailbox: MailboxKind,
    uid_validity: u32,
    expected_uids: &[u32],
    input: &[u8],
) -> MailResult<Vec<FetchedMessage>> {
    let mut messages = Vec::new();
    messages.try_reserve(expected_uids.len()).map_err(|_| {
        MailFailure::new(
            MailFailureKind::ResponseTooLarge,
            "IMAP parsed message allocation was refused",
        )
    })?;
    parse_fetch_messages_with_admission(
        sync_generation,
        account_id,
        mailbox,
        uid_validity,
        expected_uids,
        input,
        |message| {
            messages.push(message);
            Ok(())
        },
    )?;
    Ok(messages)
}
