use std::fmt::Write as _;

use super::{
    PREFERENCES_ACTIONS_PER_RULE_MAX, PREFERENCES_ALLOW_LIST_MAX, PREFERENCES_BLOCK_LIST_MAX,
    PREFERENCES_BODY_MAX, PREFERENCES_CONDITIONS_PER_RULE_MAX, PREFERENCES_FAVORITES_MAX,
    PREFERENCES_FIELD_DECODED_BYTES_MAX, PREFERENCES_IDENTITY_BYTES_MAX, PREFERENCES_RECORDS_MAX,
    PREFERENCES_RETAINED_BYTES_MAX, PREFERENCES_RULES_MAX, PREFERENCES_SMART_MAILBOXES_MAX,
    Preferences, SEP, admit_next, bool_from_token, bool_token, combinator_from_token,
    combinator_token, compose_format_from_token, compose_format_token, escape, escaped_field_bytes,
    favorite_fields, favorite_from_fields, field_from_token, field_token, mailbox_from_token,
    mailbox_token, match_from_token, match_token, policy_from_token, policy_token, unescape,
    validate_encoded_preferences,
};
use crate::model::compose::ComposeFormat;
use crate::model::privacy::RemoteContentPolicy;
use crate::model::rules::{Action, Condition, Rule, RuleSet};
use crate::model::sidebar_layout::{FavoriteTarget, SidebarLayout, SmartMailboxKind};
use enzui_iced_core::persist::Persistable;

#[derive(Default)]
struct PreferencesBudget {
    records: usize,
    retained_bytes: usize,
}

impl PreferencesBudget {
    fn records(&mut self, additional: usize) -> Result<(), String> {
        self.records = self
            .records
            .checked_add(additional)
            .ok_or_else(|| "preferences record count overflow".to_owned())?;
        if self.records > PREFERENCES_RECORDS_MAX {
            return Err(format!(
                "preferences has too many records: {} exceeds {PREFERENCES_RECORDS_MAX}",
                self.records
            ));
        }
        Ok(())
    }

    fn field(&mut self, value: &str, name: &str) -> Result<(), String> {
        self.field_with_limit(value, PREFERENCES_FIELD_DECODED_BYTES_MAX, name)
    }

    fn identity(&mut self, value: &str, name: &str) -> Result<(), String> {
        self.field_with_limit(value, PREFERENCES_IDENTITY_BYTES_MAX, name)
    }

    fn field_with_limit(&mut self, value: &str, maximum: usize, name: &str) -> Result<(), String> {
        if value.len() > maximum {
            return Err(format!("{name} is too large"));
        }
        let encoded_maximum = maximum
            .checked_mul(2)
            .ok_or_else(|| "preferences encoded field limit overflow".to_owned())?;
        let encoded = escaped_field_bytes(value)
            .ok_or_else(|| "preferences encoded field length overflow".to_owned())?;
        if encoded > encoded_maximum {
            return Err(format!("encoded {name} is too large"));
        }
        self.retained_bytes = self
            .retained_bytes
            .checked_add(value.len())
            .ok_or_else(|| "preferences retained byte count overflow".to_owned())?;
        if self.retained_bytes > PREFERENCES_RETAINED_BYTES_MAX {
            return Err("preferences retained strings exceed the aggregate limit".to_owned());
        }
        Ok(())
    }
}

fn validate_count(actual: usize, maximum: usize, name: &str) -> Result<(), String> {
    if actual > maximum {
        Err(format!("too many {name}: {actual} exceeds {maximum}"))
    } else {
        Ok(())
    }
}

fn validate_preferences_domain(preferences: &Preferences) -> Result<(), String> {
    preferences.notifications.validate()?;
    let mut budget = PreferencesBudget::default();
    budget.records(1 + preferences.notifications.overrides.len())?;
    for target in preferences.notifications.overrides.keys() {
        budget.field(&target.account_id, "notification account")?;
        if let crate::model::types::MessageLocation::Folder(id) = &target.location {
            budget.field(id, "notification folder")?;
        }
    }
    budget.records(3)?;
    validate_count(
        preferences.allow_list.len(),
        PREFERENCES_ALLOW_LIST_MAX,
        "allowed senders",
    )?;
    validate_count(
        preferences.block_list.len(),
        PREFERENCES_BLOCK_LIST_MAX,
        "blocked senders",
    )?;
    validate_count(
        preferences.rules.rules.len(),
        PREFERENCES_RULES_MAX,
        "rules",
    )?;
    validate_count(
        preferences.sidebar_layout.favorites().len(),
        PREFERENCES_FAVORITES_MAX,
        "sidebar favorites",
    )?;
    validate_count(
        preferences.sidebar_layout.smart().len(),
        PREFERENCES_SMART_MAILBOXES_MAX,
        "smart mailboxes",
    )?;
    budget.records(preferences.allow_list.len())?;
    budget.records(preferences.block_list.len())?;
    budget.records(preferences.sidebar_layout.favorites().len())?;
    budget.records(preferences.sidebar_layout.smart().len())?;

    for sender in &preferences.allow_list {
        budget.field(sender, "allowed sender")?;
    }
    for sender in &preferences.block_list {
        budget.field(sender, "blocked sender")?;
    }
    for favorite in preferences.sidebar_layout.favorites() {
        match favorite {
            FavoriteTarget::Unified(_) => {}
            FavoriteTarget::Mailbox { account_id, .. } => {
                budget.identity(account_id, "favorite account identity")?;
            }
            FavoriteTarget::Folder {
                account_id,
                folder_id,
            } => {
                budget.identity(account_id, "favorite account identity")?;
                budget.identity(folder_id, "favorite folder identity")?;
            }
        }
    }
    for rule in &preferences.rules.rules {
        validate_count(
            rule.conditions.len(),
            PREFERENCES_CONDITIONS_PER_RULE_MAX,
            "rule conditions",
        )?;
        validate_count(
            rule.actions.len(),
            PREFERENCES_ACTIONS_PER_RULE_MAX,
            "rule actions",
        )?;
        budget.records(1)?;
        budget.records(rule.conditions.len())?;
        budget.records(rule.actions.len())?;
        budget.identity(&rule.id, "rule identity")?;
        budget.field(&rule.name, "rule name")?;
        for condition in &rule.conditions {
            budget.field(&condition.value, "rule condition value")?;
        }
    }
    Ok(())
}

impl Persistable for Preferences {
    fn persist_serialize(&self) -> Result<String, String> {
        validate_preferences_domain(self)?;
        let mut out = String::new();
        writeln!(&mut out, "policy{SEP}{}", policy_token(self.policy))
            .map_err(|_| "preferences formatting failed".to_owned())?;
        for addr in &self.allow_list {
            writeln!(&mut out, "allow{SEP}{}", escape(addr))
                .map_err(|_| "preferences formatting failed".to_owned())?;
        }
        for addr in &self.block_list {
            writeln!(&mut out, "block{SEP}{}", escape(addr))
                .map_err(|_| "preferences formatting failed".to_owned())?;
        }
        // The marker distinguishes an explicitly empty layout from a document
        // that has no sidebar-layout records.
        writeln!(
            &mut out,
            "sections{SEP}{}{SEP}{}",
            bool_token(self.sidebar_layout.favorites_visible()),
            bool_token(self.sidebar_layout.smart_visible()),
        )
        .map_err(|_| "preferences formatting failed".to_owned())?;
        writeln!(
            &mut out,
            "composeformat{SEP}{}",
            compose_format_token(self.compose_format)
        )
        .map_err(|_| "preferences formatting failed".to_owned())?;
        writeln!(
            &mut out,
            "readerplain{SEP}{}",
            bool_token(self.prefer_plain_text)
        )
        .map_err(|_| "preferences formatting failed".to_owned())?;
        writeln!(
            &mut out,
            "notifications{SEP}{}{SEP}{}{SEP}{}",
            bool_token(self.notifications.enabled),
            bool_token(self.notifications.all_mailboxes),
            self.notifications.preview.token()
        )
        .map_err(|_| "notification formatting failed".to_owned())?;
        let mut notification_rules = self
            .notifications
            .overrides
            .iter()
            .map(|(target, enabled)| {
                let (kind, value) = match &target.location {
                    crate::model::types::MessageLocation::Mailbox(mailbox) => {
                        ("mailbox", mailbox_token(*mailbox).to_owned())
                    }
                    crate::model::types::MessageLocation::Folder(id) => ("folder", id.clone()),
                };
                format!(
                    "notify-mailbox{SEP}{}{SEP}{kind}{SEP}{}{SEP}{}",
                    escape(&target.account_id),
                    escape(&value),
                    bool_token(*enabled)
                )
            })
            .collect::<Vec<_>>();
        notification_rules.sort();
        for record in notification_rules {
            writeln!(&mut out, "{record}")
                .map_err(|_| "notification formatting failed".to_owned())?;
        }
        writeln!(&mut out, "darkhtml{SEP}{}", bool_token(self.dark_html))
            .map_err(|_| "preferences formatting failed".to_owned())?;
        for target in self.sidebar_layout.favorites() {
            writeln!(&mut out, "favorite{SEP}{}", favorite_fields(target))
                .map_err(|_| "preferences formatting failed".to_owned())?;
        }
        for kind in self.sidebar_layout.smart() {
            writeln!(&mut out, "smart{SEP}{}", kind.token())
                .map_err(|_| "preferences formatting failed".to_owned())?;
        }
        if let Some(id) = self.rules.duplicate_id() {
            return Err(format!("duplicate rule id {id:?}"));
        }
        for rule in &self.rules.rules {
            writeln!(
                &mut out,
                "rule{SEP}{}{SEP}{}{SEP}{}{SEP}{}",
                escape(&rule.id),
                bool_token(rule.enabled),
                combinator_token(rule.combinator),
                escape(&rule.name),
            )
            .map_err(|_| "preferences formatting failed".to_owned())?;
            for c in &rule.conditions {
                writeln!(
                    &mut out,
                    "cond{SEP}{}{SEP}{}{SEP}{}",
                    field_token(c.field),
                    match_token(c.op),
                    escape(&c.value),
                )
                .map_err(|_| "preferences formatting failed".to_owned())?;
            }
            for a in &rule.actions {
                match a {
                    Action::MoveTo(kind) => {
                        writeln!(&mut out, "action{SEP}move{SEP}{}", mailbox_token(*kind))
                    }
                    Action::SetFlagged(value) => {
                        writeln!(&mut out, "action{SEP}flag{SEP}{}", bool_token(*value))
                    }
                    Action::SetUnread(value) => {
                        writeln!(&mut out, "action{SEP}unread{SEP}{}", bool_token(*value))
                    }
                    Action::Delete => writeln!(&mut out, "action{SEP}delete"),
                }
                .map_err(|_| "preferences formatting failed".to_owned())?;
            }
        }
        let body_bytes = u64::try_from(out.len())
            .map_err(|_| "serialized preferences body length overflow".to_owned())?;
        if body_bytes > PREFERENCES_BODY_MAX {
            return Err("serialized preferences exceed their derived body ceiling".to_owned());
        }
        Ok(out)
    }

    fn persist_deserialize(body: &str) -> Result<Self, String> {
        validate_encoded_preferences(body)?;
        // Start from an empty document, not `persist_default`: a stored empty
        // allow, block or rule list is authoritative and must survive the read.
        let mut prefs = Preferences {
            rules: RuleSet::default(),
            policy: RemoteContentPolicy::default(),
            allow_list: Vec::new(),
            block_list: Vec::new(),
            sidebar_layout: SidebarLayout::default(),
            compose_format: ComposeFormat::default(),
            prefer_plain_text: false,
            dark_html: false,
            notifications: Default::default(),
        };
        // The section marker distinguishes absent records from empty lists.
        let mut sections: Option<(bool, bool)> = None;
        let mut favorites: Vec<FavoriteTarget> = Vec::new();
        let mut smart: Vec<SmartMailboxKind> = Vec::new();
        let mut rule_ids = std::collections::HashSet::new();
        for (n, line) in body.lines().enumerate() {
            let line = line.trim_end_matches(['\r']);
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split(SEP);
            let keyword = parts.next().unwrap_or("");
            let rest: Vec<&str> = parts.collect();
            let lineno = n + 1;
            match keyword {
                "policy" => {
                    let token = rest
                        .first()
                        .ok_or_else(|| format!("line {lineno}: policy record has no value"))?;
                    prefs.policy = policy_from_token(token)
                        .ok_or_else(|| format!("line {lineno}: unknown policy {token:?}"))?;
                }
                "allow" => {
                    admit_next(
                        prefs.allow_list.len(),
                        PREFERENCES_ALLOW_LIST_MAX,
                        "allowed senders",
                        lineno,
                    )?;
                    let v = rest
                        .first()
                        .ok_or_else(|| format!("line {lineno}: allow record has no address"))?;
                    prefs.allow_list.push(unescape(v));
                }
                "notifications" => {
                    if rest.len() != 3 {
                        return Err(format!("line {lineno}: invalid notification settings"));
                    }
                    prefs.notifications.enabled = bool_from_token(rest[0])
                        .ok_or_else(|| format!("line {lineno}: invalid notification setting"))?;
                    prefs.notifications.all_mailboxes =
                        bool_from_token(rest[1]).ok_or_else(|| {
                            format!("line {lineno}: invalid mailbox notification default")
                        })?;
                    prefs.notifications.preview =
                        crate::notification_settings::Preview::parse(rest[2]).ok_or_else(|| {
                            format!("line {lineno}: invalid notification preview")
                        })?;
                }
                "notify-mailbox" => {
                    if rest.len() != 4 || prefs.notifications.overrides.len() >= 512 {
                        return Err(format!("line {lineno}: invalid notification override"));
                    }
                    let account_id = unescape(rest[0]);
                    let value = unescape(rest[2]);
                    let location = match rest[1] {
                        "mailbox" => crate::model::types::MessageLocation::Mailbox(
                            mailbox_from_token(&value).ok_or_else(|| {
                                format!("line {lineno}: invalid notification mailbox")
                            })?,
                        ),
                        "folder" => crate::model::types::MessageLocation::Folder(value),
                        _ => {
                            return Err(format!(
                                "line {lineno}: invalid notification mailbox type"
                            ));
                        }
                    };
                    let enabled = bool_from_token(rest[3])
                        .ok_or_else(|| format!("line {lineno}: invalid notification override"))?;
                    if prefs
                        .notifications
                        .overrides
                        .insert(
                            crate::notification_settings::MailboxTarget {
                                account_id,
                                location,
                            },
                            enabled,
                        )
                        .is_some()
                    {
                        return Err(format!("line {lineno}: duplicate notification mailbox"));
                    }
                }
                "darkhtml" => {
                    prefs.dark_html = rest
                        .first()
                        .and_then(|t| bool_from_token(t))
                        .ok_or_else(|| format!("line {lineno}: invalid dark HTML setting"))?;
                }
                "readerplain" => {
                    prefs.prefer_plain_text = rest
                        .first()
                        .and_then(|t| bool_from_token(t))
                        .ok_or_else(|| format!("line {lineno}: invalid reading format"))?;
                }
                "composeformat" => {
                    // An unreadable value keeps the default rather than
                    // rejecting a document that also carries rules and lists.
                    if let Some(format) = rest.first().and_then(|t| compose_format_from_token(t)) {
                        prefs.compose_format = format;
                    }
                }
                "block" => {
                    admit_next(
                        prefs.block_list.len(),
                        PREFERENCES_BLOCK_LIST_MAX,
                        "blocked senders",
                        lineno,
                    )?;
                    let v = rest
                        .first()
                        .ok_or_else(|| format!("line {lineno}: block record has no address"))?;
                    prefs.block_list.push(unescape(v));
                }
                "rule" => {
                    admit_next(
                        prefs.rules.rules.len(),
                        PREFERENCES_RULES_MAX,
                        "rules",
                        lineno,
                    )?;
                    if rest.len() < 4 {
                        return Err(format!(
                            "line {lineno}: rule record needs id, enabled, combinator, name"
                        ));
                    }
                    let combinator = combinator_from_token(rest[2]).ok_or_else(|| {
                        format!("line {lineno}: unknown combinator {:?}", rest[2])
                    })?;
                    let enabled = bool_from_token(rest[1])
                        .ok_or_else(|| format!("line {lineno}: unknown enabled {:?}", rest[1]))?;
                    let id = unescape(rest[0]);
                    if !rule_ids.insert(id.clone()) {
                        return Err(format!("line {lineno}: duplicate rule id {id:?}"));
                    }
                    let mut rule = Rule::new(id, unescape(rest[3]));
                    rule.enabled = enabled;
                    rule.combinator = combinator;
                    prefs.rules.push(rule);
                }
                "cond" => {
                    let rule = prefs.rules.rules.last_mut().ok_or_else(|| {
                        format!("line {lineno}: cond record before any rule record")
                    })?;
                    admit_next(
                        rule.conditions.len(),
                        PREFERENCES_CONDITIONS_PER_RULE_MAX,
                        "rule conditions",
                        lineno,
                    )?;
                    if rest.len() < 3 {
                        return Err(format!("line {lineno}: cond record needs field, op, value"));
                    }
                    let field = field_from_token(rest[0])
                        .ok_or_else(|| format!("line {lineno}: unknown field {:?}", rest[0]))?;
                    let op = match_from_token(rest[1])
                        .ok_or_else(|| format!("line {lineno}: unknown match {:?}", rest[1]))?;
                    rule.conditions
                        .push(Condition::new(field, op, unescape(rest[2])));
                }
                "action" => {
                    let rule = prefs.rules.rules.last_mut().ok_or_else(|| {
                        format!("line {lineno}: action record before any rule record")
                    })?;
                    admit_next(
                        rule.actions.len(),
                        PREFERENCES_ACTIONS_PER_RULE_MAX,
                        "rule actions",
                        lineno,
                    )?;
                    let kind = rest
                        .first()
                        .ok_or_else(|| format!("line {lineno}: action record has no kind"))?;
                    let arg = rest.get(1).copied();
                    let action = match (*kind, arg) {
                        ("move", Some(m)) => Action::MoveTo(
                            mailbox_from_token(m)
                                .ok_or_else(|| format!("line {lineno}: unknown mailbox {m:?}"))?,
                        ),
                        ("flag", Some(v)) => Action::SetFlagged(
                            bool_from_token(v)
                                .ok_or_else(|| format!("line {lineno}: unknown flag {v:?}"))?,
                        ),
                        ("unread", Some(v)) => Action::SetUnread(
                            bool_from_token(v)
                                .ok_or_else(|| format!("line {lineno}: unknown unread {v:?}"))?,
                        ),
                        ("delete", _) => Action::Delete,
                        _ => {
                            return Err(format!("line {lineno}: unusable action record"));
                        }
                    };
                    rule.actions.push(action);
                }
                "sections" => {
                    if rest.len() < 2 {
                        return Err(format!(
                            "line {lineno}: sections record needs favorites and smart visibility"
                        ));
                    }
                    let favorites_visible = bool_from_token(rest[0]).ok_or_else(|| {
                        format!("line {lineno}: unknown favorites visibility {:?}", rest[0])
                    })?;
                    let smart_visible = bool_from_token(rest[1]).ok_or_else(|| {
                        format!("line {lineno}: unknown smart visibility {:?}", rest[1])
                    })?;
                    sections = Some((favorites_visible, smart_visible));
                }
                "favorite" => {
                    admit_next(
                        favorites.len(),
                        PREFERENCES_FAVORITES_MAX,
                        "sidebar favorites",
                        lineno,
                    )?;
                    if let Some(target) = favorite_from_fields(&rest) {
                        favorites.push(target);
                    }
                }
                "smart" => {
                    admit_next(
                        smart.len(),
                        PREFERENCES_SMART_MAILBOXES_MAX,
                        "smart mailboxes",
                        lineno,
                    )?;
                    if let Some(kind) = rest.first().and_then(|t| SmartMailboxKind::from_token(t)) {
                        smart.push(kind);
                    }
                }
                // Forward compatibility inside a version: an unknown record
                // type is skipped, not fatal.
                _ => {}
            }
        }
        // Documents without the marker retain the default layout.
        if let Some((favorites_visible, smart_visible)) = sections {
            prefs.sidebar_layout =
                SidebarLayout::from_parts(favorites_visible, smart_visible, favorites, smart);
        }
        validate_preferences_domain(&prefs)?;
        Ok(prefs)
    }

    fn persist_default() -> Self {
        Self::defaults()
    }
}
