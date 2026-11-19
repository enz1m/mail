//! Desktop mail client with isolated accounts, drafts and durable Outbox settlement.

#![deny(unconditional_recursion)]

#[path = "app_parts/account_edit.rs"]
mod account_edit;
#[path = "app_parts/actions.rs"]
mod actions;
#[path = "app_parts/boot.rs"]
mod boot;
#[path = "app_parts/close_prefs.rs"]
mod close_prefs;
#[path = "app_parts/compose_persist.rs"]
mod compose_persist;
#[path = "app_parts/message_types.rs"]
mod message_types;
pub(crate) use message_types::{
    AccountEdit, AccountEditPhase, AccountRemoval, AccountRemovalPhase, AddressMenuTarget,
    AttachmentPreview, ClosePhase, CloseSaveKey, CloseSettlement, CredentialRecovery,
    FrameCompletion, FrameLoad, FrameLoadHandoff, FrameStatus, LocalTimestampFailure,
    LocalTimestampRetry, MailIntent, MailNotice, PendingSyncAdoption, PendingUndoToast,
    PersistedFrames, ServerSearch, StartupLoads, UndoMove, UnsubscribeProgress,
};
pub(crate) use message_types::{
    AttachmentRetry, CredentialCompletion, CredentialStage, CredentialStoreHandle, FrameKind,
    Message, OAuthHandoff, PersistenceDebt, PreviewPayload, RecipientField, SearchScope,
};
#[path = "app_parts/classify.rs"]
mod classify;
#[path = "app_parts/inbox_push.rs"]
mod inbox_push;
#[path = "app_parts/later.rs"]
mod later;
#[path = "app_parts/mail_ops.rs"]
mod mail_ops;
#[path = "app_parts/reader_extras.rs"]
mod reader_extras;
#[path = "app_parts/reduce_batch1.rs"]
mod reduce_batch1;
#[path = "app_parts/reduce_batch2.rs"]
mod reduce_batch2;
#[path = "app_parts/reduce_batch3.rs"]
mod reduce_batch3;
#[path = "app_parts/reduce_batch4.rs"]
mod reduce_batch4;
#[path = "app_parts/reduce_batch5.rs"]
mod reduce_batch5;
#[path = "app_parts/reduce_batch6.rs"]
mod reduce_batch6;
#[path = "app_parts/startup.rs"]
mod startup;
#[path = "app_parts/sync_adopt.rs"]
mod sync_adopt;
#[path = "app_parts/tasks.rs"]
mod tasks;

mod attachment_store;
mod backend;
#[cfg(feature = "center")]
mod background;

mod app_instance;
mod bulk_view;
#[cfg(feature = "center")]
mod center;
mod compose_view;
mod context_menu;
mod credential_mutation;
mod desktop;
mod desktop_search;

#[cfg(test)]
mod external_uri_tests;
mod html_view;
mod html_viewport;
mod l10n;
mod link_launcher;

mod local_clock;
mod local_state;
mod mail_actions;
mod mail_html;
mod modal_focus;
mod model;

mod native_drop;
mod new_mail_notify;
mod notification_settings;

mod picker;
mod preferences;

mod rules_view;
mod schedule;
mod settings_view;
mod shortcuts;
mod theme;

mod view;

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iced::widget::text_editor;
use iced::{Size, Subscription, Task, Theme, advanced::widget::operate};

pub(crate) use context_menu::OpenContextMenu;
use context_menu::context_menu_invoker;
use enzui_iced_core::theme_file::integration::{ThemeEvent, ThemeIntegration};
use enzui_iced_core::{
    focus::{Traversal, traversal_operation as traverse},
    modal::DismissCause,
};
use enzui_iced_theme::EnzTheme;
use enzui_iced_widgets::app_shell::{ShellLayout, enz_window_settings};
use enzui_iced_widgets::toast::ToastTone;
use enzui_iced_widgets::toast_host::{
    ToastEvent, ToastExpirySchedule, ToastHost, ToastId, ToastSpec, WaitingBudget,
};
#[cfg(not(test))]
use enzui_services::credential_store::UnavailableCredentialStore;
use enzui_services::credential_store::{
    CancelHandle as CredentialCancelHandle, CredentialKey, CredentialOutcome, CredentialStore,
    Secret,
};
use model::rules::{demo_rules, next_rule_id_seed};
use model::{
    compose::{ComposeController, ComposeMode, ComposeState, PendingSend},
    fixture,
    layout::{LayoutMode, NarrowNav, layout_mode},
    privacy::RemoteContentPolicy,
    settings::{
        DraftField, OAuthSignInStatus, SettingsState, SettingsTab, admit_account_id,
        commit_account_config, commit_add_account,
    },
    types::{MailboxKind, MailboxStore, SidebarSelection},
};

const UNDO_SEND_WINDOW: Duration = Duration::from_secs(10);
const UNDO_SEND_COALESCE_KEY: &str = "mail-pending-send";
const SEND_NOW_ACTION: &str = "send-now";

const UNDO_MOVE_WINDOW: Duration = Duration::from_secs(8);
const UNDO_MOVE_LABEL: &str = "Undo";

fn undo_move_toast_spec(mailbox: MailboxKind, conversations: usize) -> ToastSpec {
    let mut arguments = enzui_iced_core::localization::Arguments::new();
    arguments.set("count", conversations);
    arguments.set("mailbox", crate::l10n::tr(mailbox.label()));
    let key = if mailbox == MailboxKind::Archive {
        "mail-archived-count"
    } else {
        "mail-moved-count"
    };
    let headline = crate::l10n::current().text_with(key, &arguments);
    ToastSpec::new(headline)
        .tone(ToastTone::Info)
        .action_label(crate::l10n::tr(UNDO_MOVE_LABEL))
        .duration(UNDO_MOVE_WINDOW)
        .coalesce_key("mail-undo-move")
}

fn undo_send_toast_spec() -> ToastSpec {
    ToastSpec::new(crate::l10n::tr(compose_view::PENDING_SEND_HEADLINE))
        .description(crate::l10n::tr(compose_view::PENDING_SEND_DESCRIPTION))
        .tone(ToastTone::Info)
        .action_label(crate::l10n::tr(compose_view::DISCARD_LOCAL_SAVE_LABEL))
        .duration(UNDO_SEND_WINDOW)
        .coalesce_key(UNDO_SEND_COALESCE_KEY)
}

const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(30);
const CLOSE_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_RETRY_INTERVAL: Duration = Duration::from_millis(200);
const BLOCKING_RETRY_INTERVAL: Duration = Duration::from_millis(200);
const MAILBOX_SYNC_INTERVAL: Duration = Duration::from_secs(5 * 60);
const MAILBOX_SYNC_RETRY_DELAYS: [Duration; 6] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(30),
    Duration::from_secs(60),
    Duration::from_secs(5 * 60),
];

const RULE_ID_EXHAUSTED_NOTICE: &str =
    "No more rules can be created. Edit an existing rule instead.";
const RULE_ID_CONFLICT_NOTICE: &str = "This rule was not saved because its identity belongs to another rule. Cancel this draft and create a new rule.";

pub(crate) struct State {
    localizer: enzui_iced_core::localization::Localizer,
    desktop: desktop::Desktop,
    native_drop: native_drop::NativeDrop,
    privacy_choices:
        Vec<enzui_iced_widgets::radio_group::RadioChoice<'static, RemoteContentPolicy>>,
    pub(crate) themes: ThemeIntegration,
    pub(crate) window_size: Size,
    fixture_mode: bool,
    pub(crate) store: MailboxStore,
    pub(crate) pending_server_mutations: Vec<local_state::PendingServerMutation>,
    pub(crate) next_server_mutation_ordinal: u64,
    pub(crate) local_state_gate: local_state::LocalStateGate,
    pub(crate) local_state_published: local_state::LocalStateToken,
    sync_adoption_revision: u64,
    pub(crate) local_state_error: Option<enzui_iced_core::persist::PersistError>,
    pub(crate) local_state_load_error: Option<String>,
    // Whether an unreadable local-state file was retained under a sibling
    // recovery name, which is the only condition under which this session may
    // publish replacement state over the refused destination.
    pub(crate) local_state_retained_for_recovery: bool,
    // Startup custody and off-thread read state for the three persisted frames.
    //
    // While any frame is pending, nothing derived from it is installed and no
    // frame publishes, so a slow or unreadable read cannot be mistaken for a
    // user who owns no mail.
    startup: StartupLoads,
    // Bounded owner for the synchronous filesystem calls this app makes.
    //
    // Wrapping a blocking call in an async block leaves it on the executor, so
    // every persistence publication and attachment maintenance call is
    // submitted here and only its settlement is awaited.
    blocking_tasks: enzui_iced_core::blocking_task::BlockingTaskOwner,
    blocking_generation: u64,
    blocking_generation_exhausted: bool,
    persistence_retries: VecDeque<PersistenceDebt>,
    attachment_retries: VecDeque<AttachmentRetry>,
    mail_busy: bool,
    local_clock: Arc<dyn local_clock::LocalClock>,
    local_timestamp_failure: Option<LocalTimestampFailure>,
    credential_store: CredentialStoreHandle,
    credential_store_ready: bool,
    credential_inflight: Option<(u64, CredentialCancelHandle, CredentialStage)>,
    credential_mutations: credential_mutation::CredentialMutationOwner,
    credential_secret: Option<Secret>,
    // The browser sign-in in flight (generation, cancel) and the
    // grant it produced, held until the account is committed.
    oauth_sign_in: Option<(
        u64,
        enzui_services::oauth::loopback::LoopbackCancel,
        model::settings::OAuthBinding,
    )>,
    oauth_generation: u64,
    oauth_grant: Option<(model::settings::OAuthBinding, Secret)>,
    credential_recovery: Option<CredentialRecovery>,
    account_removal: Option<AccountRemoval>,
    account_edit: Option<AccountEdit>,
    pub(crate) confirm_remove_account: enzui_iced_core::confirm::ConfirmState,
    mail_backend: Arc<dyn backend::curl_backend::MailBackend>,
    mail_generation: u64,
    mail_intent: Option<MailIntent>,
    mail_queue: VecDeque<MailIntent>,
    mail_cancel: Option<(u64, backend::curl_backend::OperationCancel)>,
    mail_discard_generation: Option<u64>,
    mail_retry: Option<MailIntent>,
    mailbox_sync_retry: Option<MailIntent>,
    mailbox_sync_generation: u64,
    mailbox_sync_generation_exhausted: bool,
    mailbox_sync_retry_at: Option<Instant>,
    mailbox_sync_retry_attempt: usize,
    pending_sync_adoption: Option<PendingSyncAdoption>,
    content_sync_inflight: Option<(u64, u64)>,
    // The encoded submission for the named durable Outbox entry.
    //
    // Encoding reads attachment payloads, so it is prepared on the bounded
    // blocking owner before any credential is acquired and consumed when the
    // transfer starts.
    outgoing_mime: Option<(String, Vec<u8>)>,
    pub(crate) mail_notice: Option<MailNotice>,
    close_guard: enzui_iced_core::close_guard::CloseGuard<CloseSaveKey>,
    close_generation: u64,
    close_settlement: Option<CloseSettlement>,
    close_committed: bool,
    pub(crate) selection: SidebarSelection,
    #[cfg(feature = "center")]
    pub(crate) center: Option<enzui_iced_agent::center::CenterHost<crate::center::CenterCall>>,
    // Single-instance ownership and the windowless runtime's state.
    #[cfg(feature = "center")]
    pub(crate) agent: enzui_iced_agent_app::AgentProcess<Message>,
    #[cfg(feature = "center")]
    pub(crate) reviews: enzui_iced_agent::center::review::ReviewBook<String>,
    #[cfg(feature = "center")]
    pub(crate) agent_review: Option<crate::center::AgentReview>,
    #[cfg(feature = "center")]
    pub(crate) review_outcomes: crate::center::ReviewOutcomes,
    #[cfg(feature = "center")]
    pub(crate) review_serial: u64,
    #[cfg(feature = "center")]
    pub(crate) waiting_center_calls: Vec<(
        enzui_iced_agent::center::CenterCall,
        crate::center::CenterCall,
    )>,
    pub(crate) selected_conversation: Option<String>,
    pub(crate) deferred_read: Option<(String, Vec<String>)>,
    // The reading-pane multi-selection over conversation rows.
    //
    // `ListSelection` owns the click contract (replace, Ctrl toggle, Shift range,
    // anchor kept as an id so a mailbox change cannot re-point it). It is distinct
    // from `selected_conversation`, the single thread the reading pane renders.
    pub(crate) row_selection: enzui_iced_core::list_selection::ListSelection,
    pub(crate) narrow_nav: NarrowNav,
    pub(crate) nav: enzui_iced_widgets::nav_split::NavSplitState,
    pub(crate) search_query: String,
    pub(crate) search_scope: SearchScope,
    search_scope_choices: Vec<enzui_iced_widgets::radio_group::RadioChoice<'static, SearchScope>>,
    pub(crate) search_open: bool,
    pub(crate) titlebar_overflow_open: bool,
    pub(crate) search_highlight: Option<usize>,
    pub(crate) collapsed_sections: std::collections::HashSet<String>,
    pub(crate) privacy: enzui_iced_core::persist::Tracked<model::types::PrivacyState>,
    pub(crate) sidebar_layout:
        enzui_iced_core::persist::Tracked<model::sidebar_layout::SidebarLayout>,
    pub(crate) compose_format: enzui_iced_core::persist::Tracked<model::compose::ComposeFormat>,
    // The full sanitised href currently shown in the external-link prompt.
    // `None` means no link is armed; only a confirmed resolution takes this
    // value and hands it to the framework OpenURI portal via `link_opener`.
    pub(crate) pending_open_url: Option<String>,
    pub(crate) open_url_focus: Option<enzui_iced_widgets::modal_host::ModalFocusLifecycle>,
    pub(crate) confirm_open_url: enzui_iced_core::confirm::ConfirmState,
    // Typed OpenURI failure shown in the global reader notice. Success and
    // cancellation clear it; transport/policy failures retain their distinct
    // user-facing sentence until the next attempt or explicit dismissal.
    pub(crate) link_open_notice: Option<String>,
    pub(crate) attachment_notice: Option<String>,
    // Corner notices the user closed, by `view::Notice::key`. A key is
    // forgotten once its notice is gone, so the same notice can come back.
    pub(crate) hidden_notices: Vec<String>,
    pub(crate) attachment_store: attachment_store::AttachmentStore,
    // Durable references are published before their retired payloads are removed.
    pending_payload_cleanup: Vec<model::types::AttachmentPayloadRef>,
    attachment_stage_inflight: usize,
    attachment_picker_generation: u64,
    attachment_cleanup_inflight: usize,
    attachment_cleanup_error: Option<attachment_store::AttachmentFailure>,
    attachment_reconcile_needed: bool,
    attachment_open_generation: u64,
    attachment_open_pending: Option<model::types::Attachment>,
    attachment_open_held: Option<Arc<attachment_store::OpenedAttachment>>,
    attachment_open_busy: bool,
    attachment_export_pending: Option<model::types::AttachmentPayloadRef>,
    pub(crate) attachment_opener: link_launcher::LinkOpener,
    // The framework OpenURI portal boundary; tests inject a mock backend.
    pub(crate) link_opener: link_launcher::LinkOpener,
    pub(crate) compose: ComposeState,
    pub(crate) compose_generation: u64,
    pub(crate) draft_save_pending: bool,
    pub(crate) compose_focus: Option<enzui_iced_widgets::modal_host::ModalFocusLifecycle>,
    pub(crate) body_editor: text_editor::Content,
    draft_pointer: Option<iced::Point>,
    draft_bounds: enzui_iced_widgets::LayoutBounds,
    quick_reply: text_editor::Content,
    quick_reply_for: Option<String>,
    quick_reply_mode: Option<ComposeMode>,
    html_reader: mail_html::MailHtml,
    remote_images_once: HashSet<String>,
    prefer_plain_text: enzui_iced_core::persist::Tracked<bool>,
    dark_html: enzui_iced_core::persist::Tracked<bool>,
    notifications: enzui_iced_core::persist::Tracked<notification_settings::NotificationSettings>,
    message_plain_text: HashMap<String, bool>,
    quoted_text_expanded: HashSet<String>,
    older_mailboxes_exhausted: HashSet<(String, MailboxKind)>,
    conversation_scroll: enzui_iced_widgets::scroll_view::ScrollTranslation,
    attachment_reminder_open: bool,
    attachment_reminder_accepted: bool,
    // Rich-text editing controller — active when `compose.format` is
    // Rich or Html. The controller is the single history owner; the
    // native text_editor undo is never wired in parallel.
    pub(crate) rich_controller: enzui_iced_widgets::rich_edit::RichEditController,
    pub(crate) rich_editor: text_editor::Content,
    pub(crate) pending_send: Option<PendingSend>,
    pub(crate) next_ordinal: u64,
    pub(crate) active_account_id: String,
    // Generation counter so a stale toast event for an earlier send cannot
    // resolve a newer pending send.
    pub(crate) send_generation: u64,
    undo_send_toasts: ToastHost,
    undo_send_expiry: ToastExpirySchedule,
    pending_undo_toast: Option<PendingUndoToast>,
    undo_moves: Vec<UndoMove>,
    // Mutations of an undoable move stay unsent until its undo window closes.
    held_mutations: std::collections::BTreeSet<u64>,
    unsubscribes: std::collections::HashMap<String, UnsubscribeProgress>,
    server_search: ServerSearch,
    // Previews of the open conversation's attachments, by payload id, and the
    // conversation and store generation they were gathered for.
    attachment_previews: std::collections::HashMap<String, AttachmentPreview>,
    previews_for: Option<(String, u64)>,
    // Messages a server search added; kept from sync retirement until the
    // search changes, then dropped.
    server_search_ids: HashSet<String>,
    pub(crate) settings: SettingsState,
    pub(crate) settings_focus: Option<enzui_iced_widgets::modal_host::ModalFocusLifecycle>,
    pub(crate) signature_editor: text_editor::Content,
    pub(crate) next_account_ordinal: u64,
    pub(crate) bulk: model::bulk::BulkState,
    pub(crate) confirm_empty: enzui_iced_core::confirm::ConfirmState,
    pub(crate) empty_confirm_focus: Option<enzui_iced_widgets::modal_host::ModalFocusLifecycle>,
    pub(crate) mailbox_overflow_open: bool,
    // Each stacked message card needs its own overflow target to prevent simultaneous menus.
    pub(crate) reader_overflow_target: Option<String>,
    pub(crate) compose_overflow_open: bool,
    pub(crate) attach_chooser: picker::Picker,
    pub(crate) compose_attach_notice: Option<String>,
    pub(crate) shortcuts_open: bool,
    pub(crate) shortcuts_focus: Option<enzui_iced_widgets::modal_host::ModalFocusLifecycle>,
    pub(crate) shortcuts: shortcuts::MailShortcuts,
    pub(crate) shortcut_armed: Option<String>,
    // `(action id, readable reason)` for the last refused candidate. Rendered
    // verbatim under that row, so `Reserved` and `Invalid` reach the user as
    // the different sentences core made them.
    pub(crate) shortcut_feedback: Option<(String, String)>,
    // Where the override map is written. `InMemoryBackend` under test, so the
    // suite never reads or writes the developer's real configuration.
    pub(crate) shortcut_store: enzui_iced_core::persist::Store<enzui_iced_core::ShortcutOverrides>,
    shortcut_gate: enzui_iced_core::persist::SaveGate<enzui_iced_core::ShortcutOverrides, u64>,
    shortcut_revision: u64,
    shortcut_published: u64,
    shortcut_saves_inflight: HashSet<u64>,
    shortcut_error_kind: Option<enzui_iced_core::persist::PersistErrorKind>,
    // Safe startup feedback when persisted shortcut overrides were refused or
    // malformed. The store remains write-protected until an explicit reset.
    pub(crate) shortcut_load_error: Option<String>,
    pub(crate) compose_format_menu: enzui_iced_core::menu_model::MenuState,
    pub(crate) discovery_net:
        Option<std::sync::Arc<dyn crate::backend::discovery::DiscoveryNet + Send + Sync>>,
    pub(crate) compose_recipient_menu: enzui_iced_core::menu_model::MenuState,
    pub(crate) compose_recipient_model: enzui_iced_core::menu_model::MenuModel,
    // The rows that menu offers. Owned by `State` because the menu button
    // borrows its model for the lifetime of the built view.
    pub(crate) compose_format_model: enzui_iced_core::menu_model::MenuModel,
    pub(crate) schedule_editor: Option<schedule::ScheduleEditor>,
    pub(crate) sender_menu_state: enzui_iced_core::menu_model::MenuState,
    pub(crate) sender_menu_closed_state: enzui_iced_core::menu_model::MenuState,
    pub(crate) sender_menu_target: Option<AddressMenuTarget>,
    pub(crate) sender_menu_model: enzui_iced_core::menu_model::MenuModel,
    pub(crate) message_selection_clear_generation: u64,
    // Move confirmation retains exactly the message IDs armed by its originating surface.
    pub(crate) pending_move_targets: Vec<String>,
    pub(crate) message_move_destination: MailboxKind,
    pub(crate) message_dialogs:
        enzui_iced_widgets::modal_host::DialogStack<modal_focus::MessageDialog>,
    pub(crate) pending_snooze_targets: Vec<String>,
    inbox_watches: std::collections::BTreeMap<String, backend::inbox_watch::InboxWatch>,
    // Accounts whose watch gave up, and why: a server without `IDLE` is not
    // asked again this run, a refused login only after a sync succeeds.
    inbox_watch_stopped: std::collections::HashMap<String, backend::curl_backend::MailFailureKind>,
    // Accounts whose Inbox has synced once this run: only later syncs
    // announce new mail, so a launch does not announce the whole Inbox.
    inbox_announce_ready: HashSet<(String, MailboxKind)>,
    new_mail: new_mail_notify::NewMailNotifier,
    pub(crate) snooze_choice: model::snooze::SnoozePreset,
    // Address armed by `Block sender…`; only the confirmation's affirmative
    // resolution consumes it.
    pub(crate) pending_block_sender: Option<String>,
    pub(crate) confirm_block_sender: enzui_iced_core::confirm::ConfirmState,
    pub(crate) rules: enzui_iced_core::persist::Tracked<model::rules::RuleSet>,
    // The change-detecting front-end for the preferences store; it owns the dirty
    // flag, so an unchanged model is never serialised.
    pub(crate) preferences_gate: preferences::PreferencesGate,
    preferences_published: preferences::PreferencesToken,
    // The last preferences write failure, tagged with the
    // [`preferences::SaveOrigin`] that armed it so the owning pane reports it.
    pub(crate) preferences_error: Option<preferences::SaveFailure>,
    // Safe startup feedback when the shared rules/privacy document was
    // refused or malformed. The fallback is usable but remains write-protected.
    pub(crate) preferences_load_error: Option<String>,
    pub(crate) rules_ui: model::rules::RulesUiState,
    pub(crate) rule_dialogs: enzui_iced_widgets::modal_host::DialogStack<modal_focus::RuleDialog>,
    pub(crate) confirm_delete_rule: enzui_iced_core::confirm::ConfirmState,
    pub(crate) last_rules_result: Option<String>,
    // Actionable refusal shown on both the Rules list and editor surfaces.
    pub(crate) rule_identity_notice: Option<&'static str>,
    pub(crate) rules_count_label: String,
    pub(crate) context_menu: OpenContextMenu,
    pub(crate) context_menu_generation: u64,
}

#[cfg(test)]
fn test_theme_integration(theme: EnzTheme) -> ThemeIntegration {
    ThemeIntegration::unmanaged_compiled_theme(theme).0
}

impl State {
    //

    // Whether any persisted frame is still being read or retained.
    //
    // The shell shows this as loading rather than as an empty mailbox.
    pub(crate) fn startup_is_loading(&self) -> bool {
        self.startup.pending()
    }

    // No account has been added yet, and the saved state has been read, so
    // the window welcomes the user instead of showing empty mailboxes.
    pub(crate) fn needs_first_account(&self) -> bool {
        self.store.accounts.is_empty() && !self.startup_is_loading()
    }
}

fn mail_window_settings() -> iced::window::Settings {
    let mut window =
        with_desktop_identity(enz_window_settings(ShellLayout::TitlebarSidebarContent));
    window.size = Size::new(1120.0, 760.0);
    window.exit_on_close_request = enzui_iced_core::close_guard::EXIT_ON_CLOSE_REQUEST;
    window
}

fn main() -> std::process::ExitCode {
    let localizer = l10n::localizer();
    localizer.scope(run_main)
}

fn run_main() -> std::process::ExitCode {
    enzui_iced_core::crash_report::CrashReporter::for_app("mail-app", env!("CARGO_PKG_VERSION"));
    // Attachment rows draw icon-theme artwork; find the theme off the UI thread.
    enzui_iced_widgets::file_icon::prewarm_icon_theme();
    #[cfg(feature = "center")]
    {
        use enzui_iced_agent::center::background::LaunchMode;
        match background::launch_mode() {
            Ok(LaunchMode::ExportManifest) => return background::export_manifest(),
            Ok(LaunchMode::Background) => {
                // No display, renderer or window until a desktop launch
                // or a prepared message asks for one; then the same state
                // opens it.
                return match background::run_background() {
                    Ok(Some(state)) => run_gui(Some(state)),
                    Ok(None) => std::process::ExitCode::SUCCESS,
                    Err(error) => {
                        eprintln!("mail-app: background mode: {error}");
                        std::process::ExitCode::FAILURE
                    }
                };
            }
            Ok(LaunchMode::Gui) => {}
            Err(error) => {
                eprintln!("mail-app: {error}");
                return std::process::ExitCode::from(2);
            }
        }
    }
    // The early install guarantees `Font::MONOSPACE` resolves to an upright face
    // before the first shaped glyph; cosmic-text can otherwise pick an oblique one.
    let _ = enzui_iced_widgets::monospace_fallback::install_monospace_fallback();

    // No `.window_size(..)`: tiling compositors may ignore it, and the layout mode
    // follows the size reported by `Resized`.
    run_gui(None)
}

fn run_gui(presented: Option<State>) -> std::process::ExitCode {
    let launch = enzui_iced_core::activation::launch_request_from_env();
    #[cfg(not(feature = "center"))]
    let ordinary_endpoint = true;
    #[cfg(feature = "center")]
    let mut ordinary_endpoint = presented.is_none();
    #[cfg(feature = "center")]
    let mut service = None;
    #[cfg(feature = "center")]
    if presented.is_none() {
        match background::claim_for_gui(launch.intent.clone()) {
            Ok(background::DesktopClaim::Owner(owned)) => {
                ordinary_endpoint = false;
                service = Some(owned);
            }
            Ok(background::DesktopClaim::Forwarded) => return std::process::ExitCode::SUCCESS,
            Ok(background::DesktopClaim::Independent) => {}
            Err(error) => {
                // Never start a second instance beside one that refused.
                eprintln!("mail-app: could not reach the running instance: {error}");
                return std::process::ExitCode::FAILURE;
            }
        }
    }
    let (desktop_instance, initial_intent) = if ordinary_endpoint {
        use app_instance::{AppInstance, InstanceStart};
        match AppInstance::start(DESKTOP_APP_ID) {
            InstanceStart::Owner(instance, intent) if instance.is_shared() => {
                (Some(instance), intent)
            }
            InstanceStart::Forwarded => return std::process::ExitCode::SUCCESS,
            InstanceStart::Owner(_, _) => {
                // Mail must not write beside an owner that refused a forwarded launch.
                eprintln!("mail-app: could not establish shared desktop ownership");
                return std::process::ExitCode::FAILURE;
            }
        }
    } else {
        (None, launch.intent)
    };
    let desktop_instance = std::cell::RefCell::new(desktop_instance);
    let initial_intent = std::cell::RefCell::new(Some(initial_intent));
    // A windowless agent worker hands over its state exactly once; every
    // other boot is a fresh desktop launch.
    #[cfg(feature = "center")]
    let pending = std::cell::RefCell::new(presented);
    #[cfg(not(feature = "center"))]
    let _ = presented;
    #[cfg(feature = "center")]
    let service = std::cell::RefCell::new(service);
    let boot = move || {
        #[cfg(feature = "center")]
        if let Some(mut state) = pending.borrow_mut().take() {
            let task = state.present_task();
            return (state, task);
        }
        let (mut state, task) = State::boot();
        state.desktop.instance = desktop_instance.borrow_mut().take();
        if let Some(intent) = initial_intent.borrow_mut().take() {
            state.desktop.enqueue(intent);
        }
        #[cfg(feature = "center")]
        {
            state.agent = enzui_iced_agent_app::AgentProcess::desktop(service.borrow_mut().take());
        }
        state.serve_desktop_search();
        (state, task)
    };
    let result = enzui_iced_widgets::app_setup::run_with_services(
        iced::application(boot, State::update, view::root)
            .title("Mail")
            .theme(State::theme)
            .subscription(State::subscription)
            // The shell draws its own titlebar; deriving decorations from the layout keeps
            // the compositor from drawing a second one.
            .window(mail_window_settings()),
        enzui_iced_widgets::app_setup::AppServices::default()
            .localizer(l10n::current())
            .traversal(enzui_iced_core::focus_traversal::TraversalBinding::Disabled),
    );
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mail-app: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod reducer_tests;

#[cfg(test)]
#[allow(
    clippy::items_after_test_module,
    reason = "Test module declarations stay beside the modules they exercise."
)]
mod composer_document_tests;

// Window and desktop-entry identities must agree for grouping, icons and activation.
const DESKTOP_APP_ID: &str = "io.github.enzui.Mail";

fn with_desktop_identity(settings: iced::window::Settings) -> iced::window::Settings {
    enzui_iced_widgets::app_shell::DesktopApplicationIdentity::new(DESKTOP_APP_ID)
        .expect("BUG: DESKTOP_APP_ID must be a portable desktop-entry basename")
        .apply_to_window_settings(settings)
}
