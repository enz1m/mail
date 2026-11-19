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
    let config = loopback_account_config("acct-push");
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
    (state, backend, config)
}

fn inbox_message(
    config: &backend::AccountConfig,
    uid: u32,
    sender: &str,
    subject: &str,
    unread: bool,
) -> model::types::Message {
    let mut message = model::fixture::seed().messages[0].clone();
    message.id = format!("imap-{}-77-{uid}", config.id);
    message.account_id = config.id.clone();
    message.location = MessageLocation::Mailbox(MailboxKind::Inbox);
    message.sender_name = sender.to_owned();
    message.subject = subject.to_owned();
    message.unread = unread;
    message.in_reply_to = None;
    message.references.clear();
    message.attachments.clear();
    message.has_attachment = false;
    message.date.day = message.date.day.saturating_add(uid % 3);
    message
}

fn sync_inbox(
    state: &mut State,
    backend: &SuccessfulMailBackend,
    config: &backend::AccountConfig,
    messages: Vec<model::types::Message>,
) {
    backend
        .sync_batches
        .lock()
        .expect("sync batches lock")
        .push_back(backend::curl_backend::SyncBatch {
            older_than_uid: None,
            retained: Vec::new(),
            account_id: config.id.clone(),
            mailbox: MailboxKind::Inbox,
            uid_validity: 77,
            sync_generation: 0,
            coverage: backend::curl_backend::SyncCoverage::Complete,
            messages: messages
                .into_iter()
                .map(|message| fetched_message(message, 0))
                .collect(),
        });
    let task = state.queue_inbox_sync(config.clone());
    let _ = apply_task(state, task);
    assert!(state.pending_sync_adoption.is_none());
}

#[test]
fn watch_events_sync_only_from_the_current_watch() {
    let (mut state, backend, config) = imap_state();

    let mut unreachable = config.clone();
    unreachable.incoming.port = 1;
    let watch = backend::inbox_watch::InboxWatch::spawn(
        unreachable,
        Secret::new(b"generated-password".to_vec()),
    )
    .expect("spawn a watch");
    let watch_id = watch.id();
    state.inbox_watches.insert(config.id.clone(), watch);
    let syncs = |backend: &SuccessfulMailBackend| {
        backend
            .calls
            .lock()
            .expect("calls")
            .iter()
            .filter(|call| **call == "sync")
            .count()
    };
    let update = |watch_id, event| {
        Message::InboxWatch(backend::inbox_watch::InboxWatchUpdate {
            account_id: config.id.clone(),
            watch_id,
            event,
        })
    };

    let task = state.update(update(
        watch_id + 1000,
        backend::inbox_watch::InboxWatchEvent::Changed,
    ));
    let _ = apply_task(&mut state, task);
    assert_eq!(syncs(&backend), 0);

    let task = state.update(update(
        watch_id,
        backend::inbox_watch::InboxWatchEvent::Changed,
    ));
    let _ = apply_task(&mut state, task);
    assert_eq!(syncs(&backend), 1);

    let failure = backend::curl_backend::MailFailure::new(
        backend::curl_backend::MailFailureKind::Unavailable,
        "no IDLE",
    );
    let _ = state.update(update(
        watch_id,
        backend::inbox_watch::InboxWatchEvent::Ended(failure),
    ));
    assert!(state.inbox_watches.is_empty());
    assert_eq!(
        state.inbox_watch_stopped.get(&config.id),
        Some(&backend::curl_backend::MailFailureKind::Unavailable)
    );
}

#[test]
fn notification_policy_keeps_accounts_private_and_queues_exact_messages_behind_composition() {
    use crate::notification_settings::{Change, MailboxTarget, Preview};
    let (mut state, backend, config) = imap_state();
    let old = inbox_message(&config, 1, "Old Friend", "Old mail", false);
    sync_inbox(&mut state, &backend, &config, vec![old.clone()]);
    assert!(state.new_mail.newest().is_none());
    let target = MailboxTarget {
        account_id: config.id.clone(),
        location: MessageLocation::Mailbox(MailboxKind::Inbox),
    };
    let task = state.update(Message::SetNotificationSettings(Change::Mailbox(
        target.clone(),
        Some(false),
    )));
    let _ = apply_task(&mut state, task);
    let arrived = inbox_message(&config, 2, "Private Sender", "Private subject", true);
    sync_inbox(
        &mut state,
        &backend,
        &config,
        vec![old.clone(), arrived.clone()],
    );
    assert!(state.new_mail.newest().is_none());
    let other = MailboxTarget {
        account_id: "another-account".into(),
        location: target.location.clone(),
    };
    assert!(state.notifications.get().allows(&other));
    let task = state.update(Message::SetNotificationSettings(Change::Mailbox(
        target, None,
    )));
    let _ = apply_task(&mut state, task);
    let newest = inbox_message(&config, 3, "Newest Sender", "Newest subject", true);
    sync_inbox(
        &mut state,
        &backend,
        &config,
        vec![old, arrived.clone(), newest.clone()],
    );
    assert_eq!(state.new_mail.newest(), Some(newest.id.as_str()));
    let private =
        crate::new_mail_notify::new_mail_notification(&[&newest], Preview::Private).unwrap();
    assert!(!private.plain_body().unwrap().contains(&newest.subject));
    assert!(!private.title().contains(&newest.sender_name));
    let _ = state.update(Message::OpenCompose(ComposeMode::New));
    state.compose.body = "Keep my draft".into();
    state.desktop.queue_notification_target(arrived.id.clone());
    let _ = state.desktop_pending();
    assert_eq!(state.compose.body, "Keep my draft");
    assert!(state.pending_send.is_none());
    let _ = state.update(Message::CloseCompose);
    let _ = state.desktop_pending();
    assert!(
        state
            .message_ids_of(state.selected_conversation.as_ref().unwrap())
            .contains(&arrived.id)
    );
    let task = state.update(Message::SetNotificationSettings(Change::Enabled(false)));
    let _ = apply_task(&mut state, task);
    assert!(!state.notifications.get().allows(&other));
    assert!(state.new_mail.newest().is_none());
    let task = state.update(Message::SetNotificationSettings(Change::Mailbox(
        other.clone(),
        Some(true),
    )));
    let _ = apply_task(&mut state, task);
    assert!(!state.notifications.get().overrides.contains_key(&other));
}
