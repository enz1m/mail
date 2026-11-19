//! Confirmed links retain URI bytes and request ownership.

use super::*;
use crate::link_launcher::LinkOpener;
use crate::link_launcher::tests::SharedMockBackend;
use enzui_iced_core::portal::PortalOutcome;
use enzui_iced_core::portal::open_uri::OpenUriReply;

fn drive_task(task: Task<Message>) -> Vec<Message> {
    match iced_test::runtime::task::into_stream(task) {
        None => Vec::new(),
        Some(stream) => iced_test::futures::futures::executor::block_on(async move {
            use iced_test::futures::futures::StreamExt;
            stream
                .filter_map(|action| async move {
                    match action {
                        iced_test::runtime::Action::Output(message) => Some(message),
                        _ => None,
                    }
                })
                .collect()
                .await
        }),
    }
}

fn confirm(cause: DismissCause) -> Message {
    Message::ConfirmOpenUrl(enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent::Resolved(cause))
}

#[test]
fn confirmed_links_keep_uri_bytes_and_stale_wakes_cannot_settle_a_replacement() {
    let mock = SharedMockBackend::new();
    let (state, _) = State::boot_for_test();
    let mut state = state.with_link_opener(LinkOpener::with_backend(mock.boxed()));
    let _ = state.update(Message::RequestOpenUrl(
        "https://example.test/cancel".into(),
    ));
    assert!(drive_task(state.update(confirm(DismissCause::CancelAction))).is_empty());
    assert!(mock.lock().started.is_empty());
    assert!(state.pending_open_url.is_none());
    let href = "https://example.test/a%20b;$(literal)?next=%24HOME";
    let _ = state.update(Message::RequestOpenUrl(href.into()));
    assert!(mock.lock().started.is_empty());
    assert!(drive_task(state.update(confirm(DismissCause::DefaultAction))).is_empty());
    assert_eq!(
        mock.lock().started[0].1.as_str().as_bytes(),
        href.as_bytes()
    );
    let stale_wake = state.link_opener.live_wake();
    let _ = state.update(Message::RequestOpenUrl("https://example.test/next".into()));
    assert!(drive_task(state.update(confirm(DismissCause::DefaultAction))).is_empty());
    assert!(drive_task(state.update(Message::OpenUrlPortalTick(stale_wake))).is_empty());
    assert!(state.link_opener.is_pending());
    let live_id = mock.lock().started[1].0;
    mock.lock().script(live_id, OpenUriReply::Success);
    let wake = state.link_opener.live_wake();
    let outputs = drive_task(state.update(Message::OpenUrlPortalTick(wake)));
    assert!(matches!(
        outputs.as_slice(),
        [Message::OpenUrlFinished(PortalOutcome::Completed(()))]
    ));
    assert!(!state.link_opener.is_pending());
}
