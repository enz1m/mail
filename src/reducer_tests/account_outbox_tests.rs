//! Outbox submission: removal only after SMTP acceptance and durable
//! retry.

use super::*;

#[test]
fn mail_backend_failed_submission_retains_durable_outbox_for_retry() {
    let mut state = production_state_for_mail_backend();
    let credentials =
        std::sync::Arc::new(enzui_services::credential_store::MemoryCredentialStore::new());
    let backend = std::sync::Arc::new(RetryMailBackend::default());
    let config = loopback_account_config("acct-retry");
    let outcome = iced_test::futures::futures::executor::block_on(credentials.set(
        State::credential_key(&config),
        Secret::new(b"generated-password".to_vec()),
    ));
    assert_eq!(outcome, CredentialOutcome::Stored);
    state.credential_store = CredentialStoreHandle(credentials);
    state.credential_store_ready = true;
    state.mail_backend = std::sync::Arc::<crate::reducer_tests::RetryMailBackend>::clone(&backend);
    state.store.accounts.push(account_from_config(config));
    state.active_account_id = "acct-retry".to_owned();
    state.compose.open = true;
    state.compose.from_account_id = "acct-retry".to_owned();
    state.compose.to = "recipient@example.com".to_owned();
    state.compose.subject = "retry me".to_owned();
    state.compose.body = "generated body".to_owned();

    let queue = state.update(Message::SendCompose);
    apply_task(&mut state, queue);
    let toast = state.pending_undo_toast.expect("undo toast");
    let first_attempt = state.update(Message::Toast(ToastEvent::Action(
        toast.id,
        SEND_NOW_ACTION.into(),
    )));
    apply_task(&mut state, first_attempt);
    assert!(state.pending_send.is_some());
    assert!(state.mail_retry.is_some());
    assert!(
        state
            .mail_notice
            .as_ref()
            .is_some_and(|notice| notice.retryable)
    );

    let retry = state.update(Message::RetryMailOperation);
    apply_task(&mut state, retry);
    assert!(state.pending_send.is_none());
    assert_eq!(
        backend.submits.load(std::sync::atomic::Ordering::Relaxed),
        2
    );
}

#[test]
fn mail_backend_post_data_cancel_retains_uncertain_outbox_and_stable_message_id() {
    let mut state = production_state_for_mail_backend();
    let config = loopback_account_config("acct-uncertain");
    state
        .store
        .accounts
        .push(account_from_config(config.clone()));
    state.active_account_id = config.id.clone();
    state.compose.open = true;
    state.compose.from_account_id = config.id.clone();
    state.compose.to = "recipient@example.com".to_owned();
    state.compose.subject = "uncertain delivery".to_owned();
    state.compose.body = "generated body".to_owned();
    assert!(state.send_compose());

    let pending_id = state
        .pending_send
        .as_ref()
        .expect("durable Outbox item")
        .id
        .clone();
    let first_mime = state
        .attachment_store
        .outgoing_mime_bytes(state.pending_send.as_ref().expect("pending send"))
        .expect("first retry MIME");
    let message_ids = |mime: &[u8]| {
        mime.split(|byte| *byte == b'\n')
            .filter(|line| line.starts_with(b"Message-ID: "))
            .map(<[u8]>::to_vec)
            .collect::<Vec<_>>()
    };
    let generation = 91;
    let operation = backend::curl_backend::ready_operation(
        generation,
        Err(backend::curl_backend::MailFailure::for_test(
            backend::curl_backend::MailFailureKind::DeliveryUncertain,
        )),
    );
    state.mail_generation = generation;
    state.mail_intent = Some(MailIntent::SubmitOutbox {
        config,
        pending_id: pending_id.clone(),
    });
    state.mail_cancel = Some((generation, operation.cancel_handle()));
    state.mail_discard_generation = Some(generation);

    drop(state.complete_mail_operation(
        generation,
        Err(backend::curl_backend::MailFailure::for_test(
            backend::curl_backend::MailFailureKind::DeliveryUncertain,
        )),
    ));

    assert!(state.pending_send.as_ref().is_some_and(|pending| {
        pending.id == pending_id && pending.subject == "uncertain delivery"
    }));
    assert!(!state.compose.open);
    assert!(matches!(
        state.mail_retry.as_ref(),
        Some(MailIntent::SubmitOutbox { pending_id: retry_id, .. }) if retry_id == &pending_id
    ));
    assert!(state.mail_notice.as_ref().is_some_and(|notice| {
        notice.title == "Delivery not confirmed"
            && notice.body.contains("Server acceptance is unknown")
            && notice.retryable
            && notice.can_cancel_outbox
    }));
    let retry_mime = state
        .attachment_store
        .outgoing_mime_bytes(state.pending_send.as_ref().expect("retained Outbox item"))
        .expect("retry MIME");
    let first_ids = message_ids(&first_mime);
    let retry_ids = message_ids(&retry_mime);
    assert_eq!(first_ids.len(), 1);
    assert_eq!(retry_ids, first_ids);
}

#[test]
fn mail_backend_cancelled_failed_outbox_returns_to_draft_without_sent_copy() {
    let mut state = production_state_for_mail_backend();
    let credentials =
        std::sync::Arc::new(enzui_services::credential_store::MemoryCredentialStore::new());
    let backend = std::sync::Arc::new(RetryMailBackend::default());
    let config = loopback_account_config("acct-cancel");
    let outcome = iced_test::futures::futures::executor::block_on(credentials.set(
        State::credential_key(&config),
        Secret::new(b"generated-password".to_vec()),
    ));
    assert_eq!(outcome, CredentialOutcome::Stored);
    state.credential_store = CredentialStoreHandle(credentials);
    state.credential_store_ready = true;
    state.mail_backend = backend;
    state.store.accounts.push(account_from_config(config));
    state.active_account_id = "acct-cancel".to_owned();
    state.compose.open = true;
    state.compose.from_account_id = "acct-cancel".to_owned();
    state.compose.to = "recipient@example.com".to_owned();
    state.compose.subject = "cancel me".to_owned();

    let queue = state.update(Message::SendCompose);
    assert_eq!(apply_task(&mut state, queue), 0);
    let sent_before = sent_count(&state);
    assert!(state.pending_send.is_some());

    let (deadline, _) = state.undo_send_expiry.armed().expect("undo deadline");
    let first_attempt = state.update(Message::Toast(ToastEvent::Tick(deadline)));
    let credential_tasks = apply_task_wave(&mut state, first_attempt);
    assert_eq!(credential_tasks.len(), 1);
    let mut backend_tasks = Vec::new();
    for task in credential_tasks {
        backend_tasks.extend(apply_task_wave(&mut state, task));
    }
    assert_eq!(backend_tasks.len(), 1);
    let active_generation = state.mail_generation;
    assert!(state.mail_cancel.is_some());

    let stale = state.update(Message::MailOperationFinished(
        active_generation.wrapping_sub(1),
        Ok(backend::curl_backend::MailOperationOutput::Submitted),
    ));
    assert_eq!(apply_task(&mut state, stale), 0);
    assert!(state.pending_send.is_some());
    assert_eq!(sent_count(&state), sent_before);

    let cancel = state.update(Message::CancelMailOperation);
    assert_eq!(apply_task(&mut state, cancel), 0);
    for task in backend_tasks {
        assert_eq!(apply_task(&mut state, task), 0);
    }
    assert!(state.pending_send.is_none());
    assert!(state.compose.open);
    assert_eq!(state.compose.subject, "cancel me");
    assert!(state.store.messages.iter().any(|message| {
        message.subject == "cancel me"
            && matches!(
                &message.location,
                MessageLocation::Mailbox(MailboxKind::Drafts)
            )
    }));
    assert!(state.local_state_is_published());
    assert_eq!(sent_count(&state), sent_before);

    let repeated_cancel = state.update(Message::CancelMailOperation);
    assert_eq!(apply_task(&mut state, repeated_cancel), 0);
    assert!(state.pending_send.is_none());
    assert_eq!(sent_count(&state), sent_before);

    let close = state.update(Message::WindowCloseResolved(Some(
        iced::window::Id::unique(),
    )));
    assert_eq!(apply_close_task(&mut state, close), 1);
    assert!(state.close_committed);
}
