use super::*;
use enzui_iced_widgets::radio_group::{RadioChoice, enz_radio_group_keyed};
use iced::widget::column;

// Filter the list pane's threaded conversations by the same matcher and scope
// as the suggestion popup, intersected with the current pane, so an
// Inbox-labelled pane never paints Sent rows.
pub(crate) fn searched_conversations(
    store: &crate::model::types::MailboxStore,
    selection: &SidebarSelection,
    scope: SearchScope,
    query: &str,
) -> Vec<Conversation> {
    let conversations = store.conversations_for(selection);
    let q = crate::model::search::parse(query);
    if q.is_empty() {
        return conversations;
    }

    let today = store.today;
    let matching: std::collections::HashSet<&str> = store
        .messages
        .iter()
        .filter(|m| match scope {
            SearchScope::AllMailboxes => true,
            SearchScope::CurrentMailbox => selection_matches(selection, m, today),
        })
        .filter(|m| message_matches_query(store, m, &q))
        .map(|m| m.id.as_str())
        .collect();

    conversations
        .into_iter()
        .filter(|c| {
            c.message_ids
                .iter()
                .any(|id| matching.contains(id.as_str()))
        })
        .collect()
}

// Whether message `m` satisfies the parsed query `q`.
//
// Terms are ANDed and support quoted phrases and the operators
// [`crate::model::search`] lists, dates included. The body
// match uses the sanitised block text, never raw markup, memoised per message
// by `MailboxStore::body_matches_query`.
pub(crate) fn message_matches_query(
    store: &crate::model::types::MailboxStore,
    m: &crate::model::types::Message,
    q: &crate::model::search::SearchQuery,
) -> bool {
    q.matches(store, m)
}

// Scope labels stay index-aligned with
// [`SearchScope::ALL`] because [`Msg::SelectSearchScope`] carries the index.
const SCOPE_LABELS: [&str; 2] = ["All Mailboxes", "This Mailbox"];

pub(crate) fn titlebar_search<'a>(s: &'a State, max_width: f32) -> Element<'a, Msg> {
    let mut results = column![search_suggestion_list(s).into_element()];
    if let Some(server) = server_search_row(s) {
        results = results.push(server);
    }
    let field: Element<'a, Msg> = enz_titlebar_search(&s.search_query, Msg::SearchEdited)
        .placeholder(crate::l10n::tr(SEARCH_PLACEHOLDER))
        .on_clear(Msg::SearchCleared)
        .id(search_field_id())
        .id_prefix(SEARCH_ID_PREFIX)
        .filters(search_filter_row(s))
        .results(results.spacing(s.themes.theme().spacing.xs()))
        .results_open(s.search_open)
        .on_results_open(|open| {
            if open {
                Msg::SearchActivated
            } else {
                Msg::SearchDismissed
            }
        })
        .max_width(max_width)
        .into_element();
    field
}

pub(crate) const SEARCH_SERVER_LABEL: &str = "Search the server for more";

fn server_search_row<'a>(s: &'a State) -> Option<Element<'a, Msg>> {
    let query = s.search_query.trim();
    if query.is_empty()
        || !s.store.accounts.iter().any(|account| {
            account
                .config
                .as_ref()
                .is_some_and(|config| config.protocol == crate::backend::MailProtocol::Imap)
        })
    {
        return None;
    }
    let enz = s.themes.theme();
    let note = |text: String| -> Element<'a, Msg> {
        enz_text(text)
            .size(TextSize::Caption)
            .color(t::text_secondary(enz))
            .into()
    };
    let ask = |label: &'static str| -> Element<'a, Msg> {
        enz_button(crate::l10n::tr(label), Msg::SearchServer)
            .kind(ButtonKind::Quiet)
            .size(ButtonSize::Sm)
            .into()
    };
    Some(match &s.server_search {
        crate::ServerSearch::Running { query: asked } if asked == query => {
            note(crate::l10n::tr("Searching the server…"))
        }
        crate::ServerSearch::Found {
            query: asked,
            count,
        } if asked == query => note(crate::l10n::count("mail-server-search-count", *count)),
        crate::ServerSearch::Failed {
            query: asked,
            reason,
        } if asked == query => row![
            note(reason.clone()),
            Space::new().width(Length::Fill),
            ask("Try again")
        ]
        .mirrored(crate::l10n::rtl())
        .align_y(Alignment::Center)
        .into(),
        _ => ask(SEARCH_SERVER_LABEL),
    })
}

pub(crate) fn search_scope_choices() -> Vec<RadioChoice<'static, SearchScope>> {
    SCOPE_LABELS
        .iter()
        .zip(SearchScope::ALL)
        .map(|(label, scope)| RadioChoice::with_label(label, scope, crate::l10n::tr(label)))
        .collect()
}

fn search_filter_row(s: &State) -> Element<'_, Msg> {
    enz_radio_group_keyed(
        crate::l10n::tr("Search scope"),
        &s.search_scope_choices,
        Some(&s.search_scope),
        |scope| Msg::SelectSearchScope(scope.index()),
    )
    .orientation(enzui_iced_widgets::radio_group::Orientation::Horizontal)
    .size(enzui_iced_widgets::radio_group::RadioGroupSize::Sm)
    .label_visible(false)
    .spacing(s.themes.theme().spacing.sm())
    .option_id_prefix("mail-search-filter")
    .into_element()
}

pub(crate) fn search_result_preview(m: &crate::model::types::Message) -> String {
    let source = if m.preview.trim().is_empty() {
        m.body.as_str()
    } else {
        m.preview.as_str()
    };
    source.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn search_suggestion_list<'a>(s: &'a State) -> EnzSuggestionList<'a, String, Msg> {
    let options = titlebar_search_results(&s.store, &s.selection, s.search_scope, &s.search_query)
        .into_iter()
        .map(|m| {
            SuggestionOption::new(m.id.clone(), m.sender_name.clone(), m.id.clone()).rich_result(
                m.date.short_label(),
                crate::model::types::shown_subject(&m.subject).into_owned(),
                search_result_preview(m),
            )
        })
        .collect();

    enz_suggestion_list(options, |id: &String| Msg::SelectSearchResult(id.clone()))
        .embedded(true)
        .highlighted(s.search_highlight)
        .size(TextFieldSize::Sm)
        .id_prefix("mail-search-result")
        .list_label("Search results")
        .empty_text(crate::l10n::tr(SEARCH_EMPTY_TEXT))
        .show_empty(true)
}
