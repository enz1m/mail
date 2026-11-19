use super::*;

impl State {
    pub(crate) fn reduce_batch6(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::ApplyRules => {
                if let Some(affected) = self.apply_rules_with_server_journal() {
                    self.last_rules_result = Some(model::rules::applied_rules_label(affected));
                    self.reconcile_after_mutation();
                }
            }
            Message::RuleNameEdited(name) => {
                if let Some(d) = self.rules_ui.draft.as_mut() {
                    d.rule.name = name;
                }
            }
            Message::RuleConditionFieldSelected(i, idx) => {
                if let Some(d) = self.rules_ui.draft.as_mut()
                    && let Some(c) = d.rule.conditions.get_mut(i)
                    && let Some(field) = model::rules::ConditionField::from_index(idx)
                {
                    c.field = field;
                }
            }
            Message::RuleConditionMatchSelected(i, idx) => {
                if let Some(d) = self.rules_ui.draft.as_mut()
                    && let Some(c) = d.rule.conditions.get_mut(i)
                    && let Some(op) = model::rules::ConditionMatch::from_index(idx)
                {
                    c.op = op;
                }
            }
            Message::RuleConditionValueEdited(i, value) => {
                if let Some(d) = self.rules_ui.draft.as_mut()
                    && let Some(c) = d.rule.conditions.get_mut(i)
                {
                    c.value = value;
                }
            }
            Message::RuleAddCondition => {
                if let Some(d) = self.rules_ui.draft.as_mut() {
                    d.add_condition();
                }
            }
            Message::RuleRemoveCondition(i) => {
                if let Some(d) = self.rules_ui.draft.as_mut() {
                    d.remove_condition(i);
                }
            }
            Message::RuleCombinatorSelected(idx) => {
                if let Some(d) = self.rules_ui.draft.as_mut()
                    && let Some(comb) = model::rules::ConditionCombinator::from_index(idx)
                {
                    d.rule.combinator = comb;
                }
            }
            Message::RuleActionKindSelected(i, idx) => {
                if let Some(d) = self.rules_ui.draft.as_mut()
                    && let Some(a) = d.rule.actions.get_mut(i)
                {
                    *a = model::rules::action_for_kind_index(idx);
                }
            }
            Message::RuleActionMailboxSelected(i, idx) => {
                if let Some(d) = self.rules_ui.draft.as_mut()
                    && let Some(a) = d.rule.actions.get_mut(i)
                    && let Some(kind) = model::rules::mailbox_for_index(idx)
                {
                    *a = model::rules::Action::MoveTo(kind);
                }
            }
            Message::RuleAddAction => {
                if let Some(d) = self.rules_ui.draft.as_mut() {
                    d.add_action();
                }
            }
            Message::RuleRemoveAction(i) => {
                if let Some(d) = self.rules_ui.draft.as_mut() {
                    d.remove_action(i);
                }
            }

            _ => unreachable!("reduce_batch6 handles only its own variants"),
        }
        Task::none()
    }
}
