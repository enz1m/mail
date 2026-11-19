use super::*;
use enzui_iced_core::persist::Persistable;

fn open_first(state: &mut State) -> String {
    let convo = state.store.conversations_for(&state.selection)[0]
        .id
        .clone();
    let _ = state.update(Message::SelectConversation {
        id: convo.clone(),
        toggle: false,
        range: false,
    });
    convo
}

fn type_reply(state: &mut State, text: &str) {
    use iced::widget::text_editor::{Action, Edit};
    let _ = state.update(Message::QuickReplyEdit(Action::Edit(Edit::Paste(
        std::sync::Arc::new(text.to_owned()),
    ))));
}

#[test]
fn html_replies_keep_the_original_and_receiving_account_when_expanded_or_queued() {
    for send in [false, true] {
        let (mut state, _) = State::boot_for_test();
        open_first(&mut state);
        let id = crate::view::action_target_for_breadth(&state)
            .unwrap()
            .anchor_message_id()
            .to_owned();
        let account = state.store.message(&id).unwrap().account_id.clone();
        let mut other = state.store.accounts[0].clone();
        other.id = "other-account".into();
        state.active_account_id = other.id.clone();
        state.store.accounts.push(other);
        let source = state
            .store
            .messages
            .iter_mut()
            .find(|message| message.id == id)
            .unwrap();
        source.body.clear();
        source.body_html = Some("<p>Original HTML-only message &amp; details.</p>".into());
        let mode = if send {
            ComposeMode::ReplyAll(id.clone())
        } else {
            ComposeMode::Reply(id.clone())
        };
        let _ = state.update(Message::StartQuickReply(mode));
        type_reply(&mut state, "My response");
        let _ = state.update(if send {
            Message::QuickReplySend
        } else {
            Message::QuickReplyExpand
        });
        let (body, owner, references) = if send {
            let pending = state.pending_send.as_ref().unwrap();
            (&pending.body, &pending.account_id, &pending.references)
        } else {
            (
                &state.compose.body,
                &state.compose.from_account_id,
                &state.compose.references,
            )
        };
        assert!(body.starts_with("My response\n\n"));
        assert!(body.contains("> Original HTML-only message & details."));
        assert_eq!(owner, &account);
        assert_eq!(references.last(), Some(&id));
        if send {
            let sent_id = state.pending_send.as_ref().unwrap().id.clone();
            let before = state
                .store
                .conversations_for(&state.selection)
                .into_iter()
                .find(|c| c.message_ids.contains(&id))
                .unwrap()
                .message_count();
            let toast = state.pending_undo_toast.unwrap();
            let _ = state.update(Message::Toast(ToastEvent::Action(
                toast.id,
                SEND_NOW_ACTION.into(),
            )));
            let conversation = state
                .store
                .conversations_for(&state.selection)
                .into_iter()
                .find(|c| c.message_ids.contains(&id))
                .unwrap();
            assert_eq!(conversation.message_count(), before + 1);
            assert!(conversation.message_ids.contains(&sent_id));
            let sent = state.store.message(&sent_id).unwrap();
            let recipients = sent.to.join(", ");
            assert_eq!(sent.account_id, account);
            assert!(state.store.quoted_body(sent).plain_end.is_some());
            let original_messages = local_state::LocalState::from_fixture(state.store.clone())
                .persist_serialize()
                .unwrap();
            let generation = state.store.generation();
            let _ = state.update(Message::ToggleQuotedText(sent_id.clone()));
            assert!(state.quoted_text_expanded.contains(&sent_id));
            assert_eq!(
                local_state::LocalState::from_fixture(state.store.clone())
                    .persist_serialize()
                    .unwrap(),
                original_messages
            );
            assert_eq!(state.store.generation(), generation);
            let _ = state.update(Message::OpenCompose(ComposeMode::Reply(sent_id)));
            assert_eq!(state.compose.to, recipients);
            assert_eq!(state.compose.from_account_id, account);
        }
    }
}

#[test]
fn incoming_reply_preserves_reader_selection_and_unsent_inline_text() {
    use enzui_iced_core::drag::ItemId;
    let (mut state, _) = State::boot_for_test();
    let mut original = state.store.messages[0].clone();
    original.id = "rethread-original".into();
    original.subject = "Planning".into();
    original.in_reply_to = None;
    original.references.clear();
    original.date = model::types::MessageDate::at(2026, 10, 21, 9, 0);
    original.location = model::types::MessageLocation::Mailbox(model::types::MailboxKind::Inbox);
    state.store.messages = vec![original.clone()];
    state.store.mark_dirty();
    state.reconcile_selection();
    let _ = state.update(Message::StartQuickReply(ComposeMode::Reply(
        original.id.clone(),
    )));
    type_reply(&mut state, "Keep this unsent response");
    let anchor = ItemId::new(original.id.clone());
    state
        .row_selection
        .replace([anchor.clone()], Some(anchor.clone()), Some(anchor));
    let mut reply = original.clone();
    reply.id = "rethread-reply".into();
    reply.subject = "Re: Planning".into();
    reply.in_reply_to = Some(original.id.clone());
    reply.date = model::types::MessageDate::at(2026, 10, 21, 10, 0);
    let mut unrelated = original.clone();
    unrelated.id = "rethread-unrelated".into();
    unrelated.subject = "Unrelated newer conversation".into();
    unrelated.date = model::types::MessageDate::at(2026, 10, 21, 11, 0);
    state
        .store
        .messages
        .extend([reply.clone(), unrelated.clone()]);
    state.store.mark_dirty();
    state.reconcile_after_mutation();
    assert_eq!(
        state.selected_conversation.as_deref(),
        Some(reply.id.as_str())
    );
    assert_eq!(
        state.quick_reply_text().as_deref(),
        Some("Keep this unsent response")
    );
    assert_eq!(
        state.quick_reply_mode,
        Some(ComposeMode::Reply(original.id))
    );
    let reply_id = ItemId::new(reply.id);
    assert!(state.row_selection.contains(&reply_id));
    assert_eq!(state.row_selection.anchor(), Some(&reply_id));
    assert_eq!(state.row_selection.active(), Some(&reply_id));
    assert!(!state.row_selection.contains(&ItemId::new(unrelated.id)));
    assert!(state.pending_send.is_none());
}

#[test]
fn click_away_retains_authored_composer_and_inline_reply_as_drafts() {
    use iced::widget::text_editor::{Action, Edit};
    let (mut state, _) = State::boot_for_test();
    let before = state.store.messages.len();
    let _ = state.update(Message::OpenCompose(ComposeMode::New));
    let _ = state.update(Message::ComposeBodyAction(Action::Edit(Edit::Paste(
        std::sync::Arc::new("Keep this draft".to_owned()),
    ))));
    state.draft_bounds.set(Some(iced::Rectangle::new(
        iced::Point::new(100.0, 100.0),
        iced::Size::new(300.0, 400.0),
    )));
    let _ = state.update(Message::DraftPointerMoved(iced::Point::new(150.0, 150.0)));
    let _ = state.update(Message::DraftClickAway);
    assert_eq!(state.store.messages.len(), before);
    let _ = state.update(Message::DraftPointerMoved(iced::Point::ORIGIN));
    let _ = state.update(Message::DraftClickAway);
    let draft = state
        .compose
        .editing_draft_id
        .clone()
        .expect("autosaved draft");
    assert!(state.compose.open);
    assert!(
        state
            .store
            .message(&draft)
            .unwrap()
            .body
            .starts_with("Keep this draft")
    );
    let token = state.local_state_token();
    let _ = state.update(Message::DraftWindowBlur);
    assert_eq!(state.local_state_token(), token);
    let mut other = state.store.accounts[0].clone();
    other.id = "second-account".into();
    other.email = "other@example.com".into();
    let account = other.id.clone();
    state.store.accounts.push(other);
    let _ = state.update(Message::SetComposeFrom(account.clone()));
    let _ = state.update(Message::DraftWindowBlur);
    assert_eq!(state.store.message(&draft).unwrap().account_id, account);
    let _ = state.update(Message::CloseCompose);
    assert!(!state.compose.open);
    assert_eq!(state.store.messages.len(), before + 1);

    for leave in [
        Message::DraftClickAway,
        Message::SelectNextConversation,
        Message::OpenSettings,
    ] {
        let (mut state, _) = State::boot_for_test();
        let conversation = open_first(&mut state);
        let source = state.newest_message_of(&conversation).unwrap();
        let _ = state.update(Message::StartQuickReply(ComposeMode::Reply(source.clone())));
        type_reply(&mut state, "Keep this inline response");
        state.draft_pointer = Some(iced::Point::ORIGIN);
        state.draft_bounds.set(Some(iced::Rectangle::new(
            iced::Point::new(100.0, 100.0),
            iced::Size::new(300.0, 200.0),
        )));
        let _ = state.update(leave);
        let draft = state
            .store
            .messages
            .iter()
            .find(|message| {
                message.body.starts_with("Keep this inline response\n\n")
                    && message.in_reply_to.as_deref() == Some(source.as_str())
            })
            .expect("inline response retained with reply linkage");
        assert!(matches!(
            draft.location,
            MessageLocation::Mailbox(MailboxKind::Drafts)
        ));
        assert!(state.quick_reply_mode.is_none());
        assert!(state.pending_send.is_none());
    }
}
