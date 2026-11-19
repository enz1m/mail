use enzui_iced_core::persist::{
    FileBackend, InMemoryBackend, PersistenceKind, RawByteLimit, Store,
};
use enzui_iced_core::shortcut::{Modifier, Shortcut};
use enzui_iced_core::{
    EditableShortcutCatalog, ShortcutBinding, ShortcutOverrides, ShortcutValidity,
    dispatch_shortcut_command,
};
use enzui_iced_widgets::shortcut_list::{ShortcutEntry, shortcut_catalog_entries};
use iced::keyboard::{self, key::Named};
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ShortcutAction {
    ArchiveSelected,
    DeleteSelected,
    ToggleReadSelected,
    ToggleFlaggedSelected,
    Compose,
    Reply,
    ReplyAll,
    Forward,
    SendCompose,
    NextConversation,
    PreviousConversation,
    FocusSearch,
    ShowShortcuts,
}

// Widget and modal ownership sampled when resolving a key press.
pub(crate) type ShortcutContext = enzui_iced_core::action::KeyRouteScope;

fn global(
    id: &'static str,
    action: ShortcutAction,
    primary: Shortcut,
) -> ShortcutBinding<ShortcutAction, ShortcutContext> {
    ShortcutBinding::new(id, action, primary).when(ShortcutContext::is_global)
}

fn presentation(
    id: &'static str,
    action: ShortcutAction,
    primary: Shortcut,
) -> ShortcutBinding<ShortcutAction, ShortcutContext> {
    ShortcutBinding::new(id, action, primary).when(ShortcutContext::is_uncaptured)
}

pub(crate) static SHORTCUTS: LazyLock<Vec<ShortcutBinding<ShortcutAction, ShortcutContext>>> =
    LazyLock::new(|| {
        vec![
            global(
                "mail.archive-selected",
                ShortcutAction::ArchiveSelected,
                Shortcut::new(Modifier::NONE, 'e'),
            )
            .with_section(crate::l10n::tr("Triage"))
            .with_description(crate::l10n::tr("Archive selected")),
            global(
                "mail.delete-selected",
                ShortcutAction::DeleteSelected,
                Shortcut::new(Modifier::NONE, Named::Delete),
            )
            .with_alternate(Shortcut::new(Modifier::NONE, '#'))
            .with_section(crate::l10n::tr("Triage"))
            .with_description(crate::l10n::tr("Delete selected (move to Trash)")),
            global(
                "mail.toggle-read-selected",
                ShortcutAction::ToggleReadSelected,
                Shortcut::new(Modifier::NONE, 'u'),
            )
            .with_section(crate::l10n::tr("Triage"))
            .with_description(crate::l10n::tr("Mark read / unread")),
            global(
                "mail.toggle-flagged-selected",
                ShortcutAction::ToggleFlaggedSelected,
                Shortcut::new(Modifier::NONE, 's'),
            )
            .with_section(crate::l10n::tr("Triage"))
            .with_description(crate::l10n::tr("Flag / unflag")),
            global(
                "mail.compose",
                ShortcutAction::Compose,
                Shortcut::new(Modifier::NONE, 'c'),
            )
            .with_section(crate::l10n::tr("Compose"))
            .with_description(crate::l10n::tr("New message")),
            global(
                "mail.reply",
                ShortcutAction::Reply,
                Shortcut::new(Modifier::NONE, 'r'),
            )
            .with_section(crate::l10n::tr("Compose"))
            .with_description(crate::l10n::tr("Reply")),
            global(
                "mail.reply-all",
                ShortcutAction::ReplyAll,
                Shortcut::new(Modifier::NONE, 'a'),
            )
            .with_section(crate::l10n::tr("Compose"))
            .with_description(crate::l10n::tr("Reply all")),
            global(
                "mail.forward",
                ShortcutAction::Forward,
                Shortcut::new(Modifier::NONE, 'f'),
            )
            .with_section(crate::l10n::tr("Compose"))
            .with_description(crate::l10n::tr("Forward")),
            // Not `.when(is_global)`: this chord fires from inside the focused body
            // editor. `Super+Enter` is the Cmd spelling, an alternate rather than a row.
            ShortcutBinding::new_in_context(
                "mail.send-compose",
                ShortcutAction::SendCompose,
                Shortcut::new(Modifier::CTRL, Named::Enter),
            )
            .with_alternate(Shortcut::new(Modifier::SUPER, Named::Enter))
            .with_section(crate::l10n::tr("Compose"))
            .with_description(crate::l10n::tr("Send message")),
            global(
                "mail.next-conversation",
                ShortcutAction::NextConversation,
                Shortcut::new(Modifier::NONE, 'j'),
            )
            .with_section(crate::l10n::tr("Navigation"))
            .with_description(crate::l10n::tr("Next conversation")),
            global(
                "mail.previous-conversation",
                ShortcutAction::PreviousConversation,
                Shortcut::new(Modifier::NONE, 'k'),
            )
            .with_section(crate::l10n::tr("Navigation"))
            .with_description(crate::l10n::tr("Previous conversation")),
            global(
                "mail.focus-search",
                ShortcutAction::FocusSearch,
                Shortcut::new(Modifier::NONE, '/'),
            )
            .with_section(crate::l10n::tr("Navigation"))
            .with_description(crate::l10n::tr("Search")),
            // Exact modifier matching needs both shifted and unshifted question-mark events.
            presentation(
                "mail.show-shortcuts",
                ShortcutAction::ShowShortcuts,
                Shortcut::new(Modifier::NONE, '?'),
            )
            .with_alternate(Shortcut::new(Modifier::SHIFT, '?'))
            .with_section(crate::l10n::tr("Help"))
            .with_description(crate::l10n::tr("Keyboard shortcuts")),
        ]
    });

pub(crate) type MailShortcuts = EditableShortcutCatalog<ShortcutAction, ShortcutContext>;

// The persistence identity of the override map. Bumping the version refuses
// an older file rather than misreading it.
pub(crate) const OVERRIDES_FORMAT: &str = "mail-app-shortcuts";
pub(crate) const OVERRIDES_VERSION: u32 = 1;
pub(crate) const SHORTCUT_CATALOG_RECORDS_MAX: u64 = 16;
const SHORTCUT_BODY_PREAMBLE_BYTES_MAX: u64 = 1024;
pub(crate) const SHORTCUTS_BODY_MAX: u64 = SHORTCUT_BODY_PREAMBLE_BYTES_MAX
    + SHORTCUT_CATALOG_RECORDS_MAX * enzui_iced_core::atomic_state::MAX_RECORD_BYTES;

// The shared record ceiling bounds each override; the catalog-count gate keeps
// the complete body derivation valid as actions are added.
pub(crate) const SHORTCUTS_RAW_BYTE_LIMIT: RawByteLimit =
    match RawByteLimit::for_body(OVERRIDES_FORMAT, OVERRIDES_VERSION, SHORTCUTS_BODY_MAX) {
        Ok(limit) => limit,
        Err(_) => panic!("shortcut body ceiling exceeds framework authority"),
    };

// The production override store: `$XDG_CONFIG_HOME/mail-app/shortcuts`.
//
// Falls back to an in-memory backend when XDG cannot be resolved (no HOME in
// a sandbox), so a rebind still works for the session instead of panicking.
pub(crate) fn file_store() -> Store<ShortcutOverrides> {
    match FileBackend::xdg(
        PersistenceKind::Config,
        "mail-app",
        "shortcuts",
        SHORTCUTS_RAW_BYTE_LIMIT,
    )
    .expect("the static shortcuts XDG descendants are valid")
    {
        Some(backend) => Store::new(backend, OVERRIDES_FORMAT, OVERRIDES_VERSION),
        None => memory_store(),
    }
}

// The production overrides path, so a refused frame can be taken into custody
// instead of being published over.
pub(crate) fn file_store_path() -> Option<std::path::PathBuf> {
    enzui_iced_core::persist::xdg_path(PersistenceKind::Config, "mail-app", "shortcuts")
        .expect("the static shortcuts XDG descendants are valid")
}

// A store that never touches disk, so tests cannot read or write the
// developer's real configuration.
pub(crate) fn memory_store() -> Store<ShortcutOverrides> {
    Store::new(InMemoryBackend::new(), OVERRIDES_FORMAT, OVERRIDES_VERSION)
}

// Build the live catalog from [`SHORTCUTS`] and retain safe feedback when
// persisted overrides could not be used. Unknown ids and default-equivalent
// overrides are dropped by core; failed loads remain write-protected.
pub(crate) fn catalog_from(store: &Store<ShortcutOverrides>) -> (MailShortcuts, Option<String>) {
    let outcome = store.load();
    let load_error = outcome.user_summary();
    let overrides = outcome.into_value();
    (catalog_with_overrides(overrides), load_error)
}

// Build the live catalog for an already-obtained override map.
//
// A boot whose read has not settled yet uses the default map, so the keyboard
// answers on the first frame without claiming the user has no overrides.
pub(crate) fn catalog_with_overrides(overrides: ShortcutOverrides) -> MailShortcuts {
    EditableShortcutCatalog::from_overrides(SHORTCUTS.clone(), overrides)
}

pub(crate) fn validate(candidate: &Shortcut) -> ShortcutValidity {
    use enzui_iced_core::shortcut::ShortcutKey;

    let mods = candidate.modifiers();
    if let ShortcutKey::Named(named) = candidate.key() {
        match named {
            Named::Escape | Named::Tab => {
                return ShortcutValidity::Invalid(
                    "Tab moves focus and Escape closes dialogs, so mail cannot take them."
                        .to_owned(),
                );
            }
            Named::Enter | Named::Space if mods == Modifier::NONE => {
                return ShortcutValidity::Invalid(
                    "Enter and Space activate whatever control has focus.".to_owned(),
                );
            }
            Named::F4 if mods == Modifier::ALT => {
                return ShortcutValidity::Reserved(
                    "your desktop closes windows with Alt+F4.".to_owned(),
                );
            }
            _ => {}
        }
    }
    if mods == Modifier::CTRL
        && let ShortcutKey::Character(key) = candidate.key()
        && let Some(reason) = desktop_reservation(key.as_str())
    {
        return ShortcutValidity::Reserved(reason.to_owned());
    }
    ShortcutValidity::Valid
}

fn desktop_reservation(key: &str) -> Option<&'static str> {
    Some(match key.to_ascii_lowercase().as_str() {
        "q" => "your desktop quits the application with Ctrl+Q.",
        "w" => "your desktop closes the window with Ctrl+W.",
        "c" => "the clipboard copies with Ctrl+C.",
        "x" => "the clipboard cuts with Ctrl+X.",
        "v" => "the clipboard pastes with Ctrl+V.",
        "a" => "text fields select all with Ctrl+A.",
        "z" => "text fields undo with Ctrl+Z.",
        _ => return None,
    })
}

// What the settings pane says when a user presses Backspace on a row.
//
// Mail's catalog has no unbound state, since an unbound row could not be
// discovered, so the refusal names the affordance that exists.
pub(crate) const CLEAR_REFUSED: &str =
    "Every mail action keeps a shortcut. Use Reset to restore the default.";

pub(crate) fn dispatch_in(
    bindings: &[ShortcutBinding<ShortcutAction, ShortcutContext>],
    event: &keyboard::Event,
    context: ShortcutContext,
) -> Option<ShortcutAction> {
    dispatch_shortcut_command(bindings.iter(), event, &context)
}

pub(crate) fn entries_for(
    bindings: &[ShortcutBinding<ShortcutAction, ShortcutContext>],
) -> Vec<ShortcutEntry> {
    shortcut_catalog_entries(bindings)
}

pub(crate) fn help_entries(catalog: &MailShortcuts) -> Vec<ShortcutEntry> {
    entries_for(catalog.resolved())
}
