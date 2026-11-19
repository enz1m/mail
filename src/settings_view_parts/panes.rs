use super::*;
use iced::widget::column;

pub(crate) fn general_pane(s: &State) -> Element<'_, Msg> {
    let enz = s.theme_ref();
    let sp = &enz.spacing;

    // The signature auto-append toggle for the active account, the only account
    // compose consults.
    let active = s.store.account(&s.active_account_id);
    let behaviors: Element<Msg> = match active {
        Some(acct) => {
            let id = acct.id.clone();
            enz_checkbox(
                acct.append_signature,
                crate::l10n::tr("Append signature to new messages"),
                move |on| Msg::SetAccountAppendSignature(id.clone(), on),
            )
            .id(iced::advanced::widget::Id::from(format!(
                "mail-signature-autoappend-{}",
                acct.id
            )))
            .description(crate::l10n::tr(
                "When off, the signature is preserved but not auto-inserted.",
            ))
            .into_element()
        }
        None => enz_text(crate::l10n::tr("No account selected."))
            .size(TextSize::Footnote)
            .color(t::text_secondary(enz))
            .into(),
    };

    // A refused preferences write is reported in the bottom-right corner.
    let mut items: Vec<Element<Msg>> = Vec::new();
    items.extend([
        // Account management lives in Accounts and privacy controls in Advanced.
        enz_preferences_group(behaviors)
            .title(&crate::l10n::tr("Behaviors"))
            .spacing_scale(*sp)
            .into_element(),
        enz_preferences_group(
            enz_preferences_row(crate::l10n::tr("Rules"))
                .subtitle(crate::l10n::tr("Automatically organize incoming mail"))
                .icon("list-filter")
                .action(&s.rules_count_label, Msg::OpenRules)
                .into_element(),
        )
        .spacing_scale(*sp)
        .into_element(),
        enz_preferences_group(
            column![
                enz_combo_box(
                    crate::l10n::tr("Default message display"),
                    vec![crate::l10n::tr("HTML"), crate::l10n::tr("Plain text")],
                    None,
                    |_| Msg::SetPreferPlainText(false),
                )
                .id(iced::advanced::widget::Id::new(
                    "mail-default-message-display"
                ))
                .selected_index(Some(usize::from(*s.prefer_plain_text.get())))
                .on_select_index(|index| Msg::SetPreferPlainText(index == 1))
                .into_element(),
                enz_checkbox(
                    *s.dark_html.get(),
                    crate::l10n::tr("Adapt HTML emails to dark mode"),
                    Msg::SetDarkHtml
                )
                .id(iced::advanced::widget::Id::new("mail-dark-html"))
                .description(crate::l10n::tr(
                    "Uses darker colors when Mail is dark. Images keep their original colors."
                ))
                .into_element(),
            ]
            .spacing(sp.md()),
        )
        .title(&crate::l10n::tr("Reading"))
        .spacing_scale(*sp)
        .into_element(),
        sidebar_sections_group(s),
    ]);
    column(items).spacing(sp.md()).into()
}

pub(crate) fn favorite_remove_button_id(
    target: &crate::model::sidebar_layout::FavoriteTarget,
) -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::from(format!("mail-settings-favorite-remove-{}", target.key()))
}

pub(crate) fn smart_add_button_id(
    kind: crate::model::sidebar_layout::SmartMailboxKind,
) -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::from(format!("mail-settings-smart-add-{}", kind.token()))
}

pub(crate) fn smart_remove_button_id(
    kind: crate::model::sidebar_layout::SmartMailboxKind,
) -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::from(format!("mail-settings-smart-remove-{}", kind.token()))
}

fn favorite_action_label(
    target: &crate::model::sidebar_layout::FavoriteTarget,
    accounts: &[crate::model::types::Account],
) -> String {
    let label = match target.account_email_in(accounts) {
        Some(account) => format!("{} — {account}", target.display_label_in(accounts)),
        None => target.display_label_in(accounts),
    };
    crate::l10n::message("mail-remove-favorite", &[("label", label)])
}

fn sidebar_sections_group(s: &State) -> Element<'_, Msg> {
    let enz = s.theme_ref();
    let sp = &enz.spacing;
    let accounts = &s.store.accounts;
    let layout = s.sidebar_layout.get();

    let mut body = column![
        enz_preferences_row(crate::l10n::tr("Favorites"))
            .subtitle(crate::l10n::tr(
                "Pin mailboxes you use most to the top of the sidebar"
            ))
            .icon("star")
            .switch(layout.favorites_visible(), Msg::SetFavoritesVisible)
            .into_element(),
    ]
    .spacing(sp.sm());

    if layout.favorites().is_empty() {
        body = body.push(
            enz_text(crate::l10n::tr(
                "No favorites yet. Right-click a mailbox in the sidebar to add one.",
            ))
            .size(TextSize::Footnote)
            .color(t::text_secondary(enz))
            .into_element(),
        );
    } else {
        let rows = layout.favorites().iter().map(|target| {
            let key = target.key();
            let sel = target.selection();
            let row = enz_preferences_row(target.display_label_in(accounts))
                .subtitle(
                    target
                        .account_email_in(accounts)
                        .map_or_else(|| crate::l10n::tr("All accounts"), str::to_owned),
                )
                .icon(target.icon())
                .control(
                    enz_button(crate::l10n::tr("Remove"), Msg::RemoveFavorite(sel))
                        .kind(ButtonKind::Quiet)
                        .size(ButtonSize::Sm)
                        .icon("star-off")
                        .id(favorite_remove_button_id(target))
                        .aria_label(favorite_action_label(target, accounts))
                        .into_element(),
                )
                .into_element();
            (key, row)
        });
        body = body.push(
            // Child buttons require drag-through for the surrounding row to
            // remain draggable.
            enz_reorder_list(rows, Msg::ReorderFavorite)
                .collection_id(crate::model::sidebar_layout::FAVORITES_COLLECTION_ID)
                .drag_through_children(true)
                .spacing(sp.xs())
                .into_element(),
        );
    }

    body = body.push(
        enz_preferences_row(crate::l10n::tr("Smart Mailboxes"))
            .subtitle(crate::l10n::tr(
                "Scheduled and Snoozed stay visible while mail is waiting.",
            ))
            .icon("sparkles")
            .switch(layout.smart_visible(), Msg::SetSmartMailboxesVisible)
            .into_element(),
    );

    if !layout.smart().is_empty() {
        let rows = layout.smart().iter().map(|kind| {
            let key = kind.token();
            let row = enz_preferences_row(crate::l10n::tr(kind.label()))
                .icon(kind.icon())
                .control(
                    enz_button(
                        crate::l10n::tr("Remove"),
                        Msg::RemoveSmartMailbox(
                            crate::model::types::SidebarSelection::SmartMailbox(*kind),
                        ),
                    )
                    .kind(ButtonKind::Quiet)
                    .size(ButtonSize::Sm)
                    .icon("star-off")
                    .id(smart_remove_button_id(*kind))
                    .aria_label(crate::l10n::message(
                        "mail-remove-smart",
                        &[("value", crate::l10n::tr(kind.label()))],
                    ))
                    .into_element(),
                )
                .into_element();
            (key, row)
        });
        body = body.push(
            enz_reorder_list(rows, Msg::ReorderSmartMailbox)
                .collection_id(crate::model::sidebar_layout::SMART_COLLECTION_ID)
                .drag_through_children(true)
                .spacing(sp.xs())
                .into_element(),
        );
    }

    let available: Vec<_> = crate::model::sidebar_layout::SmartMailboxKind::BUILT_INS
        .into_iter()
        .filter(|kind| !layout.smart().contains(kind))
        .collect();
    if !available.is_empty() {
        body = body.push(
            enz_text(crate::l10n::tr("Add a smart mailbox"))
                .size(TextSize::Footnote)
                .color(t::text_secondary(enz))
                .into_element(),
        );
        let mut add_smart = enz_flow()
            .gap(SpaceToken::Sm)
            .cross_gap(SpaceToken::Sm)
            .fill(true)
            .spacing_scale(*sp);
        for kind in available {
            add_smart = add_smart.push(
                enz_button(
                    crate::l10n::message(
                        "mail-add-named",
                        &[("value", crate::l10n::tr(kind.label()))],
                    ),
                    Msg::AddSmartMailbox(kind),
                )
                .kind(ButtonKind::Quiet)
                .size(ButtonSize::Sm)
                .icon(kind.icon())
                .id(smart_add_button_id(kind))
                .aria_label(crate::l10n::message(
                    "mail-add-smart",
                    &[("value", crate::l10n::tr(kind.label()))],
                ))
                .into_element(),
            );
        }
        body = body.push(add_smart.into_element());
    }

    enz_preferences_group(body)
        .title(&crate::l10n::tr("Sidebar"))
        .spacing_scale(*sp)
        .into_element()
}

// The editable keyboard-shortcut pane.
//
// `enz_editable_shortcuts` renders the app's one
// [`crate::shortcuts::MailShortcuts`] catalog; mail supplies the catalog, the
// recording row and the last refusal, shown as `RebindRefusal::message()`
// unmodified so reserved and invalid chords read differently.
pub(crate) fn shortcuts_pane(s: &State) -> Element<'_, Msg> {
    // A shortcut load failure is reported in the bottom-right corner.
    column![]
        .spacing(s.theme_ref().spacing.md())
        .push(
            enz_editable_shortcuts(&s.shortcuts, s.shortcut_armed.as_deref(), Msg::Shortcuts)
                .feedback(
                    s.shortcut_feedback
                        .as_ref()
                        .map(|(action, message)| (action.as_str(), message.as_str())),
                )
                .into_element(),
        )
        .into()
}

// Returns the published Rules-row id used for modal focus restoration.
//
// The row surface owns the derivation; callers must not rebuild its string.
pub(crate) fn rules_invoker_id() -> iced::advanced::widget::Id {
    enzui_iced_widgets::list_row_surface::list_row_id(
        enzui_iced_widgets::list_row_surface::DEFAULT_ID_PREFIX,
        "Rules",
    )
}

pub(crate) fn rules_count_label(n: usize) -> String {
    crate::l10n::count("mail-rules-count", n)
}

pub(crate) fn advanced_pane(s: &State) -> Element<'_, Msg> {
    let enz = s.theme_ref();
    let sp = &enz.spacing;

    let policy = enz_radio_group_keyed(
        crate::l10n::tr("When a message references remote images or tracking pixels:"),
        &s.privacy_choices,
        Some(&s.privacy.policy),
        |policy| Msg::SetRemoteContentPolicy(*policy),
    )
    .size(RadioGroupSize::Md)
    .id(iced::advanced::widget::Id::new("mail-privacy-policy"));

    let advanced = enz_expander(
        crate::l10n::tr("Advanced"),
        s.settings.privacy_advanced_expanded,
        Msg::TogglePrivacyAdvanced,
    )
    .subtitle(crate::l10n::tr("IP privacy and read receipts"))
    .icon("settings")
    .content(
        column![
            enz_checkbox(
                s.privacy.proxy_remote_content,
                crate::l10n::tr("Hide my IP address (privacy proxy)"),
                Msg::SetProxyRemoteContent,
            )
            .id(iced::advanced::widget::Id::new("mail-privacy-proxy"))
            .description(crate::l10n::tr(
                "Remote images stay blocked until a privacy proxy is available."
            ))
            .into_element(),
            enz_checkbox(
                s.privacy.auto_decline_read_receipts,
                crate::l10n::tr("Never automatically send read receipts"),
                Msg::SetAutoDeclineReadReceipts,
            )
            .id(iced::advanced::widget::Id::new(
                "mail-decline-read-receipts"
            ))
            .description(crate::l10n::tr(
                "Not yet in effect \u{2014} saved for when read-receipt handling is available."
            ))
            .into_element(),
        ]
        .spacing(sp.sm())
        .into(),
    );

    let allow = sender_list_expander(
        s,
        "Allowlisted senders",
        "shield-check",
        &s.privacy.allow_list,
        s.settings.allow_list_expanded,
        Msg::ToggleAllowListExpanded,
        &s.settings.allow_input,
        Msg::AllowInputEdited,
        Msg::AddAllowedSender,
        "Add",
        BadgeTone::Success,
        Msg::RemoveAllowedSender,
        "Allowlisted senders override the policy above (except when explicitly blocked).",
    );

    let block = sender_list_expander(
        s,
        "Blocked senders",
        "shield-ban",
        &s.privacy.block_list,
        s.settings.block_list_expanded,
        Msg::ToggleBlockListExpanded,
        &s.settings.block_input,
        Msg::BlockInputEdited,
        Msg::AddBlockedSender,
        "Block Sender",
        BadgeTone::Error,
        Msg::RemoveBlockedSender,
        "Blocked senders never load remote content, even under the Never-block policy.",
    );

    // A failed preferences load or write (the remote-content policy did not
    // reach disk) is reported in the bottom-right corner, not in this pane.
    let items: Vec<Element<Msg>> = vec![
        enz_preferences_group(policy.into_element())
            .title(&crate::l10n::tr("Remote content"))
            .spacing_scale(*sp)
            .into_element(),
        advanced.into_element(),
        allow,
        block,
    ];

    column(items).spacing(sp.md()).into()
}

#[allow(
    clippy::too_many_arguments,
    reason = "Mail operation inputs retain their distinct identities at this boundary."
)]
fn sender_list_expander<'a>(
    s: &'a State,
    title: &'a str,
    icon: &'a str,
    entries: &'a [String],
    expanded: bool,
    on_toggle: fn(bool) -> Msg,
    input: &'a str,
    on_input: fn(String) -> Msg,
    on_add: Msg,
    add_label: &'a str,
    tone: BadgeTone,
    on_remove: fn(String) -> Msg,
    blurb: &'a str,
) -> Element<'a, Msg> {
    let enz = s.theme_ref();
    let sp = &enz.spacing;

    let subtitle = crate::l10n::count("mail-senders-count", entries.len());

    let mut items: Vec<Element<Msg>> = vec![
        enz_text(crate::l10n::tr(blurb))
            .size(TextSize::Footnote)
            .color(t::text_secondary(enz))
            .into(),
    ];
    for entry in entries {
        let addr = entry.clone();
        items.push(
            row![
                enz_badge(entry)
                    .tone(tone)
                    .size(BadgeSize::Sm)
                    .icon(icon)
                    .into_element(),
                Space::new().width(Length::Fill),
                enz_button(crate::l10n::tr("Remove"), on_remove(addr))
                    .kind(ButtonKind::Ghost)
                    .size(ButtonSize::Sm),
            ]
            .mirrored(crate::l10n::rtl())
            .align_y(Alignment::Center)
            .spacing(sp.sm())
            .into(),
        );
    }
    items.push(
        row![
            enz_text_field("", "sender@example.com", input, on_input)
                .id_prefix(format!("mail-settings-{}", title.to_ascii_lowercase()))
                .into_element(),
            enz_button(crate::l10n::tr(add_label), on_add)
                .kind(ButtonKind::Secondary)
                .size(ButtonSize::Sm),
        ]
        .mirrored(crate::l10n::rtl())
        .align_y(Alignment::Center)
        .spacing(sp.sm())
        .into(),
    );

    enz_expander(title, expanded, on_toggle)
        .subtitle(subtitle)
        .icon(icon)
        .content(column(items).spacing(sp.sm()).into())
        .into_element()
}
