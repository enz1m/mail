//! Account editing: authenticated candidates, auth/store failures, and
//! key cleanup.

use super::*;

#[test]
fn account_edit_auth_failure_keeps_old_account_secret_and_visible_draft() {
    use enzui_services::credential_store::CredentialPresence;

    let mut state = production_state_for_mail_backend();
    let credentials =
        std::sync::Arc::new(enzui_services::credential_store::MemoryCredentialStore::new());
    let old = loopback_account_config("acct-edit-failure");
    let old_key = State::credential_key(&old);
    let outcome = iced_test::futures::futures::executor::block_on(credentials.set(
        old_key.clone(),
        Secret::new(b"old-generated-password".to_vec()),
    ));
    assert_eq!(outcome, CredentialOutcome::Stored);
    state.credential_store = CredentialStoreHandle(std::sync::Arc::<
        enzui_services::credential_store::MemoryCredentialStore,
    >::clone(&credentials));
    state.mail_backend = std::sync::Arc::new(RejectingEditMailBackend);
    state.store.accounts.push(account_from_config(old.clone()));

    let begin = state.update(Message::BeginEditAccount(old.id.clone()));
    assert_eq!(apply_task(&mut state, begin), 0);
    let edit = state.update(Message::EditAccountFieldEdited(
        DraftField::IncomingServer,
        "rejected.example.com".to_owned(),
    ));
    assert_eq!(apply_task(&mut state, edit), 0);
    let password = state.update(Message::EditAccountPasswordEdited(
        "rejected-password".to_owned(),
    ));
    assert_eq!(apply_task(&mut state, password), 0);
    let commit = state.update(Message::CommitEditAccount);
    assert_eq!(apply_task(&mut state, commit), 0);

    assert_eq!(
        state
            .store
            .account(&old.id)
            .and_then(|account| account.config.as_ref()),
        Some(&old)
    );
    assert_eq!(credentials.contains(&old_key), CredentialPresence::Present);
    assert_eq!(credentials.entry_count(), 1);
    assert_eq!(
        state.settings.edit_account.draft.imap_server,
        "rejected.example.com"
    );

    assert!(state.account_edit_status(&old.id).is_none());
    assert_eq!(
        state.settings.edit_account.account_id.as_deref(),
        Some(old.id.as_str())
    );
    assert!(
        state
            .settings
            .edit_account
            .error
            .as_deref()
            .is_some_and(|error| error.contains("rejected")),
        "the form says why: {:?}",
        state.settings.edit_account.error
    );
    let cancel = state.update(Message::CancelEditAccount);
    assert_eq!(apply_task(&mut state, cancel), 0);
    assert!(state.settings.edit_account.account_id.is_none());
    assert_eq!(credentials.contains(&old_key), CredentialPresence::Present);
    let task = state.update(Message::BeginEditAccount(old.id));
    apply_task(&mut state, task);
    let _ = state.update(Message::EditAccountPasswordEdited(
        "rejected-password".into(),
    ));
    let task = state.update(Message::CommitEditAccount);
    apply_task(&mut state, task);
    let retained = iced_test::futures::futures::executor::block_on(credentials.get(old_key));
    assert!(matches!(retained, CredentialOutcome::Retrieved(secret)
        if secret.expose_secret() == b"old-generated-password"));
}

#[test]
fn account_edit_stale_authentication_cannot_replace_a_changed_account() {
    use enzui_services::credential_store::CredentialPresence;

    let old = loopback_account_config("acct-edit-stale");
    let old_key = State::credential_key(&old);
    let credentials =
        std::sync::Arc::new(enzui_services::credential_store::MemoryCredentialStore::new());
    let outcome = iced_test::futures::futures::executor::block_on(credentials.set(
        old_key.clone(),
        Secret::new(b"old-generated-password".to_vec()),
    ));
    assert_eq!(outcome, CredentialOutcome::Stored);
    let backend = std::sync::Arc::new(SuccessfulMailBackend::default());
    let mut state = production_state_for_mail_backend();
    state.credential_store = CredentialStoreHandle(std::sync::Arc::<
        enzui_services::credential_store::MemoryCredentialStore,
    >::clone(&credentials));
    state.mail_backend =
        std::sync::Arc::<crate::reducer_tests::SuccessfulMailBackend>::clone(&backend);
    state.store.accounts.push(account_from_config(old.clone()));

    let begin = state.update(Message::BeginEditAccount(old.id.clone()));
    assert_eq!(apply_task(&mut state, begin), 0);
    let edit = state.update(Message::EditAccountFieldEdited(
        DraftField::IncomingServer,
        "stale.example.com".to_owned(),
    ));
    assert_eq!(apply_task(&mut state, edit), 0);
    let password = state.update(Message::EditAccountPasswordEdited(
        "generated-password".to_owned(),
    ));
    assert_eq!(apply_task(&mut state, password), 0);
    let commit = state.update(Message::CommitEditAccount);

    let mut task = super::settle_credential_journal(&mut state, commit);
    let authenticating = |state: &State| {
        matches!(
            state.account_edit.as_ref().map(|edit| &edit.phase),
            Some(
                AccountEditPhase::AuthenticatingBeforeStore(_)
                    | AccountEditPhase::AuthenticatingStoredCandidate(_)
            )
        )
    };
    for _ in 0..8 {
        if authenticating(&state) {
            break;
        }
        task = iced::Task::batch(super::apply_task_wave(&mut state, task));
    }
    assert!(authenticating(&state));

    let mut replacement = old.clone();
    replacement.display_name = "Replacement account".to_owned();
    state.store.accounts[0] = account_from_config(replacement.clone());
    state.store.mark_dirty();
    assert_eq!(apply_task(&mut state, task), 0);

    assert!(state.store.account(&replacement.id).is_some_and(|account| {
        account.display_name == "Replacement account"
            && account.config.as_ref() == Some(&replacement)
    }));
    assert_eq!(credentials.contains(&old_key), CredentialPresence::Present);
    assert_eq!(credentials.entry_count(), 1);
    assert_eq!(
        backend.calls.lock().expect("mail calls lock").as_slice(),
        ["probe"]
    );
    assert!(state.account_edit_status(&old.id).is_none());
    assert_eq!(
        state.settings.edit_account.error.as_deref(),
        Some("The account changed while authentication was settling.")
    );
    let close = state.update(Message::WindowCloseResolved(Some(
        iced::window::Id::unique(),
    )));
    assert_eq!(apply_close_task(&mut state, close), 1);
    assert!(state.close_committed);
}
