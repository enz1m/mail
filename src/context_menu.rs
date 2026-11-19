use enzui_iced_widgets::context_menu_host::{ContextMenu, ContextMenuItem, FocusRestoreTarget};

use crate::Message as Msg;
use crate::mail_actions::{self, ActionState, ActionTarget};
use crate::model::bulk::{emptiable_mailbox, empty_action_label, mailbox_count_for};
use crate::model::sidebar_layout::SidebarLayout;
use crate::model::types::{MailboxKind, MailboxStore, SidebarSelection};

pub(crate) fn conversation_target(
    store: &MailboxStore,
    selection: &SidebarSelection,
    conversation_id: &str,
) -> Option<ActionTarget> {
    let convo = store
        .conversations_for(selection)
        .into_iter()
        .find(|c| c.id == conversation_id)?;
    Some(ActionTarget::Conversation {
        conversation_id: conversation_id.to_owned(),
        newest_message_id: convo.message_ids.last()?.clone(),
    })
}

pub(crate) fn conversation_items(
    state: ActionState,
    target: &ActionTarget,
) -> Vec<ContextMenuItem<Msg>> {
    let mut items = Vec::new();
    let mut previous = None;
    for action in mail_actions::actions_for(state, target) {
        let group = action.group();
        if previous.is_some_and(|previous| previous != group) {
            items.push(ContextMenuItem::separator());
        }
        previous = Some(group);
        items.push(
            ContextMenuItem::command(action.message(target), action.label(target))
                .icon(action.icon())
                .destructive(action.destructive()),
        );
    }
    items
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SidebarRow {
    MarkAllRead,
    AddFavorite,
    // Unpin it. Offered instead of [`Self::AddFavorite`], never beside it.
    RemoveFavorite,
    // Take this smart mailbox out of its section. The row appears only on a
    // smart mailbox, which is also the only row that cannot be favourited.
    RemoveSmart,
    Empty(MailboxKind),
}

impl SidebarRow {
    pub(crate) fn label(self) -> String {
        crate::l10n::tr(match self {
            SidebarRow::MarkAllRead => "Mark All as Read",
            SidebarRow::AddFavorite => "Add to Favorites",
            SidebarRow::RemoveFavorite => "Remove from Favorites",
            SidebarRow::RemoveSmart => "Remove from Smart Mailboxes",
            SidebarRow::Empty(kind) => empty_action_label(kind).unwrap_or("Empty"),
        })
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            SidebarRow::MarkAllRead => "mail-open",
            SidebarRow::AddFavorite => "star",
            SidebarRow::RemoveFavorite | SidebarRow::RemoveSmart => "star-off",
            SidebarRow::Empty(_) => "trash-2",
        }
    }

    pub(crate) fn destructive(self) -> bool {
        matches!(self, SidebarRow::Empty(_))
    }
}

pub(crate) fn sidebar_rows(
    store: &MailboxStore,
    layout: &SidebarLayout,
    selection: &SidebarSelection,
) -> Vec<SidebarRow> {
    let mut rows = vec![SidebarRow::MarkAllRead];
    match selection {
        SidebarSelection::SmartMailbox(_) => rows.push(SidebarRow::RemoveSmart),
        _ if layout.is_favorite(selection) => rows.push(SidebarRow::RemoveFavorite),
        _ => rows.push(SidebarRow::AddFavorite),
    }
    if let SidebarSelection::AccountMailbox { account_id, .. } = selection
        && let Some(kind) = emptiable_mailbox(selection)
        && empty_action_label(kind).is_some()
        && mailbox_count_for(store, kind, account_id) > 0
    {
        rows.push(SidebarRow::Empty(kind));
    }
    rows
}

pub(crate) fn sidebar_items(
    store: &MailboxStore,
    layout: &SidebarLayout,
    selection: &SidebarSelection,
) -> Option<Vec<ContextMenuItem<Msg>>> {
    let items: Vec<_> = sidebar_rows(store, layout, selection)
        .into_iter()
        .map(|row| {
            ContextMenuItem::command(sidebar_message(row, selection), row.label())
                .icon(row.icon())
                .destructive(row.destructive())
        })
        .collect();
    (!items.is_empty()).then_some(items)
}

// The message a sidebar row dispatches for the right-clicked row, which is
// usually not the viewed mailbox `Msg::MarkAllRead` would act on.
pub(crate) fn sidebar_message(row: SidebarRow, selection: &SidebarSelection) -> Msg {
    match row {
        SidebarRow::MarkAllRead => Msg::ContextMarkAllRead(selection.clone()),
        SidebarRow::AddFavorite => Msg::AddFavorite(selection.clone()),
        SidebarRow::RemoveFavorite => Msg::RemoveFavorite(selection.clone()),
        SidebarRow::RemoveSmart => Msg::RemoveSmartMailbox(selection.clone()),
        SidebarRow::Empty(kind) => Msg::RequestEmptyMailbox(
            kind,
            match selection {
                SidebarSelection::AccountMailbox { account_id, .. } => account_id.clone(),
                _ => String::new(),
            },
        ),
    }
}

pub(crate) fn message_selection_items(text: &str) -> Vec<ContextMenuItem<Msg>> {
    vec![
        ContextMenuItem::command(
            Msg::CopyMessageSelection(text.to_owned()),
            crate::l10n::tr("Copy"),
        )
        .icon("copy"),
        ContextMenuItem::command(
            Msg::ClearMessageSelection,
            crate::l10n::tr("Clear selection"),
        )
        .icon("x"),
    ]
}

pub(crate) fn attachment_items(message_id: &str, name: &str) -> Vec<ContextMenuItem<Msg>> {
    vec![
        ContextMenuItem::command(
            Msg::ShowAttachmentDetails {
                message_id: message_id.to_owned(),
                name: name.to_owned(),
            },
            crate::l10n::tr("Attachment details"),
        )
        .icon("info"),
    ]
}

// Which surface a right-click opened the menu on.
//
// A menu retains the invoked row rather than following the viewed mailbox.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ContextTarget {
    Conversation {
        id: String,
    },
    Sidebar {
        selection: SidebarSelection,
        row_key: String,
    },
    Attachment {
        message_id: String,
        name: String,
    },
    MessageSelection {
        text: String,
    },
}

impl ContextTarget {
    // The accessible name of the menu surface, so AT announces which menu it
    // entered rather than a bare "Menu".
    pub(crate) fn menu_label(&self) -> &'static str {
        match self {
            ContextTarget::Conversation { .. } => "Conversation actions",
            ContextTarget::Sidebar { .. } => "Mailbox actions",
            ContextTarget::Attachment { .. } => "Attachment actions",
            ContextTarget::MessageSelection { .. } => "Selected text actions",
        }
    }
}

// The focus target to restore when a menu on `target` closes. Sidebar
// targets carry their exact section-qualified row key because a pinned
// mailbox and its account-section row share the same `SidebarSelection`.
pub(crate) fn context_menu_invoker(
    target: &ContextTarget,
    store: &MailboxStore,
    selection: &SidebarSelection,
) -> Option<iced::advanced::widget::Id> {
    match target {
        ContextTarget::Conversation { id } => store
            .conversations_for(selection)
            .into_iter()
            .find(|c| &c.id == id)
            .map(|c| crate::view::conversation_row_id(&c)),
        ContextTarget::Sidebar {
            selection: row_sel,
            row_key,
        } => crate::view::sidebar_row_id_for_key(row_sel, row_key, store),
        ContextTarget::Attachment { message_id, name } => {
            store.message(message_id).and_then(|message| {
                message
                    .attachments
                    .iter()
                    .any(|attachment| attachment.name == *name)
                    .then(|| crate::view::attachment_row_id(message_id, name))
            })
        }
        // A thread's cards have distinct focus stops; use the request's origin.
        ContextTarget::MessageSelection { .. } => None,
    }
}

pub(crate) type OpenContextMenu = ContextMenu<ContextTarget, Msg>;

pub(crate) fn open(
    target: ContextTarget,
    items: Vec<ContextMenuItem<Msg>>,
    at: iced::Point,
    invoker: Option<iced::advanced::widget::Id>,
) -> OpenContextMenu {
    with_invocation(
        target,
        items,
        enzui_iced_core::context_menu::InvocationKind::Pointer(
            enzui_iced_core::context_menu::Point::new(at.x, at.y),
        ),
        invoker,
    )
}

pub(crate) fn with_invocation(
    target: ContextTarget,
    items: Vec<ContextMenuItem<Msg>>,
    invocation: enzui_iced_core::context_menu::InvocationKind,
    invoker: Option<iced::advanced::widget::Id>,
) -> OpenContextMenu {
    let mut menu = ContextMenu::new();
    match invoker {
        Some(id) => menu.open_restoring(target, invocation, items, FocusRestoreTarget::widget(id)),
        None => menu.open(target, invocation, items),
    }
    menu
}

impl crate::State {
    // A reopened target must not accept queued input from an earlier showing.
    pub(crate) fn replace_context_menu(&mut self, menu: OpenContextMenu) {
        if let Some(next) = self.context_menu_generation.checked_add(1) {
            self.context_menu_generation = next;
            self.context_menu = menu;
        } else {
            self.context_menu.close();
        }
    }
}
