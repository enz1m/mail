//! Hiding a sidebar section preserves its saved membership and order.

use super::types::{Account, FavoriteKind, MailboxKind, Message, MessageDate, SidebarSelection};

// A rule-defined mailbox: a saved query over every account's mail rather
// than a place mail is stored.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum SmartMailboxKind {
    Today,
    Unread,
    Attachments,
    // Demo/legacy query retained so persisted layouts keep their meaning.
    FamilyPictures,
    // Conversations snoozed until later.
    Snoozed,
    Scheduled,
}

impl SmartMailboxKind {
    pub(crate) const ALL: [SmartMailboxKind; 6] = [
        SmartMailboxKind::Today,
        SmartMailboxKind::Unread,
        SmartMailboxKind::Attachments,
        SmartMailboxKind::FamilyPictures,
        SmartMailboxKind::Snoozed,
        SmartMailboxKind::Scheduled,
    ];

    pub(crate) const BUILT_INS: [SmartMailboxKind; 5] = [
        SmartMailboxKind::Today,
        SmartMailboxKind::Unread,
        SmartMailboxKind::Attachments,
        SmartMailboxKind::Snoozed,
        SmartMailboxKind::Scheduled,
    ];

    // The persisted token. Written out rather than derived from [`Self::label`]
    // so renaming a row in the UI cannot invalidate files already on disk.
    pub(crate) fn token(self) -> &'static str {
        match self {
            SmartMailboxKind::Today => "today",
            SmartMailboxKind::Unread => "unread",
            SmartMailboxKind::Attachments => "attachments",
            SmartMailboxKind::FamilyPictures => "family-pictures",
            SmartMailboxKind::Snoozed => "snoozed",
            SmartMailboxKind::Scheduled => "scheduled",
        }
    }

    // Inverse of [`Self::token`]. `None` for a token this build does not
    // know, which the reader treats as a row to skip rather than an error.
    pub(crate) fn from_token(t: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.token() == t)
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            SmartMailboxKind::Today => "Today",
            SmartMailboxKind::Unread => "Unread",
            SmartMailboxKind::Attachments => "Attachments",
            SmartMailboxKind::FamilyPictures => "Family Pictures",
            SmartMailboxKind::Snoozed => "Snoozed",
            SmartMailboxKind::Scheduled => "Scheduled",
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            SmartMailboxKind::Today => "sparkles",
            SmartMailboxKind::Unread => "mail",
            SmartMailboxKind::Attachments => "paperclip",
            SmartMailboxKind::FamilyPictures => "image",
            SmartMailboxKind::Snoozed => "alarm-clock",
            SmartMailboxKind::Scheduled => "clock",
        }
    }

    pub(crate) fn matches(self, m: &Message, today: MessageDate) -> bool {
        match self {
            SmartMailboxKind::Today => {
                m.date.year == today.year && m.date.month == today.month && m.date.day == today.day
            }
            SmartMailboxKind::Unread => {
                m.unread
                    && !matches!(
                        m.location,
                        super::types::MessageLocation::Mailbox(
                            super::types::MailboxKind::Trash
                                | super::types::MailboxKind::Junk
                                | super::types::MailboxKind::Sent
                                | super::types::MailboxKind::Drafts
                        )
                    )
            }
            SmartMailboxKind::Attachments => m.has_attachment,
            SmartMailboxKind::FamilyPictures => {
                m.has_attachment && m.subject.to_lowercase().contains("family")
            }
            SmartMailboxKind::Snoozed => {
                m.snoozed_until.is_some()
                    && !matches!(
                        m.location,
                        super::types::MessageLocation::Mailbox(
                            super::types::MailboxKind::Trash | super::types::MailboxKind::Junk
                        )
                    )
            }
            SmartMailboxKind::Scheduled => {
                m.send_at.is_some()
                    && matches!(
                        m.location,
                        super::types::MessageLocation::Mailbox(super::types::MailboxKind::Drafts)
                    )
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) enum FavoriteTarget {
    Unified(FavoriteKind),
    Mailbox {
        account_id: String,
        mailbox: MailboxKind,
    },
    Folder {
        account_id: String,
        folder_id: String,
    },
}

impl FavoriteTarget {
    pub(crate) fn selection(&self) -> SidebarSelection {
        match self {
            FavoriteTarget::Unified(kind) => SidebarSelection::Favorite(*kind),
            FavoriteTarget::Mailbox {
                account_id,
                mailbox,
            } => SidebarSelection::AccountMailbox {
                account_id: account_id.clone(),
                mailbox: *mailbox,
            },
            FavoriteTarget::Folder {
                account_id,
                folder_id,
            } => SidebarSelection::Folder {
                account_id: account_id.clone(),
                folder_id: folder_id.clone(),
            },
        }
    }

    // The favourite that would pin `sel`, or `None` when `sel` cannot be one.
    //
    // Only a smart mailbox returns `None`: it is this app's own saved query
    // and owns the section below.
    pub(crate) fn from_selection(sel: &SidebarSelection) -> Option<Self> {
        match sel {
            SidebarSelection::Favorite(kind) => Some(FavoriteTarget::Unified(*kind)),
            SidebarSelection::AccountMailbox {
                account_id,
                mailbox,
            } => Some(FavoriteTarget::Mailbox {
                account_id: account_id.clone(),
                mailbox: *mailbox,
            }),
            SidebarSelection::Folder {
                account_id,
                folder_id,
            } => Some(FavoriteTarget::Folder {
                account_id: account_id.clone(),
                folder_id: folder_id.clone(),
            }),
            SidebarSelection::SmartMailbox(_) => None,
        }
    }

    // Unified favorites survive an empty account list; account-owned pins require a live destination.
    pub(crate) fn resolves_in(&self, accounts: &[Account]) -> bool {
        match self {
            FavoriteTarget::Unified(_) => true,
            FavoriteTarget::Mailbox {
                account_id,
                mailbox,
            } => accounts
                .iter()
                .any(|a| &a.id == account_id && a.mailboxes.contains(mailbox)),
            FavoriteTarget::Folder {
                account_id,
                folder_id,
            } => accounts
                .iter()
                .any(|a| &a.id == account_id && a.folders.iter().any(|f| &f.id == folder_id)),
        }
    }

    pub(crate) fn label_in<'a>(&'a self, accounts: &'a [Account]) -> &'a str {
        match self {
            FavoriteTarget::Unified(kind) => kind.label(),
            FavoriteTarget::Mailbox { mailbox, .. } => mailbox.label(),
            FavoriteTarget::Folder {
                account_id,
                folder_id,
            } => accounts
                .iter()
                .find(|a| &a.id == account_id)
                .and_then(|a| a.folders.iter().find(|f| &f.id == folder_id))
                .map_or(folder_id.as_str(), |f| f.name.as_str()),
        }
    }

    pub(crate) fn display_label_in(&self, accounts: &[Account]) -> String {
        match self {
            Self::Unified(kind) => crate::l10n::tr(kind.label()),
            Self::Mailbox { mailbox, .. } => crate::l10n::tr(mailbox.label()),
            Self::Folder { .. } => self.label_in(accounts).to_owned(),
        }
    }

    pub(crate) fn account_email_in<'a>(&self, accounts: &'a [Account]) -> Option<&'a str> {
        match self {
            FavoriteTarget::Unified(_) => None,
            FavoriteTarget::Mailbox { account_id, .. }
            | FavoriteTarget::Folder { account_id, .. } => accounts
                .iter()
                .find(|a| &a.id == account_id)
                .map(|a| a.email.as_str()),
        }
    }

    pub(crate) fn icon(&self) -> &'static str {
        match self {
            FavoriteTarget::Unified(kind) => kind.icon(),
            FavoriteTarget::Mailbox { mailbox, .. } => mailbox.icon(),
            FavoriteTarget::Folder { .. } => "folder",
        }
    }

    pub(crate) fn key(&self) -> String {
        match self {
            FavoriteTarget::Unified(kind) => {
                format!("unified:{}", favorite_kind_identity(*kind))
            }
            FavoriteTarget::Mailbox {
                account_id,
                mailbox,
            } => format!(
                "mailbox:{}:{}",
                encode_identity_component(account_id),
                mailbox_identity(*mailbox)
            ),
            FavoriteTarget::Folder {
                account_id,
                folder_id,
            } => format!(
                "folder:{}:{}",
                encode_identity_component(account_id),
                encode_identity_component(folder_id)
            ),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SidebarLayout {
    favorites_visible: bool,
    smart_visible: bool,
    favorites: Vec<FavoriteTarget>,
    smart: Vec<SmartMailboxKind>,
}

impl Default for SidebarLayout {
    fn default() -> Self {
        Self::with_smart(vec![SmartMailboxKind::Today, SmartMailboxKind::Unread])
    }
}

impl SidebarLayout {
    pub(crate) fn demo_fixture() -> Self {
        Self::with_smart(vec![
            SmartMailboxKind::Today,
            SmartMailboxKind::FamilyPictures,
        ])
    }

    fn with_smart(smart: Vec<SmartMailboxKind>) -> Self {
        Self {
            favorites_visible: true,
            smart_visible: true,
            favorites: vec![
                FavoriteTarget::Unified(FavoriteKind::Inbox),
                FavoriteTarget::Unified(FavoriteKind::Flagged),
                FavoriteTarget::Unified(FavoriteKind::Drafts),
                FavoriteTarget::Unified(FavoriteKind::Sent),
            ],
            smart,
        }
    }

    pub(crate) fn from_parts(
        favorites_visible: bool,
        smart_visible: bool,
        favorites: Vec<FavoriteTarget>,
        smart: Vec<SmartMailboxKind>,
    ) -> Self {
        let mut layout = Self {
            favorites_visible,
            smart_visible,
            favorites: Vec::new(),
            smart: Vec::new(),
        };
        for f in favorites {
            if !layout.favorites.contains(&f) {
                layout.favorites.push(f);
            }
        }
        for k in smart {
            if !layout.smart.contains(&k) {
                layout.smart.push(k);
            }
        }
        layout
    }

    pub(crate) fn favorites_visible(&self) -> bool {
        self.favorites_visible
    }

    pub(crate) fn smart_visible(&self) -> bool {
        self.smart_visible
    }

    // The favourites, in order. Membership only — a caller that also needs
    // the section hidden must check [`Self::favorites_visible`].
    pub(crate) fn favorites(&self) -> &[FavoriteTarget] {
        &self.favorites
    }

    pub(crate) fn smart(&self) -> &[SmartMailboxKind] {
        &self.smart
    }

    pub(crate) fn visible_favorites(&self) -> &[FavoriteTarget] {
        if self.favorites_visible {
            &self.favorites
        } else {
            &[]
        }
    }

    pub(crate) fn visible_smart(&self) -> &[SmartMailboxKind] {
        if self.smart_visible { &self.smart } else { &[] }
    }

    pub(crate) fn visible_selections(&self) -> Vec<SidebarSelection> {
        self.visible_favorites()
            .iter()
            .map(FavoriteTarget::selection)
            .chain(
                self.visible_smart()
                    .iter()
                    .copied()
                    .map(SidebarSelection::SmartMailbox),
            )
            .collect()
    }

    pub(crate) fn set_favorites_visible(&mut self, visible: bool) {
        self.favorites_visible = visible;
    }

    pub(crate) fn set_smart_visible(&mut self, visible: bool) {
        self.smart_visible = visible;
    }

    pub(crate) fn is_favorite(&self, sel: &SidebarSelection) -> bool {
        FavoriteTarget::from_selection(sel).is_some_and(|t| self.favorites.contains(&t))
    }

    // Pin `sel` to the end of Favorites, preserving the user's existing order.
    // Returns whether anything changed.
    pub(crate) fn add_favorite(&mut self, sel: &SidebarSelection) -> bool {
        let Some(target) = FavoriteTarget::from_selection(sel) else {
            return false;
        };
        if self.favorites.contains(&target) {
            return false;
        }
        self.favorites.push(target);
        true
    }

    pub(crate) fn remove_favorite(&mut self, sel: &SidebarSelection) -> bool {
        let Some(target) = FavoriteTarget::from_selection(sel) else {
            return false;
        };
        let before = self.favorites.len();
        self.favorites.retain(|t| t != &target);
        self.favorites.len() != before
    }

    pub(crate) fn remove_smart(&mut self, kind: SmartMailboxKind) -> bool {
        let before = self.smart.len();
        self.smart.retain(|k| *k != kind);
        self.smart.len() != before
    }

    pub(crate) fn add_smart(&mut self, kind: SmartMailboxKind) -> bool {
        if self.smart.contains(&kind) {
            return false;
        }
        self.smart.push(kind);
        true
    }

    pub(crate) fn reorder_favorites(
        &mut self,
        request: &enzui_iced_core::drag::DropRequest,
    ) -> bool {
        reorder_items(
            &mut self.favorites,
            request,
            FAVORITES_COLLECTION_ID,
            FavoriteTarget::key,
        )
    }

    pub(crate) fn reorder_smart(&mut self, request: &enzui_iced_core::drag::DropRequest) -> bool {
        reorder_items(&mut self.smart, request, SMART_COLLECTION_ID, |kind| {
            kind.token().to_owned()
        })
    }

    pub(crate) fn prune(&mut self, accounts: &[Account]) -> bool {
        let before = self.favorites.len();
        self.favorites.retain(|t| t.resolves_in(accounts));
        self.favorites.len() != before
    }
}

pub(crate) const FAVORITES_COLLECTION_ID: &str = "mail-settings-favorites";
pub(crate) const SMART_COLLECTION_ID: &str = "mail-settings-smart";

fn encode_identity_component(value: &str) -> String {
    use std::fmt::Write;

    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value.as_bytes() {
        write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

fn favorite_kind_identity(kind: FavoriteKind) -> &'static str {
    match kind {
        FavoriteKind::Inbox => "inbox",
        FavoriteKind::Flagged => "flagged",
        FavoriteKind::Drafts => "drafts",
        FavoriteKind::Sent => "sent",
    }
}

fn mailbox_identity(mailbox: MailboxKind) -> &'static str {
    match mailbox {
        MailboxKind::Inbox => "inbox",
        MailboxKind::Drafts => "drafts",
        MailboxKind::Sent => "sent",
        MailboxKind::Junk => "junk",
        MailboxKind::Trash => "trash",
        MailboxKind::Archive => "archive",
    }
}

fn reorder_items<T: Clone>(
    items: &mut Vec<T>,
    request: &enzui_iced_core::drag::DropRequest,
    collection_id: &str,
    key: impl Fn(&T) -> String,
) -> bool {
    use enzui_iced_core::drag::{CollectionId, DragOperation, ItemId, reorder};

    let collection = CollectionId::new(collection_id);
    if request.source != collection
        || request.destination != collection
        || request.operation != DragOperation::Move
    {
        return false;
    }

    let order: Vec<ItemId> = items.iter().map(|item| ItemId::new(key(item))).collect();
    let Ok(diff) = reorder(&order, &request.items, request.slot) else {
        return false;
    };
    if !diff.changed {
        return false;
    }

    let Some(reordered) = diff
        .order
        .iter()
        .map(|id| items.iter().find(|item| key(item) == id.as_str()).cloned())
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    *items = reordered;
    true
}
