use super::*;

#[test]
fn the_picked_from_account_is_the_account_the_send_uses() {
    let mut s = boot_state();

    let mut work = s.store.accounts[0].clone();
    work.id = "work-acct".into();
    work.display_name = s.store.accounts[0].display_name.clone();
    work.email = "danny@work.example.com".into();
    s.store.accounts.push(work);
    assert!(s.store.accounts.len() >= 2);
    let default_id = s.active_account_id.clone();
    let other = s
        .store
        .accounts
        .iter()
        .find(|a| a.id != default_id)
        .expect("a second account")
        .clone();

    let _ = s.update(Message::OpenCompose(ComposeMode::New));
    assert_eq!(s.compose.from_account_id, default_id);

    let _ = s.update(Message::SetComposeFrom(other.id.clone()));
    assert_eq!(s.compose.from_account_id, other.id);

    let _ = s.update(Message::SetComposeFrom("removed-account".into()));
    assert_eq!(s.compose.from_account_id, other.id);

    let _ = s.update(Message::ComposeToEdited("dest@example.com".into()));
    let _ = s.update(Message::SendCompose);
    let pending = s.pending_send.as_ref().expect("a send was queued");
    assert_eq!(pending.account_id, other.id);
    assert_eq!(pending.sender_email, other.email);
    assert_ne!(pending.account_id, default_id);
}

#[test]
fn current_toast_commits_once_at_its_deadline_or_explicit_send() {
    for send_now in [false, true] {
        let mut s = boot_state();
        s.compose.open = true;
        s.compose.to = "y@example.com".into();
        s.compose.subject = "later".into();
        let sent_before = sent_count(&s);

        let _ = s.update(Message::SendCompose);
        let (deadline, _) = s.undo_send_expiry.armed().expect("wake is armed");
        let _ = s.update(Message::Toast(ToastEvent::Tick(
            deadline - Duration::from_millis(1),
        )));
        assert_eq!(sent_count(&s), sent_before);
        assert!(s.pending_send.is_some());

        let tracked = s.pending_undo_toast.unwrap();
        let event = if send_now {
            ToastEvent::Action(tracked.id, SEND_NOW_ACTION.into())
        } else {
            ToastEvent::Tick(deadline)
        };
        let _ = s.update(Message::Toast(event.clone()));
        assert_eq!(sent_count(&s), sent_before + 1);
        assert!(s.pending_send.is_none());
        assert!(s.undo_send_expiry.armed().is_none());

        let _ = s.update(Message::Toast(event));
        assert_eq!(sent_count(&s), sent_before + 1);
    }
}
