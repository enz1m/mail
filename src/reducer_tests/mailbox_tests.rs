use super::*;

#[test]
fn mailbox_menu_and_confirmation_keep_account_identity_across_reopen() {
    use enzui_iced_core::modal::DismissCause;
    use enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent;
    let mut state = boot_state();
    let mut other_account = state.store.accounts[0].clone();
    other_account.id = "other".into();
    state.store.accounts.push(other_account);
    let mut other_message = state
        .store
        .messages
        .iter()
        .find(|m| m.location == model::types::MessageLocation::Mailbox(MailboxKind::Trash))
        .expect("synthetic Trash message")
        .clone();
    other_message.id = "other-trash".into();
    other_message.account_id = "other".into();
    state.store.messages.push(other_message);
    state.store.mark_dirty();
    for account_id in ["icloud", "other"] {
        let selection = SidebarSelection::AccountMailbox {
            account_id: account_id.into(),
            mailbox: MailboxKind::Trash,
        };
        let _ = state.update(Message::OpenSidebarMenu {
            row_key: crate::view::sidebar_row_key(&selection),
            selection,
            at: iced::Point::ORIGIN,
        });
    }
    let showing = state.context_menu_generation;
    let old_dismissal = enzui_iced_widgets::context_menu_host::ContextMenuEvent::Dismissed(
        enzui_iced_core::menu_model::CloseCause::Escape,
    );
    let _ = state.update(Message::ContextMenuEvent(
        showing - 1,
        old_dismissal.clone(),
    ));
    assert!(state.context_menu.is_open());
    assert!(matches!(
        state.context_menu.target(),
        Some(crate::context_menu::ContextTarget::Sidebar {
            selection: SidebarSelection::AccountMailbox { account_id, mailbox: MailboxKind::Trash },
            ..
        }) if account_id == "other"
    ));
    let _ = state.update(Message::ContextMenuEvent(showing, old_dismissal));
    assert!(!state.context_menu.is_open());
    let origin_count = model::bulk::mailbox_count_for(&state.store, MailboxKind::Trash, "icloud");
    let _ = state.update(Message::RequestEmptyMailbox(
        MailboxKind::Trash,
        "icloud".into(),
    ));
    state.selection = SidebarSelection::AccountMailbox {
        account_id: "other".into(),
        mailbox: MailboxKind::Trash,
    };
    state
        .row_selection
        .insert(enzui_iced_core::drag::ItemId::new("other-trash"));
    let _ = state.update(Message::ConfirmEmptyEvent(ConfirmDialogEvent::Resolved(
        DismissCause::DefaultAction,
    )));
    assert!(origin_count > 0);
    assert_eq!(
        model::bulk::mailbox_count_for(&state.store, MailboxKind::Trash, "icloud"),
        0
    );
    assert_eq!(
        model::bulk::mailbox_count_for(&state.store, MailboxKind::Trash, "other"),
        1
    );
    assert!(state.bulk.pending_empty_account.is_none());
    assert!(
        state
            .row_selection
            .contains(&enzui_iced_core::drag::ItemId::new("other-trash"))
    );
}

#[test]
fn address_menus_keep_recipient_identity_and_refuse_replaced_targets() {
    use enzui_iced_core::menu_model::{EntryId, OpenCause};
    use enzui_iced_widgets::menu_button::MenuButtonEvent;
    let mut state = boot_state();
    let id = state.store.messages[0].id.clone();
    state.store.messages[0].to = vec![
        "Recipient <recipient@example.invalid>".into(),
        "xi".into(),
        "xi@".into(),
        "injected@example.invalid\r\nX-Header: value".into(),
    ];
    let contacts = model::recipients::recent_contacts(&state.store);
    assert!(
        contacts
            .iter()
            .any(|contact| contact.email == "recipient@example.invalid")
    );
    assert!(contacts.iter().all(|contact| !matches!(
        contact.email.as_str(),
        "xi" | "xi@" | "injected@example.invalid\r\nx-header: value"
    )));
    assert_eq!(state.store.messages[0].to[1], "xi");
    let _ = state.update(Message::SenderMenuEvent(
        id.clone(),
        None,
        MenuButtonEvent::Open(OpenCause::Pointer),
    ));
    let _ = state.update(Message::SenderMenuEvent(
        id.clone(),
        Some(0),
        MenuButtonEvent::Open(OpenCause::Pointer),
    ));
    let activate = |command| MenuButtonEvent::Activated {
        id: EntryId::new(command),
        check: enzui_iced_core::menu_model::CheckState::None,
    };
    let _ = state.update(Message::SenderMenuEvent(
        id.clone(),
        None,
        activate("compose-to"),
    ));
    assert!(!state.compose.open);
    let _ = state.update(Message::SenderMenuEvent(
        id.clone(),
        Some(9),
        activate("compose-to"),
    ));
    assert!(!state.compose.open);
    state.store.messages[0].to[0] = "Changed <changed@example.invalid>".into();
    let _ = state.update(Message::SenderMenuEvent(
        id.clone(),
        Some(0),
        activate("compose-to"),
    ));
    assert!(!state.compose.open);
    state.store.messages[0].to[0] = "Recipient <recipient@example.invalid>".into();
    let _ = state.update(Message::SenderMenuEvent(
        id.clone(),
        Some(0),
        MenuButtonEvent::Open(OpenCause::Pointer),
    ));
    let _ = state.update(Message::SenderMenuEvent(
        id.clone(),
        Some(0),
        activate("filter-from"),
    ));
    assert_eq!(state.search_query, "recipient@example.invalid");
    let _ = state.update(Message::SenderMenuEvent(
        id.clone(),
        Some(0),
        MenuButtonEvent::Open(OpenCause::Pointer),
    ));
    let _ = state.update(Message::SenderMenuEvent(
        id,
        Some(0),
        activate("compose-to"),
    ));
    assert!(state.compose.open);
    assert_eq!(state.compose.to, "Recipient <recipient@example.invalid>");
}

#[test]
fn conversation_navigation_follows_filtered_order_and_stops_at_edges() {
    let mut state = boot_state();
    let convos = state.store.conversations_for(&state.selection);
    assert!(convos.len() >= 3);
    let ids: Vec<_> = convos.iter().take(3).map(|c| c.id.clone()).collect();
    for message in &mut state.store.messages {
        message.subject = if ids.contains(&message.id) {
            "Navigation fixture"
        } else {
            "Excluded"
        }
        .into();
    }
    state.store.mark_dirty();
    state.search_query = "Navigation fixture".into();
    state.selected_conversation = None;
    let visible = crate::view::searched_conversations(
        &state.store,
        &state.selection,
        state.search_scope,
        &state.search_query,
    );
    assert_eq!(visible.len(), 3);
    for convo in &visible {
        let _ = state.update(Message::SelectNextConversation);
        assert_eq!(
            state.selected_conversation.as_deref(),
            Some(convo.id.as_str())
        );
        assert_eq!(
            crate::view::explicit_selection(&state),
            vec![convo.id.clone()]
        );
    }
    let _ = state.update(Message::SelectNextConversation);
    assert_eq!(
        state.selected_conversation.as_deref(),
        Some(visible[2].id.as_str())
    );
    for convo in visible.iter().rev().skip(1) {
        let _ = state.update(Message::SelectPreviousConversation);
        assert_eq!(
            state.selected_conversation.as_deref(),
            Some(convo.id.as_str())
        );
    }
    let _ = state.update(Message::SelectPreviousConversation);
    assert_eq!(
        state.selected_conversation.as_deref(),
        Some(visible[0].id.as_str())
    );
}
