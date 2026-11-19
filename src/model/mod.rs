pub(crate) mod bulk;
pub(crate) mod compose;
pub(crate) mod fixture;
pub(crate) mod header_text;
pub(crate) mod html;
pub(crate) mod layout;
pub(crate) mod privacy;
pub(crate) mod quoted;
pub(crate) mod recipients;
pub(crate) mod rules;
pub(crate) mod search;
pub(crate) mod settings;
pub(crate) mod sidebar_layout;
pub(crate) mod snooze;
pub(crate) mod thread;
pub(crate) mod types;

#[allow(
    unused_imports,
    reason = "The module facade serves production and feature-specific tests."
)]
pub(crate) use bulk::{
    BulkState, bulk_move, bulk_set_flagged, bulk_set_unread, emptiable_mailbox, empty_action_label,
    empty_mailbox, empty_mailbox_prompt, mark_all_read, unread_in_selection,
};
#[allow(
    unused_imports,
    reason = "The module facade serves production and feature-specific tests."
)]
pub(crate) use compose::{
    ComposeController, ComposeMode, ComposeState, PendingSend, compose_body_with_account_signature,
    compose_body_with_signature,
};
#[allow(
    unused_imports,
    reason = "The module facade serves production and feature-specific tests."
)]
pub(crate) use html::{
    HtmlBlock, InlineRun, ListMarker, RenderedHtml, SanitizeOptions, blocks_to_plain_text,
    runs_to_text, sanitize_and_render_html,
};
#[allow(
    unused_imports,
    reason = "The module facade serves production and feature-specific tests."
)]
pub(crate) use layout::{
    LayoutMode, NARROW_MAX_WIDTH, NarrowNav, NarrowPane, layout_mode, wide_min_width,
};
#[allow(
    unused_imports,
    reason = "The module facade serves production and feature-specific tests."
)]
pub(crate) use privacy::{
    AllowedReason, BlockedReason, RemoteContentDecision, RemoteContentPolicy, Tracker, TrackerKind,
    detect_trackers, remote_content_decision,
};
#[allow(
    unused_imports,
    reason = "The module facade serves production and feature-specific tests."
)]
pub(crate) use recipients::{
    Contact, active_token, contact_matches, recent_contacts, replace_active_token, suggest,
};
#[allow(
    unused_imports,
    reason = "The module facade serves production and feature-specific tests."
)]
pub(crate) use rules::{
    ACTION_MAILBOXES, Action, Condition, ConditionCombinator, ConditionField, ConditionMatch, Rule,
    RuleDraft, RuleSet, RulesUiState, action_for_kind_index, action_kind_index,
    applied_rules_label, apply_rules, mailbox_for_index, mailbox_index,
};
#[allow(
    unused_imports,
    reason = "The module facade serves production and feature-specific tests."
)]
pub(crate) use search::{Field as SearchField, SearchQuery, Term as SearchTerm};
#[allow(
    unused_imports,
    reason = "The module facade serves production and feature-specific tests."
)]
pub(crate) use settings::{
    AddAccountState, AddAccountStep, DraftField, SettingsState, SettingsTab, account_count_label,
    commit_add_account,
};
#[allow(
    unused_imports,
    reason = "The module facade serves production and feature-specific tests."
)]
pub(crate) use thread::{Conversation, normalize_subject, thread_messages};
#[allow(
    unused_imports,
    reason = "The module facade serves production and feature-specific tests."
)]
pub(crate) use types::{
    Account, Attachment, FavoriteKind, Folder, MailboxKind, MailboxStore, Message, MessageDate,
    MessageLocation, PrivacyState, SidebarSelection, format_size, mailbox_summary_label,
    selection_matches,
};
