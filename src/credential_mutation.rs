use enzui_services::credential_store::CredentialKey;

pub(crate) const MAX_CREDENTIAL_TRANSITIONS: usize = 32;
pub(crate) const CREDENTIAL_KEY_COMPONENT_BYTES_MAX: usize = 1024;
pub(crate) const MAIL_CREDENTIAL_APP_ID: &str = "io.github.enzui.Mail";
pub(crate) const MAIL_CREDENTIAL_LABEL: &str = "password";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CredentialMutationKind {
    Add,
    Edit,
    Remove,
}

impl CredentialMutationKind {
    pub(crate) fn token(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Edit => "edit",
            Self::Remove => "remove",
        }
    }

    pub(crate) fn parse(token: &str) -> Option<Self> {
        match token {
            "add" => Some(Self::Add),
            "edit" => Some(Self::Edit),
            "remove" => Some(Self::Remove),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct DurableCredentialKey {
    app_id: String,
    owner: String,
    label: String,
}

impl DurableCredentialKey {
    pub(crate) fn from_key(key: &CredentialKey) -> Self {
        Self {
            app_id: key.app_id().to_owned(),
            owner: key.owner().to_owned(),
            label: key.label().to_owned(),
        }
    }

    pub(crate) fn from_parts(app_id: String, owner: String, label: String) -> Result<Self, String> {
        let key = Self {
            app_id,
            owner,
            label,
        };
        key.validate()?;
        Ok(key)
    }

    pub(crate) fn to_key(&self) -> CredentialKey {
        CredentialKey::new(self.app_id.clone(), self.owner.clone(), self.label.clone())
    }

    pub(crate) fn app_id(&self) -> &str {
        &self.app_id
    }

    pub(crate) fn owner(&self) -> &str {
        &self.owner
    }

    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    fn validate(&self) -> Result<(), String> {
        for (value, name) in [
            (&self.app_id, "credential application id"),
            (&self.owner, "credential owner"),
            (&self.label, "credential label"),
        ] {
            if value.is_empty() {
                return Err(format!("{name} is empty"));
            }
            if value.len() > CREDENTIAL_KEY_COMPONENT_BYTES_MAX {
                return Err(format!("{name} is too large"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CredentialTransition {
    pub(crate) kind: CredentialMutationKind,
    pub(crate) sequence: u64,
    pub(crate) target_public_generation: u64,
    pub(crate) config_changed: bool,
    pub(crate) account_id: String,
    pub(crate) old_key: Option<DurableCredentialKey>,
    pub(crate) new_key: Option<DurableCredentialKey>,
}

impl CredentialTransition {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.sequence == 0 {
            return Err("credential transition sequence is zero".to_owned());
        }
        if self.account_id.is_empty() {
            return Err("credential transition account identity is empty".to_owned());
        }
        if self.account_id.len() > CREDENTIAL_KEY_COMPONENT_BYTES_MAX {
            return Err("credential transition account identity is too large".to_owned());
        }
        if let Some(key) = &self.old_key {
            key.validate()?;
        }
        if let Some(key) = &self.new_key {
            key.validate()?;
        }
        for key in [self.old_key.as_ref(), self.new_key.as_ref()]
            .into_iter()
            .flatten()
        {
            if key.app_id() != MAIL_CREDENTIAL_APP_ID
                || key.label() != MAIL_CREDENTIAL_LABEL
                || !credential_owner_belongs_to_account(key.owner(), &self.account_id)
            {
                return Err("credential transition key is outside the account namespace".to_owned());
            }
        }
        let shape_is_valid = match self.kind {
            CredentialMutationKind::Add => {
                self.config_changed && self.old_key.is_none() && self.new_key.is_some()
            }
            CredentialMutationKind::Edit if self.config_changed => {
                let expected_owner = format!("{}-credential-{}", self.account_id, self.sequence);
                matches!(
                    (&self.old_key, &self.new_key),
                    (Some(old), Some(new))
                        if old != new && new.owner() == expected_owner
                )
            }
            CredentialMutationKind::Edit => {
                matches!((&self.old_key, &self.new_key), (Some(old), Some(new)) if old == new)
            }
            CredentialMutationKind::Remove => {
                self.config_changed && self.old_key.is_some() && self.new_key.is_none()
            }
        };
        if !shape_is_valid {
            return Err("credential transition shape does not match its operation".to_owned());
        }
        Ok(())
    }
}

fn credential_owner_belongs_to_account(owner: &str, account_id: &str) -> bool {
    if owner == account_id {
        return true;
    }
    let Some(suffix) = owner
        .strip_prefix(account_id)
        .and_then(|owner| owner.strip_prefix("-credential-"))
    else {
        return false;
    };
    !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
}

fn credential_owner_sequence(owner: &str) -> Option<u64> {
    let (_, suffix) = owner.rsplit_once("-credential-")?;
    if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    suffix.parse().ok()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CredentialReservation {
    pub(crate) sequence: u64,
    pub(crate) target_public_generation: u64,
}

pub(crate) struct TransitionKeys {
    pub(crate) old_key: Option<CredentialKey>,
    pub(crate) new_key: Option<CredentialKey>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CredentialMutationJournal {
    sequence_high_water: u64,
    public_generation: u64,
    records: Vec<CredentialTransition>,
}

impl CredentialMutationJournal {
    pub(crate) fn from_parts(
        sequence_high_water: u64,
        public_generation: u64,
        records: Vec<CredentialTransition>,
    ) -> Result<Self, String> {
        let journal = Self {
            sequence_high_water,
            public_generation,
            records,
        };
        journal.validate()?;
        Ok(journal)
    }

    pub(crate) fn sequence_high_water(&self) -> u64 {
        self.sequence_high_water
    }

    pub(crate) fn public_generation(&self) -> u64 {
        self.public_generation
    }

    pub(crate) fn records(&self) -> &[CredentialTransition] {
        &self.records
    }

    pub(crate) fn active(&self) -> Option<&CredentialTransition> {
        self.records.first()
    }

    pub(crate) fn reserve(
        &mut self,
        kind: CredentialMutationKind,
        account_id: String,
        config_changed: bool,
        referenced_keys: &[DurableCredentialKey],
        build_keys: impl FnOnce(u64) -> TransitionKeys,
    ) -> Result<CredentialReservation, &'static str> {
        if !self.records.is_empty() || self.records.len() >= MAX_CREDENTIAL_TRANSITIONS {
            return Err("another credential transition still needs reconciliation");
        }
        let observed_sequence = referenced_keys
            .iter()
            .filter_map(|key| credential_owner_sequence(key.owner()))
            .max()
            .unwrap_or(0);
        let sequence = self
            .sequence_high_water
            .max(observed_sequence)
            .checked_add(1)
            .ok_or("the credential transition sequence is exhausted")?;
        let target_public_generation = if config_changed {
            self.public_generation
                .checked_add(1)
                .ok_or("the public account generation is exhausted")?
        } else {
            self.public_generation
        };
        let keys = build_keys(sequence);
        let record = CredentialTransition {
            kind,
            sequence,
            target_public_generation,
            config_changed,
            account_id,
            old_key: keys.old_key.as_ref().map(DurableCredentialKey::from_key),
            new_key: keys.new_key.as_ref().map(DurableCredentialKey::from_key),
        };
        record
            .validate()
            .map_err(|_| "the credential transition is invalid")?;
        if config_changed
            && record
                .new_key
                .as_ref()
                .is_some_and(|key| referenced_keys.contains(key))
        {
            return Err("the candidate credential key is already in use");
        }
        self.sequence_high_water = sequence;
        self.records.push(record);
        Ok(CredentialReservation {
            sequence,
            target_public_generation,
        })
    }

    pub(crate) fn mark_public(&mut self, sequence: u64) -> bool {
        let Some(record) = self
            .records
            .first()
            .filter(|record| record.sequence == sequence && record.config_changed)
        else {
            return false;
        };
        if self.public_generation.checked_add(1) != Some(record.target_public_generation) {
            return false;
        }
        self.public_generation = record.target_public_generation;
        true
    }

    pub(crate) fn finish(&mut self, sequence: u64) -> bool {
        if self.records.first().map(|record| record.sequence) != Some(sequence) {
            return false;
        }
        self.records.remove(0);
        true
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.records.len() > MAX_CREDENTIAL_TRANSITIONS {
            return Err("too many credential transition records".to_owned());
        }
        if self.public_generation > self.sequence_high_water {
            return Err(
                "public credential generation exceeds the sequence high-water mark".to_owned(),
            );
        }
        for (index, record) in self.records.iter().enumerate() {
            record.validate()?;
            if record.sequence > self.sequence_high_water {
                return Err("credential transition exceeds the sequence high-water mark".to_owned());
            }
            if record.config_changed {
                let next_public = self.public_generation.checked_add(1);
                if record.target_public_generation > self.public_generation
                    && Some(record.target_public_generation) != next_public
                {
                    return Err(
                        "credential transition targets a future public generation".to_owned()
                    );
                }
            } else if record.target_public_generation != self.public_generation {
                return Err("password-only transition changed the public generation".to_owned());
            }
            if self.records[..index]
                .iter()
                .any(|prior| prior.account_id == record.account_id)
            {
                return Err("duplicate credential transition account".to_owned());
            }
            if index > 0 && self.records[index - 1].sequence >= record.sequence {
                return Err("credential transition sequences are not ordered".to_owned());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RecoveryPlan {
    ConfirmCurrent {
        key: DurableCredentialKey,
        delete_after: Option<DurableCredentialKey>,
    },
    DeleteExact(DurableCredentialKey),
    Clear,
}

pub(crate) struct CredentialMutationOwner {
    journal: CredentialMutationJournal,
}

impl CredentialMutationOwner {
    pub(crate) fn new(journal: CredentialMutationJournal) -> Self {
        Self { journal }
    }

    pub(crate) fn journal(&self) -> &CredentialMutationJournal {
        &self.journal
    }

    pub(crate) fn reserve(
        &mut self,
        kind: CredentialMutationKind,
        account_id: String,
        config_changed: bool,
        referenced_keys: &[DurableCredentialKey],
        build_keys: impl FnOnce(u64) -> TransitionKeys,
    ) -> Result<CredentialReservation, &'static str> {
        self.journal.reserve(
            kind,
            account_id,
            config_changed,
            referenced_keys,
            build_keys,
        )
    }

    pub(crate) fn mark_public(&mut self, sequence: u64) -> bool {
        self.journal.mark_public(sequence)
    }

    pub(crate) fn finish(&mut self, sequence: u64) -> bool {
        self.journal.finish(sequence)
    }

    pub(crate) fn recovery_plan(
        &self,
        record: &CredentialTransition,
        current_account_key: Option<&DurableCredentialKey>,
        referenced_keys: &[DurableCredentialKey],
    ) -> RecoveryPlan {
        let current_is = |key: &DurableCredentialKey| current_account_key == Some(key);
        let referenced = |key: &DurableCredentialKey| referenced_keys.contains(key);
        let public_committed = self.journal.public_generation == record.target_public_generation;

        match record.kind {
            CredentialMutationKind::Add => {
                let Some(new_key) = record.new_key.as_ref() else {
                    return RecoveryPlan::Clear;
                };
                if public_committed && current_is(new_key) {
                    RecoveryPlan::ConfirmCurrent {
                        key: new_key.clone(),
                        delete_after: None,
                    }
                } else if referenced(new_key) {
                    RecoveryPlan::Clear
                } else {
                    RecoveryPlan::DeleteExact(new_key.clone())
                }
            }
            CredentialMutationKind::Edit if !record.config_changed => {
                let Some(key) = record.new_key.as_ref().filter(|new_key| {
                    record.old_key.as_ref() == Some(*new_key) && current_is(new_key)
                }) else {
                    return RecoveryPlan::Clear;
                };
                // A same-key transition is safe only for an identical public config.
                RecoveryPlan::ConfirmCurrent {
                    key: key.clone(),
                    delete_after: None,
                }
            }
            CredentialMutationKind::Edit => {
                let (Some(old_key), Some(new_key)) =
                    (record.old_key.as_ref(), record.new_key.as_ref())
                else {
                    return RecoveryPlan::Clear;
                };
                if public_committed && current_is(new_key) {
                    RecoveryPlan::ConfirmCurrent {
                        key: new_key.clone(),
                        delete_after: (!referenced(old_key)).then(|| old_key.clone()),
                    }
                } else if !referenced(new_key) {
                    RecoveryPlan::DeleteExact(new_key.clone())
                } else {
                    RecoveryPlan::Clear
                }
            }
            CredentialMutationKind::Remove => {
                let Some(old_key) = record.old_key.as_ref() else {
                    return RecoveryPlan::Clear;
                };
                if public_committed && !referenced(old_key) {
                    RecoveryPlan::DeleteExact(old_key.clone())
                } else if current_is(old_key) {
                    RecoveryPlan::Clear
                } else if !referenced(old_key) {
                    RecoveryPlan::DeleteExact(old_key.clone())
                } else {
                    RecoveryPlan::Clear
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(owner: &str) -> CredentialKey {
        CredentialKey::new(MAIL_CREDENTIAL_APP_ID, owner, MAIL_CREDENTIAL_LABEL)
    }

    fn config_edit(
        owner: &mut CredentialMutationOwner,
        current: &DurableCredentialKey,
    ) -> (CredentialReservation, DurableCredentialKey) {
        let referenced = [current.clone()];
        let reservation = owner
            .reserve(
                CredentialMutationKind::Edit,
                "account-1".to_owned(),
                true,
                &referenced,
                |sequence| TransitionKeys {
                    old_key: Some(current.to_key()),
                    new_key: Some(key(&format!("account-1-credential-{sequence}"))),
                },
            )
            .expect("config edit admitted");
        let candidate = owner
            .journal()
            .active()
            .and_then(|record| record.new_key.clone())
            .expect("candidate key");
        (reservation, candidate)
    }

    #[test]
    fn crash_after_second_candidate_store_rolls_back_without_touching_current_key() {
        let initial = DurableCredentialKey::from_key(&key("account-1"));
        let mut owner = CredentialMutationOwner::new(CredentialMutationJournal::default());
        let (first, first_key) = config_edit(&mut owner, &initial);
        assert!(owner.mark_public(first.sequence));
        assert!(owner.finish(first.sequence));
        let (second, second_key) = config_edit(&mut owner, &first_key);

        assert_eq!(
            owner.journal().public_generation(),
            first.target_public_generation
        );
        assert_eq!(
            second.target_public_generation,
            first.target_public_generation + 1
        );
        assert_eq!(
            owner.recovery_plan(
                owner.journal().active().expect("second edit"),
                Some(&first_key),
                std::slice::from_ref(&first_key),
            ),
            RecoveryPlan::DeleteExact(second_key)
        );
    }

    #[test]
    fn password_only_same_key_crash_confirms_safely() {
        let current = DurableCredentialKey::from_key(&key("account-1-credential-8"));
        let mut owner = CredentialMutationOwner::new(
            CredentialMutationJournal::from_parts(8, 3, Vec::new()).expect("journal"),
        );
        let reservation = owner
            .reserve(
                CredentialMutationKind::Edit,
                "account-1".to_owned(),
                false,
                std::slice::from_ref(&current),
                |_| TransitionKeys {
                    old_key: Some(current.to_key()),
                    new_key: Some(current.to_key()),
                },
            )
            .expect("password edit admitted");

        assert_eq!(reservation.sequence, 9);
        assert_eq!(reservation.target_public_generation, 3);
        assert!(owner.finish(reservation.sequence));
        assert_eq!(owner.journal().sequence_high_water(), 9);
        assert_eq!(owner.journal().public_generation(), 3);
        let owner = CredentialMutationOwner::new(
            CredentialMutationJournal::from_parts(
                9,
                3,
                vec![CredentialTransition {
                    kind: CredentialMutationKind::Edit,
                    sequence: 9,
                    target_public_generation: 3,
                    config_changed: false,
                    account_id: "account-1".to_owned(),
                    old_key: Some(current.clone()),
                    new_key: Some(current.clone()),
                }],
            )
            .expect("crash journal"),
        );
        assert_eq!(
            owner.recovery_plan(
                owner.journal().active().expect("password edit"),
                Some(&current),
                std::slice::from_ref(&current),
            ),
            RecoveryPlan::ConfirmCurrent {
                key: current,
                delete_after: None,
            }
        );
    }
}
