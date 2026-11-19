use super::*;

#[test]
fn mail_backend_smtp_loopback_reducer_sends_exact_envelope_and_body_before_sent_publication() {
    use enzui_services::credential_store::MemoryCredentialStore;

    let (port, capture, server) = generated_smtp_server(true);
    let local_backend = enzui_iced_core::persist::InMemoryBackend::new();
    let (mut state, _) =
        State::boot_for_test_with_local_state(crate::local_state::store_on(&local_backend), false);
    let credentials = std::sync::Arc::new(MemoryCredentialStore::new());
    let mut config = loopback_account_config("acct-smtp-loopback");
    config.smtp.port = port;
    let outcome = iced_test::futures::futures::executor::block_on(credentials.set(
        State::credential_key(&config),
        Secret::new(b"generated-password".to_vec()),
    ));
    assert_eq!(outcome, CredentialOutcome::Stored);
    state.credential_store = CredentialStoreHandle(credentials);
    state.credential_store_ready = true;
    state.mail_backend =
        std::sync::Arc::new(backend::curl_backend::CurlMailBackend::with_protocols(&[
            "smtp",
        ]));
    state
        .store
        .accounts
        .push(account_from_config(config.clone()));
    state.active_account_id = config.id.clone();
    state.compose.open = true;
    state.compose.from_account_id = config.id;
    state.compose.to = "recipient@example.com".to_owned();
    state.compose.subject = "loopback delivered".to_owned();
    state.compose.body = "generated loopback body".to_owned();

    let draft_id = state.save_compose_draft().expect("local draft");
    let _ = state.update(Message::OpenCompose(ComposeMode::EditDraft(
        draft_id.clone(),
    )));
    let queue = state.update(Message::SendCompose);
    assert_eq!(apply_task(&mut state, queue), 0);
    let sent_before = sent_count(&state);
    assert!(state.pending_send.is_some());
    assert!(state.local_state_is_published());
    assert_eq!(sent_count(&state), sent_before);

    let (deadline, _) = state.undo_send_expiry.armed().expect("undo deadline");
    let delivery = state.update(Message::Toast(ToastEvent::Tick(deadline)));
    assert!(state.pending_send.is_some());
    assert_eq!(sent_count(&state), sent_before);
    assert_eq!(apply_task(&mut state, delivery), 0);

    let capture = capture
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("bounded SMTP capture");
    server.join().expect("SMTP fixture exits");
    assert_eq!(
        capture
            .commands
            .iter()
            .filter(|line| line.to_ascii_uppercase().starts_with("MAIL FROM:"))
            .count(),
        1
    );
    assert!(capture.commands.iter().any(|line| {
        line.to_ascii_uppercase()
            .starts_with("MAIL FROM:<SENDER@EXAMPLE.COM>")
    }));
    assert_eq!(
        capture
            .commands
            .iter()
            .filter(|line| line.to_ascii_uppercase().starts_with("RCPT TO:"))
            .count(),
        1
    );
    assert!(capture.commands.iter().any(|line| {
        line.to_ascii_uppercase()
            .starts_with("RCPT TO:<RECIPIENT@EXAMPLE.COM>")
    }));
    assert!(capture.data.len() <= backend::curl_backend::MAX_SMTP_MESSAGE_BYTES);
    let data = String::from_utf8(capture.data).expect("generated MIME is UTF-8");
    assert!(data.contains("To: recipient@example.com\r\n"));
    assert!(data.contains("Subject: loopback delivered\r\n"));
    assert!(
        data.contains(
            "Content-Transfer-Encoding: 8bit\r\n\r\ngenerated loopback body\r\n--mail-app-"
        )
    );
    assert!(state.pending_send.is_none());
    assert!(state.local_state_is_published());
    assert_eq!(sent_count(&state), sent_before + 1);
    assert!(state.mail_notice.as_ref().is_some_and(|notice| {
        notice.title == "Message sent" && notice.body.contains("SMTP server accepted")
    }));
    assert!(state.store.message(&draft_id).is_none());
}

#[test]
fn imap_mutation_captures_source_identity_before_optimistic_move_and_waits_for_save() {
    let mut state = production_state_for_mail_backend();
    let backend = std::sync::Arc::new(SuccessfulMailBackend::default());
    state.mail_backend =
        std::sync::Arc::<crate::reducer_tests::SuccessfulMailBackend>::clone(&backend);
    let config = loopback_account_config("acct-mutation");
    state
        .store
        .accounts
        .push(account_from_config(config.clone()));
    let mut message = model::fixture::seed()
        .messages
        .into_iter()
        .next()
        .expect("generated message");
    message.id = "imap-acct-mutation-archive-71-9".to_owned();
    message.account_id = config.id.clone();
    message.location = MessageLocation::Mailbox(MailboxKind::Archive);
    state.store.messages.push(message.clone());
    state.store.mark_dirty();

    state.perform_move(&[message.id.clone()], MailboxKind::Trash);

    assert_eq!(state.pending_server_mutations.len(), 1);
    let pending = &state.pending_server_mutations[0];
    assert_eq!(pending.target_ids, vec![message.id.clone()]);
    assert_eq!(pending.server_targets.len(), 1);
    assert_eq!(pending.server_targets[0].account_id, config.id);
    assert_eq!(pending.server_targets[0].mailbox, MailboxKind::Archive);
    assert_eq!(pending.server_targets[0].uid_validity, 71);
    assert_eq!(pending.server_targets[0].uid, 9);
    assert!(state.store.message(&message.id).is_some_and(|current| {
        current.location == MessageLocation::Mailbox(MailboxKind::Trash)
    }));
    assert!(!state.local_state_is_published());
    drop(state.start_next_mail_operation());
    assert!(state.mail_intent.is_none());
    assert!(backend.calls.lock().expect("mail calls lock").is_empty());
}
