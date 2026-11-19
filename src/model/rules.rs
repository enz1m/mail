//! Rules run in authored order and report distinct messages actually changed, not rule firings.

use std::collections::HashSet;

use super::types::{MailboxKind, MailboxStore, Message, MessageLocation};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(crate) enum ConditionField {
    #[default]
    From,
    Subject,
    Recipient,
}

impl ConditionField {
    pub(crate) fn label(self) -> &'static str {
        match self {
            ConditionField::From => "From",
            ConditionField::Subject => "Subject",
            ConditionField::Recipient => "To",
        }
    }

    pub(crate) const ALL: [ConditionField; 3] = [
        ConditionField::From,
        ConditionField::Subject,
        ConditionField::Recipient,
    ];

    // Out-of-range yields `None` rather than clamping, which would make two
    // inputs indistinguishable.
    pub(crate) fn from_index(i: usize) -> Option<Self> {
        Self::ALL.get(i).copied()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(crate) enum ConditionMatch {
    #[default]
    Contains,
    Is,
    BeginsWith,
    EndsWith,
}

impl ConditionMatch {
    pub(crate) const ALL: [ConditionMatch; 4] = [
        ConditionMatch::Contains,
        ConditionMatch::Is,
        ConditionMatch::BeginsWith,
        ConditionMatch::EndsWith,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            ConditionMatch::Contains => "contains",
            ConditionMatch::Is => "is",
            ConditionMatch::BeginsWith => "begins with",
            ConditionMatch::EndsWith => "ends with",
        }
    }

    pub(crate) fn from_index(i: usize) -> Option<Self> {
        Self::ALL.get(i).copied()
    }

    pub(crate) fn matches(self, haystack: &str, needle: &str) -> bool {
        let h = haystack.to_lowercase();
        let n = needle.to_lowercase();
        match self {
            ConditionMatch::Contains => h.contains(&n),
            ConditionMatch::Is => h == n,
            ConditionMatch::BeginsWith => h.starts_with(&n),
            ConditionMatch::EndsWith => h.ends_with(&n),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Condition {
    pub(crate) field: ConditionField,
    pub(crate) op: ConditionMatch,
    pub(crate) value: String,
}

impl Condition {
    pub(crate) fn new(field: ConditionField, op: ConditionMatch, value: impl Into<String>) -> Self {
        Self {
            field,
            op,
            value: value.into(),
        }
    }

    fn subject_text<'a>(&self, m: &'a Message) -> Vec<&'a str> {
        match self.field {
            ConditionField::From => vec![m.sender_name.as_str(), m.sender_email.as_str()],
            ConditionField::Subject => vec![m.subject.as_str()],
            ConditionField::Recipient => m.to.iter().map(String::as_str).collect(),
        }
    }

    pub(crate) fn matches(&self, m: &Message) -> bool {
        self.subject_text(m)
            .iter()
            .any(|t| self.op.matches(t, &self.value))
    }

    // Whether the value is non-empty; [`Rule::compiled_conditions`] drops
    // empty-valued conditions so a half-typed row cannot widen a rule.
    pub(crate) fn is_meaningful(&self) -> bool {
        !self.value.trim().is_empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Action {
    // Matching rules apply their actions in authored order.
    MoveTo(MailboxKind),
    SetFlagged(bool),
    SetUnread(bool),
    // Move to Trash; differs from `MoveTo(Trash)` only in label.
    Delete,
}

impl Action {
    // Labels for the action-kind selector, indexed by kind only.
    pub(crate) const ALL_KINDS: [&'static str; 4] =
        ["Move to mailbox", "Flag", "Mark as read", "Delete"];

    fn apply(&self, m: &mut Message) -> bool {
        match self {
            Action::MoveTo(kind) => set_location(m, *kind),
            Action::Delete => set_location(m, MailboxKind::Trash),
            Action::SetFlagged(f) => {
                if m.flagged != *f {
                    m.flagged = *f;
                    true
                } else {
                    false
                }
            }
            Action::SetUnread(u) => {
                if m.unread != *u {
                    m.unread = *u;
                    true
                } else {
                    false
                }
            }
        }
    }
}

// The mailbox kinds a `Move to` action can target, in selector-index order.
//
// The index is what the view sends the reducer; [`mailbox_for_index`] and
// [`mailbox_index`] are the only crossing points.
pub(crate) const ACTION_MAILBOXES: [MailboxKind; 4] = [
    MailboxKind::Archive,
    MailboxKind::Junk,
    MailboxKind::Trash,
    MailboxKind::Inbox,
];

// The mailbox for combo index `i`, or `None` out of range (never clamped).
pub(crate) fn mailbox_for_index(i: usize) -> Option<MailboxKind> {
    ACTION_MAILBOXES.get(i).copied()
}

pub(crate) fn mailbox_index(kind: MailboxKind) -> usize {
    ACTION_MAILBOXES
        .iter()
        .position(|k| *k == kind)
        .unwrap_or(0)
}

pub(crate) fn action_for_kind_index(i: usize) -> Action {
    match i {
        0 => Action::MoveTo(MailboxKind::Archive),
        1 => Action::SetFlagged(true),
        2 => Action::SetUnread(false),
        3 => Action::Delete,
        _ => Action::MoveTo(MailboxKind::Archive),
    }
}

pub(crate) fn action_kind_index(a: &Action) -> usize {
    match a {
        Action::MoveTo(_) => 0,
        Action::SetFlagged(_) => 1,
        Action::SetUnread(_) => 2,
        Action::Delete => 3,
    }
}

fn set_location(m: &mut Message, kind: MailboxKind) -> bool {
    let target = MessageLocation::Mailbox(kind);
    if m.location != target {
        m.location = target;
        true
    } else {
        false
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(crate) enum ConditionCombinator {
    // Every condition must match ("all of the following").
    #[default]
    All,
    Any,
}

impl ConditionCombinator {
    pub(crate) const ALL: [ConditionCombinator; 2] =
        [ConditionCombinator::All, ConditionCombinator::Any];

    pub(crate) fn label(self) -> &'static str {
        match self {
            ConditionCombinator::All => "all",
            ConditionCombinator::Any => "any",
        }
    }

    pub(crate) fn from_index(i: usize) -> Option<Self> {
        Self::ALL.get(i).copied()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Rule {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) enabled: bool,
    pub(crate) combinator: ConditionCombinator,
    pub(crate) conditions: Vec<Condition>,
    pub(crate) actions: Vec<Action>,
}

impl Rule {
    pub(crate) fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            enabled: true,
            combinator: ConditionCombinator::default(),
            conditions: Vec::new(),
            actions: Vec::new(),
        }
    }

    // Only the conditions with a non-empty value participate in matching.
    fn compiled_conditions(&self) -> Vec<&Condition> {
        self.conditions
            .iter()
            .filter(|c| c.is_meaningful())
            .collect()
    }

    // Whether `m` matches this rule.
    //
    // Zero meaningful conditions means match all, as in Apple Mail and
    // Thunderbird; otherwise a conditionless rule would never run.
    pub(crate) fn matches(&self, m: &Message) -> bool {
        let conds = self.compiled_conditions();
        if conds.is_empty() {
            return true;
        }
        match self.combinator {
            ConditionCombinator::All => conds.iter().all(|c| c.matches(m)),
            ConditionCombinator::Any => conds.iter().any(|c| c.matches(m)),
        }
    }

    // Whether the rule has actions. A rule without actions is valid but inert and
    // never counts as affecting a message.
    pub(crate) fn is_actionable(&self) -> bool {
        self.enabled && !self.actions.is_empty()
    }

    pub(crate) fn summary(&self) -> String {
        let mut arguments = enzui_iced_core::localization::Arguments::new();
        arguments.set("conditions", self.conditions.len());
        arguments.set("actions", self.actions.len());
        crate::l10n::current().text_with("mail-rule-summary", &arguments)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct RuleSet {
    pub(crate) rules: Vec<Rule>,
}

impl RuleSet {
    pub(crate) fn len(&self) -> usize {
        self.rules.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub(crate) fn position(&self, id: &str) -> Option<usize> {
        self.rules.iter().position(|r| r.id == id)
    }

    pub(crate) fn get(&self, id: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.id == id)
    }

    pub(crate) fn push(&mut self, rule: Rule) {
        self.rules.push(rule);
    }

    // The first repeated identity, if the set cannot name rules uniquely.
    pub(crate) fn duplicate_id(&self) -> Option<&str> {
        let mut seen = HashSet::new();
        self.rules
            .iter()
            .map(|rule| rule.id.as_str())
            .find(|id| !seen.insert(*id))
    }

    // Commit only when a new identity is unoccupied or an edit still has one
    // owner.
    pub(crate) fn commit_draft(&mut self, draft: RuleDraft) -> Result<(), RuleCommitError> {
        let matches = self
            .rules
            .iter()
            .filter(|rule| rule.id.as_str() == draft.rule.id.as_str())
            .count();
        if draft.is_new {
            if matches != 0 {
                return Err(RuleCommitError::IdentityOccupied);
            }
            self.rules.push(draft.rule);
            return Ok(());
        }
        if draft.original_id.as_deref() != Some(draft.rule.id.as_str()) || matches != 1 {
            return Err(RuleCommitError::EditOwnerUnavailable);
        }
        let Some(position) = self.position(&draft.rule.id) else {
            return Err(RuleCommitError::EditOwnerUnavailable);
        };
        self.rules[position] = draft.rule;
        Ok(())
    }

    pub(crate) fn remove(&mut self, id: &str) -> bool {
        if let Some(i) = self.position(id) {
            self.rules.remove(i);
            true
        } else {
            false
        }
    }

    pub(crate) fn move_up(&mut self, id: &str) -> bool {
        match self.position(id) {
            Some(i) if i > 0 => {
                self.rules.swap(i, i - 1);
                true
            }
            _ => false,
        }
    }

    pub(crate) fn move_down(&mut self, id: &str) -> bool {
        match self.position(id) {
            Some(i) if i + 1 < self.rules.len() => {
                self.rules.swap(i, i + 1);
                true
            }
            _ => false,
        }
    }

    pub(crate) fn move_to_slot(&mut self, id: &str, slot: usize) -> bool {
        let Some(from) = self.position(id) else {
            return false;
        };
        let clamped = slot.min(self.rules.len());
        let to = if clamped > from { clamped - 1 } else { clamped };
        if to == from {
            return false;
        }
        let rule = self.rules.remove(from);
        self.rules.insert(to, rule);
        true
    }
}

// A message changed by several rules counts once; matching without a change counts zero.
pub(crate) fn apply_rules(store: &mut MailboxStore, rules: &RuleSet) -> usize {
    store.mark_dirty();
    let mut affected: HashSet<String> = HashSet::new();
    for rule in &rules.rules {
        if !rule.is_actionable() {
            continue;
        }
        // Matching precedes mutation, so a rule cannot match a message it just moved
        // into range within its own pass.
        let matched: Vec<String> = store
            .messages
            .iter()
            .filter(|m| rule.matches(m))
            .map(|m| m.id.clone())
            .collect();
        for id in matched {
            if let Some(m) = store.messages.iter_mut().find(|m| m.id == id) {
                let mut changed = false;
                for action in &rule.actions {
                    changed |= action.apply(m);
                }
                if changed {
                    affected.insert(id);
                }
            }
        }
    }
    affected.len()
}

pub(crate) fn applied_rules_label(n: usize) -> String {
    crate::l10n::count("mail-rules-applied", n)
}

// A draft commit was refused because its identity cannot safely name one
// owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RuleCommitError {
    IdentityOccupied,
    EditOwnerUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RuleIdExhausted;

// A rule being created or edited before it is admitted to the ordered set.
//
// New drafts append only while their identity is unoccupied. Edit drafts
// retain their original identity and replace exactly that owner in place.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuleDraft {
    pub(crate) rule: Rule,
    pub(crate) is_new: bool,
    original_id: Option<String>,
}

impl RuleDraft {
    pub(crate) fn new_rule(id: impl Into<String>) -> Self {
        let mut rule = Rule::new(id, "New Rule");
        rule.conditions.push(Condition::new(
            ConditionField::From,
            ConditionMatch::Contains,
            String::new(),
        ));
        rule.actions.push(Action::MoveTo(MailboxKind::Archive));
        Self {
            rule,
            is_new: true,
            original_id: None,
        }
    }

    pub(crate) fn edit(rule: Rule) -> Self {
        let original_id = Some(rule.id.clone());
        Self {
            rule,
            is_new: false,
            original_id,
        }
    }

    pub(crate) fn add_condition(&mut self) {
        self.rule.conditions.push(Condition::new(
            ConditionField::From,
            ConditionMatch::Contains,
            String::new(),
        ));
    }

    pub(crate) fn remove_condition(&mut self, i: usize) {
        if i < self.rule.conditions.len() {
            self.rule.conditions.remove(i);
        }
    }

    pub(crate) fn add_action(&mut self) {
        self.rule.actions.push(Action::MoveTo(MailboxKind::Archive));
    }

    pub(crate) fn remove_action(&mut self, i: usize) {
        if i < self.rule.actions.len() {
            self.rule.actions.remove(i);
        }
    }

    pub(crate) fn title(&self) -> &'static str {
        if self.is_new { "New rule" } else { "Edit rule" }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct RulesUiState {
    pub(crate) open: bool,
    pub(crate) draft: Option<RuleDraft>,
    pub(crate) pending_delete: Option<String>,
    pub(crate) next_id: u64,
}

impl RulesUiState {
    pub(crate) fn open(&mut self) {
        self.open = true;
    }

    pub(crate) fn close(&mut self) {
        self.open = false;
        self.draft = None;
        self.pending_delete = None;
    }

    pub(crate) fn begin_new(&mut self) -> Result<(), RuleIdExhausted> {
        let next_id = self.next_id.checked_add(1).ok_or(RuleIdExhausted)?;
        let id = format!("rule-{next_id}");
        self.next_id = next_id;
        self.draft = Some(RuleDraft::new_rule(id));
        Ok(())
    }

    pub(crate) fn begin_edit(&mut self, rule: Rule) {
        self.draft = Some(RuleDraft::edit(rule));
    }

    pub(crate) fn cancel_edit(&mut self) {
        self.draft = None;
    }
}

pub(crate) fn demo_rules() -> RuleSet {
    let mut newsletters = Rule::new("rule-1", "File newsletters");
    newsletters.conditions.push(Condition::new(
        ConditionField::From,
        ConditionMatch::Contains,
        "newsletter",
    ));
    newsletters
        .actions
        .push(Action::MoveTo(MailboxKind::Archive));
    newsletters.actions.push(Action::SetUnread(false));

    let mut flag_boss = Rule::new("rule-2", "Flag the boss");
    flag_boss.conditions.push(Condition::new(
        ConditionField::From,
        ConditionMatch::Contains,
        "ceo@example.com",
    ));
    flag_boss.actions.push(Action::SetFlagged(true));

    RuleSet {
        rules: vec![newsletters, flag_boss],
    }
}

fn persisted_rule_sequence(id: &str) -> Option<u64> {
    let suffix = id.strip_prefix("rule-")?;
    if suffix.is_empty()
        || !suffix.bytes().all(|byte| byte.is_ascii_digit())
        || (suffix.len() > 1 && suffix.starts_with('0'))
    {
        return None;
    }
    suffix.parse().ok()
}

// The highest generated rule number in use, so new ids never collide.
pub(crate) fn next_rule_id_seed(rules: &RuleSet) -> u64 {
    rules
        .rules
        .iter()
        .filter_map(|rule| persisted_rule_sequence(&rule.id))
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
