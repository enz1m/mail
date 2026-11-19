//! Sync engine: second sync, snapshots, attachment completion, mutation
//! identity, refusals, and refresh coalescing.

use super::*;

#[test]
fn complete_mailbox_snapshot_durably_retires_only_absent_exact_scope_rows() {
    let local_backend = enzui_iced_core::persist::InMemoryBackend::new();
    let (mut state, _) =
        State::boot_for_test_with_local_state(crate::local_state::store_on(&local_backend), false);
    let config = loopback_account_config("acct-sync");
    let mut account = account_from_config(config.clone());
    account.config = None;
    let mut other = account.clone();
    other.id = "other".to_owned();
    other.email = "other@example.com".to_owned();
    state.store.accounts.extend([account, other]);
    state.active_account_id = config.id.clone();

    let template = model::fixture::seed()
        .messages
        .into_iter()
        .next()
        .expect("generated message");
    let mut present = template;
    present.id = "imap-acct-sync-77-1".to_owned();
    present.account_id = config.id.clone();
    present.location = MessageLocation::Mailbox(MailboxKind::Inbox);
    present.subject = "last-good subject".to_owned();
    present.unread = false;
    present.snoozed_until = Some(MessageDate::at(2026, 10, 30, 8, 0));
    present.send_at = Some(MessageDate::at(2026, 10, 31, 9, 0));
    let mut deleted = present.clone();
    deleted.id = "imap-acct-sync-77-2".to_owned();
    let mut stale_generation = present.clone();
    stale_generation.id = "imap-acct-sync-66-8".to_owned();
    let mut other_mailbox = present.clone();
    other_mailbox.id = "imap-acct-sync-archive-77-10".to_owned();
    other_mailbox.location = MessageLocation::Mailbox(MailboxKind::Archive);
    let mut local_only = present.clone();
    local_only.id = "local-import".to_owned();
    let mut other_account = present.clone();
    other_account.id = "imap-other-77-9".to_owned();
    other_account.account_id = "other".to_owned();
    state.store.messages = vec![
        present.clone(),
        deleted.clone(),
        stale_generation.clone(),
        local_only.clone(),
        other_account.clone(),
        other_mailbox.clone(),
    ];
    state.store.mark_dirty();

    let mut refreshed = present.clone();
    refreshed.subject = "server subject".to_owned();
    refreshed.unread = true;
    refreshed.snoozed_until = None;
    refreshed.send_at = None;
    let task = state.stage_sync_adoption(
        41,
        config,
        backend::curl_backend::SyncBatch {
            older_than_uid: None,
            retained: Vec::new(),
            account_id: "acct-sync".to_owned(),
            mailbox: MailboxKind::Inbox,
            uid_validity: 77,
            sync_generation: 41,
            coverage: backend::curl_backend::SyncCoverage::Complete,
            messages: vec![fetched_message(refreshed, 41)],
        },
    );

    assert_eq!(
        state
            .store
            .message(&present.id)
            .map(|message| message.subject.as_str()),
        Some("last-good subject")
    );
    assert!(state.store.message(&deleted.id).is_some());
    assert_eq!(apply_task(&mut state, task), 0);
    assert!(state.store.message(&deleted.id).is_none());
    assert!(state.store.message(&stale_generation.id).is_none());
    assert!(state.store.message(&local_only.id).is_some());
    assert!(state.store.message(&other_account.id).is_some());
    assert!(state.store.message(&other_mailbox.id).is_some());
    assert!(
        state
            .store
            .message(&present.id)
            .is_some_and(|message| { message.subject == "server subject" && message.unread })
    );
    let saved = state.store.message(&present.id).unwrap();
    assert_eq!(saved.snoozed_until, present.snoozed_until);
    assert_eq!(saved.send_at, present.send_at);
    assert!(state.local_state_is_published());

    let (restarted, _) =
        State::boot_for_test_with_local_state(crate::local_state::store_on(&local_backend), false);
    assert!(restarted.store.message(&deleted.id).is_none());
    assert!(restarted.store.message(&local_only.id).is_some());
    assert!(restarted.store.message(&other_account.id).is_some());
    assert!(restarted.store.message(&other_mailbox.id).is_some());
}

#[test]
fn stale_attachment_completion_cannot_attach_bytes_to_a_reused_uid_row() {
    use crate::link_launcher::tests::SharedMockBackend;

    let fixture =
        std::env::temp_dir().join(format!("mail-app-content-authority-{}", std::process::id()));
    let moved = fixture.with_extension("moved");
    let export = fixture.with_extension("export");
    let _ = std::fs::remove_dir_all(&fixture);
    let _ = std::fs::remove_dir_all(&moved);
    let _ = std::fs::remove_file(&export);
    let store = crate::attachment_store::AttachmentStore::for_root(&fixture)
        .expect("retained attachment root");
    let attachment = store
        .stage_inbound("report.txt", "text/plain", b"generation 72")
        .expect("stage generated payload");

    let mut state = production_state_for_mail_backend();
    state.attachment_store = store.clone();
    let mut message = model::fixture::seed()
        .messages
        .into_iter()
        .next()
        .expect("generated message");
    message.id = "imap-acct-stale-77-9".to_owned();
    message.account_id = "acct-stale".to_owned();
    message.location = MessageLocation::Mailbox(MailboxKind::Inbox);
    message.attachments = vec![attachment.clone()];
    message.has_attachment = true;
    let mut fetched = fetched_message(message, 72);
    fetched.attachment_completions[0].identity.sync_generation = 71;

    let result = state.complete_sync_candidate(
        72,
        backend::curl_backend::SyncBatch {
            older_than_uid: None,
            retained: Vec::new(),
            account_id: "acct-stale".to_owned(),
            mailbox: MailboxKind::Inbox,
            uid_validity: 77,
            sync_generation: 72,
            coverage: backend::curl_backend::SyncCoverage::Complete,
            messages: vec![fetched],
        },
    );
    assert_eq!(
        result.unwrap_err(),
        "a MIME completion crossed its exact message or part identity"
    );
    assert!(state.store.message("imap-acct-stale-77-9").is_none());

    let payload = attachment.payload_ref().expect("payload authority");
    let held_attachment = store
        .stage_inbound("held.txt", "text/plain", b"held inode")
        .expect("stage held payload");
    let handoff_backend = SharedMockBackend::new();
    state.attachment_opener =
        crate::link_launcher::LinkOpener::with_backend(handoff_backend.boxed());
    let open = state.update(Message::OpenAttachment(attachment.clone()));
    assert_eq!(apply_task(&mut state, open), 0);
    let retained_identity = state
        .attachment_open_held
        .as_ref()
        .expect("held attachment descriptor")
        .retained_file()
        .expect("validated descriptor handoff")
        .identity();
    let (request_id, backend_identity) = {
        let backend = handoff_backend.lock();
        (backend.started_files[0].0, backend.started_files[0].1)
    };
    assert_eq!(backend_identity, retained_identity);
    std::fs::remove_file(fixture.join(payload.id.file_name())).expect("remove original payload");
    std::fs::write(fixture.join(payload.id.file_name()), b"replacement")
        .expect("plant same-name replacement");
    let replacement_identity = enzui_iced_core::portal::open_uri::RetainedFile::new(
        std::fs::File::open(fixture.join(payload.id.file_name()))
            .expect("open replacement fixture"),
    )
    .expect("retain replacement fixture")
    .identity();
    assert_ne!(retained_identity, replacement_identity);
    assert!(matches!(
        state
            .attachment_open_held
            .as_ref()
            .expect("held attachment descriptor")
            .verify_path(),
        Err(crate::attachment_store::AttachmentFailure::PayloadChanged)
    ));
    handoff_backend.script_and_notify(
        request_id,
        enzui_iced_core::portal::open_uri::OpenUriReply::Success,
    );
    let wake = state.attachment_opener.live_wake();
    let terminal = state.update(Message::AttachmentOpenPortalTick(wake));
    assert_eq!(apply_task(&mut state, terminal), 0);
    assert!(state.attachment_open_held.is_none());
    assert!(store.has_cleanup_debt());
    assert!(
        state
            .attachment_notice
            .as_deref()
            .is_some_and(|notice| notice.contains("Attachment maintenance is required"))
    );
    assert_eq!(
        store.export(&attachment, &export),
        Err(crate::attachment_store::AttachmentFailure::PayloadChanged)
    );
    assert!(matches!(
        store.open(&attachment),
        Err(crate::attachment_store::AttachmentFailure::PayloadChanged)
    ));
    let held_payload = held_attachment
        .payload_ref()
        .expect("held payload authority");
    std::fs::remove_file(fixture.join(payload.id.file_name())).expect("remove substituted payload");
    store
        .cleanup_unreferenced(&std::collections::HashMap::from([(
            held_payload.id.clone(),
            held_payload.identity,
        )]))
        .expect("complete bounded cleanup census");
    assert!(!store.has_cleanup_debt());
    let resumed = store
        .stage_inbound("resumed.txt", "text/plain", b"resumed")
        .expect("resume writes after complete census");
    store
        .remove_payloads(&[resumed.payload_ref().expect("resumed payload")])
        .expect("remove resumed payload");

    let opened = store.open(&held_attachment).expect("hold exact descriptor");
    std::fs::rename(&fixture, &moved).expect("move retained root");
    std::fs::create_dir(&fixture).expect("plant replacement root");
    std::fs::write(fixture.join(held_payload.id.file_name()), b"redirected")
        .expect("plant replacement-root payload");
    assert_eq!(store.read(&held_attachment).unwrap(), b"held inode");
    assert!(matches!(
        opened.retained_file(),
        Err(crate::attachment_store::AttachmentFailure::OpenUncertain)
    ));

    let _ = std::fs::remove_dir_all(&fixture);
    let _ = std::fs::remove_dir_all(&moved);
    let _ = std::fs::remove_file(&export);
}

#[test]
fn pending_mutation_identity_survives_complete_snapshot_merge() {
    let mut state = production_state_for_mail_backend();
    let template = model::fixture::seed()
        .messages
        .into_iter()
        .next()
        .expect("generated message");
    let mut moved = template;
    moved.id = "imap-acct-overlap-77-3".to_owned();
    moved.account_id = "acct-overlap".to_owned();
    moved.location = MessageLocation::Mailbox(MailboxKind::Archive);
    moved.subject = "pending local move".to_owned();
    let mut flagged = moved.clone();
    flagged.id = "imap-acct-overlap-77-4".to_owned();
    flagged.location = MessageLocation::Mailbox(MailboxKind::Inbox);
    flagged.subject = "pending local flag".to_owned();
    flagged.flagged = true;
    state
        .store
        .messages
        .extend([moved.clone(), flagged.clone()]);
    state.pending_server_mutations.extend([
        crate::local_state::PendingServerMutation {
            id: 1,
            kind: crate::local_state::ServerMutationKind::Move(MailboxKind::Archive),
            target_ids: vec![moved.id.clone()],
            server_targets: vec![crate::local_state::ServerMutationTarget {
                message_id: moved.id.clone(),
                account_id: "acct-overlap".to_owned(),
                mailbox: MailboxKind::Inbox,
                uid_validity: 77,
                uid: 3,
            }],
        },
        crate::local_state::PendingServerMutation {
            id: 2,
            kind: crate::local_state::ServerMutationKind::SetFlagged(true),
            target_ids: vec![flagged.id.clone()],
            server_targets: vec![crate::local_state::ServerMutationTarget {
                message_id: flagged.id.clone(),
                account_id: "acct-overlap".to_owned(),
                mailbox: MailboxKind::Inbox,
                uid_validity: 77,
                uid: 4,
            }],
        },
    ]);
    let mut stale_move = moved.clone();
    stale_move.location = MessageLocation::Mailbox(MailboxKind::Inbox);
    stale_move.subject = "stale server location".to_owned();
    let mut stale_flags = flagged.clone();
    stale_flags.subject = "stale server flags".to_owned();
    stale_flags.flagged = false;

    let candidate = state
        .complete_sync_candidate(
            43,
            backend::curl_backend::SyncBatch {
                older_than_uid: None,
                retained: Vec::new(),
                account_id: "acct-overlap".to_owned(),
                mailbox: MailboxKind::Inbox,
                uid_validity: 77,
                sync_generation: 43,
                coverage: backend::curl_backend::SyncCoverage::Complete,
                messages: vec![
                    fetched_message(stale_move, 43),
                    fetched_message(stale_flags, 43),
                ],
            },
        )
        .expect("complete exact snapshot builds a candidate");

    assert!(candidate.message(&moved.id).is_some_and(|message| {
        message.location == MessageLocation::Mailbox(MailboxKind::Archive)
            && message.subject == "pending local move"
    }));
    assert!(
        candidate
            .message(&flagged.id)
            .is_some_and(|message| { message.subject == "pending local flag" && message.flagged })
    );
    assert_eq!(
        candidate
            .messages
            .iter()
            .filter(|message| message.id == moved.id)
            .count(),
        1
    );
}

#[test]
fn capped_mailbox_response_never_authorizes_absence_or_replacement() {
    let mut state = production_state_for_mail_backend();
    let config = loopback_account_config("acct-partial");
    let mut current = model::fixture::seed()
        .messages
        .into_iter()
        .next()
        .expect("generated message");
    current.id = "imap-acct-partial-9-1".to_owned();
    current.account_id = config.id.clone();
    current.subject = "last-good subject".to_owned();
    state.store.messages.push(current.clone());
    state.store.mark_dirty();
    let before = state.local_state_token();
    let task = state.stage_sync_adoption(
        42,
        config,
        backend::curl_backend::SyncBatch {
            older_than_uid: None,
            retained: Vec::new(),
            account_id: "acct-partial".to_owned(),
            mailbox: MailboxKind::Inbox,
            uid_validity: 9,
            sync_generation: 42,
            coverage: backend::curl_backend::SyncCoverage::Capped {
                completed: 0,
                total: backend::curl_backend::MAX_MESSAGES_PER_SYNC + 1,
            },
            messages: Vec::new(),
        },
    );

    drop(task);
    assert!(state.pending_sync_adoption.is_none());
    assert_eq!(state.local_state_token(), before);
    assert_eq!(
        state
            .store
            .message(&current.id)
            .map(|message| message.subject.as_str()),
        Some("last-good subject")
    );
    assert!(state.mail_notice.as_ref().is_some_and(|notice| {
        notice.title == "Mailbox synchronization incomplete"
            && notice
                .body
                .contains("No local messages were replaced or removed")
    }));
}

#[test]
fn recent_window_sync_preserves_older_cached_mail_but_not_old_uidvalidity() {
    let mut state = production_state_for_mail_backend();
    let mut message = model::fixture::seed().messages.remove(0);
    message.account_id = "acct-window".into();
    message.location = MessageLocation::Mailbox(MailboxKind::Inbox);
    message.id = "imap-acct-window-7-2".into();
    let mut stale = message.clone();
    stale.id = "imap-acct-window-6-2".into();
    let mut deleted = message.clone();
    deleted.id = "imap-acct-window-7-205".into();
    state.store.messages = vec![message, stale, deleted];
    let candidate = state
        .complete_sync_candidate(
            9,
            backend::curl_backend::SyncBatch {
                older_than_uid: Some(200),
                retained: vec![backend::curl_backend::RetainedMessage {
                    uid: 200,
                    seen: true,
                    flagged: false,
                }],
                account_id: "acct-window".into(),
                mailbox: MailboxKind::Inbox,
                uid_validity: 7,
                sync_generation: 9,
                coverage: backend::curl_backend::SyncCoverage::Complete,
                messages: Vec::new(),
            },
        )
        .unwrap();
    assert!(candidate.message("imap-acct-window-7-2").is_some());
    assert!(candidate.message("imap-acct-window-6-2").is_none());
    assert!(candidate.message("imap-acct-window-7-205").is_none());
}
