//! Settings-subsystem state: the pure half of the Settings surface, whose view
//! half is `settings_view.rs`.
//!
//! It makes the account-configuration layer in `backend/config.rs` reachable
//! from the UI. The Add-Account password is moved into the framework
//! credential store before an account is published; `AccountConfig` carries
//! no secret bytes.

use crate::backend::config::{
    AccountConfig, AccountConfigDraft, DraftError, ParsedEmail, ProviderPreset, parse_email,
    preset_for_domain,
};

use super::types::{Account, MailboxKind, MailboxStore};
use enzui_iced_widgets::modal_host::{ModalPresentation, ModalPresentationKey};

// The wired Settings panes. `Junk` is absent: no server-side junk policy
// exists and local controls would duplicate Privacy's blocked senders.
// `Rules` has its own modal.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(crate) enum SettingsTab {
    #[default]
    General,
    Advanced,
    Accounts,
    // The Shortcuts page ID must match the framework settings page’s stable ID.
    Shortcuts,
    Notifications,
}

impl SettingsTab {
    pub(crate) const ALL: [SettingsTab; 5] = [
        SettingsTab::General,
        SettingsTab::Advanced,
        SettingsTab::Accounts,
        SettingsTab::Shortcuts,
        SettingsTab::Notifications,
    ];

    pub(crate) fn id(self) -> &'static str {
        match self {
            SettingsTab::General => "general",
            SettingsTab::Advanced => "advanced",
            SettingsTab::Accounts => "accounts",
            // Must equal `shortcuts_settings_page().id()`.
            SettingsTab::Shortcuts => "shortcuts",
            SettingsTab::Notifications => "notifications",
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            SettingsTab::General => "General",
            SettingsTab::Advanced => "Advanced",
            SettingsTab::Accounts => "Accounts",
            SettingsTab::Shortcuts => "Shortcuts",
            SettingsTab::Notifications => "Notifications",
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            SettingsTab::General => "settings",
            SettingsTab::Advanced => "settings-2",
            SettingsTab::Accounts => "user",
            SettingsTab::Shortcuts => "keyboard",
            SettingsTab::Notifications => "bell",
        }
    }

    pub(crate) fn index(self) -> usize {
        match self {
            SettingsTab::General => 0,
            SettingsTab::Advanced => 1,
            SettingsTab::Accounts => 2,
            SettingsTab::Shortcuts => 3,
            SettingsTab::Notifications => 4,
        }
    }

    // Parse a tab-strip index back into a tab; out-of-range yields `None` rather
    // than selecting `General`.
    pub(crate) fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    // Parse a stable page id back into a tab. Unknown ids fall back to the first
    // tab, a defensive floor since the preset fires only declared ids.
    pub(crate) fn from_id(id: &str) -> Self {
        Self::ALL
            .iter()
            .find(|t| t.id() == id)
            .copied()
            .unwrap_or(SettingsTab::General)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(crate) enum AddAccountStep {
    #[default]
    Email,
    Configure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum DraftField {
    DisplayName,
    IncomingServer,
    IncomingPort,
    IncomingUsername,
    SmtpServer,
    SmtpPort,
    SmtpUsername,
}

// What a sign-in was obtained for: the address and incoming server of the
// draft when it started. A grant is committed only for the same pair, so a
// grant from one address can never be stored as another's credential.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OAuthBinding {
    email: String,
    server: String,
}

impl OAuthBinding {
    pub(crate) fn of(add: &AddAccountState) -> Self {
        Self {
            email: add.draft.email.trim().to_ascii_lowercase(),
            server: add.draft.imap_server.trim().to_ascii_lowercase(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) enum OAuthSignInStatus {
    #[default]
    NotStarted,
    Waiting {
        provider: &'static str,
    },
    // A grant is held for the commit.
    SignedIn {
        provider: &'static str,
    },
    Failed(String),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) enum ServerLookup {
    #[default]
    Idle,
    Looking {
        domain: String,
    },
    Found(String),
    NotFound {
        domain: String,
    },
}

#[derive(Clone, Default, Eq, PartialEq)]
pub(crate) struct AddAccountState {
    pub(crate) open: bool,
    pub(crate) name: String,
    // The server lookup for the address; a result from an earlier run is dropped.
    pub(crate) lookup: ServerLookup,
    pub(crate) lookup_generation: u64,
    pub(crate) step: AddAccountStep,
    // Step-1 address. Detection keys off this rather than `draft.email`, so it
    // reacts before Continue.
    pub(crate) email: String,
    pub(crate) draft: AccountConfigDraft,
    // Moved into the credential store before the account is published.
    pub(crate) password: String,
    pub(crate) password_revealed: bool,
    pub(crate) advanced_expanded: bool,
    pub(crate) error: Option<String>,
    // The browser sign-in, for an OAuth account. The grant itself is held
    // by the app state, never here.
    pub(crate) oauth: OAuthSignInStatus,
}

impl std::fmt::Debug for AddAccountState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AddAccountState")
            .field("open", &self.open)
            .field("step", &self.step)
            .field("draft", &self.draft)
            .field("password", &"[redacted]")
            .finish_non_exhaustive()
    }
}

impl AddAccountState {
    // The provider to sign in with, when the draft uses OAuth and its server
    // is one Mail knows.
    pub(crate) fn oauth_provider(
        &self,
    ) -> Option<crate::backend::curl_backend::oauth::OAuthProvider> {
        (self.draft.auth == crate::backend::AuthMethod::OAuth2)
            .then(|| {
                crate::backend::curl_backend::oauth::OAuthProvider::for_server(
                    &self.draft.imap_server,
                )
            })
            .flatten()
    }

    // Open a fresh flow, discarding everything from the previous run, including
    // the password.
    pub(crate) fn begin(&mut self) {
        *self = AddAccountState {
            open: true,
            ..AddAccountState::default()
        };
        self.draft = AccountConfigDraft::new();
    }

    // Close the flow and clear every field, including the password buffer.
    pub(crate) fn reset(&mut self) {
        *self = AddAccountState::default();
    }

    pub(crate) fn parsed(&self) -> Option<ParsedEmail> {
        parse_email(&self.email)
    }

    pub(crate) fn detected_preset(&self) -> Option<&'static ProviderPreset> {
        let parsed = self.parsed()?;
        preset_for_domain(&parsed.domain)
    }

    pub(crate) fn detected_label(&self) -> Option<String> {
        self.detected_preset()
            .map(|p| format!("Detected {}", p.display_name))
    }

    pub(crate) fn can_continue(&self) -> bool {
        self.parsed().is_some()
    }

    pub(crate) fn continue_to_configure(&mut self) -> bool {
        let Some(parsed) = self.parsed() else {
            return false;
        };
        self.draft = match self.detected_preset() {
            Some(preset) => AccountConfigDraft::from_preset(&parsed, preset),
            None => AccountConfigDraft::from_email(&parsed),
        };
        // The sender name is the person's, never the provider's.
        let name = self.name.trim();
        self.draft.display_name = if name.is_empty() {
            parsed.local
        } else {
            name.to_owned()
        };
        self.lookup = match self.detected_preset() {
            Some(preset) => ServerLookup::Found(crate::l10n::message(
                "mail-discovery-preset",
                &[("provider", preset.display_name.to_owned())],
            )),
            None => ServerLookup::Idle,
        };
        self.step = AddAccountStep::Configure;
        self.error = None;
        self.oauth = OAuthSignInStatus::NotStarted;
        true
    }

    // Step 2 to step 1. The draft is kept, so hand-edited server fields survive
    // unless the address changes, which rebuilds the draft on Continue.
    pub(crate) fn back_to_email(&mut self) {
        self.step = AddAccountStep::Email;
        self.error = None;
    }

    // Start a server lookup for the typed address; returns the run's
    // generation and the address to look up.
    pub(crate) fn begin_lookup(&mut self) -> Option<(u64, ParsedEmail)> {
        let parsed = self.parsed()?;
        self.lookup_generation = self.lookup_generation.wrapping_add(1);
        self.lookup = ServerLookup::Looking {
            domain: parsed.domain.clone(),
        };
        Some((self.lookup_generation, parsed))
    }

    // Apply a lookup's result, unless it belongs to an earlier run or the
    // user has already typed a server of their own.
    pub(crate) fn finish_lookup(
        &mut self,
        generation: u64,
        found: Option<crate::backend::discovery::Discovered>,
    ) {
        let ServerLookup::Looking { domain } = &self.lookup else {
            return;
        };
        if generation != self.lookup_generation || self.step != AddAccountStep::Configure {
            return;
        }
        let domain = domain.clone();
        let Some(found) = found else {
            self.lookup = ServerLookup::NotFound { domain };
            return;
        };
        let typed =
            !self.draft.imap_server.trim().is_empty() || !self.draft.smtp_server.trim().is_empty();
        if !typed {
            self.draft.apply_discovered(&found);
        }
        self.lookup = ServerLookup::Found(found.source.describe(&domain));
    }

    pub(crate) fn can_commit(&self) -> bool {
        !matches!(self.lookup, ServerLookup::Looking { .. })
    }

    pub(crate) fn set_field(&mut self, field: DraftField, value: String) {
        let d = &mut self.draft;
        match field {
            DraftField::DisplayName => d.display_name = value,
            DraftField::IncomingServer => d.imap_server = value,
            DraftField::IncomingPort => d.imap_port = value,
            DraftField::IncomingUsername => d.imap_username = value,
            DraftField::SmtpServer => d.smtp_server = value,
            DraftField::SmtpPort => d.smtp_port = value,
            DraftField::SmtpUsername => d.smtp_username = value,
        }
        self.error = None;
    }
}

// A reason an account identity cannot be admitted or committed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AccountIdRefusal {
    Exhausted,
    Occupied(String),
    Inconsistent { next: u64, highest: u64 },
    CandidateChanged { expected: String, actual: String },
    Duplicate(String),
}

impl AccountIdRefusal {
    pub(crate) fn user_message(&self) -> &'static str {
        match self {
            Self::Exhausted => "No more accounts can be added. Edit an existing account instead.",
            Self::Occupied(_) => {
                "This account was not added because its local identity is already in use."
            }
            Self::CandidateChanged { .. } => {
                "Account setup changed before it could be saved. Cancel and add the account again."
            }
            Self::Inconsistent { .. } | Self::Duplicate(_) => {
                "Saved account identities are inconsistent. Restore valid local state before adding another account."
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AccountIdAdmission {
    pub(crate) id: String,
    next_ordinal: u64,
}

fn canonical_account_ordinal(id: &str) -> Option<u64> {
    let suffix = id.strip_prefix("acct-")?;
    if suffix.is_empty()
        || !suffix.bytes().all(|byte| byte.is_ascii_digit())
        || (suffix.len() > 1 && suffix.starts_with('0'))
    {
        return None;
    }
    suffix.parse().ok()
}

fn account_ids(
    accounts: &[Account],
) -> Result<(std::collections::HashSet<&str>, Option<u64>), AccountIdRefusal> {
    let mut ids = std::collections::HashSet::new();
    for account in accounts {
        if !ids.insert(account.id.as_str()) {
            return Err(AccountIdRefusal::Duplicate(account.id.clone()));
        }
    }
    let highest = accounts
        .iter()
        .filter_map(|account| canonical_account_ordinal(&account.id))
        .max();
    Ok((ids, highest))
}

// Normalize an equal-to-highest persisted ordinal without wrapping or skipping conflicts.
pub(crate) fn recover_account_allocator(
    accounts: &[Account],
    next_ordinal: u64,
) -> Result<u64, AccountIdRefusal> {
    let (_, highest) = account_ids(accounts)?;
    let Some(highest) = highest else {
        return Ok(next_ordinal);
    };
    if next_ordinal < highest {
        return Err(AccountIdRefusal::Inconsistent {
            next: next_ordinal,
            highest,
        });
    }
    if next_ordinal == highest {
        return Ok(highest.saturating_add(1));
    }
    Ok(next_ordinal)
}

// Validate unique account ownership and a non-regressing allocator.
pub(crate) fn validate_account_allocator(
    accounts: &[Account],
    next_ordinal: u64,
) -> Result<(), AccountIdRefusal> {
    let (ids, highest) = account_ids(accounts)?;
    if highest == Some(u64::MAX) && next_ordinal == u64::MAX {
        return Ok(());
    }
    let candidate = format!("acct-{next_ordinal}");
    if ids.contains(candidate.as_str()) {
        return Err(AccountIdRefusal::Occupied(candidate));
    }
    if let Some(highest) = highest
        && next_ordinal < highest
    {
        return Err(AccountIdRefusal::Inconsistent {
            next: next_ordinal,
            highest,
        });
    }
    Ok(())
}

// Admit the current candidate without mutating the account set or allocator.
pub(crate) fn admit_account_id(
    accounts: &[Account],
    next_ordinal: u64,
) -> Result<AccountIdAdmission, AccountIdRefusal> {
    validate_account_allocator(accounts, next_ordinal)?;
    let id = format!("acct-{next_ordinal}");
    if next_ordinal == u64::MAX && accounts.iter().any(|account| account.id == id) {
        return Err(AccountIdRefusal::Exhausted);
    }
    Ok(AccountIdAdmission {
        id,
        next_ordinal: next_ordinal.saturating_add(1),
    })
}

// Commit a previously validated configuration only if its identity is still admissible.
pub(crate) fn commit_account_config(
    store: &mut MailboxStore,
    next_ordinal: &mut u64,
    config: AccountConfig,
) -> Result<String, AccountIdRefusal> {
    let admission = admit_account_id(&store.accounts, *next_ordinal)?;
    if config.id != admission.id {
        return Err(AccountIdRefusal::CandidateChanged {
            expected: admission.id,
            actual: config.id,
        });
    }
    let id = config.id.clone();
    let account = account_from_config(config);
    store.accounts.push(account);
    *next_ordinal = admission.next_ordinal;
    Ok(id)
}

// A fixture/local account commit refusal from validation or identity admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AddAccountCommitError {
    Draft(DraftError),
    Identity(AccountIdRefusal),
}

// Validate and commit the local account flow without consuming a refused identity.
pub(crate) fn commit_add_account(
    store: &mut MailboxStore,
    add: &mut AddAccountState,
    next_ordinal: &mut u64,
) -> Result<String, AddAccountCommitError> {
    let admission = admit_account_id(&store.accounts, *next_ordinal).map_err(|error| {
        add.error = Some(error.user_message().to_owned());
        AddAccountCommitError::Identity(error)
    })?;
    let config = add.draft.validate(admission.id).map_err(|error| {
        add.error = Some(error.message.to_owned());
        AddAccountCommitError::Draft(error)
    })?;
    let id = commit_account_config(store, next_ordinal, config).map_err(|error| {
        add.error = Some(error.user_message().to_owned());
        AddAccountCommitError::Identity(error)
    })?;
    add.reset();
    Ok(id)
}

pub(crate) fn account_from_config(config: AccountConfig) -> Account {
    Account {
        id: config.id.clone(),
        display_name: config.display_name.clone(),
        email: config.email.clone(),
        mailboxes: vec![
            MailboxKind::Inbox,
            MailboxKind::Drafts,
            MailboxKind::Sent,
            MailboxKind::Junk,
            MailboxKind::Trash,
            MailboxKind::Archive,
        ],
        folders: Vec::new(),
        config: Some(config),
        signature: String::new(),
        append_signature: false,
    }
}

#[derive(Clone, Default, Eq, PartialEq)]
pub(crate) struct EditAccountState {
    pub(crate) account_id: Option<String>,
    pub(crate) draft: AccountConfigDraft,
    pub(crate) password: String,
    pub(crate) password_revealed: bool,
    pub(crate) error: Option<String>,
}

impl std::fmt::Debug for EditAccountState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditAccountState")
            .field("account_id", &self.account_id)
            .field("draft", &self.draft)
            .field("password", &"[redacted]")
            .finish_non_exhaustive()
    }
}

impl EditAccountState {
    pub(crate) fn begin(&mut self, config: &AccountConfig) {
        *self = Self {
            account_id: Some(config.id.clone()),
            draft: AccountConfigDraft::from_config(config),
            password: String::new(),
            password_revealed: false,
            error: None,
        };
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn is_open_for(&self, account_id: &str) -> bool {
        self.account_id.as_deref() == Some(account_id)
    }

    pub(crate) fn set_field(&mut self, field: DraftField, value: String) {
        let d = &mut self.draft;
        match field {
            DraftField::DisplayName => d.display_name = value,
            DraftField::IncomingServer => d.imap_server = value,
            DraftField::IncomingPort => d.imap_port = value,
            DraftField::IncomingUsername => d.imap_username = value,
            DraftField::SmtpServer => d.smtp_server = value,
            DraftField::SmtpPort => d.smtp_port = value,
            DraftField::SmtpUsername => d.smtp_username = value,
        }
        self.error = None;
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SettingsState {
    pub(crate) open: bool,
    // Generation-safe lifetime for the settings surface. The settings fields
    // remain available while this value is closing so the host can rebuild
    // the outgoing sheet; they are torn down only by [`Self::finish_exit`].
    presentation: ModalPresentation<()>,
    pub(crate) tab: SettingsTab,
    // Stacked-rung drill state: `false` is the page list, `true` shows the picked
    // page's pane with Back.
    //
    // Meaningful only when the dialog resolves to `SettingsNav::Stacked`;
    // `with_settings` folds it into `drilled_in` only then.
    pub(crate) stacked_tab_open: bool,
    pub(crate) add_account: AddAccountState,
    pub(crate) edit_account: EditAccountState,
    // The open per-account expander on the Accounts pane. Single-open because the
    // one signature editor buffer follows it.
    pub(crate) expanded_account: Option<String>,
    pub(crate) privacy_advanced_expanded: bool,
    pub(crate) allow_list_expanded: bool,
    pub(crate) block_list_expanded: bool,
    pub(crate) allow_input: String,
    pub(crate) block_input: String,
}

// Which page the Settings dialog is showing: the modelled navigation depth.
//
// [`SettingsState::page`] derives it once, in the order
// `settings_view::accounts_pane` renders (`add_account.open` before
// `expanded_account`), so the header cannot offer Done on a subpage or Back
// on the root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SettingsPage {
    // The tab strip and one of the panes; the only page carrying `Done`.
    Root,
    AccountDetail { account_id: String },
    AddAccount { step: AddAccountStep },
}

impl SettingsState {
    // The page the dialog is currently showing.
    //
    // Consults `store` because `expanded_account` can name a removed account,
    // which renders the list, and the tab because `select_tab` keeps drill-ins
    // that must not make another tab look like a subpage.
    #[must_use]
    pub(crate) fn page(&self, store: &MailboxStore) -> SettingsPage {
        if self.tab != SettingsTab::Accounts {
            return SettingsPage::Root;
        }
        if self.add_account.open {
            return SettingsPage::AddAccount {
                step: self.add_account.step,
            };
        }
        match self.expanded_account.as_deref() {
            Some(id) if store.account(id).is_some() => SettingsPage::AccountDetail {
                account_id: id.to_owned(),
            },
            _ => SettingsPage::Root,
        }
    }

    pub(crate) fn open(&mut self) {
        self.presentation.present(());
        self.open = true;
        // A Stacked presentation lands on its page list, not a stale pane.
        self.stacked_tab_open = false;
    }

    // Begin dismissal without destroying the model needed to paint the exit.
    pub(crate) fn close(&mut self) {
        self.open = false;
        self.presentation.dismiss();
    }

    // Whether an open or outgoing settings surface must still be rebuilt.
    #[must_use]
    pub(crate) fn is_retained(&self) -> bool {
        self.presentation.value().is_some()
    }

    // Generation captured by the host's settlement callback.
    #[must_use]
    pub(crate) fn presentation_key(&self) -> Option<ModalPresentationKey> {
        self.presentation.key()
    }

    // Complete a matching exit and only then scrub session-owned model data.
    pub(crate) fn finish_exit(&mut self, key: ModalPresentationKey) -> bool {
        if !self.presentation.finish_exit(key) {
            return false;
        }
        self.add_account.reset();
        self.stacked_tab_open = false;
        true
    }

    pub(crate) fn select_tab(&mut self, tab: SettingsTab) {
        self.tab = tab;
        self.stacked_tab_open = true;
    }

    pub(crate) fn stacked_back_to_list(&mut self) {
        self.stacked_tab_open = false;
    }

    pub(crate) fn toggle_account(&mut self, account_id: &str) -> Option<String> {
        if self.expanded_account.as_deref() == Some(account_id) {
            self.expanded_account = None;
        } else {
            self.expanded_account = Some(account_id.to_owned());
        }
        self.expanded_account.clone()
    }
}

pub(crate) fn account_count_label(n: usize) -> String {
    crate::l10n::count("mail-accounts-count", n)
}

#[cfg(test)]
mod credential_debug_tests {}
