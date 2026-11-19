use super::*;
use crate::model::types::MessageLocation;
use crate::notification_settings::{Change, MailboxTarget, Preview};
use iced::widget::column;

pub(super) fn pane(s: &State) -> Element<'_, Msg> {
    let tr = |key| crate::l10n::current().text(key);
    let settings = s.notifications.get();
    let general = column![
        enz_checkbox(
            settings.enabled,
            tr("mail-notifications-enabled"),
            move |value| { Msg::SetNotificationSettings(Change::Enabled(value)) }
        )
        .id(iced::advanced::widget::Id::new(
            "mail-notifications-enabled"
        ))
        .into_element(),
        enz_combo_box(
            tr("mail-notifications-default"),
            vec![tr("mail-notifications-inbox"), tr("mail-notifications-all")],
            None,
            |_| Msg::DesktopActivated
        )
        .id(iced::advanced::widget::Id::new(
            "mail-notifications-default"
        ))
        .selected_index(Some(usize::from(settings.all_mailboxes)))
        .on_select_index(move |index| { Msg::SetNotificationSettings(Change::Scope(index == 1)) })
        .into_element(),
        enz_combo_box(
            tr("mail-notifications-preview"),
            vec![
                tr("mail-notifications-preview-full"),
                tr("mail-notifications-preview-sender"),
                tr("mail-notifications-preview-private")
            ],
            None,
            |_| Msg::DesktopActivated
        )
        .id(iced::advanced::widget::Id::new(
            "mail-notifications-preview"
        ))
        .selected_index(Some(match settings.preview {
            Preview::Full => 0,
            Preview::Sender => 1,
            Preview::Private => 2,
        }))
        .on_select_index(move |index| {
            Msg::SetNotificationSettings(Change::Preview(match index {
                0 => Preview::Full,
                1 => Preview::Sender,
                _ => Preview::Private,
            }))
        })
        .into_element(),
        enz_text(tr("mail-notifications-desktop-policy")).size(TextSize::Footnote),
        enz_button(tr("mail-notifications-test"), Msg::TestNotification)
            .id(iced::advanced::widget::Id::new("mail-notifications-test"))
            .enabled(settings.enabled)
            .into_element(),
        enz_text(tr(if s.startup.preferences.publishable() {
            s.new_mail.status.unwrap_or("mail-notification-idle")
        } else {
            "mail-notification-paused"
        }))
        .size(TextSize::Footnote),
    ]
    .spacing(s.theme_ref().spacing.md());
    // Preferences load and save failures are reported in the bottom-right corner.
    let mut items: Vec<Element<'_, Msg>> = vec![
        enz_preferences_group(general)
            .title(&tr("mail-notifications-title"))
            .into_element(),
        enz_text(tr("mail-notifications-mailbox-help"))
            .size(TextSize::Footnote)
            .into(),
    ];
    for account in &s.store.accounts {
        let mut rows: Vec<Element<'_, Msg>> = Vec::new();
        for mailbox in &account.mailboxes {
            rows.push(mailbox_row(
                s,
                MailboxTarget {
                    account_id: account.id.clone(),
                    location: MessageLocation::Mailbox(*mailbox),
                },
                crate::l10n::tr(mailbox.label()),
            ));
        }
        items.push(
            enz_preferences_group(column(rows).spacing(s.theme_ref().spacing.sm()))
                .title(&account.email)
                .into_element(),
        );
    }
    column(items).spacing(s.theme_ref().spacing.md()).into()
}

fn mailbox_row(s: &State, target: MailboxTarget, label: String) -> Element<'_, Msg> {
    let tr = |key| crate::l10n::current().text(key);
    let settings = s.notifications.get();
    let index = settings
        .overrides
        .get(&target)
        .map_or(0, |value| if *value { 1 } else { 2 });
    let id = format!(
        "mail-notify-mailbox/{:?}/{:?}",
        target.account_id, target.location
    );
    enz_combo_box(
        label,
        vec![
            tr("mail-notifications-inherit"),
            tr("mail-notifications-on"),
            tr("mail-notifications-off"),
        ],
        None,
        |_| Msg::DesktopActivated,
    )
    .id(iced::advanced::widget::Id::from(id))
    .selected_index(Some(index))
    .on_select_index(move |index| {
        Msg::SetNotificationSettings(Change::Mailbox(
            target.clone(),
            match index {
                1 => Some(true),
                2 => Some(false),
                _ => None,
            },
        ))
    })
    .into_element()
}
