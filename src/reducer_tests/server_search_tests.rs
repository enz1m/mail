use super::*;

fn imap_state() -> (
    State,
    std::sync::Arc<SuccessfulMailBackend>,
    backend::AccountConfig,
) {
    let local_backend = enzui_iced_core::persist::InMemoryBackend::new();
    let (mut state, _) =
        State::boot_for_test_with_local_state(crate::local_state::store_on(&local_backend), false);
    let credentials =
        std::sync::Arc::new(enzui_services::credential_store::MemoryCredentialStore::new());
    let backend = std::sync::Arc::new(SuccessfulMailBackend::default());
    let config = loopback_account_config("acct-find");
    let outcome = iced_test::futures::futures::executor::block_on(credentials.set(
        State::credential_key(&config),
        Secret::new(b"generated-password".to_vec()),
    ));
    assert_eq!(outcome, CredentialOutcome::Stored);
    state.credential_store = CredentialStoreHandle(credentials);
    state.credential_store_ready = true;
    state.mail_backend =
        std::sync::Arc::<crate::reducer_tests::SuccessfulMailBackend>::clone(&backend);
    state
        .store
        .accounts
        .push(account_from_config(config.clone()));
    state.active_account_id = config.id.clone();
    state.selection = SidebarSelection::AccountMailbox {
        account_id: config.id.clone(),
        mailbox: MailboxKind::Inbox,
    };
    (state, backend, config)
}

fn old_message(config: &backend::AccountConfig, uid: u32, subject: &str) -> model::types::Message {
    let mut message = model::fixture::seed().messages[0].clone();
    message.id = format!("imap-{}-77-{uid}", config.id);
    message.account_id = config.id.clone();
    message.location = MessageLocation::Mailbox(MailboxKind::Inbox);
    message.subject = subject.to_owned();
    message.attachments.clear();
    message.has_attachment = false;
    message
}

#[test]
fn an_answer_for_an_edited_search_is_dropped() {
    let (mut state, backend, config) = imap_state();
    backend
        .search_batches
        .lock()
        .expect("search batches lock")
        .push_back(backend::curl_backend::SyncBatch {
            older_than_uid: None,
            retained: Vec::new(),
            account_id: config.id.clone(),
            mailbox: MailboxKind::Inbox,
            uid_validity: 77,
            sync_generation: 0,
            coverage: backend::curl_backend::SyncCoverage::Complete,
            messages: vec![fetched_message(old_message(&config, 4, "Stale"), 0)],
        });
    let _ = state.update(Message::SearchEdited("stale".to_owned()));
    let task = state.update(Message::SearchServer);
    state.search_query = "something else".to_owned();
    let _ = apply_task(&mut state, task);
    assert!(
        state
            .store
            .message(&format!("imap-{}-77-4", config.id))
            .is_none()
    );
}
