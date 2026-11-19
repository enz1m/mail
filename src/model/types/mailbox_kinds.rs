#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) enum SidebarSelection {
    Favorite(FavoriteKind),
    SmartMailbox(super::super::sidebar_layout::SmartMailboxKind),
    AccountMailbox {
        account_id: String,
        mailbox: MailboxKind,
    },
    Folder {
        account_id: String,
        folder_id: String,
    },
}

impl Default for SidebarSelection {
    fn default() -> Self {
        SidebarSelection::Favorite(FavoriteKind::Inbox)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Copy)]
pub(crate) enum FavoriteKind {
    Inbox,
    Flagged,
    Drafts,
    Sent,
}

impl FavoriteKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            FavoriteKind::Inbox => "Inbox",
            FavoriteKind::Flagged => "Flagged",
            FavoriteKind::Drafts => "Drafts",
            FavoriteKind::Sent => "Sent",
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            FavoriteKind::Inbox => "inbox",
            FavoriteKind::Flagged => "flag",
            FavoriteKind::Drafts => "file-pen",
            FavoriteKind::Sent => "send",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Copy, Ord, PartialOrd)]
pub(crate) enum MailboxKind {
    Inbox,
    Drafts,
    Sent,
    Junk,
    Trash,
    Archive,
}

impl MailboxKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            MailboxKind::Inbox => "Inbox",
            MailboxKind::Drafts => "Drafts",
            MailboxKind::Sent => "Sent",
            MailboxKind::Junk => "Junk",
            MailboxKind::Trash => "Trash",
            MailboxKind::Archive => "Archive",
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            MailboxKind::Inbox => "inbox",
            MailboxKind::Drafts => "file-pen",
            MailboxKind::Sent => "send",
            MailboxKind::Junk => "shield-alert",
            MailboxKind::Trash => "trash-2",
            MailboxKind::Archive => "archive",
        }
    }
}
