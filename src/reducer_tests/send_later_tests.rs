use super::*;
use crate::model::sidebar_layout::SmartMailboxKind;
use crate::model::snooze::SnoozePreset;
use crate::model::types::MessageDate;

fn at(day: u32, hour: u32) -> MessageDate {
    MessageDate {
        year: 2026,
        month: 10,
        day,
        minute: hour * 60,
    }
}

fn clock(state: &mut State, now: MessageDate) {
    state.local_clock = std::sync::Arc::new(crate::local_clock::FixedLocalClock::new(now));
}

fn scheduled(state: &mut State) -> String {
    let _ = state.update(Message::OpenCompose(model::compose::ComposeMode::New));
    state.compose.to = "someone@example.com".to_owned();
    state.compose.subject = "Later".to_owned();
    state.compose.body = "Sent while you slept".to_owned();
    state.sync_body_editor();
    let _ = state.schedule_send(SnoozePreset::Tomorrow);
    state
        .store
        .messages
        .iter()
        .find(|m| m.send_at.is_some())
        .map(|m| m.id.clone())
        .expect("a scheduled draft")
}

fn open_custom_schedule(state: &mut State) {
    let _ = state.update(Message::OpenSchedule);
}

fn compose_ready(state: &mut State) {
    let _ = state.update(Message::OpenCompose(model::compose::ComposeMode::New));
    state.compose.to = "someone@example.com".into();
    state.compose.subject = "Exact scheduled time".into();
    state.compose.body = "Ready for later".into();
    state.sync_body_editor();
}

#[test]
fn send_later_confirms_an_exact_time_and_routes_to_a_visible_scheduled_message() {
    let (mut state, _) = State::boot_for_test();
    clock(&mut state, at(7, 22));
    state.sidebar_layout.get_mut().set_smart_visible(false);
    state
        .collapsed_sections
        .insert(crate::view::SMART_SECTION.into());
    compose_ready(&mut state);
    open_custom_schedule(&mut state);
    assert!(state.schedule_editor.is_some());
    assert!(state.compose.open);
    assert!(!state.store.messages.iter().any(|m| m.send_at.is_some()));
    let _ = state.update(Message::ScheduleChoice(
        SnoozePreset::LaterToday.label().into(),
    ));
    let editor = state.schedule_editor.as_ref().unwrap();
    assert_eq!(
        editor.date,
        enzui_iced_core::civil_date::Date::new(2026, 10, 8).unwrap()
    );
    assert_eq!(
        editor.draft,
        crate::schedule::time_format()
            .format(enzui_iced_core::time_of_day::TimeOfDay::hm(1, 0).unwrap())
    );
    let _ = state.update(Message::ScheduleDate(
        enzui_iced_widgets::date_picker::DatePickerEvent::Calendar(
            enzui_iced_widgets::calendar::CalendarEvent::DaySelected(
                enzui_iced_core::civil_date::Date::new(2026, 10, 9).unwrap(),
            ),
        ),
    ));
    let _ = state.update(Message::ScheduleTime(
        enzui_iced_widgets::time_field::TimeFieldEvent::Drafted(
            crate::schedule::time_format()
                .format(enzui_iced_core::time_of_day::TimeOfDay::hm(13, 42).unwrap()),
        ),
    ));
    assert!(
        state
            .resolve_shortcut(crate::shortcuts::ShortcutAction::SendCompose)
            .is_none()
    );
    let _ = state.update(Message::Shortcut(
        crate::shortcuts::ShortcutAction::SendCompose,
    ));
    let _ = state.update(Message::SendCompose);
    assert!(state.pending_send.is_none());
    let _ = state.update(Message::ConfirmSchedule);
    assert!(state.schedule_editor.is_none());
    assert!(!state.compose.open);
    let message = state
        .store
        .messages
        .iter()
        .find(|m| m.send_at.is_some())
        .unwrap();
    assert_eq!(
        message.send_at,
        Some(MessageDate {
            minute: 13 * 60 + 42,
            ..at(9, 0)
        })
    );
    assert_eq!(
        state.selection,
        SidebarSelection::SmartMailbox(SmartMailboxKind::Scheduled)
    );
    assert!(
        !state
            .collapsed_sections
            .contains(crate::view::SMART_SECTION)
    );
    assert!(state.sidebar_layout.smart_visible());
    let conversation = state
        .store
        .conversations_for(&state.selection)
        .into_iter()
        .find(|c| Some(&c.id) == state.selected_conversation.as_ref())
        .unwrap();
    assert!(conversation.message_ids.contains(&message.id));
}

#[test]
fn deleting_a_scheduled_message_keeps_it_in_trash_and_never_sends_it() {
    let (mut state, _) = State::boot_for_test();
    clock(&mut state, at(7, 22));
    let id = scheduled(&mut state);
    let _ = state.update(Message::MailAction(
        crate::mail_actions::MailAction::Delete,
        crate::mail_actions::ActionTarget::Message {
            message_id: id.clone(),
        },
    ));
    assert!(
        state
            .store
            .conversations_for(&SidebarSelection::AccountMailbox {
                account_id: "icloud".into(),
                mailbox: MailboxKind::Trash
            })
            .iter()
            .any(|c| c.message_ids.contains(&id))
    );
    assert!(
        state
            .store
            .conversations_for(&SidebarSelection::SmartMailbox(SmartMailboxKind::Scheduled))
            .is_empty()
    );
    clock(&mut state, at(9, 12));
    let _ = state.update(Message::SnoozeWake);
    assert!(state.pending_send.is_none());
}
