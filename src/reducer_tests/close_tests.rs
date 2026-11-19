use super::*;

#[test]
fn dirty_state_closes_only_after_the_current_publication() {
    let mut state = boot_state();
    state.store.mark_dirty();
    let stale_token = state.local_state_token();
    state.store.mark_dirty();
    let window = iced::window::Id::unique();

    let close = state.update(Message::WindowCloseResolved(Some(window)));
    assert!(state.close_settlement.is_some());
    let stale = state.update(Message::LocalStateSaved(stale_token, Ok(())));
    drop(stale);
    assert!(!state.close_committed);
    assert!(!state.local_state_is_published());
    assert_eq!(apply_close_task(&mut state, close), 1);

    assert!(state.local_state_is_published());
    assert!(state.close_settlement.is_none());
    assert!(!state.close_guard.is_pending());
    assert!(state.close_committed);
}

#[test]
fn close_autosaves_the_open_compose_before_window_exit() {
    use enzui_iced_core::persist::InMemoryBackend;

    let backend = InMemoryBackend::new();
    let (mut state, _) =
        State::boot_for_test_with_local_state(crate::local_state::store_on(&backend), true);
    let _ = state.update(Message::OpenCompose(ComposeMode::New));
    state.compose.to = "close@example.com".into();
    state.compose.subject = "Close-safe draft".into();
    state.compose.body = "Persist before exiting".into();

    let close = state.update(Message::WindowCloseResolved(Some(
        iced::window::Id::unique(),
    )));
    assert_eq!(apply_close_task(&mut state, close), 1);

    let (restarted, _) =
        State::boot_for_test_with_local_state(crate::local_state::store_on(&backend), false);
    assert!(restarted.store.messages.iter().any(|message| {
        message.subject == "Close-safe draft"
            && matches!(
                &message.location,
                MessageLocation::Mailbox(MailboxKind::Drafts)
            )
    }));
}

#[test]
fn local_timestamp_failure_retains_draft_and_pending_send_until_retry() {
    let mut state = boot_state();
    let clock = std::sync::Arc::new(ManualLocalClock::new(Err(
        enzui_iced_core::local_time::LocalCivilTimeError::TimezoneConversionFailed,
    )));
    state.local_clock = std::sync::Arc::<crate::reducer_tests::ManualLocalClock>::clone(&clock);

    let _ = state.update(Message::OpenCompose(ComposeMode::New));
    state.compose.to = "draft@example.com".into();
    state.compose.subject = "Retry draft timestamp".into();
    state.compose.body = "must stay recoverable".into();
    let failed = state.update(Message::SaveDraft);
    drop(failed);
    assert!(state.compose.open);
    assert!(
        !state
            .store
            .messages
            .iter()
            .any(|message| message.subject == "Retry draft timestamp")
    );
    assert!(state.local_timestamp_notice().is_some());

    let drafted_at = MessageDate::at(2025, 3, 9, 3, 0);
    clock.set(Ok(drafted_at));
    let retry = state.update(Message::RetryLocalTimestamp);
    let _ = apply_task(&mut state, retry);
    assert!(!state.compose.open);
    assert_eq!(
        state
            .store
            .messages
            .iter()
            .find(|message| message.subject == "Retry draft timestamp")
            .expect("retried draft")
            .date,
        drafted_at
    );

    let _ = state.update(Message::OpenCompose(ComposeMode::New));
    state.compose.to = "sent@example.com".into();
    state.compose.subject = "Retry sent timestamp".into();
    state.compose.body = "pending send remains recoverable".into();
    let _ = state.update(Message::SendCompose);
    clock.set(Err(
        enzui_iced_core::local_time::LocalCivilTimeError::TimezoneConversionFailed,
    ));
    expire_pending_send(&mut state);
    assert!(state.pending_send.is_some());
    assert!(state.local_timestamp_notice().is_some());
    assert!(
        !state
            .store
            .messages
            .iter()
            .any(|message| message.subject == "Retry sent timestamp")
    );

    let sent_at = MessageDate::at(2025, 11, 2, 1, 15);
    clock.set(Ok(sent_at));
    let retry = state.update(Message::RetryLocalTimestamp);
    let _ = apply_task(&mut state, retry);
    assert!(state.pending_send.is_none());
    assert_eq!(
        state
            .store
            .messages
            .iter()
            .find(|message| message.subject == "Retry sent timestamp")
            .expect("retried local sent copy")
            .date,
        sent_at
    );
}

#[test]
fn close_deadline_blocks_mutation_and_requires_retry_cancel_or_discard() {
    let backend = CloseRefusingBackend::default();
    let saves = std::sync::Arc::clone(&backend.saves);
    let pause = std::sync::Arc::clone(&backend.pause);
    let store = enzui_iced_core::persist::Store::<crate::local_state::LocalState>::new(
        backend,
        crate::local_state::FORMAT_ID,
        crate::local_state::VERSION,
    );
    let (mut state, _) = State::boot_for_test_with_local_state(store, true);
    state.store.mark_dirty();
    let close = state.update(Message::WindowCloseResolved(Some(
        iced::window::Id::unique(),
    )));
    assert_eq!(apply_task_allowing_local_failure(&mut state, close), 0);

    let (generation, retry_at, deadline) = state
        .close_settlement
        .as_ref()
        .map(|settlement| {
            (
                settlement.generation,
                settlement.next_retry,
                settlement.deadline,
            )
        })
        .expect("close drain is active");
    let retry_task = state.update(Message::CloseSettlementTick(generation, retry_at));
    assert_eq!(apply_task_allowing_local_failure(&mut state, retry_task), 0);
    assert!(saves.load(std::sync::atomic::Ordering::Relaxed) >= 2);
    let _ = state.update(Message::CloseSettlementTick(generation, deadline));
    assert!(state.close_notice().is_some_and(|(_, failed)| failed));

    state.compose.subject = "unchanged".into();
    let _ = state.update(Message::ComposeSubjectEdited("blocked".into()));
    assert_eq!(state.compose.subject, "unchanged");

    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *pause.lock().expect("install close write pause") = Some(PausedCloseWrite {
        entered: entered_tx,
        release: release_rx,
    });
    let retry = state.update(Message::RetryCloseSettlement);
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("retried persistence must reach the backend");
    let retry_generation = state
        .close_settlement
        .as_ref()
        .map(|settlement| settlement.generation)
        .expect("retry starts a new drain");
    assert_ne!(retry_generation, generation);
    let _ = state.update(Message::CloseSettlementTick(generation, deadline));
    assert!(state.close_notice().is_some_and(|(_, failed)| !failed));
    drop(retry);
    assert_eq!(state.blocking_pool_live(), 1);

    let discard = state.update(Message::DiscardCloseSettlement);
    assert_eq!(apply_task(&mut state, discard), 0);
    assert!(!state.close_committed);
    assert!(state.close_notice().is_some_and(|(_, failed)| failed));
    assert_eq!(state.blocking_pool_live(), 1);

    release_tx
        .send(())
        .expect("release the held persistence call");
    let drain_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state.blocking_pool_live() != 0 && std::time::Instant::now() < drain_deadline {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(state.blocking_pool_live(), 0);
    let discard = state.update(Message::DiscardCloseSettlement);
    assert_eq!(apply_task(&mut state, discard), 1);
    assert!(state.close_settlement.is_none());
    assert!(state.close_committed);
}
