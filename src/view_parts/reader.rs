use super::*;
use iced::widget::column;

pub(crate) fn reader_pane(s: &State, w: Length) -> Element<'_, Msg> {
    if s.compose.open {
        let pane = crate::compose_view::compose_pane(s).unwrap_or_else(|| Space::new().into());
        return container(pane).width(w).height(Length::Fill).into();
    }
    let multi = explicit_selection(s).len();
    let inner: Element<Msg> = if multi >= 2 {
        enz_status_page(multi_selection_label(multi))
            .description(crate::l10n::tr(
                "Choose one conversation to read it, or use the actions above.",
            ))
            .icon("mail")
            .tone(StatusPageTone::Empty)
            .into()
    } else {
        match focused_messages(s) {
            Some(msgs) if !msgs.is_empty() => reader_thread(s, msgs),
            _ => reader_empty(s.themes.theme()),
        }
    };
    container(inner).width(w).height(Length::Fill).into()
}

pub(crate) fn titlebar_actions(s: &State) -> Element<'_, Msg> {
    if s.compose.open || explicit_selection(s).len() >= 2 {
        return Space::new().into();
    }
    let Some(messages) = focused_messages(s) else {
        return Space::new().into();
    };
    let Some(m) = messages.last() else {
        return Space::new().into();
    };
    let enz = s.themes.theme();
    let sp = enz.spacing;
    let is_draft = matches!(
        m.location,
        crate::model::types::MessageLocation::Mailbox(MailboxKind::Drafts)
    );
    let sender_blocked =
        crate::mail_actions::sender_is_blocked(&s.privacy.block_list, &m.sender_email);

    let action_row: Element<Msg> = if is_draft {
        row![
            enz_toolbar_icon(
                "file-pen",
                Msg::OpenCompose(ComposeMode::EditDraft(m.id.clone()))
            )
            .label(crate::l10n::tr("Edit Draft"))
            .id(iced::advanced::widget::Id::new("mail-titlebar-edit-draft"))
        ]
        .mirrored(crate::l10n::rtl())
        .spacing(sp.space3)
        .into()
    } else {
        // Sender commands must never target the account owner's own outgoing mail.
        let action_state =
            crate::mail_actions::ActionState::of_message(&s.store, &s.privacy.block_list, m);

        let unsubscribe = m.unsubscribe.as_ref().map(|route| {
            match s.unsubscribes.get(&m.sender_email.to_ascii_lowercase()) {
                Some(crate::UnsubscribeProgress::Done) => ("Unsubscribed".to_owned(), false),
                Some(crate::UnsubscribeProgress::Sending) => ("Unsubscribing…".to_owned(), false),
                Some(crate::UnsubscribeProgress::Failed(_)) => {
                    ("Retry unsubscribe".to_owned(), true)
                }
                None => (
                    if route.one_click && route.https().is_some() {
                        "Unsubscribe"
                    } else {
                        "Unsubscribe…"
                    }
                    .to_owned(),
                    true,
                ),
            }
        });
        let bar = reader_action_bar_with_unsubscribe(
            &m.id,
            action_state,
            s.reader_overflow_target.as_deref() == Some(m.id.as_str()),
            enz,
            unsubscribe,
            m.body_html.as_ref().map(|_| s.shows_plain_text(&m.id)),
            s.mode() == LayoutMode::Narrow,
        );

        if sender_blocked {
            row![
                enz_badge(&crate::l10n::tr("Blocked Sender"))
                    .tone(BadgeTone::Error)
                    .size(BadgeSize::Sm),
                bar
            ]
            .mirrored(crate::l10n::rtl())
            .align_y(Alignment::Center)
            .spacing(sp.space3)
            .into()
        } else {
            bar
        }
    };
    action_row
}

fn focused_messages(s: &State) -> Option<Vec<&Message>> {
    let convo_id = s.selected_conversation.as_ref()?;
    let convos = s.store.conversations_shared(&s.selection);
    let c = convos.iter().find(|c| &c.id == convo_id)?;
    Some(
        c.message_ids
            .iter()
            .filter_map(|id| s.store.message(id))
            .collect(),
    )
}

fn reader_thread<'a>(s: &'a State, messages: Vec<&'a Message>) -> Element<'a, Msg> {
    let sp = s.themes.theme().spacing;
    let mut stack: Vec<Element<Msg>> = Vec::new();
    for message in messages.iter().rev() {
        stack.push(message_card(s, message));
    }
    if s.quick_reply_mode.is_some() && s.quick_reply_for == s.selected_conversation {
        stack.push(quick_reply_bar(s));
    }
    let top = sp.xs();
    enz_scroll_view(column(stack).spacing(sp.md()).padding(Padding {
        top,
        right: sp.xs(),
        bottom: sp.md(),
        left: sp.xs(),
    }))
    .id(iced::advanced::widget::Id::new("mail-reader-scroll"))
    .height(Length::Fill)
    .into()
}

pub(crate) const QUICK_REPLY_LABEL: &str = "Quick reply";

pub(crate) const QUICK_REPLY_EXPAND_LABEL: &str = "Open in the full composer";

fn quick_reply_bar(s: &State) -> Element<'_, Msg> {
    use enzui_iced_widgets::composer::enz_composer;
    use enzui_iced_widgets::text_area::SubmitKey;
    let sp = s.themes.theme().spacing;
    let expand = enz_toolbar_icon("maximize-2", Msg::QuickReplyExpand)
        .id(iced::advanced::widget::Id::new("mail-inline-reply-expand"))
        .aria_label(crate::l10n::tr(QUICK_REPLY_EXPAND_LABEL))
        .tooltip(QUICK_REPLY_EXPAND_LABEL)
        .into();
    let editor: Element<Msg> = container(
        column![
            row![
                enz_text(
                    if matches!(s.quick_reply_mode, Some(ComposeMode::ReplyAll(_))) {
                        "Reply all"
                    } else {
                        "Reply"
                    }
                )
                .width(Length::Fill),
                enz_toolbar_icon("x", Msg::CancelQuickReply)
                    .id(iced::advanced::widget::Id::new("mail-inline-reply-discard"))
                    .aria_label(crate::l10n::tr("Discard inline reply"))
                    .into_element()
            ]
            .mirrored(crate::l10n::rtl()),
            enz_composer(&s.quick_reply, Msg::QuickReplyEdit)
                .placeholder(crate::l10n::tr("Reply… (Ctrl+Enter to send)"))
                .max_rows(6)
                .submit_key(SubmitKey::CommandEnter)
                .on_send(Msg::QuickReplySend)
                .rail_leading(vec![expand])
                .id(iced::advanced::widget::Id::new("mail-inline-reply"))
                .aria_label(crate::l10n::tr(QUICK_REPLY_LABEL)),
        ]
        .spacing(sp.sm()),
    )
    .padding(Padding::from([sp.sm(), sp.space5]))
    .width(Length::Fill)
    .into();
    enzui_iced_widgets::track_bounds(editor, &s.draft_bounds)
}

// The reader action bar's overflow-trigger id for one message card.
//
// One id per message: a shared id collides in the widget tree and misroutes
// focus restoration.
pub(crate) fn reader_overflow_trigger_id(message_id: &str) -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::from(format!("mail-reader-overflow-{message_id}"))
}

// Distinct document/context ids preserve a separately routable actions group.
pub(crate) fn message_selection_document_id(message_id: &str) -> iced::advanced::widget::Id {
    enzui_iced_core::identity::derive_id("mail-message-selection-document", message_id)
}

// The menu host shares this group's label, not its identity.
pub(crate) fn message_selection_context_id(message_id: &str) -> iced::advanced::widget::Id {
    enzui_iced_core::identity::derive_id("mail-message-selection", message_id)
}

pub(crate) fn attachment_row_id(message_id: &str, name: &str) -> iced::advanced::widget::Id {
    enzui_iced_core::identity::derive_id(
        "mail-attachment-row",
        &format!("{message_id}\u{1f}{name}"),
    )
}

pub(crate) fn attachment_context_id(message_id: &str, name: &str) -> iced::advanced::widget::Id {
    enzui_iced_core::identity::derive_id(
        "mail-attachment-context",
        &format!("{message_id}\u{1f}{name}"),
    )
}

const PREVIEW_IMAGE_HEIGHT: f32 = 180.0;

// An image draws as a thumbnail and plain text as its opening lines; other
// types, and anything still loading or unreadable, show only the row.
fn attachment_preview<'a>(s: &'a State, attachment: &'a Attachment) -> Option<Element<'a, Msg>> {
    let key = attachment.payload_id.as_ref()?.as_str();
    let enz = s.themes.theme();
    match s.attachment_previews.get(key)? {
        crate::AttachmentPreview::Image(handle) => Some(
            enzui_iced_widgets::image_widget::enz_image_handle(handle.clone())
                .hug_picture()
                .radius(enz.radius.step(enz.shape.media))
                .alt_text(crate::l10n::message(
                    "mail-preview-file",
                    &[("file", attachment.name.clone())],
                ))
                .width(Length::Fill)
                .height(Length::Fixed(PREVIEW_IMAGE_HEIGHT))
                .into_element(),
        ),
        crate::AttachmentPreview::Text(text) => Some(
            enz_surface(
                enz_text(text.clone())
                    .size(TextSize::Caption)
                    .color(t::text_secondary(enz)),
            )
            .tone(SurfaceTone::SurfaceAlt)
            .radius(enz.radius.step(enz.shape.card))
            .padding(Padding::from(enz.spacing.sm()))
            .fill_width()
            .into(),
        ),
        crate::AttachmentPreview::Loading | crate::AttachmentPreview::Unavailable => None,
    }
}

// Theme artwork beside an attachment's two-line name and status.
const ATTACHMENT_ART: f32 = 28.0;

// The attachment's file-type icon from the desktop icon theme, else the
// bundled glyph in the row's tertiary ink.
fn attachment_icon<'a>(attachment: &Attachment, tertiary: iced::Color) -> Element<'a, Msg> {
    use enzui_iced_widgets::file_icon::{FileIconQuery, themed_file_icon};
    // A generic or missing type says less than the file's own name.
    let query = match attachment.mime.as_str() {
        "" | "application/octet-stream" => FileIconQuery::file_name(attachment.name.clone()),
        mime => FileIconQuery::mime(mime),
    };
    match themed_file_icon(&query, ATTACHMENT_ART) {
        Some(art) => art.into_element(ATTACHMENT_ART),
        None => enz_icon(attachment.icon()).size(13.0).tint(tertiary).into(),
    }
}

pub(crate) fn attachment_row<'a>(
    message_id: &'a str,
    attachment: &'a Attachment,
    enz: &'a EnzTheme,
) -> Element<'a, Msg> {
    let sp = &enz.spacing;
    let tertiary = t::text_tertiary_on(enz, t::reader_background(enz));
    let status = if attachment.payload_ref().is_some() {
        "Local payload reference"
    } else {
        "Attachment data unavailable — metadata only"
    };
    let mut controls: Vec<Element<Msg>> = vec![
        attachment_icon(attachment, tertiary),
        column![
            enz_text(attachment.name.clone())
                .size(TextSize::Footnote)
                .color(t::text_primary(enz)),
            enz_text(crate::l10n::tr(status))
                .size(TextSize::Caption)
                .color(tertiary),
        ]
        .spacing(sp.space1)
        .into(),
        Space::new().width(Length::Fill).into(),
        enz_text(attachment.size_label())
            .size(TextSize::Caption)
            .color(tertiary)
            .into(),
    ];
    if attachment.payload_ref().is_some() {
        controls.push(
            enz_button(
                crate::l10n::tr("Open"),
                Msg::OpenAttachment(attachment.clone()),
            )
            .aria_label(crate::l10n::message(
                "mail-open-file",
                &[("file", attachment.name.clone())],
            ))
            .kind(ButtonKind::Ghost)
            .size(ButtonSize::Xs)
            .into(),
        );
        controls.push(
            enz_button(
                crate::l10n::tr("Save"),
                Msg::SaveAttachment(attachment.clone()),
            )
            .aria_label(crate::l10n::message(
                "mail-save-file",
                &[("file", attachment.name.clone())],
            ))
            .kind(ButtonKind::Ghost)
            .size(ButtonSize::Xs)
            .into(),
        );
    }
    let content = container(row(controls).align_y(Alignment::Center).spacing(sp.sm()))
        .padding(Padding::from([sp.space3, 0.0]))
        .width(Length::Fill);

    let activate_message_id = message_id.to_owned();
    let activate_name = attachment.name.clone();
    let context_message_id = message_id.to_owned();
    let context_name = attachment.name.clone();
    enz_list_row_surface(
        content,
        Msg::ShowAttachmentDetails {
            message_id: activate_message_id,
            name: activate_name,
        },
    )
    .label(crate::l10n::message(
        "mail-attachment-file",
        &[("file", attachment.name.clone())],
    ))
    .id(attachment_row_id(message_id, &attachment.name))
    .on_context(move |at| Msg::OpenAttachmentMenu {
        message_id: context_message_id.clone(),
        name: context_name.clone(),
        at,
    })
    .context_id(attachment_context_id(message_id, &attachment.name))
    .context_label(crate::l10n::message(
        "mail-attachment-actions-file",
        &[("file", attachment.name.clone())],
    ))
    .content_height(true)
    .into()
}

// UI typography for one reader-header run.
//
// Subject and metadata share the 13 px UI scale; emphasis changes only the
// weight, so no document-heading rung can reach the paint sites.
pub(crate) fn reader_header_text(
    content: impl Into<String>,
    emphasized: bool,
    enz: &EnzTheme,
) -> EnzText {
    enz_text(content)
        .size(if emphasized {
            TextSize::Headline
        } else {
            TextSize::Body
        })
        .font_from_theme(enz)
}

fn grouped_titlebar_icon(button: Element<'static, Msg>) -> Element<'static, Msg> {
    row![
        container(enz_divider().vertical())
            .height(16)
            .padding([0, 4]),
        button
    ]
    .align_y(Alignment::Center)
    .into()
}

fn reader_action_bar_with_unsubscribe(
    message_id: &str,
    state: crate::mail_actions::ActionState,
    overflow_open: bool,
    enz: &EnzTheme,
    unsubscribe: Option<(String, bool)>,
    plain_text: Option<bool>,
    compact: bool,
) -> Element<'static, Msg> {
    use crate::mail_actions::{self, MailAction};

    let target = mail_actions::ActionTarget::Message {
        message_id: message_id.to_owned(),
    };
    let verbs = mail_actions::actions_for(state, &target);
    let include = |verb: MailAction| compact || verb.group() != mail_actions::ActionGroup::Compose;
    let sender_index = verbs
        .iter()
        .copied()
        .filter(|verb| include(*verb))
        .position(|verb| verb.group() == mail_actions::ActionGroup::Sender);
    let has_unsubscribe = unsubscribe.is_some();
    let action_message_id = message_id.to_owned();
    let mut actions = mail_actions::bar_items(state, &target, include, move |action, verb| {
        let button = enz_toolbar_icon(verb.icon(), action.message().clone())
            .id(iced::advanced::widget::Id::from(format!(
                "mail-reader-action/{action_message_id}/{}",
                verb.id()
            )))
            .variant(ToolbarIconVariant::InkOnly)
            .label(action.label())
            .enabled(action.is_enabled())
            .into_element();
        if !has_unsubscribe && matches!(verb, MailAction::BlockSender | MailAction::UnblockSender) {
            grouped_titlebar_icon(button)
        } else {
            button
        }
    });

    if let Some(plain) = plain_text {
        actions.push(enzui_iced_widgets::action_bar::ActionItem::command(
            enzui_iced_core::action_overflow::Descriptor::new("mail.display-format")
                .always_overflow(),
            enzui_iced_core::Action::new(
                "mail.display-format",
                Msg::SetMessagePlainText(message_id.to_owned(), !plain),
            )
            .with_label(if plain {
                "Show HTML"
            } else {
                "Show plain text"
            }),
        ));
    }
    if let Some((label, enabled)) = unsubscribe {
        let unsubscribe_id =
            iced::advanced::widget::Id::from(format!("mail-reader-unsubscribe/{message_id}"));
        let action = enzui_iced_core::Action::new(
            "mail.unsubscribe",
            Msg::Unsubscribe(message_id.to_owned()),
        )
        .with_label(label)
        .with_enabled(enabled);
        actions.insert(
            sender_index.unwrap_or_else(|| {
                actions
                    .len()
                    .saturating_sub(usize::from(plain_text.is_some()))
            }),
            enzui_iced_widgets::action_bar::ActionItem::with_inline(
                enzui_iced_core::action_overflow::Descriptor::new("mail.unsubscribe")
                    .with_priority(enzui_iced_core::action_overflow::VisibilityPriority(50)),
                action,
                move |action, _| {
                    grouped_titlebar_icon(
                        enz_toolbar_icon("mail-minus", action.message().clone())
                            .id(unsubscribe_id.clone())
                            .variant(ToolbarIconVariant::InkOnly)
                            .label(action.label())
                            .enabled(action.is_enabled())
                            .into_element(),
                    )
                },
            )
            .group("mail-sender"),
        );
    }

    // Keyed by message: any value shared between stacked cards opens every menu
    // at once and collides in the widget tree.
    let toggle_msg_id = message_id.to_owned();
    let close_msg_id = message_id.to_owned();
    let trigger_id = reader_overflow_trigger_id(message_id);
    let rtl = crate::l10n::rtl();
    let bar = enz_action_bar(actions, Msg::ToggleReaderOverflow(toggle_msg_id))
        .fold_policy(FoldPolicy::Measured)
        .collector(Collector::Kebab {
            icon: "ellipsis-vertical".to_owned(),
            tokens: None,
        })
        .collector_placement(CollectorPlacement::TrailingCluster)
        .overflow_open(overflow_open)
        .on_dismiss(Msg::CloseReaderOverflow(close_msg_id))
        .breakpoints(enz.breakpoints)
        .overflow_trigger_id(trigger_id)
        .spacing(SpaceToken::Xs);
    if compact {
        return bar
            .align_x(if rtl {
                iced::alignment::Horizontal::Left
            } else {
                iced::alignment::Horizontal::Right
            })
            .into_element();
    }
    let primary = row(verbs
        .iter()
        .filter(|verb| verb.group() == mail_actions::ActionGroup::Compose)
        .map(|verb| {
            enz_toolbar_icon(verb.icon(), verb.message(&target))
                .id(iced::advanced::widget::Id::from(format!(
                    "mail-reader-action/{message_id}/{}",
                    verb.id()
                )))
                .variant(ToolbarIconVariant::InkOnly)
                .label(verb.label(&target))
                .into_element()
        }))
    .spacing(SpaceToken::Xs.resolve(&enzui_iced_theme::SpacingScale::default()))
    .mirrored(rtl)
    .align_y(Alignment::Center);
    row![primary, bar.into_element()]
        .spacing(enz.spacing.md())
        .mirrored(rtl)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into()
}

fn message_card<'a>(s: &'a State, m: &'a Message) -> Element<'a, Msg> {
    let enz = s.themes.theme();
    let ty = &enz.typography;
    let sp = &enz.spacing;
    let reader_tertiary = t::text_tertiary_on(enz, t::reader_background(enz));
    let quotes = s.store.quoted_body(m);
    let show_quotes = s.quoted_text_expanded.contains(&m.id);
    let plain_body = if !show_quotes && let Some(end) = quotes.plain_end {
        m.body.get(..end).unwrap_or(&m.body).trim_end()
    } else {
        &m.body
    };
    let recipients = iced::widget::column(
        m.to.iter()
            .enumerate()
            .map(|(index, address)| address_menu(s, m, Some(index), address.clone())),
    )
    .spacing(sp.xs());
    let date: Element<Msg> = reader_header_text(m.date.relative_label(s.store.today), false, enz)
        .color(reader_tertiary)
        .single_line_elided()
        .into();
    let recipient_line = row![
        enz_text(crate::l10n::message(
            "mail-to-recipients",
            &[("recipients", String::new())]
        )),
        recipients,
    ]
    .align_y(Alignment::Center)
    .spacing(sp.xs());
    let recipients_date: Element<Msg> = if s.mode() == LayoutMode::Narrow {
        column![recipient_line, date].spacing(sp.xs()).into()
    } else {
        row![recipient_line, Space::new().width(Length::Fill), date]
            .align_y(Alignment::Center)
            .spacing(sp.sm())
            .into()
    };
    let subject = enz_text(crate::model::types::shown_subject(&m.subject).into_owned())
        .size(TextSize::Title3)
        .width(Length::Fill)
        .wrap(true)
        .lines(2);

    let sender_address = address_menu(s, m, None, format!("<{}>", m.sender_email));
    let sender_name: Element<Msg> = reader_header_text(
        crate::l10n::message("mail-from-sender", &[("sender", m.sender_name.clone())]),
        false,
        enz,
    )
    .color(t::text_secondary(enz))
    .single_line_elided()
    .into();
    let sender_line: Element<Msg> = if s.mode() == LayoutMode::Narrow {
        // At the narrow rung the address gets its own line so a long display name
        // cannot push it past the 375 px clip.
        column![sender_name, sender_address].spacing(sp.xs()).into()
    } else {
        row![sender_name, sender_address]
            .mirrored(crate::l10n::rtl())
            .align_y(Alignment::Center)
            .spacing(sp.xs())
            .into()
    };
    let header = column![subject, sender_line, recipients_date].spacing(3);

    let (body, block_info): (Element<Msg>, Element<Msg>) = match m.body_state {
        state @ crate::model::types::MessageBodyState::Incomplete { .. } => (
            enz_text(state.disclosure())
                .size_px(ty.body_size)
                .font_role(FontRole::Document)
                .font_from_theme(enz)
                .color(t::text_secondary(enz))
                .into(),
            Space::new().into(),
        ),
        state @ crate::model::types::MessageBodyState::Malformed => (
            enz_text(state.disclosure())
                .size_px(ty.body_size)
                .font_role(FontRole::Document)
                .font_from_theme(enz)
                .color(t::text_secondary(enz))
                .into(),
            Space::new().into(),
        ),
        crate::model::types::MessageBodyState::Complete => match &m.body_html {
            Some(_) if s.shows_plain_text(&m.id) => {
                let strip: Element<Msg> = if m.body.trim().is_empty() {
                    enzui_iced_widgets::flow::enz_flow()
                        .spacing_scale(enz.spacing)
                        .wrap(true)
                        .gap(SpaceToken::Xs)
                        .cross_gap(SpaceToken::Xs)
                        .align_items(Alignment::Center)
                        .push(
                            enz_text(crate::l10n::tr("HTML-only message"))
                                .size(TextSize::Footnote)
                                .color(t::text_secondary(enz)),
                        )
                        .push(
                            enz_button(
                                crate::l10n::tr("Show HTML"),
                                Msg::SetMessagePlainText(m.id.clone(), false),
                            )
                            .kind(ButtonKind::Quiet)
                            .size(ButtonSize::Xs),
                        )
                        .into()
                } else {
                    Space::new().into()
                };
                (
                    enz_text(plain_body.to_owned())
                        .size_px(ty.body_size)
                        .font_role(FontRole::Document)
                        .font_from_theme(enz)
                        .color(t::text_primary(enz))
                        .into(),
                    strip,
                )
            }
            Some(source) => {
                let display_source = s.reader_html_source(m, &quotes).unwrap_or(source);
                // Sanitization never turns remote URLs into fetchable view state.
                let rendered = s.store.render_html_cached(&m.id, source, false);
                let strip = block_info_strip(s, m, &rendered, enz);
                let body: Element<Msg> = if let Some(pane) =
                    s.html_reader.panes.get(&m.id).filter(|pane| {
                        pane.display_matches(display_source, s.html_images_allowed(m))
                    }) {
                    if let Some(presented) = &pane.presented {
                        html_frame(pane, presented, pane.can_input(presented))
                    } else if pane.failed {
                        column![
                            enz_text(crate::l10n::tr(
                                "HTML display unavailable. Showing readable text."
                            )),
                            html_blocks_view(rendered.blocks, enz)
                        ]
                        .spacing(sp.sm())
                        .into()
                    } else {
                        loading_html(s, m, pane.height)
                    }
                } else {
                    loading_html(s, m, 480.0)
                };
                let body = crate::html_viewport::HtmlViewport::new(body, m.id.clone()).into();
                (body, strip)
            }
            None => (
                enz_text(plain_body.to_owned())
                    .size_px(ty.body_size)
                    .font_role(FontRole::Document)
                    .font_from_theme(enz)
                    .color(t::text_primary(enz))
                    .into(),
                Space::new().into(),
            ),
        },
    };

    let quoted_control: Element<Msg> = if m.body_state
        == crate::model::types::MessageBodyState::Complete
        && if m.body_html.is_some() && !s.shows_plain_text(&m.id) {
            quotes.html_collapsed.is_some()
        } else {
            quotes.plain_end.is_some()
        } {
        enz_button(
            crate::l10n::tr(if show_quotes {
                "Hide quoted text"
            } else {
                "Show quoted text"
            }),
            Msg::ToggleQuotedText(m.id.clone()),
        )
        .icon(if show_quotes {
            "chevron-up"
        } else {
            "ellipsis"
        })
        .kind(ButtonKind::Secondary)
        .size(ButtonSize::Xs)
        .id(enzui_iced_core::identity::derive_id(
            "mail-quoted-text",
            &m.id,
        ))
        .into()
    } else {
        Space::new().into()
    };

    let attach_strip: Element<Msg> = if m.attachments.is_empty() {
        Space::new().into()
    } else {
        column(
            m.attachments
                .iter()
                .map(|a| {
                    let row = crate::native_drop::attachment_source(
                        attachment_row(&m.id, a, enz),
                        &s.attachment_store,
                        a,
                    );
                    match attachment_preview(s, a) {
                        Some(preview) => column![preview, row].spacing(sp.xs()).into(),
                        None => row,
                    }
                })
                .collect::<Vec<_>>(),
        )
        .spacing(sp.xs())
        .into()
    };

    // The message is not a card: the surface is transparent and square, kept
    // only for its text inset. The inline image note covers MIME-bundled images,
    // not remote `<img src>`.
    let inline_image_note: Element<Msg> = if m.has_inline_image {
        enz_text(crate::l10n::tr("Inline image bundled with the message"))
            .size(TextSize::Caption)
            .color(t::text_secondary(enz))
            .into()
    } else {
        Space::new().height(0).into()
    };

    let selectable = enz_selection_scope(column![
        header,
        Space::new().height(SpaceToken::Space3.resolve(sp)),
        container(enz_divider()).padding(Padding::from([sp.sm(), 0.0])),
        scheduled_banner(s, m),
        snooze_banner(s, m),
        block_info,
        Space::new().height(SpaceToken::Sm.resolve(sp)),
        body,
        quoted_control,
        Space::new().height(SpaceToken::Space3.resolve(sp)),
        inline_image_note,
        Space::new().height(SpaceToken::Space5.resolve(sp)),
        attach_strip,
    ])
    .on_copy(Msg::CopyMessageSelection)
    .on_context_invocation(Msg::OpenMessageSelectionMenu)
    // Keyboard selection needs an addressable focus stop for each card.
    .id(message_selection_document_id(&m.id))
    .context_id(message_selection_context_id(&m.id))
    .context_label(crate::l10n::tr("Selected text actions"))
    .document_key(&m.id)
    .clear_generation(s.message_selection_clear_generation)
    .into_element();

    enz_surface(selectable)
        .tone(SurfaceTone::Transparent)
        .radius_size(RadiusSize::None)
        .padding(Padding {
            top: sp.xs(),
            bottom: sp.space7,
            left: sp.space5,
            right: sp.space5,
        })
        .fill_width()
        .into()
}

// A scheduled draft says when it sends, and can stay a draft instead.
fn scheduled_banner<'a>(s: &'a State, m: &'a Message) -> Element<'a, Msg> {
    if !matches!(
        m.location,
        crate::model::types::MessageLocation::Mailbox(MailboxKind::Drafts)
    ) {
        return Space::new().into();
    }
    let Some(at) = m.send_at else {
        return Space::new().into();
    };
    let sp = &s.themes.theme().spacing;
    column![
        enz_banner(crate::l10n::message(
            "mail-scheduled-for",
            &[("time", at.detail_label())]
        ))
        .tone(BannerTone::Info)
        .action_text(crate::l10n::tr("Keep as draft"))
        .on_action(Msg::CancelScheduledSend(m.id.clone()))
        .dismissible(false)
        .id_prefix(format!("mail-scheduled-{}", m.id))
        .into_element(),
        enz_button(
            crate::l10n::tr("Change time"),
            Msg::ChangeSchedule(m.id.clone())
        )
        .kind(ButtonKind::Quiet)
        .size(ButtonSize::Sm),
        Space::new().height(SpaceToken::Sm.resolve(sp)),
    ]
    .into()
}

fn snooze_banner<'a>(s: &'a State, m: &'a Message) -> Element<'a, Msg> {
    if matches!(
        m.location,
        crate::model::types::MessageLocation::Mailbox(MailboxKind::Trash | MailboxKind::Junk)
    ) {
        return Space::new().into();
    }
    let Some(until) = m.snoozed_until else {
        return Space::new().into();
    };
    let sp = &s.themes.theme().spacing;
    column![
        enz_banner(crate::l10n::message(
            "mail-snoozed-until",
            &[("time", super::dialogs::snooze_time_label(until))]
        ))
        .tone(BannerTone::Info)
        .action_text("Bring back now")
        .on_action(Msg::Unsnooze)
        .dismissible(false)
        .id_prefix(format!("mail-snooze-{}", m.id))
        .into_element(),
        Space::new().height(SpaceToken::Sm.resolve(sp)),
    ]
    .into()
}

fn block_info_strip<'a>(
    s: &'a State,
    m: &'a Message,
    rendered: &RenderedHtml,
    enz: &EnzTheme,
) -> Element<'a, Msg> {
    if s.html_images_allowed(m)
        || (!m.has_remote_content
            && rendered.blocked_remote_urls.is_empty()
            && rendered.trackers.is_empty())
    {
        return Space::new().height(0).into();
    }
    let enabled = !s.privacy.proxy_remote_content
        && !crate::mail_actions::sender_is_blocked(&s.privacy.block_list, &m.sender_email);
    let label = row![
        enz_icon("image-off")
            .size(13.0)
            .tint(t::text_secondary(enz)),
        enz_text(crate::l10n::tr("Images blocked"))
            .size(TextSize::Footnote)
            .color(t::text_secondary(enz))
    ]
    .mirrored(crate::l10n::rtl())
    .align_y(Alignment::Center)
    .spacing(enz.spacing.xs());
    enzui_iced_widgets::flow::enz_flow()
        .spacing_scale(enz.spacing)
        .wrap(true)
        .gap(SpaceToken::Xs)
        .cross_gap(SpaceToken::Xs)
        .align_items(Alignment::Center)
        .push(label)
        .push(
            enz_button(
                crate::l10n::tr("Allow once"),
                Msg::AllowImagesOnce(m.id.clone()),
            )
            .id(iced::advanced::widget::Id::from(format!(
                "mail-allow-images-once-{}",
                m.id
            )))
            .kind(ButtonKind::Quiet)
            .size(ButtonSize::Xs)
            .enabled(enabled),
        )
        .push(
            enz_button(
                crate::l10n::tr("Always allow"),
                Msg::AlwaysAllowSender(m.sender_email.clone()),
            )
            .id(iced::advanced::widget::Id::from(format!(
                "mail-allow-sender-{}",
                m.id
            )))
            .kind(ButtonKind::Quiet)
            .size(ButtonSize::Xs)
            .enabled(enabled),
        )
        .into()
}

fn reader_empty<'a>(_enz: &EnzTheme) -> Element<'a, Msg> {
    enz_status_page(crate::l10n::tr("No message selected"))
        .description(crate::l10n::tr(
            "Choose a conversation from the list to read it here.",
        ))
        .icon("mail")
        .tone(StatusPageTone::Empty)
        .into()
}

fn address_menu<'a>(
    s: &'a State,
    message: &Message,
    recipient: Option<usize>,
    label: String,
) -> Element<'a, Msg> {
    let address = recipient.map_or_else(
        || Some((message.sender_name.clone(), message.sender_email.clone())),
        |index| {
            message
                .to
                .get(index)
                .and_then(|value| crate::model::recipients::parse_recipient(value))
        },
    );
    let state = if s.sender_menu_target.as_ref().is_some_and(|target| {
        target.message_id == message.id
            && target.recipient == recipient
            && address
                .as_ref()
                .is_some_and(|(name, email)| name == &target.name && email == &target.email)
    }) {
        &s.sender_menu_state
    } else {
        &s.sender_menu_closed_state
    };
    let message_id = message.id.clone();
    enz_menu_button(label, &s.sender_menu_model, state, move |event| {
        Msg::SenderMenuEvent(message_id.clone(), recipient, event)
    })
    .menu_id(iced::advanced::widget::Id::from(format!(
        "mail-reader-address/{}/{recipient:?}",
        message.id
    )))
    .kind(MenuButtonKind::Quiet(ButtonSize::Sm))
    .into()
}

fn loading_html<'a>(s: &'a State, message: &Message, height: f32) -> Element<'a, Msg> {
    let previous = s.html_reader.previous_pane(&message.id, |id, pane| {
        s.store.message(id).is_some_and(|previous| {
            previous.account_id == message.account_id
                && !s.shows_plain_text(id)
                && previous.body_html.as_deref().is_some_and(|source| {
                    let quotes = s.store.quoted_body(previous);
                    pane.display_matches(
                        s.reader_html_source(previous, &quotes).unwrap_or(source),
                        s.html_images_allowed(previous),
                    )
                })
        })
    });
    if let Some(pane) = previous
        && let Some(presented) = &pane.presented
    {
        // A retained preview cannot receive input or publish the old page's accessibility tree.
        html_frame(pane, presented, false)
    } else {
        container(enz_text(crate::l10n::tr("Loading message…")))
            .width(Length::Fill)
            .height(height)
            .into()
    }
}

fn html_frame<'a>(
    pane: &'a crate::mail_html::HtmlPane,
    presented: &'a crate::mail_html::PresentedHtml,
    interactive: bool,
) -> Element<'a, Msg> {
    let frame = &presented.frame;
    let view = presented.view;
    let mut widget = enzui_iced_webview::enz_web_view(view, Some(frame.as_ref()), true)
        .max_width(presented.logical_width as f32)
        .title(crate::l10n::tr("HTML message"));
    widget = widget.presented(presented.allocation.handle().clone());
    if interactive {
        widget = widget.page_cursor(pane.cursor.as_deref());
        if let (Some(accessibility), Some(router)) = (&pane.accessibility, &pane.router)
            && let Some(published) = accessibility.published_page(router, view)
        {
            widget = widget.page_a11y(published, router);
        }
        widget = widget.on_input(move |input| Msg::HtmlInput(view, input));
    }
    container(widget.into_element()).width(Length::Fill).into()
}
