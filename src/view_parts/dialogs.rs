use super::*;
use iced::widget::column;

pub(crate) const SHORTCUTS_SCOPE_KEY: &str = "mail-shortcuts-presentation";

pub(crate) fn message_move_picker(s: &State) -> Element<'_, Msg> {
    // The same ordered destinations as the Rules `Move to` picker, so the two
    // cannot disagree.
    let options: Vec<String> = crate::model::rules::ACTION_MAILBOXES
        .iter()
        .map(|kind| crate::l10n::tr(kind.label()))
        .collect();
    let picker = enz_combo_box(
        crate::l10n::tr("Destination"),
        options,
        Some(crate::l10n::tr(s.message_move_destination.label())),
        Msg::SetMessageMoveDestination,
    )
    .on_select_index(|index| {
        Msg::SetMessageMoveDestination(
            crate::model::rules::ACTION_MAILBOXES
                .get(index)
                .map_or_else(String::new, |kind| kind.label().to_owned()),
        )
    })
    .into_element();
    let sp = s.themes.theme().spacing;
    let actions = row![
        Space::new().width(Length::Fill),
        enz_button(crate::l10n::tr("Cancel"), Msg::CancelMessageMove)
            .kind(ButtonKind::Quiet)
            .size(ButtonSize::Sm),
        enz_button(crate::l10n::tr("Move"), Msg::ConfirmMessageMove)
            .kind(ButtonKind::Primary)
            .size(ButtonSize::Sm),
    ]
    .mirrored(crate::l10n::rtl())
    .spacing(sp.sm())
    .align_y(Alignment::Center);
    let title = move_dialog_title(s.pending_move_targets.len());
    let content: Element<Msg> = column![
        enz_text(title)
            .size(TextSize::Title3)
            .heading(HeadingLevel::Page),
        picker,
        actions,
    ]
    .spacing(sp.md())
    .into();
    let dialog: Element<Msg> = enz_modal_surface(content)
        .width(Length::Fixed(s.themes.theme().sizing.dialog_width_sm))
        .into();

    dialog
}

pub(crate) fn snooze_picker(s: &State) -> Element<'_, Msg> {
    let now = s.local_clock.now().ok();
    let option = |preset: crate::model::snooze::SnoozePreset| -> String {
        match now {
            Some(now) => format!(
                "{} \u{2014} {}",
                crate::l10n::tr(preset.label()),
                snooze_time_label(preset.until(now))
            ),
            None => crate::l10n::tr(preset.label()),
        }
    };
    let options: Vec<String> = crate::model::snooze::SnoozePreset::ALL
        .into_iter()
        .map(option)
        .collect();
    let picker = enz_combo_box(
        crate::l10n::tr("Snooze until"),
        options,
        Some(option(s.snooze_choice)),
        |_| Msg::SetSnoozeChoice(String::new()),
    )
    .on_select_index(|index| {
        Msg::SetSnoozeChoice(
            crate::model::snooze::SnoozePreset::ALL
                .get(index)
                .map_or_else(String::new, |preset| preset.label().to_owned()),
        )
    })
    .into_element();
    let sp = s.themes.theme().spacing;
    let snoozed = s.pending_snooze_targets.iter().any(|id| {
        s.store
            .message(id)
            .is_some_and(|m| m.snoozed_until.is_some())
    });
    let mut actions = row![Space::new().width(Length::Fill)]
        .mirrored(crate::l10n::rtl())
        .spacing(sp.sm())
        .align_y(Alignment::Center);
    if snoozed {
        actions = actions.push(
            enz_button(crate::l10n::tr("Remove Snooze"), Msg::RemoveSnooze)
                .kind(ButtonKind::Quiet)
                .size(ButtonSize::Sm),
        );
    }
    actions = actions
        .push(
            enz_button(crate::l10n::tr("Cancel"), Msg::CancelSnooze)
                .kind(ButtonKind::Quiet)
                .size(ButtonSize::Sm),
        )
        .push(
            enz_button(crate::l10n::tr("Snooze"), Msg::ConfirmSnooze)
                .kind(ButtonKind::Primary)
                .size(ButtonSize::Sm),
        );
    let title = crate::l10n::tr("Snooze");
    let content: Element<Msg> = column![
        enz_text(title)
            .size(TextSize::Title3)
            .heading(HeadingLevel::Page),
        enz_text(crate::l10n::tr(
            "The conversation leaves its mailbox and comes back unread at this time."
        ))
        .size(TextSize::Footnote)
        .color(t::text_secondary(s.themes.theme())),
        picker,
        actions,
    ]
    .spacing(sp.md())
    .into();
    let dialog: Element<Msg> = enz_modal_surface(content)
        .width(Length::Fixed(s.themes.theme().sizing.dialog_width_sm))
        .into();
    dialog
}

pub(crate) fn snooze_time_label(at: crate::model::types::MessageDate) -> String {
    at.detail_label()
}

// The move picker's heading and accessible name for `count` armed messages.
//
// `count` is messages, not threads: the picker moves every message of every
// selected thread.
pub(crate) fn move_dialog_title(count: usize) -> String {
    crate::l10n::count("mail-move-count", count)
}

pub(crate) fn with_account_remove_confirm<'a>(
    s: &'a State,
    base: Element<'a, Msg>,
) -> Element<'a, Msg> {
    let dialog = s
        .account_removal_confirmation()
        .map(|(display_name, email)| {
            enz_confirm_dialog(
                crate::l10n::tr(crate::settings_view::REMOVE_ACCOUNT_CONFIRM_TITLE),
                crate::l10n::message(
                    "mail-remove-account-body",
                    &[
                        ("name", display_name.to_owned()),
                        ("email", email.to_owned()),
                    ],
                ),
                &s.confirm_remove_account,
                Msg::ConfirmRemoveAccount,
            )
            .confirm_label(crate::l10n::tr("Remove Account"))
            .cancel_label(crate::l10n::tr("Cancel"))
            .destructive(true)
            .id_prefix("mail-remove-account")
        });
    enzui_iced_widgets::confirm_dialog::confirm_dialog_host(base, dialog, Msg::ConfirmRemoveAccount)
}

pub(crate) fn with_block_sender_confirm<'a>(
    s: &'a State,
    base: Element<'a, Msg>,
) -> Element<'a, Msg> {
    let dialog = s.pending_block_sender.as_deref().map(|addr| {
        enz_confirm_dialog(
            crate::l10n::tr("Block sender?"),
            crate::l10n::message("mail-block-sender-body", &[("address", addr.to_owned())]),
            &s.confirm_block_sender,
            Msg::ConfirmBlockSender,
        )
        .confirm_label(crate::l10n::tr("Block Sender"))
        .cancel_label(crate::l10n::tr("Cancel"))
        .destructive(true)
        .id_prefix("mail-block-sender")
    });
    enzui_iced_widgets::confirm_dialog::confirm_dialog_host(base, dialog, Msg::ConfirmBlockSender)
}

pub(crate) fn with_open_url_confirm<'a>(s: &'a State, base: Element<'a, Msg>) -> Element<'a, Msg> {
    let href = s.pending_open_url.as_deref().unwrap_or_default();
    use enzui_iced_core::modal::DismissCause;
    use enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent;
    use enzui_iced_widgets::modal_host::enz_modal_host;
    let sp = s.themes.theme().spacing;
    let url = enz_scroll_view(
        enz_text(href)
            .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
            .width(Length::Fill),
    )
    .height(Length::Fixed(120.0));
    let content = column![
        enz_text(crate::l10n::tr("Open external link?")).heading(HeadingLevel::Page),
        enz_text(crate::l10n::tr("Email links can hide their real destination. Open this address in your default application?")).width(Length::Fill),
        url,
        row![
            enz_toolbar_icon("copy", Msg::CopyMessageSelection(href.to_owned())).aria_label(crate::l10n::tr("Copy link address")).into_element(),
            Space::new().width(Length::Fill),
            enz_button(crate::l10n::tr("Cancel"), Msg::ConfirmOpenUrl(ConfirmDialogEvent::Resolved(DismissCause::CancelAction))).id(iced::advanced::widget::Id::new("mail-open-link-cancel")),
            enz_button(crate::l10n::tr("Open Link"), Msg::ConfirmOpenUrl(ConfirmDialogEvent::Resolved(DismissCause::DefaultAction))).kind(ButtonKind::Primary),
        ].mirrored(crate::l10n::rtl()).spacing(sp.sm()).align_y(Alignment::Center),
    ].spacing(sp.md()).padding(sp.lg());
    let dialog = enz_modal_surface(content.into())
        .width(Length::Fixed(
            (s.window_size.width - 32.0).clamp(240.0, 460.0),
        ))
        .into();
    let host = enz_modal_host(
        base,
        s.pending_open_url.is_some().then_some(dialog),
        Msg::OpenUrlDialogEvent,
    )
    .label(crate::l10n::tr("Open external link?"));
    if let Some(focus) = &s.open_url_focus {
        host.scoped_to(focus).into()
    } else {
        host.into()
    }
}
