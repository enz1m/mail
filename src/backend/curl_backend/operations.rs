use super::fetch::{
    UidPopulation, bounded_sync_coverage, bounded_uid_population, fetch_uid_pages,
    parse_message_count, parse_responses, parse_search_uids, parse_uid_validity,
    window_search_command,
};
use super::transport::{
    IdleOutcome, ImapSession, imap_endpoint_url, imap_mailbox_name, perform_imap, perform_imap_on,
    submit_message, uid_set, validate_submission_config, validate_supported_config,
};
use super::types::{
    MailBackend, MailFailure, MailFailureKind, MailOperation, MailOperationInner,
    MailOperationOutput, MailResult, MutationDisposition, MutationRequest, MutationResult,
    OperationCancel, RetainedMessage, SendRequest, SmtpWireState, SyncBatch, SyncCoverage,
    SyncRequest,
};
use super::{
    AccountConfig, Encryption, MAX_CONTROL_RESPONSE_BYTES, MAX_FETCH_RESPONSE_BYTES,
    MAX_MESSAGE_BODY_BYTES, MAX_MESSAGES_PER_SYNC, SYNC_WINDOW_MESSAGES,
};
use crate::model::types::MailboxKind;
use curl::easy::Easy;
use enzui_iced_core::blocking_task::{BlockingTaskDispatcher, BlockingTaskOwner};
use enzui_services::credential_store::Secret;
use imap_proto::{AttributeValue, Response};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

// Workers of the self-contained transport pool used without an external
// dispatcher. One worker runs libcurl serially, with up to
// [`MAIL_TRANSPORT_QUEUE`] queued behind it; since the reducer keeps one mail
// intent in flight, the slack absorbs a tight cancel-and-reissue window.
const MAIL_TRANSPORT_WORKERS: NonZeroUsize = NonZeroUsize::new(1).unwrap();
const MAIL_TRANSPORT_QUEUE: NonZeroUsize = NonZeroUsize::new(2).unwrap();

#[derive(Clone, Debug)]
pub(crate) struct CurlMailBackend {
    protocols: Arc<[String]>,
    dispatcher: BlockingTaskDispatcher,
    // Present only for a self-contained backend, keeping its internal pool alive.
    _owner: Option<Arc<BlockingTaskOwner>>,
}

fn detected_protocols() -> Arc<[String]> {
    curl::Version::get()
        .protocols()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .into()
}

pub(super) fn internal_owner() -> (BlockingTaskDispatcher, Arc<BlockingTaskOwner>) {
    let owner = BlockingTaskOwner::new(
        "mail-transport",
        MAIL_TRANSPORT_WORKERS,
        MAIL_TRANSPORT_QUEUE,
    )
    .unwrap_or_else(|_| BlockingTaskOwner::unavailable(MAIL_TRANSPORT_QUEUE));
    let dispatcher = owner.dispatcher();
    (dispatcher, Arc::new(owner))
}

impl Default for CurlMailBackend {
    fn default() -> Self {
        let (dispatcher, owner) = internal_owner();
        Self {
            protocols: detected_protocols(),
            dispatcher,
            _owner: Some(owner),
        }
    }
}

impl CurlMailBackend {
    // Builds a backend that submits to `dispatcher` rather than an internal
    // pool.
    //
    // The caller owns the pool lifetime so shell shutdown can drain queued
    // work through its own close settlement rather than through the backend's
    // drop timing.
    #[must_use]
    pub(crate) fn with_dispatcher(dispatcher: BlockingTaskDispatcher) -> Self {
        Self {
            protocols: detected_protocols(),
            dispatcher,
            _owner: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_protocols(protocols: &[&str]) -> Self {
        let (dispatcher, owner) = internal_owner();
        Self {
            protocols: protocols
                .iter()
                .map(|protocol| protocol.to_ascii_lowercase())
                .collect::<Vec<_>>()
                .into(),
            dispatcher,
            _owner: Some(owner),
        }
    }

    fn required_incoming_protocol(config: &AccountConfig) -> &'static str {
        if config.incoming.encryption == Encryption::Tls {
            "imaps"
        } else {
            "imap"
        }
    }

    fn required_outgoing_protocol(config: &AccountConfig) -> &'static str {
        if config.smtp.encryption == Encryption::Tls {
            "smtps"
        } else {
            "smtp"
        }
    }
}

impl MailBackend for CurlMailBackend {
    fn validate_configuration(&self, config: &AccountConfig) -> MailResult<()> {
        validate_supported_config(config)?;
        let required = Self::required_incoming_protocol(config);
        if !self.protocols.iter().any(|protocol| protocol == required) {
            return Err(MailFailure::new(
                MailFailureKind::Unavailable,
                format!("libcurl protocol unavailable: {required}"),
            ));
        }
        Ok(())
    }

    fn validate_submission(&self, config: &AccountConfig) -> MailResult<()> {
        validate_submission_config(config)?;
        let required = Self::required_outgoing_protocol(config);
        if !self.protocols.iter().any(|protocol| protocol == required) {
            return Err(MailFailure::new(
                MailFailureKind::Unavailable,
                format!("libcurl protocol unavailable: {required}"),
            ));
        }
        Ok(())
    }

    fn probe(&self, generation: u64, request: SyncRequest) -> MailOperation {
        let validation = self.validate_configuration(&request.config);
        submit_operation(&self.dispatcher, generation, move |cancel| {
            validation?;
            probe_account(&request.config, &request.secret, &cancel)?;
            Ok(MailOperationOutput::Authenticated)
        })
    }

    fn sync_mailbox(
        &self,
        generation: u64,
        request: SyncRequest,
        mailbox: MailboxKind,
    ) -> MailOperation {
        let validation = self.validate_configuration(&request.config);
        submit_operation(&self.dispatcher, generation, move |cancel| {
            validation?;
            sync_mailbox(
                generation,
                &request.config,
                &request.secret,
                &cancel,
                mailbox,
                &request.known,
            )
            .map(MailOperationOutput::Inbox)
        })
    }

    fn mutate(&self, generation: u64, request: MutationRequest) -> MailOperation {
        let validation = self.validate_configuration(&request.config);
        submit_operation(&self.dispatcher, generation, move |cancel| {
            validation?;
            reconcile_and_apply_mutation(request, &cancel).map(MailOperationOutput::Mutation)
        })
    }

    fn search_mailbox(
        &self,
        generation: u64,
        request: SyncRequest,
        mailbox: MailboxKind,
        criteria: String,
    ) -> MailOperation {
        let validation = self.validate_configuration(&request.config);
        submit_operation(&self.dispatcher, generation, move |cancel| {
            validation?;
            search_mailbox(
                generation,
                &request.config,
                &request.secret,
                &cancel,
                mailbox,
                &request.known,
                &criteria,
            )
            .map(MailOperationOutput::Search)
        })
    }

    fn submit(&self, generation: u64, request: SendRequest) -> MailOperation {
        let validation = self.validate_submission(&request.config);
        submit_smtp_operation(&self.dispatcher, generation, move |cancel, smtp_wire| {
            validation?;
            submit_message(request, cancel, smtp_wire)?;
            Ok(MailOperationOutput::Submitted)
        })
    }

    fn watch_inbox(
        &self,
        config: &AccountConfig,
        secret: Secret,
    ) -> Option<crate::backend::inbox_watch::InboxWatch> {
        crate::backend::inbox_watch::InboxWatch::spawn(config.clone(), secret).ok()
    }
}

#[cfg(test)]
pub(crate) fn ready_operation(
    generation: u64,
    result: MailResult<MailOperationOutput>,
) -> MailOperation {
    MailOperation {
        generation,
        cancel: OperationCancel {
            cancelled: Arc::new(AtomicBool::new(false)),
            smtp_wire: None,
            task_cancel: None,
        },
        inner: MailOperationInner::Settled(result),
    }
}

// Dispatches one libcurl transport call onto `dispatcher`.
//
// The returned [`MailOperation`] settles exactly once per generation. `run`
// receives the flag [`OperationCancel::cancel`] sets, and a dispatcher
// shutdown sets it too, so a cancelled or queued call aborts or never opens a
// socket.
pub(super) fn submit_operation(
    dispatcher: &BlockingTaskDispatcher,
    generation: u64,
    run: impl FnOnce(Arc<AtomicBool>) -> MailResult<MailOperationOutput> + Send + 'static,
) -> MailOperation {
    submit_operation_with_smtp_wire(dispatcher, generation, None, run)
}

pub(super) fn submit_smtp_operation(
    dispatcher: &BlockingTaskDispatcher,
    generation: u64,
    run: impl FnOnce(Arc<AtomicBool>, Arc<SmtpWireState>) -> MailResult<MailOperationOutput>
    + Send
    + 'static,
) -> MailOperation {
    let smtp_wire = Arc::new(SmtpWireState::new());
    let closure_wire = Arc::clone(&smtp_wire);
    submit_operation_with_smtp_wire(dispatcher, generation, Some(smtp_wire), move |cancel| {
        run(cancel, closure_wire)
    })
}

fn submit_operation_with_smtp_wire(
    dispatcher: &BlockingTaskDispatcher,
    generation: u64,
    smtp_wire: Option<Arc<SmtpWireState>>,
    run: impl FnOnce(Arc<AtomicBool>) -> MailResult<MailOperationOutput> + Send + 'static,
) -> MailOperation {
    let cancelled = Arc::new(AtomicBool::new(false));
    let closure_cancelled = Arc::clone(&cancelled);
    let task = dispatcher.submit(generation, move |ctx| {
        if ctx.is_cancelled() {
            closure_cancelled.store(true, Ordering::Release);
        }
        run(closure_cancelled)
    });
    let task_cancel = task.cancellation();
    MailOperation {
        generation,
        cancel: OperationCancel {
            cancelled,
            smtp_wire,
            task_cancel: Some(task_cancel),
        },
        inner: MailOperationInner::Dispatched(task),
    }
}

fn probe_account(config: &AccountConfig, secret: &Secret, cancel: &AtomicBool) -> MailResult<()> {
    validate_supported_config(config)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(MailFailure::new(
            MailFailureKind::Cancelled,
            "operation cancelled before IMAP probe",
        ));
    }
    let url = imap_endpoint_url(
        &config.incoming.server,
        config.incoming.port,
        config.incoming.encryption,
        "INBOX",
    )?;
    let _ = perform_imap(
        config,
        secret,
        cancel,
        &url,
        Some("NOOP"),
        MAX_CONTROL_RESPONSE_BYTES,
    )?;
    Ok(())
}

pub(super) fn sync_mailbox(
    sync_generation: u64,
    config: &AccountConfig,
    secret: &Secret,
    cancel: &AtomicBool,
    mailbox: MailboxKind,
    known: &[(u32, u32)],
) -> MailResult<SyncBatch> {
    validate_supported_config(config)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(MailFailure::new(
            MailFailureKind::Cancelled,
            "operation cancelled before IMAP sync",
        ));
    }
    let root_url = imap_endpoint_url(
        &config.incoming.server,
        config.incoming.port,
        config.incoming.encryption,
        "",
    )?;
    let mailbox_name = imap_mailbox_name(mailbox);
    let status_command = format!("STATUS {mailbox_name} (MESSAGES UIDVALIDITY)");
    // One connection and one login for the whole sync, and one command per
    // page of messages rather than one per message.
    let mut session = ImapSession::open(config, secret, cancel, &root_url)?;
    let status = session.run(&status_command, MAX_CONTROL_RESPONSE_BYTES)?;
    let uid_validity = parse_uid_validity(&status)?;
    let message_count = parse_message_count(&status)?;
    let window_search = window_search_command(message_count, SYNC_WINDOW_MESSAGES);
    session.run(
        &format!("EXAMINE \"{mailbox_name}\""),
        MAX_CONTROL_RESPONSE_BYTES,
    )?;
    let search_window = |session: &mut ImapSession<'_>| -> MailResult<UidPopulation> {
        let Some(command) = window_search.as_deref() else {
            return bounded_uid_population(Vec::new());
        };
        let search = session.run(command, MAX_CONTROL_RESPONSE_BYTES)?;
        bounded_uid_population(parse_search_uids(&search)?)
    };
    let population = search_window(&mut session)?;
    let body_probe_bytes = MAX_MESSAGE_BODY_BYTES.checked_add(1).ok_or_else(|| {
        MailFailure::new(MailFailureKind::Configuration, "IMAP body limit overflow")
    })?;
    let items = format!("(UID FLAGS INTERNALDATE ENVELOPE BODY.PEEK[]<0.{body_probe_bytes}>)");
    // Only messages the app does not already hold complete are downloaded;
    // the rest need nothing but their current flags.
    let known: std::collections::HashSet<u32> = known
        .iter()
        .filter(|(validity, _)| *validity == uid_validity)
        .map(|(_, uid)| *uid)
        .collect();
    let (kept, fresh): (Vec<u32>, Vec<u32>) =
        population.uids.iter().partition(|uid| known.contains(uid));
    let retained = if kept.is_empty() {
        Vec::new()
    } else {
        let response = session.run(
            &format!("UID FETCH {} (UID FLAGS)", uid_set(&kept)),
            MAX_FETCH_RESPONSE_BYTES,
        )?;
        let flags = parse_uid_flags(&response)?;
        if flags.len() != kept.len() {
            return Err(MailFailure::new(
                MailFailureKind::Protocol,
                "IMAP flags response missed a message",
            ));
        }
        flags
            .into_iter()
            .map(|(uid, seen, flagged)| RetainedMessage { uid, seen, flagged })
            .collect()
    };
    let fresh = UidPopulation { uids: fresh };
    let messages = fetch_uid_pages(
        sync_generation,
        &config.id,
        mailbox,
        uid_validity,
        &fresh,
        |uids| {
            session.run(
                &format!("UID FETCH {} {items}", uid_set(uids)),
                MAX_FETCH_RESPONSE_BYTES,
            )
        },
    )?;
    let verified = search_window(&mut session)?;
    let final_status = session.run(&status_command, MAX_CONTROL_RESPONSE_BYTES)?;
    if verified.uids != population.uids || parse_uid_validity(&final_status)? != uid_validity {
        return Err(MailFailure::new(
            MailFailureKind::Protocol,
            "IMAP mailbox identity changed during synchronization",
        ));
    }
    let coverage = bounded_sync_coverage(SyncCoverage::Complete, fresh.uids.len(), &messages);
    Ok(SyncBatch {
        older_than_uid: if message_count > SYNC_WINDOW_MESSAGES {
            population.uids.first().copied()
        } else {
            None
        },
        retained,
        account_id: config.id.clone(),
        mailbox,
        uid_validity,
        sync_generation,
        coverage,
        messages,
    })
}

pub(crate) const SERVER_SEARCH_LIMIT: usize = 50;

pub(super) fn search_mailbox(
    sync_generation: u64,
    config: &AccountConfig,
    secret: &Secret,
    cancel: &AtomicBool,
    mailbox: MailboxKind,
    known: &[(u32, u32)],
    criteria: &str,
) -> MailResult<SyncBatch> {
    validate_supported_config(config)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(MailFailure::new(
            MailFailureKind::Cancelled,
            "operation cancelled before IMAP search",
        ));
    }
    if criteria.is_empty()
        || criteria
            .chars()
            .any(|c| !c.is_ascii() || c.is_ascii_control())
    {
        return Err(MailFailure::new(
            MailFailureKind::Configuration,
            "IMAP search criteria must be plain ASCII",
        ));
    }
    let root_url = imap_endpoint_url(
        &config.incoming.server,
        config.incoming.port,
        config.incoming.encryption,
        "",
    )?;
    let mailbox_name = imap_mailbox_name(mailbox);
    let mut session = ImapSession::open(config, secret, cancel, &root_url)?;
    let status = session.run(
        &format!("STATUS {mailbox_name} (MESSAGES UIDVALIDITY)"),
        MAX_CONTROL_RESPONSE_BYTES,
    )?;
    let uid_validity = parse_uid_validity(&status)?;
    session.run(
        &format!("EXAMINE \"{mailbox_name}\""),
        MAX_CONTROL_RESPONSE_BYTES,
    )?;
    let criteria = if let Some(before) = criteria.strip_prefix("X-MAIL-OLDER ") {
        let uid = before
            .parse::<u32>()
            .ok()
            .filter(|uid| *uid > 0)
            .ok_or_else(|| {
                MailFailure::new(MailFailureKind::Configuration, "invalid older-page cursor")
            })?;
        let position = session.run(
            &format!("UID FETCH {uid} (UID)"),
            MAX_CONTROL_RESPONSE_BYTES,
        )?;
        let sequence = parse_responses(&position)?.into_iter().find_map(|response| match response {
            imap_proto::Response::Fetch(sequence, attributes) if attributes.iter().any(|attribute| matches!(attribute, imap_proto::AttributeValue::Uid(value) if *value == uid)) => Some(sequence),
            _ => None,
        }).ok_or_else(|| MailFailure::new(MailFailureKind::Protocol, "older-page cursor was removed; refresh the mailbox"))?;
        if sequence <= 1 {
            return Ok(SyncBatch {
                older_than_uid: None,
                retained: Vec::new(),
                account_id: config.id.clone(),
                mailbox,
                uid_validity,
                sync_generation,
                coverage: SyncCoverage::Complete,
                messages: Vec::new(),
            });
        } else {
            format!(
                "{}:{}",
                sequence.saturating_sub(SYNC_WINDOW_MESSAGES).max(1),
                sequence - 1
            )
        }
    } else {
        criteria.to_owned()
    };
    let found = session.run(
        &format!("UID SEARCH {criteria}"),
        MAX_CONTROL_RESPONSE_BYTES,
    )?;
    let known: std::collections::HashSet<u32> = known
        .iter()
        .filter(|(validity, _)| *validity == uid_validity)
        .map(|(_, uid)| *uid)
        .collect();
    let mut fresh: Vec<u32> = parse_search_uids(&found)?
        .into_iter()
        .filter(|uid| !known.contains(uid))
        .collect();
    fresh.sort_unstable();
    let newest = fresh.split_off(fresh.len().saturating_sub(SERVER_SEARCH_LIMIT));
    let population = bounded_uid_population(newest)?;
    let body_probe_bytes = MAX_MESSAGE_BODY_BYTES.checked_add(1).ok_or_else(|| {
        MailFailure::new(MailFailureKind::Configuration, "IMAP body limit overflow")
    })?;
    let items = format!("(UID FLAGS INTERNALDATE ENVELOPE BODY.PEEK[]<0.{body_probe_bytes}>)");
    let messages = fetch_uid_pages(
        sync_generation,
        &config.id,
        mailbox,
        uid_validity,
        &population,
        |uids| {
            session.run(
                &format!("UID FETCH {} {items}", uid_set(uids)),
                MAX_FETCH_RESPONSE_BYTES,
            )
        },
    )?;
    let coverage = bounded_sync_coverage(SyncCoverage::Complete, population.uids.len(), &messages);
    Ok(SyncBatch {
        older_than_uid: None,
        retained: Vec::new(),
        account_id: config.id.clone(),
        mailbox,
        uid_validity,
        sync_generation,
        coverage,
        messages,
    })
}

fn reconcile_and_apply_mutation(
    request: MutationRequest,
    cancel: &AtomicBool,
) -> MailResult<MutationResult> {
    use crate::local_state::ServerMutationKind;

    let MutationRequest {
        config,
        mutation,
        secret,
    } = request;
    let first = mutation.server_targets.first().ok_or_else(|| {
        MailFailure::new(
            MailFailureKind::Configuration,
            "IMAP mutation has no exact server targets",
        )
    })?;
    if mutation.server_targets.len() > MAX_MESSAGES_PER_SYNC {
        return Err(MailFailure::new(
            MailFailureKind::Configuration,
            "IMAP mutation exceeds the bounded target count",
        ));
    }
    if mutation.server_targets.iter().any(|target| {
        target.account_id != config.id
            || target.account_id != first.account_id
            || target.mailbox != first.mailbox
            || target.uid_validity != first.uid_validity
            || target.uid == 0
    }) {
        return Err(MailFailure::new(
            MailFailureKind::Configuration,
            "IMAP mutation targets cross an identity boundary",
        ));
    }
    let mailbox_name = imap_mailbox_name(first.mailbox);
    let root_url = imap_endpoint_url(
        &config.incoming.server,
        config.incoming.port,
        config.incoming.encryption,
        "",
    )?;
    let status_command = format!("STATUS {mailbox_name} (UIDVALIDITY)");
    let status = perform_imap(
        &config,
        &secret,
        cancel,
        &root_url,
        Some(&status_command),
        MAX_CONTROL_RESPONSE_BYTES,
    )?;
    if parse_uid_validity(&status)? != first.uid_validity {
        return Ok(MutationResult {
            mutation_id: mutation.id,
            disposition: MutationDisposition::StaleIdentity,
            missing_target_ids: mutation.target_ids,
        });
    }
    let mailbox_url = imap_endpoint_url(
        &config.incoming.server,
        config.incoming.port,
        config.incoming.encryption,
        mailbox_name,
    )?;
    let requested_uids = mutation
        .server_targets
        .iter()
        .map(|target| target.uid)
        .collect::<Vec<_>>();
    let before = fetch_uid_flags(&config, &secret, cancel, &mailbox_url, &requested_uids)?;
    let mut missing_target_ids = mutation
        .server_targets
        .iter()
        .filter(|target| !before.iter().any(|(uid, _, _)| *uid == target.uid))
        .map(|target| target.message_id.clone())
        .collect::<Vec<_>>();
    let present = requested_uids
        .iter()
        .copied()
        .filter(|uid| before.iter().any(|(present, _, _)| present == uid))
        .collect::<Vec<_>>();
    if present.is_empty() {
        return Ok(MutationResult {
            mutation_id: mutation.id,
            disposition: MutationDisposition::AlreadyApplied,
            missing_target_ids,
        });
    }

    let present_set = present
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let command = match &mutation.kind {
        ServerMutationKind::SetUnread(unread) => {
            let flag = "\\Seen";
            let all_match = present.iter().all(|uid| {
                before
                    .iter()
                    .find(|(candidate, _, _)| candidate == uid)
                    .is_some_and(|(_, seen, _)| *seen == !*unread)
            });
            if all_match {
                return Ok(MutationResult {
                    mutation_id: mutation.id,
                    disposition: MutationDisposition::AlreadyApplied,
                    missing_target_ids,
                });
            }
            format!(
                "UID STORE {present_set} {}FLAGS.SILENT ({flag})",
                if *unread { "-" } else { "+" }
            )
        }
        ServerMutationKind::SetFlagged(flagged) => {
            let flag = "\\Flagged";
            let all_match = present.iter().all(|uid| {
                before
                    .iter()
                    .find(|(candidate, _, _)| candidate == uid)
                    .is_some_and(|(_, _, current)| *current == *flagged)
            });
            if all_match {
                return Ok(MutationResult {
                    mutation_id: mutation.id,
                    disposition: MutationDisposition::AlreadyApplied,
                    missing_target_ids,
                });
            }
            format!(
                "UID STORE {present_set} {}FLAGS.SILENT ({flag})",
                if *flagged { "+" } else { "-" }
            )
        }
        ServerMutationKind::Move(destination) => {
            if *destination == first.mailbox {
                return Ok(MutationResult {
                    mutation_id: mutation.id,
                    disposition: MutationDisposition::AlreadyApplied,
                    missing_target_ids,
                });
            }
            format!("UID MOVE {present_set} {}", imap_mailbox_name(*destination))
        }
        ServerMutationKind::EmptyMailbox(mailbox) if *mailbox == first.mailbox => {
            format!("UID STORE {present_set} +FLAGS.SILENT (\\Deleted)")
        }
        _ => {
            return Err(MailFailure::new(
                MailFailureKind::Configuration,
                "unsupported or mismatched IMAP mutation kind",
            ));
        }
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(MailFailure::new(
            MailFailureKind::Cancelled,
            "operation cancelled before IMAP mutation",
        ));
    }
    perform_imap(
        &config,
        &secret,
        cancel,
        &mailbox_url,
        Some(&command),
        MAX_CONTROL_RESPONSE_BYTES,
    )
    .map_err(mutation_uncertain)?;
    if matches!(&mutation.kind, ServerMutationKind::EmptyMailbox(_)) {
        let expunge = format!("UID EXPUNGE {present_set}");
        perform_imap(
            &config,
            &secret,
            cancel,
            &mailbox_url,
            Some(&expunge),
            MAX_CONTROL_RESPONSE_BYTES,
        )
        .map_err(mutation_uncertain)?;
    }
    let after = fetch_uid_flags(&config, &secret, cancel, &mailbox_url, &present)
        .map_err(mutation_uncertain)?;
    if matches!(
        &mutation.kind,
        ServerMutationKind::SetUnread(_) | ServerMutationKind::SetFlagged(_)
    ) {
        for target in &mutation.server_targets {
            if !after.iter().any(|(uid, _, _)| *uid == target.uid)
                && !missing_target_ids.contains(&target.message_id)
            {
                missing_target_ids.push(target.message_id.clone());
            }
        }
    }
    let confirmed = match &mutation.kind {
        ServerMutationKind::SetUnread(unread) => after.iter().all(|(_, seen, _)| *seen == !*unread),
        ServerMutationKind::SetFlagged(flagged) => {
            after.iter().all(|(_, _, current)| *current == *flagged)
        }
        ServerMutationKind::Move(_) | ServerMutationKind::EmptyMailbox(_) => after.is_empty(),
        _ => false,
    };
    if !confirmed {
        return Err(MailFailure::new(
            MailFailureKind::MutationUncertain,
            "IMAP mutation could not be confirmed after the command",
        ));
    }
    Ok(MutationResult {
        mutation_id: mutation.id,
        disposition: MutationDisposition::Applied,
        missing_target_ids,
    })
}

// One `UID FETCH` per UID, over one connection. libcurl treats a custom
// `UID FETCH` whose set lists several UIDs as a listing and writes none of
// its responses, so a set is never sent.
#[allow(
    clippy::too_many_arguments,
    reason = "Mail operation inputs retain their distinct identities at this boundary."
)]
fn fetch_each_uid(
    easy: &mut Easy,
    config: &AccountConfig,
    secret: &Secret,
    cancel: &AtomicBool,
    mailbox_url: &str,
    uids: &[u32],
    items: &str,
    limit: usize,
) -> MailResult<Vec<u8>> {
    let mut response = Vec::new();
    for uid in uids {
        let remaining = limit
            .checked_sub(response.len())
            .filter(|remaining| *remaining > 0)
            .ok_or_else(|| {
                MailFailure::new(
                    MailFailureKind::ResponseTooLarge,
                    "IMAP page response exceeded configured limit",
                )
            })?;
        let command = format!("UID FETCH {uid} {items}");
        let mut one = perform_imap_on(
            easy,
            config,
            secret,
            cancel,
            mailbox_url,
            Some(&command),
            remaining,
        )?;
        close_trailing_literal(&mut one);
        response.extend(one);
    }
    Ok(response)
}

// libcurl omits the FETCH closing parenthesis after a complete literal.
// Restore it only at an exact literal boundary; missing UID metadata still refuses adoption.
fn close_trailing_literal(response: &mut Vec<u8>) {
    // The literal is the `{n}\r\n` whose `n` bytes end the response. A
    // message body can itself contain `{`, so the last brace is not it.
    let mut at = 0;
    while let Some(offset) = response[at..].iter().position(|byte| *byte == b'{') {
        let open = at + offset;
        at = open + 1;
        let after = &response[open + 1..];
        let Some(close) = after.iter().position(|byte| *byte == b'}') else {
            return;
        };
        let Some(size) = std::str::from_utf8(&after[..close])
            .ok()
            .filter(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|digits| digits.parse::<usize>().ok())
        else {
            continue;
        };
        let literal_start = open + 1 + close + 1 + 2;
        if after.get(close + 1..close + 3) == Some(b"\r\n".as_slice())
            && literal_start.checked_add(size) == Some(response.len())
        {
            response.extend_from_slice(b")\r\n");
            return;
        }
    }
}

fn fetch_uid_flags(
    config: &AccountConfig,
    secret: &Secret,
    cancel: &AtomicBool,
    mailbox_url: &str,
    uids: &[u32],
) -> MailResult<Vec<(u32, bool, bool)>> {
    let response = fetch_each_uid(
        &mut Easy::new(),
        config,
        secret,
        cancel,
        mailbox_url,
        uids,
        "(UID FLAGS)",
        MAX_CONTROL_RESPONSE_BYTES,
    )?;
    parse_uid_flags(&response)
}

fn parse_uid_flags(response: &[u8]) -> MailResult<Vec<(u32, bool, bool)>> {
    let mut found = Vec::new();
    for response in parse_responses(response)? {
        let Response::Fetch(_, attributes) = response else {
            continue;
        };
        let mut uid = None;
        let mut flags = Vec::new();
        for attribute in attributes {
            match attribute {
                AttributeValue::Uid(value) => uid = Some(value),
                AttributeValue::Flags(values) => flags = values,
                _ => {}
            }
        }
        if let Some(uid) = uid {
            let seen = flags.iter().any(|flag| flag.eq_ignore_ascii_case("\\Seen"));
            let flagged = flags
                .iter()
                .any(|flag| flag.eq_ignore_ascii_case("\\Flagged"));
            found.push((uid, seen, flagged));
        }
    }
    Ok(found)
}

fn mutation_uncertain(error: MailFailure) -> MailFailure {
    MailFailure::new(
        MailFailureKind::MutationUncertain,
        format!("{} after IMAP mutation began", error.diagnostic()),
    )
}

// How long one `IDLE` waits before it is renewed: under the 29 minutes RFC
// 2177 allows, and under the ten minutes after which some providers drop a
// silent connection.
const IDLE_RENEWAL: std::time::Duration = std::time::Duration::from_secs(9 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WatchProgress {
    Ready,
    Changed,
}

// IDLE blocks until server activity, renewal or wake.
// Connection/authentication failures and unsupported IDLE return a MailFailure.
pub(crate) fn watch_inbox(
    config: &AccountConfig,
    secret: &Secret,
    cancel: &AtomicBool,
    wake: &std::os::unix::net::UnixStream,
    mut progress: impl FnMut(WatchProgress),
) -> MailResult<()> {
    validate_supported_config(config)?;
    let root_url = imap_endpoint_url(
        &config.incoming.server,
        config.incoming.port,
        config.incoming.encryption,
        "",
    )?;
    let mut session = ImapSession::open(config, secret, cancel, &root_url)?;
    let capabilities = session.run("CAPABILITY", MAX_CONTROL_RESPONSE_BYTES)?;
    let offers_idle = capabilities
        .split(u8::is_ascii_whitespace)
        .any(|word| word.eq_ignore_ascii_case(b"IDLE"));
    if !offers_idle {
        return Err(MailFailure::new(
            MailFailureKind::Unavailable,
            "the IMAP server does not offer IDLE",
        ));
    }
    let mailbox_name = imap_mailbox_name(MailboxKind::Inbox);
    session.run(
        &format!("EXAMINE \"{mailbox_name}\""),
        MAX_CONTROL_RESPONSE_BYTES,
    )?;
    progress(WatchProgress::Ready);
    loop {
        match session.idle(wake, IDLE_RENEWAL)? {
            IdleOutcome::Changed => progress(WatchProgress::Changed),
            IdleOutcome::Quiet => {}
            IdleOutcome::Stopped => return Ok(()),
        }
    }
}
