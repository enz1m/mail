use super::*;

// Separate subscription identities let an open search popup own navigation keys.
pub(crate) fn classify_closed_search_event(
    event: iced::Event,
    status: iced::event::Status,
    id: iced::window::Id,
) -> Option<Message> {
    classify_event_with_search(&event, status, false, id)
}

pub(crate) fn classify_open_search_event(
    event: iced::Event,
    status: iced::event::Status,
    id: iced::window::Id,
) -> Option<Message> {
    classify_event_with_search(&event, status, true, id)
}

// Search-aware event classifier shared by both subscription function pointers
// and route tests. While the suggestion list is open it owns Escape, arrows,
// Home/End, Enter and Tab, so Tab advances the highlight instead of moving
// focus into the obscured shell.
pub(crate) fn classify_event_with_search(
    event: &iced::Event,
    status: iced::event::Status,
    search_popup_open: bool,
    window: iced::window::Id,
) -> Option<Message> {
    use enzui_iced_core::context_menu::MenuMove;
    use iced::keyboard::key::Named;

    if search_popup_open
        && let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Named(key),
            modifiers,
            ..
        }) = event
    {
        return match key {
            Named::Escape => Some(Message::SearchDismissed),
            Named::ArrowDown => Some(Message::SearchNavigate(MenuMove::Next)),
            Named::ArrowUp => Some(Message::SearchNavigate(MenuMove::Previous)),
            Named::Home => Some(Message::SearchNavigate(MenuMove::First)),
            Named::End => Some(Message::SearchNavigate(MenuMove::Last)),
            Named::Tab if modifiers.shift() => Some(Message::SearchNavigate(MenuMove::Previous)),
            Named::Tab => Some(Message::SearchNavigate(MenuMove::Next)),
            Named::Enter => Some(Message::SearchCommitHighlighted),
            _ => None,
        };
    }

    if let Some(traversal) = enzui_iced_core::focus::traversal_from_event(event, status) {
        return Some(match traversal {
            Traversal::Next => Message::FocusNext,
            Traversal::Previous => Message::FocusPrevious,
        });
    }

    match event {
        iced::Event::Window(iced::window::Event::CloseRequested) => {
            Some(Message::WindowCloseResolved(Some(window)))
        }
        iced::Event::Window(iced::window::Event::FileDropped(path)) => {
            Some(Message::DroppedAttachment(path.clone()))
        }
        iced::Event::Window(iced::window::Event::Resized(size)) => {
            Some(Message::WindowResized(*size))
        }
        iced::Event::Keyboard(kb_event @ iced::keyboard::Event::KeyPressed { .. }) => {
            // Preserve capture and repeat flags so typing cannot trigger bare-key mail commands.
            Some(Message::KeyPressed {
                event: kb_event.clone(),
                captured: status == iced::event::Status::Captured,
            })
        }
        _ => None,
    }
}

pub(crate) fn classify_draft_event(
    event: iced::Event,
    _status: iced::event::Status,
    _window: iced::window::Id,
) -> Option<Message> {
    match event {
        iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
            Some(Message::DraftPointerMoved(position))
        }
        iced::Event::Mouse(iced::mouse::Event::ButtonPressed(_)) => Some(Message::DraftClickAway),
        iced::Event::Window(iced::window::Event::Unfocused) => Some(Message::DraftWindowBlur),
        _ => None,
    }
}
