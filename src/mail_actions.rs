use enzui_iced_core::action_overflow::{Descriptor, VisibilityPriority};
use enzui_iced_core::{Action, Icon};
use enzui_iced_widgets::action_bar::ActionItem;

use crate::Message as Msg;
use crate::model::types::{MailboxKind, MailboxStore, Message, SidebarSelection};

// What a [`MailAction`] acts on.
//
// Constructed by the surface that publishes the action, so the breadth of a
// verb is decided where the user's gesture happened rather than guessed in the
// reducer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ActionTarget {
    Message {
        message_id: String,
    },
    Conversation {
        conversation_id: String,
        newest_message_id: String,
    },
    Selection {
        conversation_ids: Vec<String>,
        // The newest message of the thread the gesture landed on, which
        // compose and sender verbs use — a reply to fifty threads at once is
        // not a thing, so those verbs stay singular even here.
        newest_message_id: String,
    },
}

impl ActionTarget {
    pub(crate) fn anchor_message_id(&self) -> &str {
        match self {
            ActionTarget::Message { message_id } => message_id,
            ActionTarget::Conversation {
                newest_message_id, ..
            }
            | ActionTarget::Selection {
                newest_message_id, ..
            } => newest_message_id,
        }
    }

    // How many threads the triage verbs will touch, or `None` when the target
    // is a single message and the count would be meaningless.
    //
    // Drives the plural labels: a `Delete` that will consume five threads must
    // say so before it is activated, not after.
    pub(crate) fn conversation_breadth(&self) -> Option<usize> {
        match self {
            ActionTarget::Message { .. } => None,
            ActionTarget::Conversation { .. } => Some(1),
            ActionTarget::Selection {
                conversation_ids, ..
            } => Some(conversation_ids.len()),
        }
    }
}

// Menu labels and their effects must derive from the same target snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ActionState {
    // Whether anything the target names is unread.
    //
    // If any named message is unread, the toggle marks the whole target read;
    // only a wholly-read target can be marked unread. This matches Apple Mail and
    // Thunderbird, and mark-read is the less damaging direction.
    pub(crate) any_unread: bool,
    pub(crate) any_flagged: bool,
    pub(crate) sender_blocked: bool,
    pub(crate) is_own_outgoing: bool,
    pub(crate) has_drafts: bool,
}

impl ActionState {
    pub(crate) fn of_message(
        store: &MailboxStore,
        block_list: &[String],
        message: &Message,
    ) -> Self {
        Self {
            any_unread: message.unread,
            any_flagged: message.flagged,
            sender_blocked: sender_is_blocked(block_list, &message.sender_email),
            is_own_outgoing: is_own_outgoing(store, message),
            has_drafts: matches!(
                message.location,
                crate::model::types::MessageLocation::Mailbox(MailboxKind::Drafts)
            ),
        }
    }

    // Triage covers the whole thread; sender actions use only the anchor message.
    pub(crate) fn for_conversation(
        store: &MailboxStore,
        selection: &SidebarSelection,
        block_list: &[String],
        conversation_id: &str,
    ) -> Option<Self> {
        let convo = store
            .conversations_for(selection)
            .into_iter()
            .find(|c| c.id == conversation_id)?;
        let anchor = store.message(convo.message_ids.last()?)?;
        Some(Self {
            any_unread: convo.unread_count > 0,
            any_flagged: convo.flagged_any,
            sender_blocked: sender_is_blocked(block_list, &anchor.sender_email),
            is_own_outgoing: is_own_outgoing(store, anchor),
            has_drafts: convo
                .message_ids
                .iter()
                .filter_map(|id| store.message(id))
                .any(|m| {
                    matches!(
                        m.location,
                        crate::model::types::MessageLocation::Mailbox(MailboxKind::Drafts)
                    )
                }),
        })
    }

    // Resolve the facts for a multi-selection.
    //
    // The read and flag facts are the union across the set, so a mixed
    // selection converges: any unread member means the set offers
    // `Mark as Read`. The sender facts come from the anchor thread.
    pub(crate) fn for_selection(
        store: &MailboxStore,
        selection: &SidebarSelection,
        block_list: &[String],
        conversation_ids: &[String],
        anchor_conversation_id: &str,
    ) -> Option<Self> {
        let convos = store.conversations_for(selection);
        let anchor = convos
            .iter()
            .find(|c| c.id == anchor_conversation_id)
            .and_then(|c| c.message_ids.last())
            .and_then(|id| store.message(id))?;
        let members: Vec<_> = convos
            .iter()
            .filter(|c| conversation_ids.iter().any(|id| id == &c.id))
            .collect();
        Some(Self {
            any_unread: members.iter().any(|c| c.unread_count > 0),
            any_flagged: members.iter().any(|c| c.flagged_any),
            sender_blocked: sender_is_blocked(block_list, &anchor.sender_email),
            is_own_outgoing: is_own_outgoing(store, anchor),
            has_drafts: members
                .iter()
                .flat_map(|c| &c.message_ids)
                .filter_map(|id| store.message(id))
                .any(|m| {
                    matches!(
                        m.location,
                        crate::model::types::MessageLocation::Mailbox(MailboxKind::Drafts)
                    )
                }),
        })
    }
}

// Whether `address` is on the user's block list, case-insensitively.
//
// Shared so the reader's pill and the context row cannot disagree about which
// side of the Block/Unblock toggle to show.
pub(crate) fn sender_is_blocked(block_list: &[String], address: &str) -> bool {
    block_list.iter().any(|b| b.eq_ignore_ascii_case(address))
}

// Whether `message` was sent from its own account's address.
//
// Compared against the owner of the message's own account, so a shared alias
// on a different account is still blockable.
pub(crate) fn is_own_outgoing(store: &MailboxStore, message: &Message) -> bool {
    store
        .account(&message.account_id)
        .is_some_and(|acc| acc.email.eq_ignore_ascii_case(&message.sender_email))
}

// One mail verb.
//
// Toggles are resolved variants rather than a verb plus a boolean: exactly one
// of `MarkRead`/`MarkUnread` reaches a surface, so a row cannot be labelled
// one way and act the other.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MailAction {
    Reply,
    ReplyAll,
    Forward,
    MarkRead,
    MarkUnread,
    Flag,
    Unflag,
    Archive,
    MoveTo,
    Snooze,
    Junk,
    Delete,
    BlockSender,
    UnblockSender,
}

// Which cluster a verb belongs to. Surfaces rule off between clusters, so a
// dropped verb cannot leave a dangling separator behind it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ActionGroup {
    Compose,
    Triage,
    // Verbs that act on the anchor message's sender rather than the mail.
    Sender,
}

impl MailAction {
    // Canonical verb IDs are shared by context menus and folded action bars.
    pub(crate) fn id(self) -> &'static str {
        match self {
            MailAction::Reply => "mail.reply",
            MailAction::ReplyAll => "mail.reply-all",
            MailAction::Forward => "mail.forward",
            MailAction::MarkRead => "mail.mark-read",
            MailAction::MarkUnread => "mail.mark-unread",
            MailAction::Flag => "mail.flag",
            MailAction::Unflag => "mail.unflag",
            MailAction::Archive => "mail.archive",
            MailAction::MoveTo => "mail.move-to",
            MailAction::Snooze => "mail.snooze",
            MailAction::Junk => "mail.junk",
            MailAction::Delete => "mail.delete",
            MailAction::BlockSender => "mail.block-sender",
            MailAction::UnblockSender => "mail.unblock-sender",
        }
    }

    pub(crate) fn group(self) -> ActionGroup {
        match self {
            MailAction::Reply | MailAction::ReplyAll | MailAction::Forward => ActionGroup::Compose,
            MailAction::MarkRead
            | MailAction::MarkUnread
            | MailAction::Flag
            | MailAction::Unflag
            | MailAction::Archive
            | MailAction::MoveTo
            | MailAction::Snooze
            | MailAction::Junk
            | MailAction::Delete => ActionGroup::Triage,
            MailAction::BlockSender | MailAction::UnblockSender => ActionGroup::Sender,
        }
    }

    pub(crate) fn bar_priority(self) -> u32 {
        match self {
            MailAction::Reply => 100,
            MailAction::ReplyAll => 90,
            MailAction::Forward => 80,
            MailAction::MarkRead | MailAction::MarkUnread => 75,
            MailAction::Flag | MailAction::Unflag => 70,
            MailAction::Snooze => 55,
            MailAction::Junk => 45,
            MailAction::Archive => 95,
            MailAction::MoveTo => 90,
            MailAction::Delete => 85,
            MailAction::BlockSender | MailAction::UnblockSender => 40,
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            MailAction::Reply => "reply",
            MailAction::ReplyAll => "reply-all",
            MailAction::Forward => "forward",
            MailAction::MarkRead | MailAction::MarkUnread => "mail-open",
            MailAction::Flag => "flag",
            MailAction::Unflag => "flag-off",
            MailAction::Archive => "archive",
            MailAction::MoveTo => "folder-input",
            MailAction::Snooze => "alarm-clock",
            MailAction::Junk => "shield-alert",
            MailAction::Delete => "trash-2",
            MailAction::BlockSender | MailAction::UnblockSender => "ban",
        }
    }

    // Whether the verb renders in the danger tone: `Delete` and `Block Sender`
    // only, since Junk and Archive are recoverable.
    pub(crate) fn destructive(self) -> bool {
        matches!(self, MailAction::Delete | MailAction::BlockSender)
    }

    // The label for this verb against `target`.
    //
    // Triage verbs acquire a count once the target names more than one thread,
    // so a destructive row states its breadth before it is activated. Compose
    // and sender verbs stay singular, because they only ever act on the anchor.
    pub(crate) fn label(self, target: &ActionTarget) -> String {
        let base = match self {
            MailAction::Reply => "Reply",
            MailAction::ReplyAll => "Reply All",
            MailAction::Forward => "Forward",
            MailAction::MarkRead => "Mark as Read",
            MailAction::MarkUnread => "Mark as Unread",
            MailAction::Flag => "Flag",
            MailAction::Unflag => "Unflag",
            MailAction::Archive => "Archive",
            MailAction::MoveTo => "Move to\u{2026}",
            MailAction::Snooze => "Snooze\u{2026}",
            MailAction::Junk => "Move to Junk",
            MailAction::Delete => "Delete",
            MailAction::BlockSender => "Block Sender",
            MailAction::UnblockSender => "Unblock Sender",
        };
        let base = crate::l10n::tr(base);
        match (self.group(), target.conversation_breadth()) {
            (ActionGroup::Triage, Some(n)) if n > 1 => crate::l10n::message(
                "mail-counted-action",
                &[("action", base), ("count", crate::l10n::number(n))],
            ),
            _ => base,
        }
    }

    pub(crate) fn message(self, target: &ActionTarget) -> Msg {
        Msg::MailAction(self, target.clone())
    }
}

// Multi-thread selections offer triage only; compose and sender commands require one anchor.
pub(crate) fn actions_for(state: ActionState, target: &ActionTarget) -> Vec<MailAction> {
    let mut out = vec![
        MailAction::Reply,
        MailAction::ReplyAll,
        MailAction::Forward,
        if state.any_unread {
            MailAction::MarkRead
        } else {
            MailAction::MarkUnread
        },
        if state.any_flagged {
            MailAction::Unflag
        } else {
            MailAction::Flag
        },
        MailAction::Archive,
        MailAction::MoveTo,
        MailAction::Snooze,
        MailAction::Junk,
        MailAction::Delete,
    ];
    if state.has_drafts {
        out.retain(|action| {
            !matches!(
                action,
                MailAction::Reply | MailAction::ReplyAll | MailAction::Snooze
            )
        });
    }
    if !state.is_own_outgoing {
        out.push(if state.sender_blocked {
            MailAction::UnblockSender
        } else {
            MailAction::BlockSender
        });
    }
    if target.conversation_breadth().is_some_and(|n| n > 1) {
        out.retain(|action| action.group() == ActionGroup::Triage);
    }
    out
}

// The mailbox a verb moves to, if it is a move at all.
//
// `MoveTo` is absent because it arms a picker rather than naming a
// destination; the reducer arm is what distinguishes the two.
pub(crate) fn move_destination(action: MailAction) -> Option<MailboxKind> {
    match action {
        MailAction::Archive => Some(MailboxKind::Archive),
        MailAction::Junk => Some(MailboxKind::Junk),
        MailAction::Delete => Some(MailboxKind::Trash),
        _ => None,
    }
}

// The action-bar group name for `group`.
//
// The bar keeps its own grouping so folded rows arrive in the overflow menu
// clustered the way the context menu rules them off.
fn bar_group(group: ActionGroup) -> &'static str {
    match group {
        ActionGroup::Compose => "mail-compose",
        ActionGroup::Triage => "mail-triage",
        ActionGroup::Sender => "mail-sender",
    }
}

// Build the [`ActionItem`]s for a folding action bar from the shared verb set.
//
// `inline` renders one verb; it is a callback rather than a fixed
// widget because the bar re-invokes it on every layout pass and `Element` is
// not `Clone`.
pub(crate) fn bar_items<F>(
    state: ActionState,
    target: &ActionTarget,
    include: impl Fn(MailAction) -> bool,
    inline: F,
) -> Vec<ActionItem<'static, Msg>>
where
    F: Fn(&Action<Msg>, MailAction) -> iced::Element<'static, Msg> + Clone + 'static,
{
    actions_for(state, target)
        .into_iter()
        .filter(|action| include(*action))
        .map(|action| {
            let built = Action::new(action.id(), action.message(target))
                .with_label(action.label(target))
                .with_icon(Icon::named(action.icon()));
            let inline = inline.clone();
            let item = ActionItem::with_inline(
                Descriptor::new(action.id())
                    .with_priority(VisibilityPriority(action.bar_priority())),
                built,
                move |built, _presentation| inline(built, action),
            );
            item.group(bar_group(action.group()))
                .destructive(action.destructive())
        })
        .collect()
}
