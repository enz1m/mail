use super::*;
use crate::mail_actions::{ActionTarget, MailAction};

fn imap_state(ids: &[&str]) -> State {
    let mut state = production_state_for_mail_backend();
    let config = loopback_account_config("acct-undo");
    state
        .store
        .accounts
        .push(account_from_config(config.clone()));
    let seed = model::fixture::seed().messages;
    for (index, id) in ids.iter().enumerate() {
        let mut message = seed[index].clone();
        message.id = (*id).to_owned();
        message.account_id = config.id.clone();
        message.location = MessageLocation::Mailbox(MailboxKind::Inbox);
        message.subject = format!("Subject {index}");
        message.unread = false;
        state.store.messages.push(message);
    }
    state.store.mark_dirty();
    state.selection = SidebarSelection::AccountMailbox {
        account_id: config.id,
        mailbox: MailboxKind::Inbox,
    };
    state
}

fn act(state: &mut State, action: MailAction, id: &str) {
    let _ = state.update(Message::MailAction(
        action,
        ActionTarget::Message {
            message_id: id.to_owned(),
        },
    ));
}

fn location(state: &State, id: &str) -> Option<MessageLocation> {
    state.store.message(id).map(|m| m.location.clone())
}

fn dispatchable(state: &State) -> Option<u64> {
    state.next_dispatchable_mutation().map(|m| m.id)
}

#[test]
fn a_move_stays_local_until_undo_expires_or_restores_the_original_selection() {
    for undo in [true, false] {
        let mut state = imap_state(&["imap-acct-undo-71-9"]);
        let conversation = state.store.conversations_for(&state.selection)[0].clone();
        let id = conversation.message_ids[0].clone();
        let _ = state.update(Message::SelectConversation {
            id: conversation.id.clone(),
            toggle: false,
            range: false,
        });
        act(
            &mut state,
            if undo {
                MailAction::Delete
            } else {
                MailAction::Archive
            },
            &id,
        );
        assert_eq!(
            location(&state, &id),
            Some(MessageLocation::Mailbox(if undo {
                MailboxKind::Trash
            } else {
                MailboxKind::Archive
            }))
        );
        let mutation = state.pending_server_mutations.last().unwrap().id;
        assert_eq!(dispatchable(&state), None);
        if undo {
            let toast = state.undo_send_toasts.visible_ids()[0];
            let _ = state.update(Message::Toast(ToastEvent::Action(
                toast,
                UNDO_MOVE_LABEL.to_owned(),
            )));
            assert_eq!(
                location(&state, &id),
                Some(MessageLocation::Mailbox(MailboxKind::Inbox))
            );
            assert!(state.pending_server_mutations.is_empty());
            assert_eq!(state.selected_conversation, Some(conversation.id));
        } else {
            let later = std::time::Instant::now() + UNDO_MOVE_WINDOW + Duration::from_secs(1);
            let _ = state.update(Message::Toast(ToastEvent::Tick(later)));
            assert_eq!(dispatchable(&state), Some(mutation));
        }
        assert!(state.held_mutations.is_empty());
        assert!(state.undo_send_toasts.is_empty());
    }
}
