use super::*;
use model::compose::ComposeFormat;

#[test]
fn undo_send_restores_the_decorations_the_user_applied() {
    use enzui_iced_widgets::rich_edit::{InlineStyle, RichEditAction};
    use iced::widget::text_editor::{Action, Motion};

    let (mut state, _) = State::boot_for_test();
    let _ = state.update(Message::OpenCompose(ComposeMode::New));
    state.compose.body.clear();
    state.sync_body_editor();
    let _ = state.update(Message::SetComposeFormat(ComposeFormat::Rich));
    let _ = state.update(Message::ComposeRichEditAction(RichEditAction::Paste(
        "control WWWWWWWW".into(),
    )));
    let _ = state.update(Message::ComposeRichEditAction(RichEditAction::Native(
        Action::Move(Motion::DocumentEnd),
    )));
    for _ in 0..8 {
        let _ = state.update(Message::ComposeRichEditAction(RichEditAction::Native(
            Action::Select(Motion::Left),
        )));
    }
    assert_eq!(state.rich_controller.selection(), 8..16);
    for style in [InlineStyle::Underline, InlineStyle::Strikethrough] {
        let _ = state.update(Message::ComposeRichEditAction(RichEditAction::ToggleStyle(
            style,
        )));
    }
    let _ = state.update(Message::SetComposeFormat(ComposeFormat::Plain));
    assert!(state.compose.body_html.is_none());
    let plain = state.compose.body.clone();
    let _ = state.update(Message::SetComposeFormat(ComposeFormat::Rich));
    assert_eq!(state.compose.body, plain);
    let _ = state.update(Message::ComposeToEdited("test@example.com".into()));
    let _ = state.update(Message::SendCompose);
    assert!(!state.compose.open);

    let id = state.pending_undo_toast.unwrap().id;
    let _ = state.update(Message::Toast(ToastEvent::Action(
        id,
        compose_view::DISCARD_LOCAL_SAVE_LABEL.into(),
    )));
    assert!(state.compose.open);
    assert_eq!(state.compose.format, ComposeFormat::Rich);
    for style in [InlineStyle::Underline, InlineStyle::Strikethrough] {
        assert!(
            state
                .rich_controller
                .document()
                .is_style_active(style, 8..16)
        );
    }
}
