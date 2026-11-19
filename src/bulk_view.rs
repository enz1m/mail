use iced::Element;

use enzui_iced_core::action_overflow::Descriptor;
use enzui_iced_core::{Action, Icon};
use enzui_iced_widgets::action_bar::{
    ActionItem, Collector, CollectorPlacement, CollectorVisibility, enz_action_bar,
};
use enzui_iced_widgets::confirm_dialog::enz_confirm_dialog;

use crate::model::bulk::{
    emptiable_mailbox, empty_action_label, empty_confirm_button_label, empty_mailbox_prompt,
    mailbox_count_for, unread_in_selection,
};
use crate::{Message as Msg, State};

pub(crate) const EMPTY_CONFIRM_SCOPE_KEY: &str = "mail-empty-mailbox-presentation";

pub(crate) const MAILBOX_ACTIONS_TRIGGER_ID: &str = "mail-mailbox-actions";

pub(crate) fn mailbox_actions_invoker_id() -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::new(MAILBOX_ACTIONS_TRIGGER_ID)
}

// Mailbox menus omit singular reader commands unless rows were explicitly selected.
pub(crate) fn mailbox_actions<'a>(s: &'a State) -> Element<'a, Msg> {
    fn menu_command<'a>(
        id: &'static str,
        label: impl Into<String>,
        icon: &'static str,
        message: Msg,
    ) -> ActionItem<'a, Msg> {
        let label = crate::l10n::tr(&label.into());
        ActionItem::command(
            Descriptor::new(id).always_overflow(),
            Action::new(id, message)
                .with_label(label)
                .with_icon(Icon::named(icon)),
        )
    }

    let mut actions: Vec<ActionItem<'a, Msg>> = Vec::new();

    if let Some(target) = crate::view::explicit_selection_target(s)
        && let Some(state) = crate::view::explicit_selection_state(s)
    {
        actions.extend(
            crate::mail_actions::actions_for(state, &target)
                .into_iter()
                .map(|action| {
                    let item = ActionItem::command(
                        Descriptor::new(action.id()).always_overflow(),
                        Action::new(action.id(), action.message(&target))
                            .with_label(action.label(&target))
                            .with_icon(Icon::named(action.icon())),
                    );
                    item.destructive(action.destructive())
                }),
        );
        actions.push(menu_command(
            "mailbox.clear-selection",
            "Clear selection",
            "x",
            Msg::ClearSelection,
        ));
    }

    actions.push(menu_command(
        "mailbox.select-all",
        "Select all",
        "list-checks",
        Msg::SelectAllConversations,
    ));

    if unread_in_selection(&s.store, &s.selection) > 0 {
        actions.push(menu_command(
            "mailbox.mark-all-read",
            "Mark all as read",
            "mail-open",
            Msg::MarkAllRead,
        ));
    }

    if let crate::model::types::SidebarSelection::AccountMailbox { account_id, .. } = &s.selection
        && let Some(kind) = emptiable_mailbox(&s.selection)
        && let Some(label) = empty_action_label(kind)
        && mailbox_count_for(&s.store, kind, account_id) > 0
    {
        actions.push(
            menu_command(
                "mailbox.empty",
                label,
                "trash-2",
                Msg::RequestEmptyMailbox(kind, account_id.clone()),
            )
            .destructive(true),
        );
    }

    enz_action_bar(actions, Msg::ToggleMailboxOverflow)
        .collector(Collector::Kebab {
            icon: "ellipsis-vertical".to_owned(),
            tokens: None,
        })
        .collector_placement(CollectorPlacement::Trailing)
        .collector_visibility(CollectorVisibility::Always)
        .fit_content()
        .overflow_open(s.mailbox_overflow_open)
        // A dedicated idempotent close, so command-side clears do not race a toggle
        // back to `true`.
        .on_dismiss(Msg::CloseMailboxOverflow)
        .overflow_trigger_id(mailbox_actions_invoker_id())
        .menu_id_prefix("mail-mailbox-action")
        .into_element()
}

pub(crate) fn with_empty_confirm<'a>(s: &'a State, base: Element<'a, Msg>) -> Element<'a, Msg> {
    let Some((kind, account_id)) = s
        .bulk
        .pending_empty
        .zip(s.bulk.pending_empty_account.as_deref())
    else {
        return enzui_iced_widgets::modal_host::enz_modal_host(base, None, |event| {
            Msg::ConfirmEmptyEvent(
                enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent::from_host(event),
            )
        })
        .scope(enzui_iced_widgets::modal_host::presentation_scope_id(
            EMPTY_CONFIRM_SCOPE_KEY,
        ))
        .into();
    };
    let count = mailbox_count_for(&s.store, kind, account_id);
    let (title, body) = empty_mailbox_prompt(kind, count);
    let confirm_label = empty_confirm_button_label(kind);
    let mut host = enz_confirm_dialog(title, body, &s.confirm_empty, Msg::ConfirmEmptyEvent)
        .confirm_label(crate::l10n::tr(confirm_label))
        .cancel_label(crate::l10n::tr("Cancel"))
        .destructive(true)
        .alert(true)
        .id_prefix("mail-empty-mailbox")
        .into_modal_host(base);
    if let Some(focus) = s.empty_confirm_focus.as_ref() {
        host = host.scoped_to(focus);
    }
    host.into_element()
}
