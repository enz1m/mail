use super::*;

fn sent_with_attachment<'a>(s: &'a State, name: &str) -> Option<&'a model::types::Message> {
    s.store.messages.iter().find(|m| {
        matches!(m.location, MessageLocation::Mailbox(MailboxKind::Sent))
            && m.attachments.iter().any(|a| a.name == name)
    })
}

#[test]
fn attaching_a_file_reaches_the_outgoing_message() {
    use crate::picker::tests::ScriptedBackend;
    use enzui_iced_core::persist::{BackendLoad, InMemoryBackend, PersistBackend};
    use enzui_iced_widgets::file_chooser::PortalOperation;

    let local_backend = InMemoryBackend::new();
    let (mut s, _) =
        State::boot_for_test_with_local_state(crate::local_state::store_on(&local_backend), true);
    let _ = s.update(Message::OpenCompose(ComposeMode::New));
    let _ = s.update(Message::ComposeToEdited("dest@example.com".into()));

    let fixture_dir = std::env::temp_dir().join(format!(
        "mail-app-attachment-reducer-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&fixture_dir);
    std::fs::create_dir_all(&fixture_dir).expect("create attachment fixture directory");
    let source = fixture_dir.join("report.pdf");
    std::fs::write(&source, b"durable attachment bytes").expect("write attachment fixture");
    let source_text = source.to_string_lossy().into_owned();
    let source_uri = enzui_iced_core::portal::open_uri::OpenUri::from_file_path(&source)
        .expect("fixture file URI")
        .as_str()
        .to_owned();
    let backend = ScriptedBackend::completed(vec![source_uri]);
    let log = backend.log();
    s.attach_chooser = crate::picker::Picker::with_backend(Box::new(backend));

    let _ = s.update(Message::AttachFile);
    let task = s.update(Message::AttachPortalTick);
    let _ = apply_task(&mut s, task);
    std::fs::remove_dir_all(fixture_dir).expect("remove attachment fixture source");

    {
        let log = log.lock().unwrap();
        assert_eq!(log.started, vec![PortalOperation::OpenFile]);
    }

    assert_eq!(s.compose.attachments.len(), 1);
    assert_eq!(s.compose.attachments[0].name, "report.pdf");
    assert!(s.compose.attachments[0].payload_id.is_some());
    let raw =
        match local_backend.load_raw(crate::local_state::FORMAT_ID, crate::local_state::VERSION) {
            BackendLoad::Loaded(raw) => raw,
            other => panic!("expected autosaved attachment state, got {other:?}"),
        };
    assert!(!raw.contains(source_text.as_str()));

    let sent_before = sent_count(&s);
    let _ = s.update(Message::SendCompose);
    let handoff = s
        .pending_send_attachment_handoff()
        .expect("pending attachment MIME handoff");
    assert!(
        String::from_utf8(handoff)
            .expect("ASCII MIME")
            .contains("ZHVyYWJsZSBhdHRhY2htZW50IGJ5dGVz")
    );
    expire_pending_send(&mut s);
    assert_eq!(sent_count(&s), sent_before + 1);

    let sent = sent_with_attachment(&s, "report.pdf")
        .expect("the attachment must reach the sent message through the real path");
    assert!(sent.has_attachment);
    assert_eq!(sent.attachments.len(), 1);
}

#[test]
fn stale_attachment_stage_is_removed_instead_of_entering_a_new_compose() {
    let fixture_dir =
        std::env::temp_dir().join(format!("mail-app-attachment-stale-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&fixture_dir);
    std::fs::create_dir_all(&fixture_dir).expect("create stale fixture directory");
    let source = fixture_dir.join("stale.txt");
    std::fs::write(&source, b"stale payload").expect("write stale source");

    let mut state = boot_state();
    let _ = state.update(Message::OpenCompose(ComposeMode::New));
    let old_generation = state.attachment_picker_generation;
    let attachment = state
        .attachment_store
        .stage_paths(vec![source], 0, 0)
        .expect("stage stale payload")
        .remove(0);
    let retained = attachment.clone();
    let _ = state.update(Message::CloseCompose);
    let _ = state.update(Message::OpenCompose(ComposeMode::New));
    state.attachment_picker_generation = old_generation + 1;

    let cleanup = state.update(Message::AttachmentsStaged(
        old_generation,
        Ok(vec![attachment]),
    ));
    let _ = apply_task(&mut state, cleanup);

    assert!(state.compose.attachments.is_empty());
    assert_eq!(
        state.attachment_store.read(&retained),
        Err(crate::attachment_store::AttachmentFailure::MissingPayload)
    );
    std::fs::remove_dir_all(fixture_dir).expect("remove stale fixture directory");
}

#[test]
fn removing_the_only_attachment_updates_the_autosaved_draft_before_cleanup() {
    let fixture_dir =
        std::env::temp_dir().join(format!("mail-app-attachment-remove-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&fixture_dir);
    std::fs::create_dir_all(&fixture_dir).expect("create removal fixture directory");
    let source = fixture_dir.join("only.txt");
    std::fs::write(&source, b"only attachment").expect("write removal source");

    let mut state = boot_state();
    let _ = state.update(Message::OpenCompose(ComposeMode::New));
    let attachment = state
        .attachment_store
        .stage_paths(vec![source], 0, 0)
        .expect("stage removal payload")
        .remove(0);
    let retained = attachment.clone();
    let staged = state.update(Message::AttachmentsStaged(
        state.attachment_picker_generation,
        Ok(vec![attachment]),
    ));
    let _ = apply_task(&mut state, staged);
    let draft_id = state
        .compose
        .editing_draft_id
        .clone()
        .expect("attachment autosaved as draft");

    let removed = state.update(Message::RemoveComposeAttachment(0));
    let _ = apply_task(&mut state, removed);

    assert!(state.compose.attachments.is_empty());
    assert!(
        state
            .store
            .message(&draft_id)
            .is_some_and(|draft| draft.attachments.is_empty() && !draft.has_attachment)
    );
    assert_eq!(
        state.attachment_store.read(&retained),
        Err(crate::attachment_store::AttachmentFailure::MissingPayload)
    );
    std::fs::remove_dir_all(fixture_dir).expect("remove removal fixture directory");
}

#[test]
fn missing_attachment_payload_prevents_pending_send() {
    let mut state = boot_state();
    let _ = state.update(Message::OpenCompose(ComposeMode::New));
    state.compose.to = "recipient@example.com".into();
    state.compose.attachments.push(model::types::Attachment {
        name: "metadata-only.pdf".into(),
        mime: "application/pdf".into(),
        size: 42,
        payload_id: None,
        payload_identity: None,
    });

    let _ = state.update(Message::SendCompose);

    assert!(state.pending_send.is_none());
    assert!(state.compose.open);
    assert!(
        state
            .compose_attach_notice
            .as_deref()
            .is_some_and(|notice| notice.contains("not queued"))
    );
}

#[test]
fn payload_backed_attachment_uses_open_and_save_portals_after_source_is_removed() {
    use crate::link_launcher::tests::SharedMockBackend;
    use crate::picker::tests::ScriptedBackend;
    use enzui_iced_widgets::file_chooser::PortalOperation;

    let fixture_dir = std::env::temp_dir().join(format!(
        "mail-app-attachment-transfer-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&fixture_dir);
    std::fs::create_dir_all(&fixture_dir).expect("create transfer fixture directory");
    let source = fixture_dir.join("source.txt");
    let destination = fixture_dir.join("saved.txt");
    std::fs::write(&source, b"restart-safe payload").expect("write transfer source");

    let store = crate::attachment_store::AttachmentStore::for_root(&fixture_dir.join("store"))
        .expect("private attachment store");
    let attachment = store
        .stage_paths(vec![source.clone()], 0, 0)
        .expect("stage transfer payload")
        .remove(0);
    std::fs::remove_file(source).expect("remove ambient source");

    let mut state = boot_state();
    state.attachment_store = store;
    state.store.messages[0].attachments = vec![attachment.clone()];
    state.store.messages[0].has_attachment = true;

    let open_backend = SharedMockBackend::new();
    state.attachment_opener = crate::link_launcher::LinkOpener::with_backend(open_backend.boxed());
    let open = state.update(Message::OpenAttachment(attachment.clone()));
    let _ = apply_task(&mut state, open);
    let expected_identity = state
        .attachment_open_held
        .as_ref()
        .expect("held attachment descriptor")
        .retained_file()
        .expect("framework retained file")
        .identity();
    assert_eq!(open_backend.lock().started_files[0].1, expected_identity);

    let destination_uri = enzui_iced_core::portal::open_uri::OpenUri::from_file_path(&destination)
        .expect("destination URI")
        .as_str()
        .to_owned();
    let save_backend = ScriptedBackend::completed(vec![destination_uri]);
    let save_log = save_backend.log();
    state.attach_chooser = crate::picker::Picker::with_backend(Box::new(save_backend));
    let _ = state.update(Message::SaveAttachment(attachment));
    let task = state.update(Message::AttachPortalTick);
    let _ = apply_task(&mut state, task);

    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"restart-safe payload"
    );
    assert_eq!(
        save_log.lock().unwrap().started,
        vec![PortalOperation::SaveFile]
    );
    assert_eq!(
        state.attachment_notice.as_deref(),
        Some("Attachment saved.")
    );
    std::fs::remove_dir_all(fixture_dir).expect("remove transfer fixture directory");
}

#[test]
fn orphan_cleanup_waits_for_the_mailbox_state_publication() {
    use enzui_iced_core::modal::DismissCause;
    use enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent;

    let fixture_dir = std::env::temp_dir().join(format!(
        "mail-app-attachment-cleanup-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&fixture_dir);
    std::fs::create_dir_all(&fixture_dir).expect("create cleanup fixture directory");
    let source = fixture_dir.join("retired.txt");
    std::fs::write(&source, b"delete after publish").expect("write cleanup source");

    let mut state = boot_state();
    let attachment = state
        .attachment_store
        .stage_paths(vec![source], 0, 0)
        .expect("stage cleanup payload")
        .remove(0);
    let retained = attachment.clone();
    let message_id = state.store.messages[0].id.clone();
    state.store.messages[0].attachments = vec![attachment];
    state.store.messages[0].has_attachment = true;
    state.store.mark_dirty();
    let task = state.save_local_state();
    let _ = apply_task(&mut state, task);

    let task = state.update(Message::MailAction(
        MailAction::Delete,
        ActionTarget::Message { message_id },
    ));
    let _ = apply_task(&mut state, task);
    let _ = state.update(Message::RequestEmptyMailbox(
        MailboxKind::Trash,
        "icloud".into(),
    ));
    let publication = state.update(Message::ConfirmEmptyEvent(ConfirmDialogEvent::Resolved(
        DismissCause::DefaultAction,
    )));

    let close = state.update(Message::WindowCloseResolved(Some(
        iced::window::Id::unique(),
    )));
    assert_eq!(
        state.attachment_store.read(&retained).unwrap(),
        b"delete after publish"
    );
    let closes = apply_close_task(&mut state, Task::batch([publication, close]));
    assert_eq!(closes, 1);
    assert_eq!(
        state.attachment_store.read(&retained),
        Err(crate::attachment_store::AttachmentFailure::MissingPayload)
    );
    std::fs::remove_dir_all(fixture_dir).expect("remove cleanup fixture directory");
}
