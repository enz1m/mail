use enzui_iced_core::persist::{BackendLoad, InMemoryBackend, PersistBackend, Persistable};

use super::{FORMAT_ID, LocalState, VERSION, store_on};
use crate::mail_actions::{ActionTarget, MailAction};
use crate::model::compose::ComposeMode;
use crate::model::types::{AttachmentPayloadId, MailboxKind, MessageLocation};
use crate::{Message, State};

fn drain(task: iced::Task<Message>) -> Vec<Message> {
    match iced_test::runtime::task::into_stream(task) {
        Some(stream) => iced_test::futures::futures::executor::block_on(async move {
            use iced_test::futures::futures::StreamExt;
            stream
                .filter_map(|action| async move {
                    match action {
                        iced_test::runtime::Action::Output(message) => Some(message),
                        _ => None,
                    }
                })
                .collect::<Vec<_>>()
                .await
        }),
        None => Vec::new(),
    }
}

fn apply(state: &mut State, message: Message) {
    let produced = drain(state.update(message));
    for message in produced {
        if let Message::LocalStateSaved(_, Err(error)) = &message {
            panic!("local-state write failed: {error}");
        }
        if matches!(message, Message::LocalStateSaved(..)) {
            let _ = state.update(message);
        }
    }
}

#[test]
fn version_one_attachment_identity_is_additive_and_legacy_records_remain_readable() {
    let migrated = LocalState::persist_deserialize("future-record\tignored\n")
        .expect("unknown additive record");
    assert!(migrated.accounts.is_empty());
    assert!(LocalState::persist_deserialize("account\tmissing-fields\n").is_err());

    let mut state = LocalState::from_fixture(crate::model::fixture::seed());
    let attachment = state
        .messages
        .iter_mut()
        .flat_map(|message| message.attachments.iter_mut())
        .next()
        .expect("fixture attachment");
    attachment.payload_id = AttachmentPayloadId::parse("1234567890abcdef1234567890abcdef");
    attachment.payload_identity = Some(crate::model::types::AttachmentPayloadIdentity {
        device: 7,
        inode: 11,
        size: attachment.size,
        mtime_secs: 13,
        mtime_nanos: 17,
        ctime_secs: 19,
        ctime_nanos: 23,
    });

    let encoded = state.persist_serialize().expect("serialize local state");
    let decoded = LocalState::persist_deserialize(&encoded).expect("decode current state");
    assert!(
        LocalState::persist_deserialize(
            &encoded.replace("1234567890abcdef1234567890abcdef", "/ambient/source/path")
        )
        .is_err()
    );
    assert_eq!(
        decoded
            .messages
            .iter()
            .flat_map(|message| message.attachments.iter())
            .next()
            .and_then(|attachment| attachment.payload_id.as_ref())
            .map(AttachmentPayloadId::as_str),
        Some("1234567890abcdef1234567890abcdef")
    );
    assert!(
        decoded
            .messages
            .iter()
            .flat_map(|message| message.attachments.iter())
            .next()
            .and_then(|attachment| attachment.payload_identity)
            .is_some()
    );

    let legacy = encoded
        .lines()
        .map(|line| {
            if line.starts_with("attachment\t") {
                line.split('\t').take(6).collect::<Vec<_>>().join("\t")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let decoded = LocalState::persist_deserialize(&legacy).expect("decode legacy state");
    let legacy_attachment = decoded
        .messages
        .iter()
        .flat_map(|message| message.attachments.iter())
        .next()
        .expect("legacy attachment metadata");
    assert_eq!(
        legacy_attachment
            .payload_id
            .as_ref()
            .map(AttachmentPayloadId::as_str),
        Some("1234567890abcdef1234567890abcdef")
    );
    assert!(legacy_attachment.payload_identity.is_none());
    assert!(legacy_attachment.payload_ref().is_none());
}

#[test]
fn account_draft_pending_send_signature_and_move_survive_restart_without_secrets() {
    let backend = InMemoryBackend::new();
    let (mut state, _) = State::boot_for_test_with_local_state(store_on(&backend), true);

    let account_id = state.store.accounts[0].id.clone();
    apply(
        &mut state,
        Message::ToggleAccountExpanded(account_id.clone()),
    );
    apply(
        &mut state,
        Message::AccountSignatureAction(
            account_id.clone(),
            iced::widget::text_editor::Action::Edit(iced::widget::text_editor::Edit::Paste(
                std::sync::Arc::new("Durable signature".into()),
            )),
        ),
    );
    apply(
        &mut state,
        Message::SetAccountAppendSignature(account_id.clone(), true),
    );

    let moved_id = state
        .store
        .messages
        .iter()
        .find(|message| {
            matches!(
                &message.location,
                MessageLocation::Mailbox(MailboxKind::Inbox)
            )
        })
        .expect("fixture inbox message")
        .id
        .clone();
    apply(
        &mut state,
        Message::MailAction(
            MailAction::Archive,
            ActionTarget::Message {
                message_id: moved_id.clone(),
            },
        ),
    );

    let fixture_dir = std::env::temp_dir().join(format!(
        "mail-app-local-state-attachments-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&fixture_dir);
    std::fs::create_dir_all(&fixture_dir).expect("create local-state attachment fixture");
    let draft_source = fixture_dir.join("durable.txt");
    let pending_source = fixture_dir.join("pending.bin");
    std::fs::write(&draft_source, b"durable draft payload").expect("write draft payload");
    std::fs::write(&pending_source, b"durable pending payload").expect("write pending payload");
    let draft_attachment = state
        .attachment_store
        .stage_paths(vec![draft_source.clone()], 0, 0)
        .expect("stage draft payload")
        .remove(0);
    let draft_payload = draft_attachment
        .payload_id
        .clone()
        .expect("draft payload identity");

    apply(&mut state, Message::OpenCompose(ComposeMode::New));
    state.compose.to = "draft@example.net".into();
    state.compose.subject = "Durable draft".into();
    state.compose.body = "kept across restart".into();
    state.compose.attachments.push(draft_attachment);
    apply(&mut state, Message::SaveDraft);

    apply(&mut state, Message::OpenCompose(ComposeMode::New));
    state.compose.to = "send@example.net".into();
    state.compose.subject = "Pending send".into();
    state.compose.body = "undo remains available".into();
    let pending_attachment = state
        .attachment_store
        .stage_paths(vec![pending_source.clone()], 0, 0)
        .expect("stage pending payload")
        .remove(0);
    let pending_payload = pending_attachment
        .payload_id
        .clone()
        .expect("pending payload identity");
    state.compose.attachments.push(pending_attachment);
    apply(&mut state, Message::SendCompose);

    apply(&mut state, Message::OpenSettings);
    apply(&mut state, Message::BeginAddAccount);
    apply(
        &mut state,
        Message::AddAccountEmailEdited("durable.user@gmail.com".into()),
    );
    apply(&mut state, Message::AddAccountContinue);
    apply(
        &mut state,
        Message::AddAccountPasswordEdited("not-written-to-local-state".into()),
    );
    apply(&mut state, Message::CommitAddAccount);
    assert!(state.local_state_is_published());

    let raw = match backend.load_raw(FORMAT_ID, VERSION) {
        BackendLoad::Loaded(raw) => raw,
        other => panic!("expected published local state, got {other:?}"),
    };
    assert!(!raw.contains("not-written-to-local-state"));
    let draft_source_text = draft_source.to_string_lossy();
    let pending_source_text = pending_source.to_string_lossy();
    assert!(!raw.contains(draft_source_text.as_ref()));
    assert!(!raw.contains(pending_source_text.as_ref()));
    assert!(raw.contains(draft_payload.as_str()));
    assert!(raw.contains(pending_payload.as_str()));

    let (restarted, _) = State::boot_for_test_with_local_state(store_on(&backend), false);
    assert!(
        restarted
            .store
            .accounts
            .iter()
            .any(|account| account.email == "durable.user@gmail.com")
    );
    assert!(restarted.store.account(&account_id).is_some_and(|account| {
        account.append_signature && account.signature.contains("Durable signature")
    }));
    assert!(restarted.store.messages.iter().any(|message| {
        message.subject == "Durable draft"
            && matches!(
                &message.location,
                MessageLocation::Mailbox(MailboxKind::Drafts)
            )
            && message
                .attachments
                .first()
                .and_then(|attachment| attachment.payload_id.as_ref())
                == Some(&draft_payload)
    }));
    assert!(restarted.store.message(&moved_id).is_some_and(|message| {
        matches!(
            &message.location,
            MessageLocation::Mailbox(MailboxKind::Archive)
        )
    }));
    assert!(restarted.pending_send.as_ref().is_some_and(|pending| {
        pending.subject == "Pending send"
            && pending.body == "undo remains available"
            && pending
                .attachments
                .first()
                .and_then(|attachment| attachment.payload_id.as_ref())
                == Some(&pending_payload)
    }));
    assert!(!restarted.pending_server_mutations.is_empty());
    assert!(restarted.local_state_is_published());

    let mut restarted = restarted;
    let draft_id = restarted
        .store
        .messages
        .iter()
        .find(|message| message.subject == "Durable draft")
        .expect("restarted draft")
        .id
        .clone();

    apply(
        &mut restarted,
        Message::OpenCompose(ComposeMode::EditDraft(draft_id.clone())),
    );
    assert!(!restarted.compose.open);
    assert!(restarted.pending_send.is_some());
    assert_eq!(
        restarted
            .store
            .message(&draft_id)
            .and_then(|message| message.attachments.first())
            .and_then(|attachment| attachment.payload_id.as_ref()),
        Some(&draft_payload)
    );

    let toast = restarted.pending_undo_toast.expect("restored undo toast");
    apply(
        &mut restarted,
        Message::Toast(enzui_iced_widgets::toast_host::ToastEvent::Dismiss(
            toast.id,
        )),
    );

    assert!(restarted.pending_send.is_some());
    assert!(
        restarted
            .mail_notice
            .as_ref()
            .is_some_and(|notice| notice.can_cancel_outbox)
    );

    let (queued, _) = State::boot_for_test_with_local_state(store_on(&backend), false);
    assert!(queued.pending_send.as_ref().is_some_and(|pending| {
        pending.subject == "Pending send"
            && pending
                .attachments
                .first()
                .and_then(|attachment| attachment.payload_id.as_ref())
                == Some(&pending_payload)
    }));
    assert!(!queued.store.messages.iter().any(|message| {
        message.subject == "Pending send"
            && matches!(
                &message.location,
                MessageLocation::Mailbox(MailboxKind::Sent)
            )
    }));
    std::fs::remove_dir_all(fixture_dir).expect("remove local-state attachment fixture");
}

#[test]
fn production_startup_recovers_cached_plain_and_html_previews_without_rewriting_mail() {
    let backend = InMemoryBackend::new();
    let mut saved = LocalState::from_fixture(crate::model::fixture::seed());
    let plain = &mut saved.messages[0];
    plain.preview.clear();
    plain.body = "Hello\n\nMore cached content".into();
    let html = &mut saved.messages[1];
    html.preview = "[remote image blocked] ".repeat(20);
    html.body.clear();
    html.body_html = Some(
        "<img src='https://example.invalid/logo.png'><p>HTML hello</p><p>More cached content</p>"
            .into(),
    );
    let raw = saved.persist_serialize().unwrap();
    backend.save_raw(FORMAT_ID, VERSION, &raw).unwrap();
    let mut loaded = super::load_with_previews(&store_on(&backend)).into_value();
    assert_eq!(loaded.messages[0].preview, "Hello More cached content");
    assert_eq!(loaded.messages[1].preview, "HTML hello More cached content");
    for (before, after) in saved.messages.iter().zip(&mut loaded.messages) {
        after.preview.clone_from(&before.preview);
    }
    assert_eq!(loaded.persist_serialize().unwrap(), raw);
    assert!(
        matches!(backend.load_raw(FORMAT_ID, VERSION), BackendLoad::Loaded(bytes) if bytes == raw)
    );
}
