//! Settings surface, backed by `model/settings.rs`.
//!
//! The framework preset owns page navigation, settings chrome, pinned actions,
//! modality and adaptive dialog/sheet geometry. Pane controls use catalog
//! widgets, so this module only maps mail's model and reducer messages into
//! that vocabulary.

use iced::widget::{Space, row};
use iced::{Alignment, Element, Length};

use enzui_iced_widgets::badge::{BadgeSize, BadgeTone, enz_badge};
use enzui_iced_widgets::banner::{BannerTone, enz_banner};
use enzui_iced_widgets::button::{ButtonKind, ButtonSize, enz_button};
use enzui_iced_widgets::checkbox::enz_checkbox;
use enzui_iced_widgets::combo_box::enz_combo_box;
use enzui_iced_widgets::divider::enz_divider;
use enzui_iced_widgets::editable_shortcuts::{
    EditableShortcutsEvent, editable_shortcuts_reset_all_action, enz_editable_shortcuts,
    shortcuts_settings_page,
};
use enzui_iced_widgets::enz_text::{TextSize, enz_text};
use enzui_iced_widgets::expander::enz_expander;
use enzui_iced_widgets::flow::enz_flow;
use enzui_iced_widgets::labeled_content::enz_labeled_content;
use enzui_iced_widgets::modal_host::{SurfacePlacement, enz_modal_host, presentation_scope_id};
use enzui_iced_widgets::preferences_group::enz_preferences_group;
use enzui_iced_widgets::preferences_row::enz_preferences_row;
use enzui_iced_widgets::radio_group::{RadioGroupSize, enz_radio_group_keyed};
use enzui_iced_widgets::reorder_list::enz_reorder_list;
use enzui_iced_widgets::secure_field::enz_secure_field;
use enzui_iced_widgets::settings_dialog::{
    DialogSize, EnzSettingsPresentation, SettingsDialogEvent, SettingsNav,
    SettingsPage as PresetSettingsPage, enz_settings_dialog, enz_settings_presentation_for,
};
use enzui_iced_widgets::surface_stack::SpaceToken;
use enzui_iced_widgets::text_area::{FieldSize, enz_text_area};
use enzui_iced_widgets::text_field::enz_text_field;

use crate::backend::config::{AuthMethod, Encryption};
use crate::model::layout::LayoutMode;

use crate::model::settings::{
    AddAccountStep, DraftField, SettingsPage, SettingsTab, account_count_label,
};
use crate::theme as t;
use crate::{Message as Msg, State};

pub(crate) const DIALOG_TITLE: &str = "Mail Settings";

// Namespace for the per-page ids `enz_settings_dialog` derives, keeping them
// apart from mail's own `enz_view_switcher` ids in the shared focus scope.
pub(crate) const SETTINGS_ID_PREFIX: &str = "mail-settings";
pub(crate) const SETTINGS_SCOPE_KEY: &str = "mail-settings-presentation";

#[path = "settings_view_parts/notifications.rs"]
mod notifications;
#[path = "settings_view_parts/panes.rs"]
mod panes;
pub(crate) use panes::rules_count_label;
#[allow(
    unused_imports,
    reason = "The module facade serves production and feature-specific tests."
)]
pub(crate) use panes::{
    favorite_remove_button_id, rules_invoker_id, smart_add_button_id, smart_remove_button_id,
};
#[path = "settings_view_parts/accounts.rs"]
mod accounts;
use accounts::accounts_pane;
use panes::{advanced_pane, general_pane, shortcuts_pane};

pub(crate) fn with_settings<'a>(s: &'a State, base: Element<'a, Msg>) -> Element<'a, Msg> {
    // The empty host stays mounted at rest so the next open animates from hidden.
    // No page body is built while settings are neither open nor retained.
    if !s.settings.is_retained() {
        let placement = match enz_settings_presentation_for(s.window_size) {
            EnzSettingsPresentation::Sheet => SurfacePlacement::BottomEdge,
            EnzSettingsPresentation::Dialog => SurfacePlacement::Centred,
            EnzSettingsPresentation::Adaptive => unreachable!(),
        };
        return enz_modal_host(base, None, |event| {
            Msg::SettingsFocusEvent(SettingsDialogEvent::from_host(event))
        })
        .label(crate::l10n::tr(DIALOG_TITLE))
        .scope(presentation_scope_id(SETTINGS_SCOPE_KEY))
        .placement(placement)
        .presentation_open(false)
        .into();
    }
    let presentation_key = s
        .settings
        .presentation_key()
        .expect("a retained settings surface has a generation");

    let page = s.settings.page(&s.store);
    let subpage_drilled = matches!(
        page,
        SettingsPage::AccountDetail { .. } | SettingsPage::AddAccount { .. }
    );
    let subpage_back: Option<Msg> = settings_back_message(&page);

    // `.selected(..)` must match a `.page(..)` id; both are `SettingsTab::id()`.
    let selected_id = s.settings.tab.id();

    // Lazy page factories build only mounted Settings content; descriptors remain eager for stable navigation.
    let builder = enz_settings_dialog::<Msg>(true, s.window_size)
        .title(crate::l10n::tr(DIALOG_TITLE))
        .presentation(EnzSettingsPresentation::Adaptive)
        .size(DialogSize::Small);

    let builder = builder
        .page_lazy(
            PresetSettingsPage::new(
                SettingsTab::General.id(),
                crate::l10n::tr(SettingsTab::General.title()),
            )
            .icon(SettingsTab::General.icon()),
            move || general_pane(s),
        )
        .page_lazy(
            PresetSettingsPage::new(
                SettingsTab::Advanced.id(),
                crate::l10n::tr(SettingsTab::Advanced.title()),
            )
            .icon(SettingsTab::Advanced.icon()),
            move || advanced_pane(s),
        )
        .page_lazy(
            PresetSettingsPage::new(
                SettingsTab::Accounts.id(),
                crate::l10n::tr(SettingsTab::Accounts.title()),
            )
            .icon(SettingsTab::Accounts.icon()),
            move || accounts_pane(s),
        )
        .page_lazy(shortcuts_settings_page(), move || shortcuts_pane(s))
        .page_lazy(
            PresetSettingsPage::new(
                "notifications",
                crate::l10n::current().text("mail-notifications-title"),
            )
            .icon("bell"),
            move || notifications::pane(s),
        );

    let mut builder = builder
        .selected(selected_id)
        .on_select(|id: String| Msg::SelectSettingsTab(SettingsTab::from_id(&id).index()))
        // The Reset-all footer belongs to the dialog so it does not scroll away with
        // the rows it resets, and shows only on the Shortcuts page.
        .page_id_prefix(SETTINGS_ID_PREFIX);
    // Only while the shortcuts page is on screen: not under the narrow
    // window's page list.
    if s.settings.tab == SettingsTab::Shortcuts
        && (builder.resolved_nav() != SettingsNav::Stacked || s.settings.stacked_tab_open)
    {
        builder = builder.action(editable_shortcuts_reset_all_action(|| {
            Msg::Shortcuts(EditableShortcutsEvent::ResetAll)
        }));
    }

    // Use the preset’s resolved navigation mode so pane mounting and its focus lifecycle agree.
    let stacked = builder.resolved_nav() == SettingsNav::Stacked;
    let stacked_drilled = stacked && s.settings.stacked_tab_open;
    let form_start = matches!(
        page,
        SettingsPage::AddAccount {
            step: AddAccountStep::Email
        }
    ) && !stacked;
    let subpage_drilled = subpage_drilled && !form_start;
    let subpage_back = subpage_back.filter(|_| !form_start);
    let drilled = subpage_drilled || stacked_drilled;

    // A subpage Back wins as the deeper level, going one level up, not to the nav
    // list. Both Stacked steps declare the route; only the drilled-in one paints it.
    let back_msg: Option<Msg> =
        subpage_back.or_else(|| stacked.then_some(Msg::SettingsStackedBack));

    builder = builder.drilled_in(drilled);
    if let Some(msg) = back_msg {
        builder = builder.on_back(msg);
    }

    let presentation = builder.resolved_presentation();
    let surface = match presentation {
        EnzSettingsPresentation::Dialog => {
            builder.into_surface(Some(Box::new(|| Msg::CloseSettings)))
        }
        EnzSettingsPresentation::Sheet => {
            builder.into_sheet_surface(Some(Box::new(|| Msg::CloseSettings)))
        }
        EnzSettingsPresentation::Adaptive => {
            unreachable!("resolved_presentation never returns Adaptive")
        }
    };

    let placement = match presentation {
        EnzSettingsPresentation::Sheet => SurfacePlacement::BottomEdge,
        EnzSettingsPresentation::Dialog => SurfacePlacement::Centred,
        EnzSettingsPresentation::Adaptive => unreachable!(),
    };
    let mut host = enz_modal_host(base, Some(surface), |event| {
        Msg::SettingsFocusEvent(SettingsDialogEvent::from_host(event))
    })
    .label(crate::l10n::tr(DIALOG_TITLE))
    .placement(placement)
    .presentation_open(s.settings.open)
    .on_exit_settled_with(move || Msg::SettingsExitSettled(presentation_key));
    if s.settings.open {
        host = host.scoped_to(
            s.settings_focus
                .as_ref()
                .expect("open settings has no focus lifecycle"),
        );
    }
    host.into()
}

pub(crate) fn dialog_geometry(mode: LayoutMode, window: iced::Size) -> (f32, f32, f32) {
    let (gutter, cap_w, cap_h): (f32, f32, f32) = match mode {
        LayoutMode::Narrow => (8.0, 360.0, 780.0),
        LayoutMode::Medium => (24.0, 560.0, 620.0),
        LayoutMode::Wide => (32.0, 620.0, 560.0),
    };
    let avail_w = (window.width - 2.0 * gutter).max(240.0);
    let avail_h = (window.height - 2.0 * gutter).max(280.0);
    (gutter, cap_w.min(avail_w), cap_h.min(avail_h))
}

pub(crate) const ADD_ACCOUNT_CONNECTION_TITLE: &str = "Connect a mail account";
pub(crate) const ADD_ACCOUNT_CONNECTION_BODY: &str = "Connect using IMAP SSL/TLS. Passwords stay in the system keyring and the account is added after sign-in succeeds. Outgoing mail uses the configured SMTP TLS mode and appears in Sent only after server acceptance.";
pub(crate) const PASSWORD_STORAGE_NOTICE: &str =
    "Stored in the system keyring; never written into Mail's local state.";
pub(crate) const PASSWORD_FIXTURE_NOTICE: &str =
    "Not retained after this test or demo dialog closes.";
pub(crate) const ACCOUNT_LOCAL_ONLY_TITLE: &str = "Fixture account";
pub(crate) const ACCOUNT_LOCAL_ONLY_BODY: &str =
    "This test or demo account has no server configuration and cannot synchronize or send.";
pub(crate) const ACCOUNT_CONNECTED_TITLE: &str = "Mail transport settings";
pub(crate) const ACCOUNT_CONNECTED_BODY: &str = "IMAP synchronization and SMTP submission use the system keyring credential. Sent is published only after the SMTP server accepts the message.";
pub(crate) const REMOVE_ACCOUNT_LABEL: &str = "Remove Account";
pub(crate) const REMOVE_ACCOUNT_CONFIRM_TITLE: &str = "Remove account?";

fn add_account_transport_banner<'a>(s: &State, id_prefix: &'a str) -> Element<'a, Msg> {
    let (title, body, tone) = if s.is_fixture_mode() {
        (
            ACCOUNT_LOCAL_ONLY_TITLE,
            ACCOUNT_LOCAL_ONLY_BODY,
            BannerTone::Warning,
        )
    } else {
        (
            ADD_ACCOUNT_CONNECTION_TITLE,
            ADD_ACCOUNT_CONNECTION_BODY,
            BannerTone::Info,
        )
    };
    enz_banner(crate::l10n::tr(title))
        .body(crate::l10n::tr(body))
        .tone(tone)
        .dismissible(false)
        .id_prefix(id_prefix)
        .into_element()
}

// Leaving Add Account clears the password; backing from Configure preserves typed server fields.
pub(crate) fn settings_back_message(page: &SettingsPage) -> Option<Msg> {
    match page {
        SettingsPage::Root => None,
        SettingsPage::AddAccount {
            step: AddAccountStep::Configure,
        } => Some(Msg::AddAccountBack),
        SettingsPage::AddAccount {
            step: AddAccountStep::Email,
        } => Some(Msg::CancelAddAccount),
        SettingsPage::AccountDetail { account_id } => {
            Some(Msg::ToggleAccountExpanded(account_id.clone()))
        }
    }
}
