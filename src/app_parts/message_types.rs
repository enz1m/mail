use super::*;

#[path = "state_types.rs"]
mod state_types;
pub(crate) use state_types::{
    AccountEdit, AccountEditPhase, AccountRemoval, AccountRemovalPhase, AddressMenuTarget,
    AttachmentPreview, ClosePhase, CloseSaveKey, CloseSettlement, CredentialRecovery,
    FrameCompletion, FrameLoad, FrameLoadHandoff, FrameStatus, LocalTimestampFailure,
    LocalTimestampRetry, MailIntent, MailNotice, PendingSyncAdoption, PendingUndoToast,
    PersistedFrames, ServerSearch, StartupLoads, UndoMove, UnsubscribeProgress,
};
pub(crate) use state_types::{
    AttachmentRetry, CredentialCompletion, CredentialStage, CredentialStoreHandle, FrameKind,
    PersistenceDebt, PreviewPayload,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecipientField {
    To,
    Cc,
    Bcc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum SearchScope {
    #[default]
    AllMailboxes,
    // Restrict search to the currently-selected sidebar row (mailbox /
    // favourite / smart mailbox / folder). Matches only messages that
    // `selection_matches` accepts under the current [`SidebarSelection`].
    CurrentMailbox,
}

impl SearchScope {
    pub(crate) const ALL: [SearchScope; 2] =
        [SearchScope::AllMailboxes, SearchScope::CurrentMailbox];

    #[must_use]
    pub(crate) fn index(self) -> usize {
        match self {
            SearchScope::AllMailboxes => 0,
            SearchScope::CurrentMailbox => 1,
        }
    }
}

type OAuthSettlement = enzui_iced_core::blocking_task::BlockingTaskSettlement<
    Result<Secret, backend::curl_backend::oauth::SignInError>,
>;

#[derive(Clone)]
pub(crate) struct OAuthHandoff(std::sync::Arc<std::sync::Mutex<Option<OAuthSettlement>>>);

impl OAuthHandoff {
    pub(crate) fn new(settlement: OAuthSettlement) -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(Some(settlement))))
    }

    // The sign-in's result; a worker that stopped is a failure.
    pub(crate) fn take(&self) -> Option<Result<Secret, backend::curl_backend::oauth::SignInError>> {
        let settlement = self.0.lock().ok()?.take()?;
        Some(settlement.into_result().unwrap_or_else(|_| {
            Err(backend::curl_backend::oauth::SignInError::Failed(
                "The sign-in worker stopped before the browser returned.".into(),
            ))
        }))
    }
}

impl std::fmt::Debug for OAuthHandoff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OAuthHandoff([redacted])")
    }
}

// Debug output contains only the event kind because payloads can contain credentials or mail content.
#[derive(Clone)]
pub(crate) enum Message {
    Theme(Box<ThemeEvent>),
    WindowResized(Size),
    SelectSidebar(SidebarSelection),
    #[cfg(feature = "center")]
    Center(enzui_iced_agent::center::ConnectorEvent),
    #[cfg(feature = "center")]
    ApplicationWake,
    #[cfg(feature = "center")]
    BackgroundTaskRefused(String),
    // A center call's deadline may have passed without an event.
    #[cfg(feature = "center")]
    CenterDeadline,
    SelectConversation {
        id: String,
        toggle: bool,
        range: bool,
    },
    NarrowBack,
    SidebarState(enzui_iced_widgets::nav_split::NavSplitState),
    AlwaysAllowSender(String),
    AllowImagesOnce(String),
    // A link in sanitised HTML mail was activated. This only arms a prompt;
    // it never launches directly because visible anchor text may conceal a
    // different destination.
    RequestOpenUrl(String),
    OpenUrlDialogEvent(enzui_iced_widgets::modal_host::ModalHostEvent),
    QuickReplyEdit(iced::widget::text_editor::Action),
    LeaveCompose,
    DraftPointerMoved(iced::Point),
    DraftClickAway,
    DraftWindowBlur,
    SendWithoutAttachment,
    DismissAttachmentReminder,
    HtmlViewports(Vec<(String, bool, u32)>),
    HtmlTick,
    HtmlNativeReady(enzui_iced_core::native_loop::NativeReady),
    HtmlAllocated(
        crate::mail_html::PresentationRequest,
        Result<iced::widget::image::Allocation, iced::widget::image::Error>,
    ),
    HtmlInput(enzui_webkit::web::ViewId, enzui_webkit::InputEvent),
    StartQuickReply(ComposeMode),
    CancelQuickReply,
    QuickReplySend,
    QuickReplyExpand,
    // An attachment preview's bytes arrived, keyed by payload id; `None` when
    // it cannot be previewed.
    AttachmentPreviewLoaded(String, Option<PreviewPayload>),
    SearchServer,
    Unsubscribe(String),
    UnsubscribeFinished(String, Result<(), String>),
    ConfirmOpenUrl(enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent),
    // Terminal typed outcome from the framework OpenURI portal, routed here
    // as the same finish path a request that never started takes.
    OpenUrlFinished(link_launcher::LinkOpenOutcome),
    // Dismiss the global link-open failure notice.
    DismissOpenUrlNotice,
    // Event-driven wake for the live OpenURI request, from
    // `LinkOpener::completion_subscription`.
    //
    // Carries the request generation its stream was built for; a wake from a
    // superseded request is rejected.
    OpenUrlPortalTick(link_launcher::LinkWake),
    // Window chrome intent. Close enters durable settlement; other commands
    // retain the framework's direct per-window mapping.
    Window(enzui_iced_widgets::title_bar_gesture::WindowCommand),
    WindowCloseResolved(Option<iced::window::Id>),
    CloseSettlementTick(u64, Instant),
    RetryCloseSettlement,
    DiscardCloseSettlement,
    CancelCloseSettlement,
    RetryLocalTimestamp,
    #[cfg_attr(
        test,
        allow(
            dead_code,
            reason = "test boot uses an in-memory credential store synchronously"
        )
    )]
    CredentialStoreReady(CredentialStoreHandle),
    CredentialFinished(u64, CredentialStage, Arc<CredentialCompletion>),
    MailOperationFinished(
        u64,
        Result<backend::curl_backend::MailOperationOutput, backend::curl_backend::MailFailure>,
    ),
    RetryMailOperation,
    CancelMailOperation,
    DismissMailNotice,
    // Closes a corner notice that only its cause can clear, until it changes.
    HideNotice(String),
    RefreshMailbox,
    LoadOlderMailbox,
    PeriodicMailboxSync(u64, Instant),
    MailboxSyncRetryTick(u64, Instant),

    OpenCompose(ComposeMode),
    // Dismiss compose without saving.
    CloseCompose,
    // Reveal the Cc field (hidden until added or seeded).
    ShowCc,
    ShowBcc,
    ComposeRecipientMenuEvent(enzui_iced_widgets::menu_button::MenuButtonEvent),
    ComposeToEdited(String),
    ComposeCcEdited(String),
    ComposeBccEdited(String),
    ComposeSubjectEdited(String),
    SetComposeFrom(String),
    ComposeRecipientSelected(RecipientField, String),
    ComposeRecipientCustom(RecipientField, String),
    ComposeBodyAction(text_editor::Action),
    SetComposeFormat(model::compose::ComposeFormat),
    ComposeRichEditAction(enzui_iced_widgets::rich_edit::RichEditAction),
    // Toggle the compose action-row overflow menu, published only by its trigger;
    // dismissals use `CloseComposeOverflow`.
    ToggleComposeOverflow,
    // Idempotent close for the compose action-row overflow menu, from
    // `enz_action_bar`'s `on_dismiss`; set-false only, so a late dismissal cannot
    // reopen it.
    CloseComposeOverflow,
    AttachFile,
    DroppedAttachment(std::path::PathBuf),
    NativeDrop(native_drop::Event),
    #[cfg_attr(
        test,
        expect(
            dead_code,
            reason = "Constructed by the native file chooser parent callback"
        )
    )]
    AttachParentReady(picker::ParentReady),
    DesktopWake,
    DesktopActivated,
    AttachPortalTick,
    AttachFallback(enzui_iced_widgets::file_chooser::FallbackEvent),
    AttachPickerOutcome(
        picker::PickerTarget,
        enzui_iced_widgets::file_chooser::PortalOutcome<
            enzui_iced_widgets::file_chooser::FileResult,
        >,
    ),
    AttachmentsStaged(
        u64,
        Result<Vec<model::types::Attachment>, attachment_store::AttachmentFailure>,
    ),
    InboundContentStaged(
        u64,
        u64,
        Box<backend::AccountConfig>,
        MailboxKind,
        Result<backend::curl_backend::SyncBatch, attachment_store::AttachmentFailure>,
    ),
    AttachmentSaved(
        model::types::AttachmentPayloadRef,
        Result<(), attachment_store::AttachmentFailure>,
    ),
    AttachmentBusy(AttachmentRetry),
    AttachmentCleanupFinished(
        Vec<model::types::AttachmentPayloadRef>,
        Result<(), attachment_store::AttachmentFailure>,
    ),
    AttachmentReconcileFinished(Result<(), attachment_store::AttachmentFailure>),
    // Startup attachment custody: the unreferenced-cleanup outcome.
    AttachmentStartupSurveyFinished(Result<(), attachment_store::AttachmentFailure>),
    AttachmentPathResolved(
        u64,
        model::types::AttachmentPayloadRef,
        Result<Arc<attachment_store::OpenedAttachment>, attachment_store::AttachmentFailure>,
    ),
    // One off-thread persistence read settled. The carrier is taken once, so a
    // duplicate delivery cannot install a second time.
    FrameLoadSettled(Arc<FrameLoadHandoff>),
    FrameLoadFailed(FrameKind, enzui_iced_core::blocking_task::BlockingTaskError),
    // Custody of one unreadable frame settled; `true` means its bytes are
    // retained and replacement is authorized.
    FrameCustodySettled(
        FrameKind,
        Result<bool, enzui_iced_core::blocking_task::BlockingTaskError>,
    ),
    // The encoded submission for one durable Outbox entry, or why it could not
    // be built.
    OutgoingMimePrepared(String, Result<Vec<u8>, attachment_store::AttachmentFailure>),
    RemoveComposeAttachment(usize),
    SendCompose,
    Toast(ToastEvent),
    SaveDraft,
    // Periodic autosave tick while compose is open; it updates one draft rather
    // than creating one per tick and leaves compose open.
    AutosaveDraftTick,

    // Tab / Shift+Tab in the main shell moves focus through its focusable
    // widgets; iced advances focus only when the app drives `focus_next`.
    FocusNext,
    FocusPrevious,

    SearchEdited(String),
    SearchCleared,
    // Toggle the [`SearchScope`] between `AllMailboxes` and `CurrentMailbox`.
    // Pick a [`SearchScope`] by Filter segment index; out-of-range indices are
    // ignored rather than clamped.
    SelectSearchScope(usize),
    SearchActivated,
    SearchDismissed,
    ToggleTitlebarOverflow,
    CloseTitlebarOverflow,
    // Move the highlighted result through `enz_suggestion_list`'s navigation
    // model. Down/Up and Tab/Shift+Tab publish this only while the desktop
    // popup is open, so focus cannot walk into controls behind the popup.
    SearchNavigate(enzui_iced_core::context_menu::MenuMove),
    SearchCommitHighlighted,
    ToggleSidebarSection(String),
    SelectSearchResult(String),
    OpenSettings,
    WelcomeAddAccount,
    CloseSettings,
    SelectSettingsTab(usize),
    SettingsStackedBack,
    SettingsFocusEvent(enzui_iced_widgets::settings_dialog::SettingsDialogEvent),
    // Matching generation emitted after the retained outgoing surface settles.
    SettingsExitSettled(enzui_iced_widgets::modal_host::ModalPresentationKey),
    SetRemoteContentPolicy(RemoteContentPolicy),
    SetProxyRemoteContent(bool),
    SetAutoDeclineReadReceipts(bool),
    TogglePrivacyAdvanced(bool),
    ToggleAllowListExpanded(bool),
    ToggleBlockListExpanded(bool),
    AllowInputEdited(String),
    BlockInputEdited(String),
    AddAllowedSender,
    AddBlockedSender,
    RemoveAllowedSender(String),
    RemoveBlockedSender(String),
    ToggleAccountExpanded(String),
    AccountSignatureAction(String, text_editor::Action),
    SetAccountAppendSignature(String, bool),
    BeginAddAccount,
    CancelAddAccount,
    // Step-1 email edits; provider detection is derived, so it cannot go stale.
    AddAccountEmailEdited(String),
    AddAccountNameEdited(String),
    AddAccountServersFound(u64, Option<crate::backend::discovery::Discovered>),
    AddAccountContinue,
    AddAccountBack,
    AddAccountFieldEdited(DraftField, String),
    AddAccountPasswordEdited(String),
    ToggleAddAccountPasswordRevealed,
    AddAccountOAuthSignIn,
    AddAccountOAuthCancel,
    // The sign-in worker finished (generation, its result).
    AddAccountOAuthFinished(u64, OAuthHandoff),
    SetAddAccountIncomingEncryption(backend::Encryption),
    SetAddAccountSmtpEncryption(backend::Encryption),
    SetAddAccountAuth(backend::AuthMethod),
    ToggleAddAccountAdvanced(bool),
    CommitAddAccount,
    RequestRemoveAccount(String),
    ConfirmRemoveAccount(enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent),
    RetryRemoveAccount,
    CancelRemoveAccount,
    BeginEditAccount(String),
    EditAccountFieldEdited(DraftField, String),
    EditAccountPasswordEdited(String),
    ToggleEditAccountPasswordRevealed,
    SetEditAccountIncomingEncryption(backend::Encryption),
    SetEditAccountSmtpEncryption(backend::Encryption),
    CommitEditAccount,
    RetryEditAccount,
    CancelEditAccount,

    SelectAllConversations,
    ClearSelection,
    SenderMenuEvent(
        String,
        Option<usize>,
        enzui_iced_widgets::menu_button::MenuButtonEvent,
    ),
    ComposeFormatMenuEvent(enzui_iced_widgets::menu_button::MenuButtonEvent),
    OpenSchedule,
    ScheduleChoice(String),
    // Keep the open scheduled draft in Drafts without sending it.
    CancelScheduledSend(String),
    ScheduleDate(enzui_iced_widgets::date_picker::DatePickerEvent),
    ScheduleTime(enzui_iced_widgets::time_field::TimeFieldEvent),
    ConfirmSchedule,
    ScheduleAllowMissingAttachment(bool),
    ScheduleDialog(enzui_iced_widgets::modal_host::ModalHostEvent),
    ChangeSchedule(String),
    ComposeColorPicked(Option<iced::Color>),
    CopyMessageSelection(String),
    ShowAttachmentDetails {
        message_id: String,
        name: String,
    },
    OpenAttachment(model::types::Attachment),
    SaveAttachment(model::types::Attachment),
    AttachmentOpenPortalTick(link_launcher::LinkWake),
    AttachmentOpenFinished(link_launcher::LinkOpenOutcome),
    OpenAttachmentMenu {
        message_id: String,
        name: String,
        at: iced::Point,
    },
    DismissAttachmentNotice,
    OpenMessageSelectionMenu(enzui_iced_widgets::selection_scope::SelectionContextInvocation),
    ClearMessageSelection,
    ClipboardWritten,
    SetMessageMoveDestination(String),
    ConfirmMessageMove,
    CancelMessageMove,
    PickerDialogEvent(enzui_iced_widgets::modal_host::DialogEvent<modal_focus::MessageDialog>),
    SetSnoozeChoice(String),
    // Snooze the armed messages until the chosen time.
    ConfirmSnooze,
    RemoveSnooze,
    CancelSnooze,
    SnoozeWake,
    InboxWatch(backend::inbox_watch::InboxWatchUpdate),
    NewMailNotificationWake,
    Unsnooze,
    // Resolve the confirmation opened by `Block sender…`. Only the
    // affirmative cause writes the address to the block list.
    ConfirmBlockSender(enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent),
    ToggleMailboxOverflow,
    CloseMailboxOverflow,
    // Toggle one message card's action-row overflow menu.
    ToggleReaderOverflow(String),
    SetPreferPlainText(bool),
    SetDarkHtml(bool),
    SetNotificationSettings(crate::notification_settings::Change),
    TestNotification,
    SetMessagePlainText(String, bool),
    ToggleQuotedText(String),
    // Close one message card's action-row overflow menu if that card is open.
    //
    // Target-matched, so a late dismissal from a closed card cannot shut a menu
    // opened since.
    CloseReaderOverflow(String),
    // Context-menu points are window-relative because scrolled rows cannot serve as measured popup anchors.
    OpenConversationMenu(String, iced::Point),
    OpenSidebarMenu {
        selection: SidebarSelection,
        row_key: String,
        at: iced::Point,
    },
    // Every event the framework context-menu host reports, through one funnel so
    // pointer and Escape dismissal cannot drift apart.
    ContextMenuEvent(u64, enzui_iced_widgets::context_menu_host::ContextMenuEvent),
    MailAction(mail_actions::MailAction, mail_actions::ActionTarget),
    // `Mark All as Read` from the sidebar context menu, on the right-clicked row.
    //
    // Carries the [`SidebarSelection`] because the right-clicked mailbox is
    // usually not the viewed one that [`Message::MarkAllRead`] acts on.
    ContextMarkAllRead(SidebarSelection),

    AddFavorite(SidebarSelection),
    RemoveFavorite(SidebarSelection),
    RemoveSmartMailbox(SidebarSelection),
    SetFavoritesVisible(bool),
    SetSmartMailboxesVisible(bool),
    AddSmartMailbox(model::sidebar_layout::SmartMailboxKind),
    ReorderFavorite(enzui_iced_core::drag::DropRequest),
    ReorderSmartMailbox(enzui_iced_core::drag::DropRequest),

    SelectNextConversation,
    SelectPreviousConversation,
    FocusSearchField,

    Shortcut(shortcuts::ShortcutAction),
    KeyPressed {
        event: iced::keyboard::Event,
        captured: bool,
    },
    Shortcuts(enzui_iced_widgets::editable_shortcuts::EditableShortcutsEvent),
    // The off-thread override write finished; a failure becomes visible text in
    // the pane.
    ShortcutsSaved(u64, Result<(), enzui_iced_core::persist::PersistError>),
    PersistenceBusy(PersistenceDebt),
    BlockingRetryTick,
    // The off-thread preferences write (rules and privacy decisions) finished.
    //
    // The error becomes visible state, reported under the origin that armed the
    // write.
    PreferencesSaved(
        preferences::SaveOrigin,
        preferences::PreferencesToken,
        Result<(), enzui_iced_core::persist::PersistError>,
    ),
    LocalStateSaved(
        local_state::LocalStateToken,
        Result<(), enzui_iced_core::persist::PersistError>,
    ),
    SyncAdoptionSaved(
        u64,
        local_state::LocalStateToken,
        Result<(), enzui_iced_core::persist::PersistError>,
    ),
    ToggleShortcuts,
    ShortcutsDialogEvent(enzui_iced_widgets::shortcuts_dialog::ShortcutsDialogEvent),
    MarkAllRead,
    // "Empty Trash" / "Empty Junk": arms `bulk.pending_empty` only. No message
    // empties a mailbox without passing through `ConfirmEmptyEvent`.
    RequestEmptyMailbox(MailboxKind, String),
    // Every exit of the empty-mailbox confirmation, through the preset's one
    // event; a scrim click maps to cancelled via `ConfirmDecision::from_cause`.
    ConfirmEmptyEvent(enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent),
    BlockSenderOf(String),
    UnblockSenderOf(String),
    OpenRules,
    CloseRules,
    RulesHostEvent(enzui_iced_widgets::modal_host::DialogEvent<modal_focus::RuleDialog>),
    NewRule,
    EditRule(String),
    SaveRule,
    CancelRuleEdit,
    // Request only arms the gate; deletion is unreachable except via confirm.
    RequestDeleteRule(String),
    ConfirmDeleteRuleEvent(enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent),
    MoveRuleUp(String),
    MoveRuleDown(String),
    RuleDropped(enzui_iced_core::drag::DropRequest),
    ApplyRules,
    RuleNameEdited(String),
    RuleConditionFieldSelected(usize, usize),
    RuleConditionMatchSelected(usize, usize),
    RuleConditionValueEdited(usize, String),
    RuleAddCondition,
    RuleRemoveCondition(usize),
    RuleCombinatorSelected(usize),
    RuleActionKindSelected(usize, usize),
    RuleActionMailboxSelected(usize, usize),
    RuleAddAction,
    RuleRemoveAction(usize),
}

impl std::fmt::Debug for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(enzui_iced_core::crash_report::MessageKind::message_kind(
            self,
        ))
    }
}

impl enzui_iced_core::crash_report::MessageKind for Message {
    fn message_kind(&self) -> &'static str {
        match self {
            Self::Theme(..) => "Theme",
            Self::WindowResized(..) => "WindowResized",
            Self::SelectSidebar(..) => "SelectSidebar",
            #[cfg(feature = "center")]
            Self::Center(..) => "Center",
            #[cfg(feature = "center")]
            Self::ApplicationWake => "ApplicationWake",
            #[cfg(feature = "center")]
            Self::BackgroundTaskRefused(..) => "BackgroundTaskRefused",
            #[cfg(feature = "center")]
            Self::CenterDeadline => "CenterDeadline",
            Self::SelectConversation { .. } => "SelectConversation",
            Self::NarrowBack => "NarrowBack",
            Self::SidebarState(..) => "SidebarState",
            Self::AlwaysAllowSender(..) => "AlwaysAllowSender",
            Self::AllowImagesOnce(..) => "AllowImagesOnce",
            Self::OpenUrlDialogEvent(..) => "OpenUrlDialogEvent",
            Self::RequestOpenUrl(..) => "RequestOpenUrl",
            Self::QuickReplyEdit(..) => "QuickReplyEdit",
            Self::LeaveCompose => "LeaveCompose",
            Self::DraftPointerMoved(..) => "DraftPointerMoved",
            Self::DraftClickAway => "DraftClickAway",
            Self::DraftWindowBlur => "DraftWindowBlur",
            Self::SendWithoutAttachment => "SendWithoutAttachment",
            Self::DismissAttachmentReminder => "DismissAttachmentReminder",
            Self::HtmlViewports(..) => "HtmlViewports",
            Self::HtmlTick => "HtmlTick",
            Self::HtmlNativeReady(..) => "HtmlNativeReady",
            Self::HtmlAllocated(..) => "HtmlAllocated",
            Self::HtmlInput(..) => "HtmlInput",
            Self::StartQuickReply(..) => "StartQuickReply",
            Self::CancelQuickReply => "CancelQuickReply",
            Self::QuickReplySend => "QuickReplySend",
            Self::QuickReplyExpand => "QuickReplyExpand",
            Self::SearchServer => "SearchServer",
            Self::AttachmentPreviewLoaded(..) => "AttachmentPreviewLoaded",
            Self::Unsubscribe(..) => "Unsubscribe",
            Self::UnsubscribeFinished(..) => "UnsubscribeFinished",
            Self::ConfirmOpenUrl(..) => "ConfirmOpenUrl",
            Self::OpenUrlFinished(..) => "OpenUrlFinished",
            Self::DismissOpenUrlNotice => "DismissOpenUrlNotice",
            Self::OpenUrlPortalTick(..) => "OpenUrlPortalTick",
            Self::Window(..) => "Window",
            Self::WindowCloseResolved(..) => "WindowCloseResolved",
            Self::CloseSettlementTick(..) => "CloseSettlementTick",
            Self::RetryCloseSettlement => "RetryCloseSettlement",
            Self::DiscardCloseSettlement => "DiscardCloseSettlement",
            Self::CancelCloseSettlement => "CancelCloseSettlement",
            Self::RetryLocalTimestamp => "RetryLocalTimestamp",
            Self::CredentialStoreReady(..) => "CredentialStoreReady",
            Self::CredentialFinished(..) => "CredentialFinished",
            Self::MailOperationFinished(..) => "MailOperationFinished",
            Self::RetryMailOperation => "RetryMailOperation",
            Self::CancelMailOperation => "CancelMailOperation",
            Self::DismissMailNotice => "DismissMailNotice",
            Self::HideNotice(..) => "HideNotice",
            Self::RefreshMailbox => "RefreshMailbox",
            Self::LoadOlderMailbox => "LoadOlderMailbox",
            Self::PeriodicMailboxSync(..) => "PeriodicMailboxSync",
            Self::MailboxSyncRetryTick(..) => "MailboxSyncRetryTick",
            Self::OpenCompose(..) => "OpenCompose",
            Self::CloseCompose => "CloseCompose",
            Self::ShowCc => "ShowCc",
            Self::ShowBcc => "ShowBcc",
            Self::ComposeRecipientMenuEvent(..) => "ComposeRecipientMenuEvent",
            Self::ComposeToEdited(..) => "ComposeToEdited",
            Self::ComposeCcEdited(..) => "ComposeCcEdited",
            Self::ComposeBccEdited(..) => "ComposeBccEdited",
            Self::ComposeSubjectEdited(..) => "ComposeSubjectEdited",
            Self::SetComposeFrom(..) => "SetComposeFrom",
            Self::ComposeRecipientSelected(..) => "ComposeRecipientSelected",
            Self::ComposeRecipientCustom(..) => "ComposeRecipientCustom",
            Self::ComposeBodyAction(..) => "ComposeBodyAction",
            Self::SetComposeFormat(..) => "SetComposeFormat",
            Self::ComposeRichEditAction(..) => "ComposeRichEditAction",
            Self::ToggleComposeOverflow => "ToggleComposeOverflow",
            Self::CloseComposeOverflow => "CloseComposeOverflow",
            Self::DroppedAttachment(..) => "DroppedAttachment",
            Self::AttachFile => "AttachFile",
            Self::NativeDrop(..) => "NativeDrop",
            Self::AttachParentReady(..) => "AttachParentReady",
            Self::DesktopWake => "DesktopWake",
            Self::DesktopActivated => "DesktopActivated",
            Self::AttachPortalTick => "AttachPortalTick",
            Self::AttachFallback(..) => "AttachFallback",
            Self::AttachPickerOutcome(..) => "AttachPickerOutcome",
            Self::AttachmentsStaged(..) => "AttachmentsStaged",
            Self::InboundContentStaged(..) => "InboundContentStaged",
            Self::AttachmentSaved(..) => "AttachmentSaved",
            Self::AttachmentBusy(..) => "AttachmentBusy",
            Self::AttachmentCleanupFinished(..) => "AttachmentCleanupFinished",
            Self::AttachmentReconcileFinished(..) => "AttachmentReconcileFinished",
            Self::AttachmentStartupSurveyFinished(..) => "AttachmentStartupSurveyFinished",
            Self::AttachmentPathResolved(..) => "AttachmentPathResolved",
            Self::FrameLoadSettled(..) => "FrameLoadSettled",
            Self::FrameLoadFailed(..) => "FrameLoadFailed",
            Self::FrameCustodySettled(..) => "FrameCustodySettled",
            Self::OutgoingMimePrepared(..) => "OutgoingMimePrepared",
            Self::RemoveComposeAttachment(..) => "RemoveComposeAttachment",
            Self::SendCompose => "SendCompose",
            Self::Toast(..) => "Toast",
            Self::SaveDraft => "SaveDraft",
            Self::AutosaveDraftTick => "AutosaveDraftTick",
            Self::FocusNext => "FocusNext",
            Self::FocusPrevious => "FocusPrevious",
            Self::SearchEdited(..) => "SearchEdited",
            Self::SearchCleared => "SearchCleared",
            Self::SelectSearchScope(..) => "SelectSearchScope",
            Self::SearchActivated => "SearchActivated",
            Self::SearchDismissed => "SearchDismissed",
            Self::ToggleTitlebarOverflow => "ToggleTitlebarOverflow",
            Self::CloseTitlebarOverflow => "CloseTitlebarOverflow",
            Self::SearchNavigate(..) => "SearchNavigate",
            Self::SearchCommitHighlighted => "SearchCommitHighlighted",
            Self::ToggleSidebarSection(..) => "ToggleSidebarSection",
            Self::SelectSearchResult(..) => "SelectSearchResult",
            Self::OpenSettings => "OpenSettings",
            Self::WelcomeAddAccount => "WelcomeAddAccount",
            Self::CloseSettings => "CloseSettings",
            Self::SelectSettingsTab(..) => "SelectSettingsTab",
            Self::SettingsStackedBack => "SettingsStackedBack",
            Self::SettingsFocusEvent(..) => "SettingsFocusEvent",
            Self::SettingsExitSettled(..) => "SettingsExitSettled",
            Self::SetRemoteContentPolicy(..) => "SetRemoteContentPolicy",
            Self::SetProxyRemoteContent(..) => "SetProxyRemoteContent",
            Self::SetAutoDeclineReadReceipts(..) => "SetAutoDeclineReadReceipts",
            Self::TogglePrivacyAdvanced(..) => "TogglePrivacyAdvanced",
            Self::ToggleAllowListExpanded(..) => "ToggleAllowListExpanded",
            Self::ToggleBlockListExpanded(..) => "ToggleBlockListExpanded",
            Self::AllowInputEdited(..) => "AllowInputEdited",
            Self::BlockInputEdited(..) => "BlockInputEdited",
            Self::AddAllowedSender => "AddAllowedSender",
            Self::AddBlockedSender => "AddBlockedSender",
            Self::RemoveAllowedSender(..) => "RemoveAllowedSender",
            Self::RemoveBlockedSender(..) => "RemoveBlockedSender",
            Self::ToggleAccountExpanded(..) => "ToggleAccountExpanded",
            Self::AccountSignatureAction(..) => "AccountSignatureAction",
            Self::SetAccountAppendSignature(..) => "SetAccountAppendSignature",
            Self::BeginAddAccount => "BeginAddAccount",
            Self::CancelAddAccount => "CancelAddAccount",
            Self::AddAccountEmailEdited(..) => "AddAccountEmailEdited",
            Self::AddAccountNameEdited(..) => "AddAccountNameEdited",
            Self::AddAccountServersFound(..) => "AddAccountServersFound",
            Self::AddAccountContinue => "AddAccountContinue",
            Self::AddAccountBack => "AddAccountBack",
            Self::AddAccountFieldEdited(..) => "AddAccountFieldEdited",
            Self::AddAccountPasswordEdited(..) => "AddAccountPasswordEdited",
            Self::ToggleAddAccountPasswordRevealed => "ToggleAddAccountPasswordRevealed",
            Self::AddAccountOAuthSignIn => "AddAccountOAuthSignIn",
            Self::AddAccountOAuthCancel => "AddAccountOAuthCancel",
            Self::AddAccountOAuthFinished(..) => "AddAccountOAuthFinished",
            Self::SetAddAccountIncomingEncryption(..) => "SetAddAccountIncomingEncryption",
            Self::SetAddAccountSmtpEncryption(..) => "SetAddAccountSmtpEncryption",
            Self::SetAddAccountAuth(..) => "SetAddAccountAuth",
            Self::ToggleAddAccountAdvanced(..) => "ToggleAddAccountAdvanced",
            Self::RequestRemoveAccount(..) => "RequestRemoveAccount",
            Self::ConfirmRemoveAccount(..) => "ConfirmRemoveAccount",
            Self::RetryRemoveAccount => "RetryRemoveAccount",
            Self::CancelRemoveAccount => "CancelRemoveAccount",
            Self::BeginEditAccount(..) => "BeginEditAccount",
            Self::EditAccountFieldEdited(..) => "EditAccountFieldEdited",
            Self::EditAccountPasswordEdited(..) => "EditAccountPasswordEdited",
            Self::ToggleEditAccountPasswordRevealed => "ToggleEditAccountPasswordRevealed",
            Self::SetEditAccountIncomingEncryption(..) => "SetEditAccountIncomingEncryption",
            Self::SetEditAccountSmtpEncryption(..) => "SetEditAccountSmtpEncryption",
            Self::CommitEditAccount => "CommitEditAccount",
            Self::RetryEditAccount => "RetryEditAccount",
            Self::CancelEditAccount => "CancelEditAccount",
            Self::CommitAddAccount => "CommitAddAccount",
            Self::SelectAllConversations => "SelectAllConversations",
            Self::ClearSelection => "ClearSelection",
            Self::SenderMenuEvent(..) => "SenderMenuEvent",
            Self::ComposeFormatMenuEvent(..) => "ComposeFormatMenuEvent",
            Self::ComposeColorPicked(..) => "ComposeColorPicked",
            Self::CopyMessageSelection(..) => "CopyMessageSelection",
            Self::ShowAttachmentDetails { .. } => "ShowAttachmentDetails",
            Self::OpenAttachment(..) => "OpenAttachment",
            Self::SaveAttachment(..) => "SaveAttachment",
            Self::AttachmentOpenPortalTick(..) => "AttachmentOpenPortalTick",
            Self::AttachmentOpenFinished(..) => "AttachmentOpenFinished",
            Self::OpenAttachmentMenu { .. } => "OpenAttachmentMenu",
            Self::DismissAttachmentNotice => "DismissAttachmentNotice",
            Self::OpenMessageSelectionMenu(..) => "OpenMessageSelectionMenu",
            Self::ClearMessageSelection => "ClearMessageSelection",
            Self::ClipboardWritten => "ClipboardWritten",
            Self::SetMessageMoveDestination(..) => "SetMessageMoveDestination",
            Self::ConfirmMessageMove => "ConfirmMessageMove",
            Self::CancelMessageMove => "CancelMessageMove",
            Self::PickerDialogEvent(..) => "PickerDialogEvent",
            Self::SetSnoozeChoice(..) => "SetSnoozeChoice",
            Self::ConfirmSnooze => "ConfirmSnooze",
            Self::RemoveSnooze => "RemoveSnooze",
            Self::CancelSnooze => "CancelSnooze",
            Self::SnoozeWake => "SnoozeWake",
            Self::InboxWatch(..) => "InboxWatch",
            Self::NewMailNotificationWake => "NewMailNotificationWake",
            Self::Unsnooze => "Unsnooze",
            Self::OpenSchedule => "OpenSchedule",
            Self::ScheduleChoice(..) => "ScheduleChoice",
            Self::ScheduleDate(..) => "ScheduleDate",
            Self::ScheduleTime(..) => "ScheduleTime",
            Self::ConfirmSchedule => "ConfirmSchedule",
            Self::ScheduleDialog(..) => "ScheduleDialog",
            Self::ChangeSchedule(..) => "ChangeSchedule",
            Self::ScheduleAllowMissingAttachment(..) => "ScheduleAllowMissingAttachment",
            Self::CancelScheduledSend(..) => "CancelScheduledSend",
            Self::ConfirmBlockSender(..) => "ConfirmBlockSender",
            Self::ToggleMailboxOverflow => "ToggleMailboxOverflow",
            Self::CloseMailboxOverflow => "CloseMailboxOverflow",
            Self::ToggleReaderOverflow(_) => "ToggleReaderOverflow",
            Self::SetPreferPlainText(_) => "SetPreferPlainText",
            Self::SetDarkHtml(_) => "SetDarkHtml",
            Self::SetNotificationSettings(_) => "SetNotificationSettings",
            Self::TestNotification => "TestNotification",
            Self::SetMessagePlainText(..) => "SetMessagePlainText",
            Self::ToggleQuotedText(..) => "ToggleQuotedText",
            Self::CloseReaderOverflow(_) => "CloseReaderOverflow",
            Self::OpenConversationMenu(..) => "OpenConversationMenu",
            Self::OpenSidebarMenu { .. } => "OpenSidebarMenu",
            Self::ContextMenuEvent(..) => "ContextMenuEvent",
            Self::MailAction(..) => "MailAction",
            Self::ContextMarkAllRead(..) => "ContextMarkAllRead",
            Self::AddFavorite(..) => "AddFavorite",
            Self::RemoveFavorite(..) => "RemoveFavorite",
            Self::RemoveSmartMailbox(..) => "RemoveSmartMailbox",
            Self::SetFavoritesVisible(..) => "SetFavoritesVisible",
            Self::SetSmartMailboxesVisible(..) => "SetSmartMailboxesVisible",
            Self::AddSmartMailbox(..) => "AddSmartMailbox",
            Self::ReorderFavorite(..) => "ReorderFavorite",
            Self::ReorderSmartMailbox(..) => "ReorderSmartMailbox",
            Self::SelectNextConversation => "SelectNextConversation",
            Self::SelectPreviousConversation => "SelectPreviousConversation",
            Self::FocusSearchField => "FocusSearchField",
            Self::Shortcut(..) => "Shortcut",
            Self::KeyPressed { .. } => "KeyPressed",
            Self::Shortcuts(..) => "Shortcuts",
            Self::ShortcutsSaved(..) => "ShortcutsSaved",
            Self::PersistenceBusy(..) => "PersistenceBusy",
            Self::BlockingRetryTick => "BlockingRetryTick",
            Self::PreferencesSaved(..) => "PreferencesSaved",
            Self::LocalStateSaved(..) => "LocalStateSaved",
            Self::SyncAdoptionSaved(..) => "SyncAdoptionSaved",
            Self::ToggleShortcuts => "ToggleShortcuts",
            Self::ShortcutsDialogEvent(..) => "ShortcutsDialogEvent",
            Self::MarkAllRead => "MarkAllRead",
            Self::RequestEmptyMailbox(..) => "RequestEmptyMailbox",
            Self::ConfirmEmptyEvent(..) => "ConfirmEmptyEvent",
            Self::BlockSenderOf(..) => "BlockSenderOf",
            Self::UnblockSenderOf(..) => "UnblockSenderOf",
            Self::OpenRules => "OpenRules",
            Self::CloseRules => "CloseRules",
            Self::RulesHostEvent(..) => "RulesHostEvent",
            Self::NewRule => "NewRule",
            Self::EditRule(..) => "EditRule",
            Self::SaveRule => "SaveRule",
            Self::CancelRuleEdit => "CancelRuleEdit",
            Self::RequestDeleteRule(..) => "RequestDeleteRule",
            Self::ConfirmDeleteRuleEvent(..) => "ConfirmDeleteRuleEvent",
            Self::MoveRuleUp(..) => "MoveRuleUp",
            Self::MoveRuleDown(..) => "MoveRuleDown",
            Self::RuleDropped(..) => "RuleDropped",
            Self::ApplyRules => "ApplyRules",
            Self::RuleNameEdited(..) => "RuleNameEdited",
            Self::RuleConditionFieldSelected(..) => "RuleConditionFieldSelected",
            Self::RuleConditionMatchSelected(..) => "RuleConditionMatchSelected",
            Self::RuleConditionValueEdited(..) => "RuleConditionValueEdited",
            Self::RuleAddCondition => "RuleAddCondition",
            Self::RuleRemoveCondition(..) => "RuleRemoveCondition",
            Self::RuleCombinatorSelected(..) => "RuleCombinatorSelected",
            Self::RuleActionKindSelected(..) => "RuleActionKindSelected",
            Self::RuleActionMailboxSelected(..) => "RuleActionMailboxSelected",
            Self::RuleAddAction => "RuleAddAction",
            Self::RuleRemoveAction(..) => "RuleRemoveAction",
        }
    }
}
