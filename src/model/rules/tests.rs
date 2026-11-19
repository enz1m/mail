use super::*;
use crate::model::types::MailboxKind;

#[test]
fn matching_rules_apply_moves_in_authored_order() {
    for destinations in [
        [MailboxKind::Archive, MailboxKind::Junk],
        [MailboxKind::Junk, MailboxKind::Archive],
    ] {
        let mut store = crate::model::fixture::seed();
        store.messages.truncate(1);
        store.messages[0].sender_email = "sender@rules.example.com".into();
        let id = store.messages[0].id.clone();
        let mut rules = RuleSet::default();
        for (index, mailbox) in destinations.iter().enumerate() {
            let mut rule = Rule::new(format!("rule-{index}"), "Move matching mail");
            rule.conditions.push(Condition::new(
                ConditionField::From,
                ConditionMatch::Contains,
                "rules.example.com",
            ));
            rule.actions.push(Action::MoveTo(*mailbox));
            rules.push(rule);
        }
        apply_rules(&mut store, &rules);
        assert_eq!(
            store.message(&id).unwrap().location,
            crate::model::types::MessageLocation::Mailbox(destinations[1])
        );
    }
}
