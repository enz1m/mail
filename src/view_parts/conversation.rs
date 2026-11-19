use super::*;
use iced::widget::column;

pub(crate) const CONVERSATION_ROW_HEIGHT: f32 = 72.0;
const DIVIDER_THICKNESS: f32 = 1.0;

// The attachment glyph uses the 3:1 non-text contrast floor against its actual row backdrop.
pub(crate) fn conversation_paperclip_ink(enz: &EnzTheme, row_backdrop: iced::Color) -> iced::Color {
    t::graphic_tertiary_on(enz, row_backdrop)
}

pub(crate) fn conversation_row(
    c: Conversation,
    selected: bool,
    has_next: bool,
    enz: &EnzTheme,
) -> Element<'static, Msg> {
    let sp = &enz.spacing;
    let rtl = crate::l10n::rtl();
    let trailing_alignment = if rtl {
        Alignment::Start
    } else {
        Alignment::End
    };

    // Tertiary row chrome resolves against the selected fill or the opaque list
    // base, as one value so date and attachment marker share an emphasis rung.
    let row_backdrop = if selected {
        enz.palette.surface_selected
    } else {
        // The hover composite is the contrast-worst case; rest only gains contrast.
        enz.palette.surface_hover
    };
    let row_tertiary = t::text_tertiary_on(enz, row_backdrop);

    let sender_line = enz_text(participants_summary(&c.participants))
        .size(TextSize::Body)
        .width(Length::Fill)
        .single_line_elided()
        .color(t::text_primary(enz));
    let subject = enz_text(crate::model::types::shown_subject(&c.subject).into_owned())
        .size(TextSize::Caption)
        .width(Length::Fill)
        .single_line_elided()
        .color(t::text_primary(enz));
    // `BadgeTone::Default` publishes no severity node: a message count is not a
    // status.
    let count_badge: Element<Msg> = if c.message_count() > 1 {
        enz_badge(&crate::l10n::number(c.message_count()))
            .tone(BadgeTone::Default)
            .size(BadgeSize::Sm)
            .into()
    } else {
        Space::new().into()
    };
    // The paperclip is the only attachment cue, so it uses the 3:1 non-text floor
    // against the date's backdrop. On the light selected row it stops at
    // `text_muted` while the date resolves to body `text`.
    let attach: Element<Msg> = if c.has_attachment_any {
        enz_icon("paperclip")
            .size(11.0)
            .tint(conversation_paperclip_ink(enz, row_backdrop))
            .into()
    } else {
        Space::new().width(0).into()
    };
    let date = enz_text(c.latest_date.short_label())
        .size(TextSize::Caption)
        .single_line_elided()
        .color(row_tertiary);

    // Unread is a presentation cue, not a severity status announced by assistive technology.
    let unread_accent = t::accent(enz);
    let unread_marker: Element<Msg> = if c.unread_count > 0 {
        container(Space::new().width(8).height(8))
            .style(move |_| container::Style {
                background: Some(unread_accent.into()),
                border: iced::Border {
                    radius: 4.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
    } else {
        Space::new().width(8).height(8).into()
    };

    let metadata = row![unread_marker, date]
        .mirrored(rtl)
        .spacing(sp.space3)
        .align_y(Alignment::Center);
    let header_row = row![sender_line, attach, count_badge]
        .mirrored(crate::l10n::rtl())
        .align_y(Alignment::Center)
        .spacing(sp.space3);
    let preview_row = enz_text(c.preview.clone())
        .size(TextSize::Caption)
        .width(Length::Fill)
        .single_line_elided()
        .color(enz.palette.text_muted_on(row_backdrop));
    let target = crate::mail_actions::ActionTarget::Conversation {
        conversation_id: c.id.clone(),
        newest_message_id: c
            .message_ids
            .last()
            .cloned()
            .unwrap_or_else(|| c.id.clone()),
    };
    let flag_action = if c.flagged_any {
        crate::mail_actions::MailAction::Unflag
    } else {
        crate::mail_actions::MailAction::Flag
    };
    let mut flag_tokens =
        enzui_iced_widgets::toolbar_icon::ToolbarIconTokens::from_palette(&enz.palette);
    if c.flagged_any {
        flag_tokens.icon_color = t::flag_accent(enz);
        flag_tokens.hover_color = t::flag_accent(enz);
        flag_tokens.pressed_color = t::flag_accent(enz);
        flag_tokens.active_color = t::flag_accent(enz);
        flag_tokens.active_background = iced::Color::TRANSPARENT;
    }
    let flag: Element<Msg> = enz_toolbar_icon(
        if c.flagged_any { "flag.fill" } else { "flag" },
        Msg::MailAction(flag_action, target.clone()),
    )
    .id(iced::advanced::widget::Id::from(format!(
        "mail-row-flag/{}",
        c.id
    )))
    .variant(ToolbarIconVariant::InkOnly)
    .aria_label(crate::l10n::tr(if c.flagged_any {
        "Unflag"
    } else {
        "Flag"
    }))
    .active(c.flagged_any)
    .tokens(flag_tokens)
    .size(24.0, 13.0)
    .tooltip(crate::l10n::tr(if c.flagged_any {
        "Unflag"
    } else {
        "Flag"
    }))
    .into();
    let (inline_flag, persistent_flag) = if c.flagged_any {
        (Space::new().width(24).into(), Some(flag))
    } else {
        (flag, None)
    };
    let actions = row![
        inline_flag,
        enz_toolbar_icon(
            "trash-2",
            Msg::MailAction(crate::mail_actions::MailAction::Delete, target)
        )
        .id(iced::advanced::widget::Id::from(format!(
            "mail-row-delete/{}",
            c.id
        )))
        .variant(ToolbarIconVariant::InkOnly)
        .aria_label(crate::l10n::tr("Delete"))
        .size(24.0, 13.0)
        .tooltip(crate::l10n::tr("Delete")),
    ]
    .spacing(sp.space1);
    let a11y_label = conversation_row_label(&c);
    let mut row_semantics =
        enzui_iced_core::semantics::Semantics::new(enzui_iced_core::semantics::Role::ListItem)
            .label(a11y_label.clone())
            .default_action();
    row_semantics = row_semantics.selected(selected);
    if c.unread_count > 0 {
        row_semantics = row_semantics.description(crate::l10n::tr("Unread conversation"));
    }
    let divider_gap = sp.space1;
    let surface_height = CONVERSATION_ROW_HEIGHT - 2.0 * divider_gap - DIVIDER_THICKNESS;
    // The surface holds only the sender line; subject and preview are one
    // overlay block (below), so they span the full row under the hover
    // actions and keep a fixed gap between them.
    let inner = container(column![header_row].padding(Padding::from([sp.space2, sp.space5])))
        .width(Length::Fill)
        .height(surface_height);

    let convo_click_id = c.id.clone();
    let surface = enz_list_row_surface(
        inner,
        Msg::SelectConversation {
            id: c.id.clone(),
            toggle: false,
            range: false,
        },
    )
    .tokens_resolved(|theme| {
        let mut tokens = enzui_iced_widgets::theme_lookup::resolve_live(
            theme,
            enzui_iced_widgets::list_row_surface::ListRowSurfaceTokens::from_palette,
        );
        tokens.row_radius = 6.0;
        tokens
    })
    .content_padding(0.0)
    .trailing(
        container(actions)
            .width(96.0)
            .align_x(trailing_alignment)
            .padding(Padding {
                top: 0.0,
                bottom: sp.space4,
                left: 0.0,
                right: sp.space3,
            }),
    )
    .trailing_visibility(enzui_iced_widgets::list_row_surface::TrailingVisibility::OnHoverOrFocus)
    .on_activate_with_modifiers(move |m| Msg::SelectConversation {
        id: convo_click_id.clone(),
        toggle: m.command(),
        range: m.shift(),
    })
    .selected(selected)
    // Constant, because a modifier click multi-selects at any time.
    .selectable(true)
    .content_height(true)
    .label(a11y_label)
    .semantics(row_semantics)
    .id_prefix(CONVO_ROW_ID_PREFIX)
    .id(conversation_row_id(&c));

    let convo_id = c.id.clone();
    let surface = surface
        .on_context(move |at| Msg::OpenConversationMenu(convo_id.clone(), at))
        .context_label(crate::l10n::tr("Conversation actions"))
        .context_id(enzui_iced_widgets::list_row_surface::list_row_context_id(
            CONVO_ROW_ID_PREFIX,
            &c.id,
        ));
    let mut layers: Vec<Element<Msg>> = vec![
        surface.into(),
        container(metadata)
            .width(Length::Fill)
            .align_x(trailing_alignment)
            .padding(Padding {
                top: sp.space2,
                right: if rtl { 0.0 } else { sp.space7 },
                bottom: 0.0,
                left: if rtl { sp.space7 } else { 0.0 },
            })
            .into(),
    ];
    // Subject and preview, indented under the sender, anchored to the row's
    // bottom edge as one block.
    let row_inset = sp.space5 + enzui_iced_widgets::list_row_surface::DEFAULT_OUTER_GUTTER;
    layers.push(
        container(column![subject, preview_row].spacing(sp.space1))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_y(Alignment::End)
            .padding(Padding {
                left: row_inset + if rtl { 0.0 } else { sp.space3 },
                right: row_inset + if rtl { sp.space3 } else { 0.0 },
                top: 0.0,
                bottom: sp.space3 + divider_gap,
            })
            .into(),
    );
    if let Some(flag) = persistent_flag {
        layers.push(
            container(flag)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(trailing_alignment)
                .align_y(Alignment::Center)
                .padding(Padding {
                    top: 0.0,
                    bottom: sp.space4,
                    left: if rtl {
                        sp.space3 + 24.0 + sp.space1 + sp.space2
                    } else {
                        0.0
                    },
                    right: if rtl {
                        0.0
                    } else {
                        sp.space3 + 24.0 + sp.space1 + sp.space2
                    },
                })
                .into(),
        );
    }
    let divider: Element<Msg> = if has_next {
        container(
            enz_divider()
                .thickness(DIVIDER_THICKNESS)
                .color(enz.palette.border.scale_alpha(0.45)),
        )
        .padding([0.0, sp.space7])
        .into()
    } else {
        Space::new().height(DIVIDER_THICKNESS).into()
    };
    column![
        container(iced::widget::Stack::with_children(layers))
            .height(Length::Fill)
            .padding([divider_gap, 0.0]),
        divider,
    ]
    .width(Length::Fill)
    .height(CONVERSATION_ROW_HEIGHT)
    .into()
}

pub(crate) fn address_action_menu_model(
    include_name_and_email: bool,
    recipient: bool,
) -> MenuModel {
    let mut entries = vec![MenuEntryModel::command(
        "copy-address",
        crate::l10n::tr("Copy address"),
    )];
    if include_name_and_email {
        entries.push(MenuEntryModel::command(
            "copy-name-and-email",
            crate::l10n::tr("Copy name and email"),
        ));
    }
    entries.push(MenuEntryModel::command(
        "compose-to",
        crate::l10n::tr(if recipient {
            "Compose to recipient"
        } else {
            "Compose to sender"
        }),
    ));
    entries.push(MenuEntryModel::command(
        "filter-from",
        crate::l10n::tr(if recipient {
            "Filter messages from recipient"
        } else {
            "Filter messages from sender"
        }),
    ));
    MenuModel::new(entries)
}

pub(crate) fn participants_summary(participants: &[String]) -> String {
    if participants.is_empty() {
        String::from("(unknown)")
    } else if participants.len() == 1 {
        participants[0].clone()
    } else if participants.len() == 2 {
        format!("{}, {}", participants[0], participants[1])
    } else {
        format!(
            "{}, {} + {}",
            participants[0],
            participants[1],
            crate::l10n::number(participants.len() - 2)
        )
    }
}

// The reading pane's label when two or more conversation rows are selected.
//
// Replaces the thread so the action scope is visible. Each plural noun is
// spelled out, so "2 conversation selected" cannot render.
pub(crate) fn multi_selection_label(n: usize) -> String {
    crate::l10n::count("mail-conversations-selected", n)
}
