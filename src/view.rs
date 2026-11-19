use enzui_iced_widgets::scroll_view::enz_scroll_view;
use enzui_iced_widgets::search_field::enz_titlebar_search;
use enzui_iced_widgets::suggestion_list::{
    EnzSuggestionList, SuggestionOption, enz_suggestion_list,
};
use enzui_iced_widgets::text_field::TextFieldSize;
use iced::widget::{Space, column, container, row};
use iced::{Alignment, Element, Length, Padding};

use crate::html_view::html_blocks_view;
use crate::model::compose::ComposeMode;
use crate::model::html::RenderedHtml;
use crate::model::types::selection_matches;
use crate::model::{
    layout::{LayoutMode, NarrowPane},
    thread::Conversation,
    types::{Account, Attachment, MailboxKind, Message, SidebarSelection},
};
use crate::theme as t;
use crate::{Message as Msg, SearchScope, State};

use enzui_iced_core::semantics::HeadingLevel;
use enzui_iced_theme::{EnzTheme, FontRole};
use enzui_iced_widgets::action_bar::{Collector, CollectorPlacement, FoldPolicy, enz_action_bar};
use enzui_iced_widgets::app_shell::{ShellLayout, ShellTokens, SidebarHeader, enz_app_shell};
use enzui_iced_widgets::badge::{BadgeSize, BadgeTone, enz_badge};
use enzui_iced_widgets::banner::{BannerTone, enz_banner};
use enzui_iced_widgets::button::{ButtonKind, ButtonSize, enz_button};
use enzui_iced_widgets::combo_box::enz_combo_box;
use enzui_iced_widgets::confirm_dialog::enz_confirm_dialog;
use enzui_iced_widgets::divider::enz_divider;
use enzui_iced_widgets::enz_pane_header;
use enzui_iced_widgets::enz_text::{EnzText, TextSize, enz_text};
use enzui_iced_widgets::icon::enz_icon;
use enzui_iced_widgets::list_row_surface::{enz_list_row_surface, list_row_id};
use enzui_iced_widgets::notice_stack::{EnzNotice, EnzNoticeStack, enz_notice_stack};
use enzui_iced_widgets::shortcuts_dialog::enz_shortcuts_dialog;
use enzui_iced_widgets::sidebar_content::enz_sidebar_content;
use enzui_iced_widgets::sidebar_row::enz_sidebar_row;
use enzui_iced_widgets::status_page::{StatusPageTone, StatusPageVariant, enz_status_page};
use enzui_iced_widgets::surface::{RadiusSize, SurfaceTone, enz_surface};
use enzui_iced_widgets::surface_stack::SpaceToken;
use enzui_iced_widgets::title_bar::enz_title_bar;
use enzui_iced_widgets::toast::ToastTone;
use enzui_iced_widgets::toolbar_icon::{ToolbarIconVariant, enz_toolbar_icon};

use enzui_iced_core::menu_model::{MenuEntryModel, MenuModel};
use enzui_iced_widgets::menu_button::{MenuButtonKind, enz_menu_button};
use enzui_iced_widgets::modal_surface::enz_modal_surface;
use enzui_iced_widgets::selection_scope::enz_selection_scope;

// Settings dismissal restores focus to this inline command when it is visible.
#[must_use]
pub(crate) fn settings_button_id() -> iced::advanced::widget::Id {
    enzui_iced_core::identity::OwnerId::of_kind(
        enzui_iced_widgets::action_bar::COMMAND_ID_KIND,
        "mail-titlebar-actions",
    )
    .id("mail.settings")
}

#[path = "view_parts/conversation.rs"]
mod conversation;
#[path = "view_parts/dialogs.rs"]
mod dialogs;
#[path = "view_parts/reader.rs"]
mod reader;
pub(crate) use conversation::address_action_menu_model;
pub(crate) use conversation::multi_selection_label;
pub(crate) use conversation::{CONVERSATION_ROW_HEIGHT, conversation_row, participants_summary};
pub(crate) use dialogs::SHORTCUTS_SCOPE_KEY;
use dialogs::{
    message_move_picker, snooze_picker, with_account_remove_confirm, with_block_sender_confirm,
    with_open_url_confirm,
};
pub(crate) use reader::attachment_row_id;
pub(crate) use reader::reader_pane;
pub(crate) use search::{message_matches_query, search_scope_choices, titlebar_search};
pub(crate) use search::{search_suggestion_list, searched_conversations};
pub(crate) use shell::{medium_with_titlebar, narrow_with_titlebar, welcome_shell, wide_shell};
pub(crate) use sidebar_list::{
    CONVO_ROW_ID_PREFIX, action_state_for_breadth, action_target_for_breadth, conversation_row_id,
    conversation_row_label, explicit_selection, explicit_selection_state,
    explicit_selection_target, sidebar_row_id_for_key, sidebar_row_key,
};
pub(crate) use sidebar_list::{SMART_SECTION, list_pane, sidebar_pane};
#[path = "view_parts/search.rs"]
mod search;
#[path = "view_parts/shell.rs"]
pub(crate) mod shell;
#[path = "view_parts/sidebar_list.rs"]
mod sidebar_list;

pub(crate) fn root(s: &State) -> Element<'_, Msg> {
    let _theme_scope = enzui_iced_theme::text_defaults::TextDefaultsScope::enter(s.theme_ref());
    crate::l10n::current().view(|| s.attach_chooser.view(localized_root(s)))
}

fn localized_root(s: &State) -> Element<'_, Msg> {
    static READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if std::env::var("ENZUI_CAPTURE_READY").as_deref() == Ok("1")
        && !s.startup.pending()
        && s.credential_store_ready
        && s.credential_inflight.is_none()
        && s.attachment_cleanup_inflight == 0
        && !READY.swap(true, std::sync::atomic::Ordering::Relaxed)
    {
        eprintln!("[CAPTURE_READY] mail-app");
    }

    let shell: Element<Msg> = match s.mode() {
        _ if s.needs_first_account() => welcome_shell(s),
        LayoutMode::Wide => wide_shell(s),
        LayoutMode::Medium => medium_with_titlebar(s),
        LayoutMode::Narrow => narrow_with_titlebar(s),
    };
    // Notices about Mail's own work float in the bottom-trailing corner, over
    // the content and never above the title bar.
    let notices = notice_stack(s);
    let shell: Element<Msg> = if let Some((notice, failed)) = s.close_notice() {
        let actions: Element<Msg> = if failed {
            column![
                enz_button(crate::l10n::tr("Retry saving"), Msg::RetryCloseSettlement)
                    .kind(ButtonKind::Primary)
                    .size(ButtonSize::Sm),
                enz_button(
                    crate::l10n::tr("Close without saving"),
                    Msg::DiscardCloseSettlement
                )
                .kind(ButtonKind::Destructive)
                .size(ButtonSize::Sm),
                enz_button(
                    crate::l10n::tr("Keep window open"),
                    Msg::CancelCloseSettlement
                )
                .kind(ButtonKind::Secondary)
                .size(ButtonSize::Sm),
            ]
            .spacing(s.themes.theme().spacing.xs())
            .into()
        } else if s.close_is_committed() {
            Space::new().into()
        } else {
            row![
                Space::new().width(Length::Fill),
                enz_button(
                    crate::l10n::tr("Keep window open"),
                    Msg::CancelCloseSettlement
                )
                .kind(ButtonKind::Secondary)
                .size(ButtonSize::Sm),
            ]
            .into()
        };
        let notice = if failed {
            let reason = notice
                .strip_prefix("Could not safely close. ")
                .unwrap_or(&notice)
                .to_owned();
            enz_banner(crate::l10n::tr("Could not safely close"))
                .body(reason)
                .tone(BannerTone::Danger)
        } else {
            enz_banner(notice).tone(BannerTone::Warning)
        };
        column![
            notice.id_prefix("mail-close-settlement").into_element(),
            actions,
            shell,
        ]
        .spacing(s.themes.theme().spacing.xs())
        .into()
    } else {
        shell
    };
    let base = shell;
    let mut host = s
        .context_menu
        .host(base, move |event| {
            Msg::ContextMenuEvent(s.context_menu_generation, event)
        })
        .id_prefix("mail-context-menu")
        .at_id("mail-context-menu-host");
    if let Some(target) = s.context_menu.target() {
        host = host.label(crate::l10n::tr(target.menu_label()));
    }
    let base: Element<Msg> = host.into_element();
    // Account removal is confirmed above Settings so its destructive choice
    // cannot be hidden behind the surface that opened it.
    let with_settings = crate::settings_view::with_settings(s, base);
    let with_account_remove = with_account_remove_confirm(s, with_settings);
    // Rules stacks over Settings; its modal hosts own scrim and focus trap.
    let with_rules = crate::rules_view::with_rules(s, with_account_remove);
    // The empty-mailbox confirmation sits above Settings and below compose, so a
    // pending purge is never hidden by the Settings scrim.
    let with_confirm = crate::bulk_view::with_empty_confirm(s, with_rules);
    // HTML mail links do not launch on activation. The full sanitised
    // href is shown in this confirmation first, because sender-controlled
    // anchor text can conceal a different destination.
    let with_link_confirm = with_open_url_confirm(s, with_confirm);
    // The move picker and block confirmation are exclusive in normal use but stack
    // deterministically if a stale message arms both.
    let with_message_move = s.message_dialogs.view_with(
        with_link_confirm,
        |key| match key {
            crate::modal_focus::MessageDialog::Move => message_move_picker(s),
            crate::modal_focus::MessageDialog::Snooze => snooze_picker(s),
        },
        Msg::PickerDialogEvent,
        |key, host| {
            host.label(match key {
                crate::modal_focus::MessageDialog::Move => {
                    dialogs::move_dialog_title(s.pending_move_targets.len())
                }
                crate::modal_focus::MessageDialog::Snooze => crate::l10n::tr("Snooze"),
            })
        },
    );
    let with_block_confirm = with_block_sender_confirm(s, with_message_move);
    let with_compose = crate::schedule::with_schedule(s, with_block_confirm);
    let presentation = if s.shortcuts_open {
        enz_shortcuts_dialog(true, crate::shortcuts::help_entries(&s.shortcuts))
            .title(crate::l10n::tr("Keyboard Shortcuts"))
            .close_label(crate::l10n::tr("Done"))
            .into_modal_host(with_compose, Msg::ShortcutsDialogEvent)
            .scoped_to(
                s.shortcuts_focus
                    .as_ref()
                    .expect("open shortcuts have no focus lifecycle"),
            )
            .into_element()
    } else {
        with_compose
    };

    let has_notices = !notices.is_empty();
    let has_toasts = !s.undo_send_toasts.is_empty();
    let toasts = s.undo_send_toasts.view_with(|toast, spec| {
        if spec.coalesce_key.as_deref() == Some(crate::UNDO_SEND_COALESCE_KEY)
            && let Some(pending) = s.pending_undo_toast
        {
            toast.secondary_action(
                crate::l10n::tr("Send now"),
                crate::ToastEvent::Action(pending.id, crate::SEND_NOW_ACTION.into()),
            )
        } else {
            toast
        }
    });
    let corner = column![notices.into_element(), toasts.map(Msg::Toast)]
        .align_x(Alignment::End)
        .spacing(if has_notices && has_toasts {
            s.themes.theme().spacing.sm()
        } else {
            0.0
        });
    // The overlay stays mounted so notices cannot re-parent focused app content.
    let root = enzui_iced_widgets::toast_host::enz_toast_layer(
        presentation,
        (has_notices || has_toasts).then(|| corner.into()),
        enzui_iced_widgets::toast_host::ToastOverlayPlacement::default().trailing(),
    );
    enzui_iced_widgets::anchored_overlay::dismiss_on_escape(
        root,
        !s.search_open && (s.compose.open || s.quick_reply_mode.is_some()),
        || Msg::LeaveCompose,
    )
}

const NOTICE_WIDTH: f32 = 360.0;

fn banner_tone(tone: BannerTone) -> ToastTone {
    match tone {
        BannerTone::Danger => ToastTone::Error,
        BannerTone::Warning => ToastTone::Warning,
        BannerTone::Success => ToastTone::Success,
        _ => ToastTone::Info,
    }
}

// Mail's own local-state progress or failure, shown first in the corner.
fn local_state_notice(s: &State) -> Option<(String, BannerTone)> {
    if s.startup_is_loading() {
        // Absence and "not read yet" are different states; only a settled read
        // may present an empty mailbox as the user's own.
        Some((
            crate::l10n::tr("Loading your saved mail, preferences and shortcuts\u{2026}"),
            BannerTone::Info,
        ))
    } else if let Some(summary) = s.local_state_load_error.as_deref() {
        // Retained bytes are the only case where this session may publish new
        // state; without custody the refused file stays authoritative.
        let consequence = if s.local_state_retained_for_recovery {
            "The unreadable file was kept as a recovery copy, so this session starts with no local mail."
        } else {
            "Local mail state is read-only until it is repaired."
        };
        Some((
            format!(
                "{}. {}",
                crate::l10n::tr(summary),
                crate::l10n::tr(consequence)
            ),
            BannerTone::Danger,
        ))
    } else {
        s.local_state_error.as_ref().map(|error| {
            (
                crate::l10n::message(
                    "mail-local-save-failed",
                    &[("error", crate::l10n::tr(&error.user_summary()))],
                ),
                BannerTone::Danger,
            )
        })
    }
}

struct Notice {
    id: &'static str,
    title: String,
    body: Option<String>,
    tone: ToastTone,
    // `None`: only its cause can clear it, so closing it hides it until it
    // changes (`Msg::HideNotice`).
    dismiss: Option<Msg>,
    action: Option<(&'static str, Msg)>,
    secondary: Option<(&'static str, Msg)>,
}

impl Notice {
    // Identifies this notice's content; a hidden notice stays hidden only
    // while the same content is showing.
    fn key(&self) -> String {
        format!(
            "{}\n{}\n{}",
            self.id,
            self.title,
            self.body.as_deref().unwrap_or_default()
        )
    }
}

// The keys of the corner notices showing now, hidden or not.
pub(crate) fn notice_keys(s: &State) -> Vec<String> {
    notices(s).iter().map(Notice::key).collect()
}

fn notices(s: &State) -> Vec<Notice> {
    let mut notices = Vec::new();
    if let Some((notice, tone)) = local_state_notice(s) {
        notices.push(Notice {
            id: "mail-local-state",
            title: notice,
            body: None,
            tone: banner_tone(tone),
            dismiss: None,
            action: None,
            secondary: None,
        });
    }
    // A refused preferences write, from Settings, Rules, the composer, the
    // reader or close. The origin's title, not `SaveFailure`'s `Display`, which
    // names a disk path.
    if let Some(failure) = s.preferences_error.as_ref() {
        let origin = failure.origin();
        notices.push(Notice {
            id: if origin == crate::preferences::SaveOrigin::CloseSettlement {
                "mail-close-preferences-error"
            } else {
                "mail-preferences-save-error"
            },
            title: origin.failure_title().to_owned(),
            body: Some(origin.failure_body().to_owned()),
            tone: ToastTone::Error,
            dismiss: None,
            action: None,
            secondary: None,
        });
    }
    if let Some(summary) = s.preferences_load_error.as_deref() {
        notices.push(Notice {
            id: "mail-preferences-load-error",
            title: summary.to_owned(),
            body: Some(
                "Safe defaults are in use. Saved rules and sender decisions are protected and will not be overwritten."
                    .to_owned(),
            ),
            tone: ToastTone::Error,
            dismiss: None,
            action: None,
            secondary: None,
        });
    }
    if let Some(summary) = s.shortcut_load_error.as_deref() {
        notices.push(Notice {
            id: "mail-shortcuts-load-error",
            title: summary.to_owned(),
            body: Some(
                "Default shortcuts are in use. Saved shortcut data is protected and will not be overwritten."
                    .to_owned(),
            ),
            tone: ToastTone::Error,
            dismiss: None,
            action: None,
            secondary: None,
        });
    }
    if let Some(notice) = &s.link_open_notice {
        notices.push(Notice {
            id: "mail-open-uri-failure",
            title: notice.clone(),
            body: None,
            tone: ToastTone::Error,
            dismiss: Some(Msg::DismissOpenUrlNotice),
            action: None,
            secondary: None,
        });
    }
    if let Some(notice) = &s.attachment_notice {
        notices.push(Notice {
            id: "mail-attachment-details",
            title: notice.clone(),
            body: None,
            tone: ToastTone::Info,
            dismiss: Some(Msg::DismissAttachmentNotice),
            action: None,
            secondary: None,
        });
    }
    if let Some(notice) = &s.mail_notice {
        let cancel = if notice.can_cancel_outbox {
            "Cancel queued send"
        } else {
            "Cancel"
        };
        // A retryable operation stays queued until Retry or Cancel decides it;
        // closing its notice only hides it until the operation's status changes.
        let (dismiss, action, secondary) = if notice.retryable {
            (
                None,
                Some(("Retry", Msg::RetryMailOperation)),
                Some((cancel, Msg::CancelMailOperation)),
            )
        } else if notice.can_cancel_outbox {
            (
                Some(Msg::DismissMailNotice),
                Some((cancel, Msg::CancelMailOperation)),
                None,
            )
        } else {
            (Some(Msg::DismissMailNotice), None, None)
        };
        notices.push(Notice {
            id: "mail-backend-status",
            title: notice.title.clone(),
            body: Some(notice.body.clone()),
            tone: if notice.retryable {
                ToastTone::Error
            } else {
                ToastTone::Info
            },
            dismiss,
            action,
            secondary,
        });
    }
    if let Some((title, body)) = s.local_timestamp_notice() {
        notices.push(Notice {
            id: "mail-local-timestamp-error",
            title,
            body: Some(body),
            tone: ToastTone::Error,
            dismiss: None,
            action: Some(("Retry", Msg::RetryLocalTimestamp)),
            secondary: None,
        });
    }
    notices
}

// The corner's toasts. Exactly one is a live region, the first error or else
// the first notice, so a stack of notices is announced once rather than
// talking over itself.
// Every notice can be closed.
fn notice_stack(s: &State) -> EnzNoticeStack<Msg> {
    let notices = notices(s)
        .into_iter()
        .filter(|notice| !s.hidden_notices.contains(&notice.key()))
        .map(|notice| {
            let dismiss = notice
                .dismiss
                .clone()
                .unwrap_or_else(|| Msg::HideNotice(notice.key()));
            let mut toast = EnzNotice::new(notice.id, crate::l10n::tr(&notice.title), notice.tone)
                .on_dismiss(dismiss);
            if let Some(body) = notice.body {
                toast = toast.description(crate::l10n::tr(&body));
            }
            if let Some((label, message)) = notice.action {
                toast = toast.action(crate::l10n::tr(label), message);
            }
            if let Some((label, message)) = notice.secondary {
                toast = toast.secondary_action(crate::l10n::tr(label), message);
            }
            toast
        })
        .collect();
    enz_notice_stack(notices)
        .spacing(s.themes.theme().spacing.xs())
        .width(Length::Fixed(NOTICE_WIDTH))
}

pub(crate) const COMPOSE_LABEL: &str = "New";

pub(crate) const COMPOSE_ICON: &str = "square-pen";

pub(crate) const TITLEBAR_SEARCH_WIDTH: f32 = 320.0;

// The `id_prefix` the collapsed `enz_titlebar_search` derives its trigger and
// clear ids under; `FocusSearchField` uses it so `/` can focus the icon.
pub(crate) const SEARCH_ID_PREFIX: &str = "mail-titlebar-search";

pub(crate) const SEARCH_PLACEHOLDER: &str = "Search mail…";

pub(crate) const SEARCH_FIELD_ID: &str = "mail-titlebar-search";

pub(crate) fn search_field_id() -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::new(SEARCH_FIELD_ID)
}

pub(crate) const SEARCH_EMPTY_TEXT: &str = "No matching messages";

pub(crate) const TITLEBAR_SEARCH_MAX_RESULTS: usize = 30;

// Search matches sanitized HTML text, not markup or URLs; CurrentMailbox shares the list predicate.
pub(crate) fn titlebar_search_results<'a>(
    store: &'a crate::model::types::MailboxStore,
    selection: &SidebarSelection,
    scope: SearchScope,
    query: &str,
) -> Vec<&'a crate::model::types::Message> {
    let q = crate::model::search::parse(query);
    if q.is_empty() {
        return Vec::new();
    }
    let today = store.today;
    let mut messages: Vec<&crate::model::types::Message> = store
        .messages
        .iter()
        .filter(|m| match scope {
            SearchScope::AllMailboxes => true,
            SearchScope::CurrentMailbox => selection_matches(selection, m, today),
        })
        .filter(|m| message_matches_query(store, m, &q))
        .collect();
    messages.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| a.id.cmp(&b.id)));
    messages.truncate(TITLEBAR_SEARCH_MAX_RESULTS);
    messages
}
