use super::*;
use iced::widget::column;

// Collapse keys for the two built-in sections; account sections use the
// account id, which never takes this prefix.
pub(crate) const FAVORITES_SECTION: &str = "section:favorites";
pub(crate) const SMART_SECTION: &str = "section:smart";
pub(super) const LIST_PANE_DIVIDER_WIDTH: f32 = 1.0;

pub(crate) fn sidebar_pane(s: &State, w: Length) -> Element<'_, Msg> {
    let sp = s.themes.theme().spacing;
    let mut body = enz_sidebar_content()
        .width(Length::Fill)
        .height(Length::Shrink)
        .padding_top_px(sp.sm())
        .padding_bottom_px(sp.sm())
        .section_inset_x_px(0.0)
        .spacing_scale(s.themes.theme().spacing);

    // A hidden section contributes neither caption nor rows; a heading over
    // nothing reads as a failed load.
    let favorites = s.sidebar_layout.visible_favorites();
    if !favorites.is_empty() {
        let expanded = !s.collapsed_sections.contains(FAVORITES_SECTION);
        let mut rows: Vec<Element<Msg>> = Vec::new();
        for target in favorites {
            let sel = target.selection();
            let unread = s.store.unread_count(&sel);
            let key = favorite_row_key(&sel);
            let label = match target.account_email_in(&s.store.accounts) {
                Some(email) => format!("{} · {email}", target.display_label_in(&s.store.accounts)),
                None => {
                    let key = match &target {
                        crate::model::sidebar_layout::FavoriteTarget::Unified(kind) => match kind {
                            crate::model::types::FavoriteKind::Inbox => "mail-all-inbox",
                            crate::model::types::FavoriteKind::Flagged => "mail-all-flagged",
                            crate::model::types::FavoriteKind::Drafts => "mail-all-drafts",
                            crate::model::types::FavoriteKind::Sent => "mail-all-sent",
                        },
                        _ => "mail-all-inbox",
                    };
                    crate::l10n::current().text(key)
                }
            };
            rows.push(sidebar_row_view(
                label,
                target.icon(),
                (unread > 0).then_some(unread),
                0,
                s.selection == sel,
                Msg::SelectSidebar(sel.clone()),
                &key,
                &sel,
                s.themes.theme(),
            ));
        }
        body = body.push_section_element(sidebar_section(
            &crate::l10n::tr("Favorites"),
            FAVORITES_SECTION,
            expanded,
            column(rows).spacing(sp.xs()).into(),
            s.themes.theme(),
        ));
    }

    let smart = s.active_smart_mailboxes();
    if !smart.is_empty() {
        let expanded = !s.collapsed_sections.contains(SMART_SECTION);
        let mut rows: Vec<Element<Msg>> = Vec::new();
        for kind in &smart {
            let sel = SidebarSelection::SmartMailbox(*kind);
            let unread = if matches!(
                kind,
                crate::model::sidebar_layout::SmartMailboxKind::Scheduled
                    | crate::model::sidebar_layout::SmartMailboxKind::Snoozed
            ) {
                s.store
                    .messages
                    .iter()
                    .filter(|m| kind.matches(m, s.store.today))
                    .count()
            } else {
                s.store.unread_count(&sel)
            };
            let key = sidebar_row_key(&sel);
            rows.push(sidebar_row_view(
                crate::l10n::tr(kind.label()),
                kind.icon(),
                (unread > 0).then_some(unread),
                0,
                s.selection == sel,
                Msg::SelectSidebar(sel.clone()),
                &key,
                &sel,
                s.themes.theme(),
            ));
        }
        body = body.push_section_element(sidebar_section(
            &crate::l10n::tr("Smart Mailboxes"),
            SMART_SECTION,
            expanded,
            column(rows).spacing(sp.xs()).into(),
            s.themes.theme(),
        ));
    }

    for acc in &s.store.accounts {
        let collapsed = s.collapsed_sections.contains(&acc.id);

        let mut account_rows: Vec<Element<Msg>> = Vec::new();
        for kind in [
            MailboxKind::Inbox,
            MailboxKind::Drafts,
            MailboxKind::Sent,
            MailboxKind::Junk,
            MailboxKind::Trash,
            MailboxKind::Archive,
        ] {
            if !acc.mailboxes.contains(&kind) {
                continue;
            }
            let sel = SidebarSelection::AccountMailbox {
                account_id: acc.id.clone(),
                mailbox: kind,
            };
            let unread = s.store.unread_count(&sel);
            let key = sidebar_row_key(&sel);
            account_rows.push(sidebar_row_view(
                crate::l10n::tr(kind.label()),
                kind.icon(),
                (unread > 0).then_some(unread),
                0,
                s.selection == sel,
                Msg::SelectSidebar(sel.clone()),
                &key,
                &sel,
                s.themes.theme(),
            ));
        }
        for folder in &acc.folders {
            let sel = SidebarSelection::Folder {
                account_id: acc.id.clone(),
                folder_id: folder.id.clone(),
            };
            let unread = s.store.unread_count(&sel);
            let key = sidebar_row_key(&sel);
            account_rows.push(sidebar_row_view(
                &folder.name,
                "folder",
                (unread > 0).then_some(unread),
                0,
                s.selection == sel,
                Msg::SelectSidebar(sel.clone()),
                &key,
                &sel,
                s.themes.theme(),
            ));
        }

        let rows: Element<Msg> = column(account_rows).spacing(sp.xs()).into();
        body = body.push_section_element(sidebar_section(
            &acc.email,
            &acc.id,
            !collapsed,
            rows,
            s.themes.theme(),
        ));
    }

    // Keep the column's stock 8 px padding: the row owns its 4 px pill gutter, and
    // zeroing either side shifts rows by 4 px.
    let inner = enz_scroll_view(body.into_element()).height(Length::Fill);
    container(inner).width(w).height(Length::Fill).into()
}

// A pinned destination and its account row share a selection, so the section
// namespace is part of the row's identity.
pub(crate) fn favorite_row_key(sel: &SidebarSelection) -> String {
    match sel {
        SidebarSelection::Favorite(_) => sidebar_row_key(sel),
        other => format!("fav-{}", sidebar_row_key(other)),
    }
}

pub(crate) fn sidebar_row_key(sel: &SidebarSelection) -> String {
    match sel {
        SidebarSelection::Favorite(f) => format!("fav-{}", f.label()),
        SidebarSelection::SmartMailbox(kind) => format!("smart-{}", kind.token()),
        SidebarSelection::AccountMailbox {
            account_id,
            mailbox,
        } => format!("acct-{account_id}-{}", mailbox.label()),
        SidebarSelection::Folder {
            account_id,
            folder_id,
        } => format!("acct-{account_id}-folder-{folder_id}"),
    }
}

#[must_use]
pub(crate) fn account_section_aria_label(email: &str, expanded: bool) -> String {
    let state = crate::l10n::current().text(if expanded {
        "mail-expanded"
    } else {
        "mail-collapsed"
    });
    crate::l10n::message(
        "mail-account-section",
        &[("email", email.to_owned()), ("state", state)],
    )
}

fn sidebar_section<'a>(
    title: &str,
    key: &str,
    expanded: bool,
    rows: Element<'a, Msg>,
    enz: &EnzTheme,
) -> Element<'a, Msg> {
    use enzui_iced_widgets::expander::{ExpanderTokens, ExpanderVariant, HoverScope, enz_expander};
    let mut tokens = ExpanderTokens::from_palette(&enz.palette);
    tokens.background = iced::Color::TRANSPARENT;
    tokens.border = iced::Color::TRANSPARENT;
    tokens.header_height = enz.spacing.lg() + enz.spacing.sm();
    tokens.content_inset = enz.spacing.xs();
    tokens.content_gap = enz.spacing.xs();
    tokens.title_size = enzui_iced_widgets::section_header::default_title_size();
    tokens.title_color = t::text_secondary(enz);
    tokens.icon_color = t::graphic_tertiary_on(enz, t::sidebar_background(enz));
    let key = key.to_owned();
    let id = enzui_iced_core::identity::derive_id("mail-sidebar-account", &key);
    enz_expander(title, expanded, move |_| {
        Msg::ToggleSidebarSection(key.clone())
    })
    .variant(ExpanderVariant::Flat)
    .hover_scope(HoverScope::Header)
    .tokens(tokens)
    .aria_label(account_section_aria_label(title, expanded))
    .id(id)
    .content(rows)
    .into_element()
}

#[allow(
    clippy::too_many_arguments,
    reason = "Mail operation inputs retain their distinct identities at this boundary."
)]
fn sidebar_row_view<'a>(
    label: impl Into<String>,
    icon: &str,
    count: Option<usize>,
    indent: u16,
    selected: bool,
    on_press: Msg,
    key: &str,
    sel: &SidebarSelection,
    enz: &EnzTheme,
) -> Element<'a, Msg> {
    // `key` is a per-section prefix so two rows sharing a label ("Inbox" in
    // Favorites and under an account) derive distinct widget ids.
    let mut r = enz_sidebar_row(label.into(), on_press)
        .icon(icon.to_owned())
        .selected(selected)
        .leading_indent(f32::from(indent))
        .sizing(enz.sizing)
        .id_prefix(format!("mail-sidebar-{key}"))
        .id(sidebar_row_focus_id(key));
    if let Some(n) = count {
        r = r.count(crate::l10n::number(n));
    }
    let menu_sel = sel.clone();
    let menu_row_key = key.to_owned();
    r.on_context(move |at| Msg::OpenSidebarMenu {
        selection: menu_sel.clone(),
        row_key: menu_row_key.clone(),
        at,
    })
    .context_label(crate::l10n::tr("Mailbox actions"))
    .into()
}

pub(crate) fn list_pane(s: &State, w: Length) -> Element<'_, Msg> {
    let ty = s.themes.theme().typography;
    let sp = s.themes.theme().spacing;
    let header_label = selection_label(&s.selection, &s.store.accounts);
    let convos = if s.search_query.trim().is_empty() {
        s.store.conversations_shared(&s.selection)
    } else {
        std::rc::Rc::new(searched_conversations(
            &s.store,
            &s.selection,
            s.search_scope,
            &s.search_query,
        ))
    };
    let summary_label = if s.search_query.trim().is_empty() {
        crate::model::types::mailbox_summary_label(&s.selection, &s.store, &s.store.accounts)
    } else {
        let count: usize = convos.iter().map(|c| c.message_ids.len()).sum();
        let mut arguments = enzui_iced_core::localization::Arguments::new();
        arguments.set("count", count);
        arguments.set("label", selection_label(&s.selection, &s.store.accounts));
        crate::l10n::current().text_with("mail-search-summary", &arguments)
    };
    let title_row = enz_pane_header(header_label)
        .title_size(ty.title_size)
        .heading(HeadingLevel::Section);
    let summary = enz_text(summary_label)
        .size(TextSize::Caption)
        .color(t::text_secondary(s.themes.theme()));
    let syncing = s.mailbox_sync_running();
    let header_actions = row![
        enz_toolbar_icon(COMPOSE_ICON, Msg::OpenCompose(ComposeMode::New))
            .label(crate::l10n::tr(COMPOSE_LABEL))
            .id(iced::advanced::widget::Id::new("mail-list-compose")),
        enzui_iced_widgets::toolbar_icon::enz_toolbar_icon("refresh-cw", Msg::RefreshMailbox)
            .id(iced::advanced::widget::Id::new("mail-mailbox-refresh"))
            .aria_label(crate::l10n::tr("Refresh"))
            .tooltip(crate::l10n::tr(if syncing {
                "Refreshing: fetching the newest messages from the server"
            } else {
                "Refresh"
            }))
            .busy(syncing)
            .into_element(),
        crate::bulk_view::mailbox_actions(s),
    ]
    .mirrored(crate::l10n::rtl())
    .spacing(sp.xs())
    .align_y(Alignment::Center);
    let header_children: Vec<Element<'_, Msg>> = vec![
        title_row.trailing(header_actions.into()).into(),
        summary.into(),
    ];
    let header = container(column(header_children).spacing(sp.xs()))
        .padding(Padding {
            top: sp.space1,
            right: sp.xs(),
            bottom: sp.sm(),
            left: sp.sm(),
        })
        .width(Length::Fill);

    let breadth = selection_breadth(s);
    let selected_flags: Vec<bool> = convos
        .iter()
        .map(|c| breadth.iter().any(|id| id == &c.id))
        .collect();
    let older: Element<Msg> = if s.older_mailbox_cursor().is_some() {
        enz_button(
            crate::l10n::tr("Load older messages"),
            Msg::LoadOlderMailbox,
        )
        .kind(ButtonKind::Quiet)
        .size(ButtonSize::Sm)
        .enabled(s.mail_intent.is_none())
        .into()
    } else {
        Space::new().height(0).into()
    };
    let body_el: Element<Msg> = if convos.is_empty() {
        empty_list_state(s.themes.theme())
    } else {
        let provider = std::rc::Rc::new(ConversationRows {
            conversations: convos,
            selected: selected_flags,
        });
        let list = enzui_iced_widgets::virtual_list::enz_virtual_list(
            provider,
            CONVERSATION_ROW_HEIGHT,
            move |_, item| {
                conversation_row(
                    item.conversation.clone(),
                    item.selected,
                    item.has_next,
                    s.themes.theme(),
                )
            },
        )
        .viewport_height(
            (s.window_size.height
                - 120.0
                - if s.older_mailbox_cursor().is_some() {
                    36.0
                } else {
                    0.0
                })
            .max(CONVERSATION_ROW_HEIGHT),
        )
        .keyboard_scroll(false)
        .self_scrolling(s.conversation_scroll.clone())
        .id(iced::advanced::widget::Id::from(format!(
            "mail-conversations-{:?}-{}",
            s.selection, s.search_query
        )))
        .into_element();
        enzui_iced_widgets::keyboard_capture::keyboard_capture(list, |key, modifiers| {
            if !modifiers.is_empty() {
                return None;
            }
            match key {
                iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowDown) => {
                    Some(Msg::SelectNextConversation)
                }
                iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowUp) => {
                    Some(Msg::SelectPreviousConversation)
                }
                _ => None,
            }
        })
    };

    let theme = s.themes.theme();
    let background = t::list_background(theme);
    let separator = t::list_separator(theme);
    container(row![
        column![header, body_el, older].spacing(sp.space0),
        container(
            Space::new()
                .width(LIST_PANE_DIVIDER_WIDTH)
                .height(Length::Fill)
        )
        .style(move |_| container::Style {
            background: Some(separator.into()),
            ..Default::default()
        })
    ])
    .style(move |_| container::Style {
        background: Some(background.into()),
        ..Default::default()
    })
    .width(w)
    .height(Length::Fill)
    .into()
}

pub(crate) fn selection_label(sel: &SidebarSelection, accounts: &[Account]) -> String {
    crate::model::types::selection_display_label(sel, accounts)
}

fn empty_list_state<'a>(_enz: &EnzTheme) -> Element<'a, Msg> {
    enz_status_page(crate::l10n::tr("No messages"))
        .description(crate::l10n::tr("This mailbox is empty."))
        .icon("inbox")
        .tone(StatusPageTone::Empty)
        .variant(StatusPageVariant::Inline)
        .compact(true)
        .into()
}

pub(crate) const CONVO_ROW_ID_PREFIX: &str = "mail-convo";

pub(crate) fn conversation_row_label(c: &Conversation) -> String {
    format!(
        "{} - {}",
        participants_summary(&c.participants),
        crate::model::types::shown_subject(&c.subject)
    )
}

// User-controlled labels may repeat; identity follows the canonical message anchor.
pub(crate) fn conversation_row_id(c: &Conversation) -> iced::advanced::widget::Id {
    list_row_id(CONVO_ROW_ID_PREFIX, &c.id)
}

// The conversations the user has explicitly picked with a modifier click, in
// the order the list renders them.
//
// Empty when the user has picked nothing. Threads that have left the current
// mailbox are filtered out, so a stale id cannot widen a verb.
pub(crate) fn explicit_selection(s: &State) -> Vec<String> {
    use enzui_iced_core::drag::ItemId;
    if s.row_selection.is_empty() {
        return Vec::new();
    }
    s.store
        .conversations_shared(&s.selection)
        .iter()
        .filter(|c| s.row_selection.contains(&ItemId::new(c.id.clone())))
        .map(|c| c.id.clone())
        .collect()
}

pub(crate) fn selection_breadth(s: &State) -> Vec<String> {
    let explicit = explicit_selection(s);
    if !explicit.is_empty() {
        return explicit;
    }
    s.selected_conversation
        .as_deref()
        .filter(|id| {
            s.store
                .conversations_for(&s.selection)
                .iter()
                .any(|c| c.id == *id)
        })
        .map(|id| vec![id.to_owned()])
        .unwrap_or_default()
}

// The action target for the explicit modifier-click selection, or `None`.
//
// The mailbox header reads this rather than [`selection_breadth`], so a
// folder menu never offers per-message verbs for the merely open conversation.
pub(crate) fn explicit_selection_target(s: &State) -> Option<crate::mail_actions::ActionTarget> {
    action_target_for(s, explicit_selection(s))
}

pub(crate) fn explicit_selection_state(s: &State) -> Option<crate::mail_actions::ActionState> {
    action_state_for(s, &explicit_selection(s))
}

// Keyboard triage uses highlighted rows, including the open conversation when no explicit selection exists.
pub(crate) fn action_target_for_breadth(s: &State) -> Option<crate::mail_actions::ActionTarget> {
    action_target_for(s, selection_breadth(s))
}

pub(crate) fn action_state_for_breadth(s: &State) -> Option<crate::mail_actions::ActionState> {
    action_state_for(s, &selection_breadth(s))
}

fn action_target_for(s: &State, ids: Vec<String>) -> Option<crate::mail_actions::ActionTarget> {
    let anchor = ids.first()?;
    let newest = s
        .store
        .conversations_for(&s.selection)
        .into_iter()
        .find(|c| &c.id == anchor)
        .and_then(|c| c.message_ids.last().cloned())?;
    Some(crate::mail_actions::ActionTarget::Selection {
        conversation_ids: ids,
        newest_message_id: newest,
    })
}

fn action_state_for(s: &State, ids: &[String]) -> Option<crate::mail_actions::ActionState> {
    let anchor = ids.first()?;
    crate::mail_actions::ActionState::for_selection(
        &s.store,
        &s.selection,
        &s.privacy.block_list,
        ids,
        anchor,
    )
}

pub(crate) fn sidebar_row_visible_label(
    sel: &SidebarSelection,
    store: &crate::model::types::MailboxStore,
) -> Option<String> {
    match sel {
        SidebarSelection::Favorite(f) => Some(crate::l10n::tr(f.label())),
        SidebarSelection::SmartMailbox(kind) => Some(crate::l10n::tr(kind.label())),
        SidebarSelection::AccountMailbox { mailbox, .. } => Some(crate::l10n::tr(mailbox.label())),
        SidebarSelection::Folder {
            account_id,
            folder_id,
        } => store
            .account(account_id)
            .and_then(|a| a.folders.iter().find(|f| &f.id == folder_id))
            .map(|f| f.name.clone()),
    }
}

fn sidebar_row_focus_id(key: &str) -> iced::advanced::widget::Id {
    enzui_iced_core::identity::derive_id(&format!("mail-sidebar-{key}"), "row")
}

pub(crate) fn sidebar_row_id_for_key(
    sel: &SidebarSelection,
    row_key: &str,
    store: &crate::model::types::MailboxStore,
) -> Option<iced::advanced::widget::Id> {
    sidebar_row_visible_label(sel, store)?;
    Some(sidebar_row_focus_id(row_key))
}

#[derive(Clone)]
struct ConversationRowData {
    conversation: Conversation,
    selected: bool,
    has_next: bool,
}

struct ConversationRows {
    conversations: std::rc::Rc<Vec<Conversation>>,
    selected: Vec<bool>,
}

impl enzui_iced_core::virtual_sequence::ItemProvider<ConversationRowData> for ConversationRows {
    fn len(&self) -> u64 {
        self.conversations.len() as u64
    }
    fn item(&self, index: u64) -> Option<ConversationRowData> {
        let index = usize::try_from(index).ok()?;
        Some(ConversationRowData {
            conversation: self.conversations.get(index)?.clone(),
            selected: self.selected[index],
            has_next: index + 1 < self.selected.len(),
        })
    }
}

impl enzui_iced_core::virtual_sequence::KeyedProvider<ConversationRowData> for ConversationRows {
    fn key(&self, index: u64) -> Option<enzui_iced_core::virtual_sequence::SequenceKey> {
        let index = usize::try_from(index).ok()?;
        self.conversations.get(index).map(|conversation| {
            enzui_iced_core::virtual_sequence::SequenceKey::new(conversation.id.clone())
        })
    }
}
