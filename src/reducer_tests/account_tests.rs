use super::*;

#[test]
fn account_id_allocation_rechecks_a_stale_candidate_after_authentication() {
    use enzui_services::credential_store::CredentialPresence;

    let mut state = production_state_for_mail_backend();
    let credentials =
        std::sync::Arc::new(enzui_services::credential_store::MemoryCredentialStore::new());
    state.credential_store = CredentialStoreHandle(std::sync::Arc::<
        enzui_services::credential_store::MemoryCredentialStore,
    >::clone(&credentials));
    state.credential_store_ready = true;
    state.mail_backend = std::sync::Arc::new(SuccessfulMailBackend::default());
    prepare_generated_account_add(&mut state);

    let task = state.update(Message::CommitAddAccount);
    let candidate = loopback_account_config("acct-1");
    let key = State::credential_key(&candidate);
    let mut survivor = account_from_config(candidate);
    survivor.display_name = "SURVIVOR".to_owned();
    state.store.accounts.push(survivor);

    state.next_account_ordinal = 2;
    state.store.mark_dirty();
    let survivor_generation = state.store.generation();

    let publish = state.save_local_state();

    assert_eq!(
        apply_task(&mut state, iced::Task::batch([task, publish])),
        0
    );

    assert_eq!(
        state
            .store
            .accounts
            .iter()
            .filter(|account| account.id == "acct-1")
            .count(),
        1
    );
    assert_eq!(state.store.accounts[0].display_name, "SURVIVOR");
    assert_eq!(state.store.generation(), survivor_generation);
    assert_eq!(state.next_account_ordinal, 2);
    assert_eq!(credentials.contains(&key), CredentialPresence::Absent);

    assert_eq!(
        state.settings.add_account.error.as_deref(),
        Some("Account setup changed before it could be saved. Cancel and add the account again.")
    );
}

#[test]
fn mail_backend_account_is_published_only_after_keyring_and_probe() {
    let mut state = production_state_for_mail_backend();
    let credentials =
        std::sync::Arc::new(enzui_services::credential_store::MemoryCredentialStore::new());
    let backend = std::sync::Arc::new(SuccessfulMailBackend::default());
    state.credential_store = CredentialStoreHandle(std::sync::Arc::<
        enzui_services::credential_store::MemoryCredentialStore,
    >::clone(&credentials));
    state.credential_store_ready = true;
    state.mail_backend =
        std::sync::Arc::<crate::reducer_tests::SuccessfulMailBackend>::clone(&backend);
    state.settings.add_account.begin();
    state.settings.add_account.draft = {
        let config = loopback_account_config("unused");
        crate::backend::config::AccountConfigDraft {
            display_name: config.display_name,
            email: config.email,
            protocol: config.protocol,
            imap_server: config.incoming.server,
            imap_port: config.incoming.port.to_string(),
            imap_username: config.incoming.username,
            imap_encryption: config.incoming.encryption,
            smtp_server: config.smtp.server,
            smtp_port: config.smtp.port.to_string(),
            smtp_username: config.smtp.username,
            smtp_encryption: config.smtp.encryption,
            auth: config.auth,
        }
    };
    state.settings.add_account.password = "generated-password".to_owned();

    let task = state.update(Message::CommitAddAccount);
    assert!(state.store.accounts.is_empty());
    assert_eq!(credentials.entry_count(), 0);
    apply_task(&mut state, task);

    assert_eq!(credentials.entry_count(), 1);
    assert_eq!(state.store.accounts.len(), 1);
    assert!(state.settings.add_account.password.is_empty());
    assert_eq!(
        backend.calls.lock().expect("mail calls lock").as_slice(),
        ["probe", "sync"]
    );
}

#[test]
fn an_oauth_grant_is_bound_to_the_address_it_was_obtained_for() {
    use crate::backend::curl_backend::oauth::{TEST_CLIENT, test_support};
    let provider = test_support::Provider::start("BOUND-CODE");
    TEST_CLIENT.with(|slot| *slot.borrow_mut() = Some(provider.client()));
    let mock = crate::link_launcher::tests::SharedMockBackend::new();
    let mut state = production_state_for_mail_backend()
        .with_link_opener(crate::link_launcher::LinkOpener::with_backend(mock.boxed()));
    let credentials =
        std::sync::Arc::new(enzui_services::credential_store::MemoryCredentialStore::new());
    state.credential_store = CredentialStoreHandle(std::sync::Arc::<
        enzui_services::credential_store::MemoryCredentialStore,
    >::clone(&credentials));
    state.credential_store_ready = true;
    state.mail_backend = std::sync::Arc::new(SuccessfulMailBackend::default());
    let sign_in = |state: &mut State| {
        let task = state.update(Message::AddAccountOAuthSignIn);
        let opened = mock
            .lock()
            .started
            .last()
            .map(|(_, uri, _)| uri.as_str().to_owned())
            .expect("sign-in URL opened");
        let browser =
            std::thread::spawn(move || test_support::browser_returns(&opened, "BOUND-CODE"));
        apply_task(state, task);
        browser.join().expect("browser");
        assert!(state.oauth_grant.is_some());
    };

    state.settings.add_account.begin();
    let _ = state.update(Message::AddAccountEmailEdited("person@gmail.com".into()));
    let _ = state.update(Message::AddAccountContinue);
    sign_in(&mut state);
    let _ = state.update(Message::AddAccountBack);
    assert!(state.oauth_grant.is_none());
    let _ = state.update(Message::AddAccountEmailEdited(
        "someone-else@gmail.com".into(),
    ));
    let _ = state.update(Message::AddAccountContinue);
    let task = state.update(Message::CommitAddAccount);
    apply_task(&mut state, task);
    assert_eq!(credentials.entry_count(), 0);
    assert_eq!(
        state.settings.add_account.error.as_deref(),
        Some("Sign in with \u{2068}Google\u{2069} first.")
    );

    sign_in(&mut state);
    state.settings.add_account.draft.email = "third@gmail.com".into();
    let task = state.update(Message::CommitAddAccount);
    apply_task(&mut state, task);
    TEST_CLIENT.with(|slot| *slot.borrow_mut() = None);
    assert_eq!(credentials.entry_count(), 0);
    assert!(state.store.accounts.is_empty());
    assert!(state.oauth_grant.is_none());
}

#[test]
fn account_removal_deletes_the_exact_credential_before_owned_state() {
    use crate::model::sidebar_layout::FavoriteTarget;
    use enzui_services::credential_store::CredentialPresence;

    let local_backend = enzui_iced_core::persist::InMemoryBackend::new();
    let (mut state, _) =
        State::boot_for_test_with_local_state(crate::local_state::store_on(&local_backend), false);
    let credentials =
        std::sync::Arc::new(enzui_services::credential_store::MemoryCredentialStore::new());
    let first = loopback_account_config("acct-remove-a");
    let second = loopback_account_config("acct-remove-b");
    let first_key = State::credential_key(&first);
    let second_key = State::credential_key(&second);
    for (key, value) in [
        (first_key.clone(), b"first-secret".to_vec()),
        (second_key.clone(), b"second-secret".to_vec()),
    ] {
        let outcome = iced_test::futures::futures::executor::block_on(
            credentials.set(key, Secret::new(value)),
        );
        assert_eq!(outcome, CredentialOutcome::Stored);
    }
    state.credential_store = CredentialStoreHandle(std::sync::Arc::<
        enzui_services::credential_store::MemoryCredentialStore,
    >::clone(&credentials));
    state.credential_store_ready = true;
    state.store.accounts.extend([
        account_from_config(first.clone()),
        account_from_config(second.clone()),
    ]);

    let mut first_draft = model::fixture::seed().messages[0].clone();
    first_draft.id = "draft-remove-a".to_owned();
    first_draft.account_id = first.id.clone();
    first_draft.location = MessageLocation::Mailbox(MailboxKind::Drafts);
    first_draft.subject = "first account draft".to_owned();
    first_draft.attachments = vec![model::types::Attachment {
        name: "first.txt".to_owned(),
        mime: "text/plain".to_owned(),
        size: 5,
        payload_id: None,
        payload_identity: None,
    }];
    first_draft.has_attachment = true;
    let mut second_message = model::fixture::seed().messages[0].clone();
    second_message.id = "message-remove-b".to_owned();
    second_message.account_id = second.id.clone();
    second_message.subject = "second account survives".to_owned();
    second_message.attachments = vec![model::types::Attachment {
        name: "second.txt".to_owned(),
        mime: "text/plain".to_owned(),
        size: 6,
        payload_id: None,
        payload_identity: None,
    }];
    second_message.has_attachment = true;
    state.store.messages.extend([first_draft, second_message]);
    state.active_account_id = first.id.clone();
    state.selection = SidebarSelection::AccountMailbox {
        account_id: first.id.clone(),
        mailbox: MailboxKind::Drafts,
    };
    state.settings.expanded_account = Some(first.id.clone());
    state.collapsed_sections.insert(first.id.clone());

    state.queue_server_mutation(
        crate::local_state::ServerMutationKind::AccountAdded,
        vec![second.id.clone()],
    );
    let first_favorite = state.selection.clone();
    let second_favorite = SidebarSelection::AccountMailbox {
        account_id: second.id.clone(),
        mailbox: MailboxKind::Inbox,
    };
    assert!(state.sidebar_layout.get_mut().add_favorite(&first_favorite));
    assert!(
        state
            .sidebar_layout
            .get_mut()
            .add_favorite(&second_favorite)
    );

    let sent_before = sent_count(&state);

    let request = state.update(Message::RequestRemoveAccount(first.id.clone()));
    assert_eq!(apply_task(&mut state, request), 0);
    assert_eq!(
        state.account_removal_confirmation(),
        Some((first.display_name.as_str(), first.email.as_str()))
    );
    let reject = state.update(Message::ConfirmRemoveAccount(
        enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent::Resolved(
            DismissCause::CancelAction,
        ),
    ));
    assert_eq!(apply_task(&mut state, reject), 0);
    assert!(state.store.account(&first.id).is_some());
    assert_eq!(
        credentials.contains(&first_key),
        CredentialPresence::Present
    );

    let request = state.update(Message::RequestRemoveAccount(first.id.clone()));
    assert_eq!(apply_task(&mut state, request), 0);
    let confirm = state.update(Message::ConfirmRemoveAccount(
        enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent::Resolved(
            DismissCause::DefaultAction,
        ),
    ));
    assert!(state.store.account(&first.id).is_some());
    assert_eq!(
        credentials.contains(&first_key),
        CredentialPresence::Present
    );
    assert_eq!(apply_task(&mut state, confirm), 0);

    assert_eq!(credentials.contains(&first_key), CredentialPresence::Absent);
    assert_eq!(
        credentials.contains(&second_key),
        CredentialPresence::Present
    );
    assert!(state.store.account(&first.id).is_none());
    assert!(state.store.account(&second.id).is_some());
    assert!(
        state
            .store
            .messages
            .iter()
            .all(|message| message.account_id != first.id)
    );
    assert!(state.store.messages.iter().any(|message| {
        message.account_id == second.id
            && message.subject == "second account survives"
            && message.attachments[0].name == "second.txt"
    }));
    assert!(state.pending_send.is_none());
    assert_eq!(sent_count(&state), sent_before);
    assert_eq!(state.active_account_id, second.id);
    assert!(matches!(
        &state.selection,
        SidebarSelection::Favorite(FavoriteKind::Inbox)
    ));
    assert!(state.settings.expanded_account.is_none());
    assert!(!state.collapsed_sections.contains(&first.id));
    assert!(
        state
            .pending_server_mutations
            .iter()
            .all(|mutation| { !mutation.target_ids.iter().any(|target| target == &first.id) })
    );
    assert!(state.pending_server_mutations.iter().any(|mutation| {
        mutation
            .target_ids
            .iter()
            .any(|target| target == &second.id)
    }));
    assert!(state.sidebar_layout.favorites().iter().all(|favorite| {
        !matches!(favorite, FavoriteTarget::Mailbox { account_id, .. } if account_id == &first.id)
    }));
    assert!(state.sidebar_layout.favorites().iter().any(|favorite| {
        matches!(favorite, FavoriteTarget::Mailbox { account_id, .. } if account_id == &second.id)
    }));

    state.pending_server_mutations.clear();
    state.compose.open = true;
    state.compose.from_account_id = second.id.clone();
    state.compose.to = "recipient@example.com".to_owned();
    state.compose.subject = "unsaved final-account compose".to_owned();
    let request = state.update(Message::RequestRemoveAccount(second.id));
    assert_eq!(apply_task(&mut state, request), 0);
    let confirm = state.update(Message::ConfirmRemoveAccount(
        enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent::Resolved(
            DismissCause::DefaultAction,
        ),
    ));
    assert_eq!(apply_task(&mut state, confirm), 0);
    assert_eq!(
        credentials.contains(&second_key),
        CredentialPresence::Absent
    );
    assert!(state.store.accounts.is_empty());
    assert!(state.store.messages.is_empty());
    assert!(state.pending_server_mutations.is_empty());
    assert!(state.active_account_id.is_empty());
    assert!(matches!(
        &state.selection,
        SidebarSelection::Favorite(FavoriteKind::Inbox)
    ));
    assert!(state.selected_conversation.is_none());
    assert!(!state.compose.open);
    assert!(state.compose.from_account_id.is_empty());
    assert_eq!(
        state.settings.page(&state.store),
        crate::model::settings::SettingsPage::Root
    );
    assert!(!state.settings.add_account.open);
    assert!(state.local_state_is_published());
    assert_eq!(state.preferences_published, state.preferences_token());

    let (restarted, _) =
        State::boot_for_test_with_local_state(crate::local_state::store_on(&local_backend), false);
    assert!(restarted.store.accounts.is_empty());
    assert!(restarted.store.messages.is_empty());
    assert!(restarted.pending_send.is_none());
    assert!(restarted.active_account_id.is_empty());
}

#[test]
fn account_removal_failures_publish_the_removal_and_keep_the_cleanup_journaled() {
    use enzui_services::credential_store::{
        FailingCredentialStore, PermissionDeniedCredentialStore, UnlockCancelledCredentialStore,
    };

    let cases: Vec<(CredentialStoreHandle, &str)> = vec![
        (
            CredentialStoreHandle(std::sync::Arc::new(
                enzui_services::credential_store::UnavailableCredentialStore,
            )),
            "unavailable",
        ),
        (
            CredentialStoreHandle(std::sync::Arc::new(PermissionDeniedCredentialStore)),
            "denied exact-key cleanup",
        ),
        (
            CredentialStoreHandle(std::sync::Arc::new(FailingCredentialStore::new(
                "generated failure",
            ))),
            "deletion was not confirmed",
        ),
        (
            CredentialStoreHandle(std::sync::Arc::new(UnlockCancelledCredentialStore)),
            "unlocking was cancelled",
        ),
    ];
    for (index, (store, expected)) in cases.into_iter().enumerate() {
        let mut state = production_state_for_mail_backend();
        let config = loopback_account_config(&format!("acct-remove-failure-{index}"));
        state
            .store
            .accounts
            .push(account_from_config(config.clone()));
        let mut message = model::fixture::seed().messages[0].clone();
        message.id = format!("message-remove-failure-{index}");
        message.account_id = config.id.clone();
        state.store.messages.push(message);
        state.credential_store = store;
        state.credential_store_ready = true;

        let request = state.update(Message::RequestRemoveAccount(config.id.clone()));
        assert_eq!(apply_task(&mut state, request), 0);
        let confirm = state.update(Message::ConfirmRemoveAccount(
            enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent::Resolved(
                DismissCause::DefaultAction,
            ),
        ));
        assert_eq!(apply_task(&mut state, confirm), 0);

        assert!(state.store.account(&config.id).is_none(), "{expected}");
        assert!(
            state
                .store
                .messages
                .iter()
                .all(|message| message.account_id != config.id)
        );
        let notice = state
            .mail_notice
            .as_ref()
            .expect("the cleanup debt is shown");
        assert_eq!(
            notice.title,
            "Configuration saved; credential cleanup pending"
        );
        assert!(notice.retryable);
        assert!(
            notice.body.contains(expected),
            "unexpected body: {}",
            notice.body
        );
        assert!(state.credential_mutations.journal().active().is_some());
    }

    let mut state = production_state_for_mail_backend();
    let config = loopback_account_config("acct-remove-retry");
    state
        .store
        .accounts
        .push(account_from_config(config.clone()));
    state.credential_store =
        CredentialStoreHandle(std::sync::Arc::new(PermissionDeniedCredentialStore));
    state.credential_store_ready = true;
    let request = state.update(Message::RequestRemoveAccount(config.id.clone()));
    assert_eq!(apply_task(&mut state, request), 0);
    let confirm = state.update(Message::ConfirmRemoveAccount(
        enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent::Resolved(
            DismissCause::DefaultAction,
        ),
    ));
    assert_eq!(apply_task(&mut state, confirm), 0);
    assert!(state.credential_mutations.journal().active().is_some());
    state.credential_store = CredentialStoreHandle(std::sync::Arc::new(
        enzui_services::credential_store::MemoryCredentialStore::new(),
    ));
    let retry = state.update(Message::RetryMailOperation);
    assert_eq!(apply_task(&mut state, retry), 0);
    assert!(state.store.account(&config.id).is_none());
    assert!(state.credential_mutations.journal().active().is_none());
}

#[test]
fn account_removal_cancellation_and_stale_completion_keep_current_account() {
    let gate = enzui_services::credential_store::Gate::new_closed();
    let credentials = std::sync::Arc::new(
        enzui_services::credential_store::MemoryCredentialStore::with_gate(gate),
    );
    let mut state = production_state_for_mail_backend();
    let config = loopback_account_config("acct-remove-cancel");
    state
        .store
        .accounts
        .push(account_from_config(config.clone()));
    state.store.mark_dirty();
    state.credential_store = CredentialStoreHandle(credentials);

    let request = state.update(Message::RequestRemoveAccount(config.id.clone()));
    assert_eq!(apply_task(&mut state, request), 0);
    let deletion = state.update(Message::ConfirmRemoveAccount(
        enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent::Resolved(
            DismissCause::DefaultAction,
        ),
    ));

    assert!(state.credential_inflight.is_none());
    assert!(
        state
            .account_removal
            .as_ref()
            .is_some_and(|removal| format!("{:?}", removal.phase).starts_with("AwaitingJournal"))
    );
    let cancel = state.update(Message::CancelRemoveAccount);
    assert_eq!(apply_task(&mut state, cancel), 0);
    assert_eq!(apply_task(&mut state, deletion), 0);
    assert!(state.store.account(&config.id).is_some());
    assert!(state.account_removal_status(&config.id).is_none());
    assert!(state.credential_mutations.journal().active().is_none());

    let stale_store =
        std::sync::Arc::new(enzui_services::credential_store::MemoryCredentialStore::new());
    state.credential_store = CredentialStoreHandle(stale_store);
    let request = state.update(Message::RequestRemoveAccount(config.id.clone()));
    assert_eq!(apply_task(&mut state, request), 0);
    let deletion = state.update(Message::ConfirmRemoveAccount(
        enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent::Resolved(
            DismissCause::DefaultAction,
        ),
    ));
    let mut replacement = config;
    replacement.display_name = "Replacement account".to_owned();
    state.store.accounts[0] = account_from_config(replacement.clone());
    assert_eq!(apply_task(&mut state, deletion), 0);
    assert!(state.store.account(&replacement.id).is_some_and(|account| {
        account.display_name == "Replacement account"
            && account.config.as_ref() == Some(&replacement)
    }));
    assert!(state.account_removal.is_none());
    assert!(state.credential_inflight.is_none());

    let close = state.update(Message::WindowCloseResolved(Some(
        iced::window::Id::unique(),
    )));
    assert_eq!(apply_close_task(&mut state, close), 1);
    assert!(state.close_committed);
}
