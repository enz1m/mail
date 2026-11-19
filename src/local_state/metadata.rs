use crate::credential_mutation::CredentialMutationJournal;

#[derive(Clone, Copy)]
pub(crate) struct LocalStateMetadata<'a> {
    pub(crate) next_message_ordinal: u64,
    pub(crate) next_account_ordinal: u64,
    pub(crate) active_account_id: &'a str,
    pub(crate) send_generation: u64,
    pub(crate) next_mutation_ordinal: u64,
    pub(crate) credential_journal: &'a CredentialMutationJournal,
}
