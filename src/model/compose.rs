//! Composition queues an immutable Outbox snapshot; acknowledgement publishes Sent.
//! Undo restores the queued snapshot without publishing a Sent copy.

use super::types::{Account, Attachment, MailboxKind, MailboxStore, Message, MessageLocation};
use crate::local_clock::LocalClock;
use enzui_services::mail_transport::RecipientList;

// Rich and HTML share an editor and emit both plain text and HTML; Plain emits no HTML.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(crate) enum ComposeFormat {
    #[default]
    Plain,
    Rich,
    Html,
}

impl ComposeFormat {
    pub(crate) const ALL: [ComposeFormat; 3] = [
        ComposeFormat::Plain,
        ComposeFormat::Rich,
        ComposeFormat::Html,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            ComposeFormat::Plain => "Plain",
            ComposeFormat::Rich => "Rich",
            ComposeFormat::Html => "HTML",
        }
    }

    // The row label in the format menu, longer than [`Self::label`] because a menu
    // row is read without its siblings.
    pub(crate) fn menu_label(self) -> &'static str {
        match self {
            ComposeFormat::Plain => "Plain text",
            ComposeFormat::Rich => "Rich text",
            ComposeFormat::Html => "HTML",
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            ComposeFormat::Plain => "type",
            ComposeFormat::Rich => "bold",
            ComposeFormat::Html => "code",
        }
    }

    // Whether this format is edited through the rich composite rather than
    // the plain text area.
    pub(crate) fn is_rich(self) -> bool {
        matches!(self, ComposeFormat::Rich | ComposeFormat::Html)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) enum ComposeMode {
    #[default]
    New,
    Reply(String),
    ReplyAll(String),
    Forward(String),
    EditDraft(String),
}

impl ComposeMode {
    pub(crate) fn source_message_id(&self) -> Option<&str> {
        match self {
            ComposeMode::New => None,
            ComposeMode::Reply(id)
            | ComposeMode::ReplyAll(id)
            | ComposeMode::Forward(id)
            | ComposeMode::EditDraft(id) => Some(id.as_str()),
        }
    }

    pub(crate) fn title(&self) -> &'static str {
        match self {
            ComposeMode::New => "New Message",
            ComposeMode::Reply(_) => "Reply",
            ComposeMode::ReplyAll(_) => "Reply All",
            ComposeMode::Forward(_) => "Forward",
            ComposeMode::EditDraft(_) => "Edit Draft",
        }
    }
}

// Initial user-controlled compose content used to distinguish app seeding
// from later edits. The empty default treats a restored snapshot as authored.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ComposeSeed {
    pub(crate) to: String,
    pub(crate) cc: String,
    pub(crate) bcc: String,
    pub(crate) subject: String,
    pub(crate) body: String,
    pub(crate) body_html: Option<String>,
    pub(crate) attachments: Vec<Attachment>,
}

// Draft state for the compose window; `open` gates the modal.
//
// `body` is authoritative for validation, send and save; the view's
// `text_editor::Content` only owns caret and undo and writes every edit
// through.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ComposeState {
    pub(crate) open: bool,
    pub(crate) mode: ComposeMode,
    pub(crate) title: String,
    pub(crate) to: String,
    pub(crate) cc: String,
    // Bcc is retained in drafts and the envelope; outgoing MIME omits the Bcc header.
    pub(crate) bcc: String,
    pub(crate) cc_shown: bool,
    // Whether the Bcc field is revealed. An undone send restores it from the
    // snapshot; `EditDraft` always seeds `false`, since drafts cannot store bcc.
    pub(crate) bcc_shown: bool,
    pub(crate) subject: String,
    pub(crate) body: String,
    pub(crate) format: ComposeFormat,
    pub(crate) body_html: Option<String>,
    // Sending account, seeded from the source message or active account; editable
    // in the header picker.
    //
    // Send and save-draft attribute the message to this id, not
    // `State::active_account_id`. Empty only on a closed compose.
    pub(crate) from_account_id: String,
    pub(crate) attachments: Vec<Attachment>,
    // When editing an existing draft, its id. Set only for `EditDraft`.
    pub(crate) editing_draft_id: Option<String>,
    pub(crate) in_reply_to: Option<String>,
    // The RFC 5322 `References:` chain this compose continues: the source's chain
    // plus its id, so replies thread even when the parent is not in view or the
    // subject changed.
    pub(crate) references: Vec<String>,
    pub(crate) seed: ComposeSeed,
    // The agent review this compose shows, if an agent prepared it.
    //
    // Carried through the pending send and undo; never persisted.
    pub(crate) review_key: Option<String>,
}

impl ComposeState {
    pub(crate) fn has_recipient(&self) -> bool {
        !self.to.trim().is_empty() || !self.cc.trim().is_empty() || !self.bcc.trim().is_empty()
    }

    pub(crate) fn has_sendable_address(&self) -> bool {
        let all = format!("{}, {}, {}", self.to, self.cc, self.bcc);
        split_addresses(&all)
            .iter()
            .any(|a| email_token(a).contains('@'))
    }

    pub(crate) fn can_send(&self) -> bool {
        self.open && self.has_sendable_address()
    }

    // Whether an open compose contains content worth a manual draft save.
    // Attachments qualify without text. New automatic drafts also require edits
    // so a seeded signature or quote alone is not saved.
    pub(crate) fn can_save_draft(&self) -> bool {
        self.open
            && (self.has_recipient()
                || !self.subject.trim().is_empty()
                || !self.body.trim().is_empty()
                || !self.attachments.is_empty())
    }

    pub(crate) fn mentions_missing_attachment(&self) -> bool {
        if !self.attachments.is_empty() {
            return false;
        }
        let authored = self
            .body
            .strip_suffix(&self.seed.body)
            .unwrap_or(&self.body);
        authored.split(|c: char| !c.is_alphabetic()).any(|word| {
            ["attached", "attachment", "enclosed"]
                .iter()
                .any(|trigger| word.eq_ignore_ascii_case(trigger))
        })
    }

    pub(crate) fn is_user_edited(&self) -> bool {
        if !self.open {
            return false;
        }
        self.attachments != self.seed.attachments
            || self.to.trim() != self.seed.to.trim()
            || self.cc.trim() != self.seed.cc.trim()
            || self.bcc.trim() != self.seed.bcc.trim()
            || self.subject.trim() != self.seed.subject.trim()
            || self.body.trim() != self.seed.body.trim()
            || self.body_html != self.seed.body_html
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingSend {
    pub(crate) id: String,
    pub(crate) account_id: String,
    pub(crate) sender_name: String,
    pub(crate) sender_email: String,
    pub(crate) to_raw: String,
    pub(crate) cc_raw: String,
    pub(crate) bcc_raw: String,
    pub(crate) bcc_shown: bool,
    pub(crate) subject: String,
    pub(crate) body: String,
    pub(crate) body_html: Option<String>,
    pub(crate) attachments: Vec<Attachment>,
    pub(crate) in_reply_to: Option<String>,
    pub(crate) references: Vec<String>,
    pub(crate) format: ComposeFormat,
    pub(crate) mode: ComposeMode,
    // The source draft retires only after send acknowledgement and remains recoverable through retry or undo.
    pub(crate) restored_draft_id: Option<String>,
    // The agent review the sent compose was showing. In memory only;
    // a restored outbox entry has none.
    pub(crate) review_key: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EnvelopeRefusal;

impl EnvelopeRefusal {
    pub(crate) fn user_message(self) -> &'static str {
        "The outgoing sender or recipient list is invalid."
    }
}

fn validate_address_shape(address: &str) -> Result<(), EnvelopeRefusal> {
    if address.is_empty() || !address.contains('@') {
        Err(EnvelopeRefusal)
    } else {
        Ok(())
    }
}

impl PendingSend {
    pub(crate) fn smtp_envelope_recipients(&self) -> Result<Vec<String>, EnvelopeRefusal> {
        validate_address_shape(&self.sender_email)?;
        let mut sender = RecipientList::new();
        sender
            .add(&self.sender_email)
            .map_err(|_| EnvelopeRefusal)?;
        let mut recipients = Vec::new();
        let mut bounded = RecipientList::new();
        for field in [&self.to_raw, &self.cc_raw, &self.bcc_raw] {
            for address in field
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
            {
                let email = email_token(address).trim();
                validate_address_shape(email)?;
                if recipients
                    .iter()
                    .any(|existing: &String| existing.eq_ignore_ascii_case(email))
                {
                    continue;
                }
                bounded.add(email).map_err(|_| EnvelopeRefusal)?;
                recipients.push(email.to_owned());
            }
        }
        if recipients.is_empty() {
            Err(EnvelopeRefusal)
        } else {
            Ok(recipients)
        }
    }
}

pub(crate) struct ComposeController<'a> {
    pub(crate) store: &'a mut MailboxStore,
    pub(crate) compose: &'a mut ComposeState,
    pub(crate) pending_send: &'a mut Option<PendingSend>,
    pub(crate) next_ordinal: &'a mut u64,
    pub(crate) active_account_id: &'a str,
    pub(crate) clock: &'a dyn LocalClock,
}

impl ComposeController<'_> {
    fn mint_id(&mut self, prefix: &str) -> String {
        let n = *self.next_ordinal;
        *self.next_ordinal = n + 1;
        format!("{prefix}-{n}")
    }

    fn active_account(&self) -> Option<&Account> {
        self.store.account(self.active_account_id)
    }

    fn sending_account(&self) -> Option<&Account> {
        self.store.account(&self.compose.from_account_id)
    }

    fn sending_email(&self) -> String {
        self.sending_account()
            .map(|a| a.email.clone())
            .unwrap_or_default()
    }

    fn sending_name(&self) -> String {
        self.sending_account()
            .map(|a| a.display_name.clone())
            .unwrap_or_default()
    }

    // Open compose, seeding fields per `mode`.
    //
    // A stale referenced id seeds a blank compose. A pending send is committed
    // first, so opening compose never drops a queued message.
    //
    // # Errors
    // Returns the clock failure without consuming a pending send.
    pub(crate) fn try_open(
        &mut self,
        mode: ComposeMode,
    ) -> Result<(), enzui_iced_core::local_time::LocalCivilTimeError> {
        if self.pending_send.is_some() {
            self.try_commit_pending_send()?;
        }
        *self.compose = self.seed(&mode);
        Ok(())
    }

    pub(crate) fn close(&mut self) {
        *self.compose = ComposeState::default();
    }

    // Build a `ComposeState` seeded from `mode`, reading the store only.
    fn seed(&self, mode: &ComposeMode) -> ComposeState {
        let title = mode.title().to_owned();
        let sending_account = mode
            .source_message_id()
            .and_then(|id| self.store.message(id))
            .and_then(|m| self.store.account(&m.account_id))
            .or_else(|| self.active_account());
        let own_email = sending_account
            .map(|a| a.email.as_str())
            .unwrap_or_default();
        let (to, cc, subject, body, body_html, editing_draft_id) = match mode {
            ComposeMode::New => (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                None,
                None,
            ),
            ComposeMode::Reply(id) => match self.store.message(id) {
                Some(m) => (
                    if m.sender_email.eq_ignore_ascii_case(own_email) {
                        reply_all_cc(&m.to, &[own_email])
                    } else {
                        format_address(&m.sender_name, &m.sender_email)
                    },
                    String::new(),
                    add_subject_prefix(&m.subject, "Re:"),
                    quote_body(m),
                    None,
                    None,
                ),
                None => blank_seed(),
            },
            ComposeMode::ReplyAll(id) => match self.store.message(id) {
                Some(m) => (
                    if m.sender_email.eq_ignore_ascii_case(own_email) {
                        reply_all_cc(&m.to, &[own_email])
                    } else {
                        format_address(&m.sender_name, &m.sender_email)
                    },
                    if m.sender_email.eq_ignore_ascii_case(own_email) {
                        let mut exclude = vec![own_email];
                        exclude.extend(m.to.iter().map(String::as_str));
                        reply_all_cc(&m.cc, &exclude)
                    } else {
                        reply_all_cc(
                            &m.to.iter().chain(&m.cc).cloned().collect::<Vec<_>>(),
                            &[own_email, m.sender_email.as_str()],
                        )
                    },
                    add_subject_prefix(&m.subject, "Re:"),
                    quote_body(m),
                    None,
                    None,
                ),
                None => blank_seed(),
            },
            ComposeMode::Forward(id) => match self.store.message(id) {
                Some(m) => (
                    String::new(),
                    String::new(),
                    add_subject_prefix(&m.subject, "Fwd:"),
                    forward_body(m),
                    None,
                    None,
                ),
                None => blank_seed(),
            },
            ComposeMode::EditDraft(id) => match self.store.message(id) {
                Some(m) => (
                    m.to.join(", "),
                    m.cc.join(", "),
                    m.subject.clone(),
                    m.body.clone(),
                    // The formatted body the draft was saved with; without it
                    // a reopened draft falls back to its plain text and the
                    // next save writes that back over the stored HTML.
                    m.body_html.clone(),
                    Some(m.id.clone()),
                ),
                None => blank_seed(),
            },
        };
        let in_reply_to = match mode {
            ComposeMode::Reply(id) | ComposeMode::ReplyAll(id) => {
                // A stale source id seeds no linkage.
                self.store.message(id).map(|_| id.clone())
            }
            ComposeMode::EditDraft(id) => {
                self.store.message(id).and_then(|m| m.in_reply_to.clone())
            }
            ComposeMode::New | ComposeMode::Forward(_) => None,
        };
        // `in_reply_to` names only the direct parent, which may not be in view.
        let references = match mode {
            ComposeMode::Reply(id) | ComposeMode::ReplyAll(id) => match self.store.message(id) {
                Some(m) => {
                    let mut chain = m.references.clone();
                    if !chain.iter().any(|r| r == &m.id) {
                        chain.push(m.id.clone());
                    }
                    chain
                }
                None => Vec::new(),
            },
            ComposeMode::EditDraft(id) => self
                .store
                .message(id)
                .map(|m| m.references.clone())
                .unwrap_or_default(),
            ComposeMode::New | ComposeMode::Forward(_) => Vec::new(),
        };
        let signed_body = compose_body_with_account_signature(&body, sending_account, mode);
        let attachments = match mode {
            ComposeMode::EditDraft(id) | ComposeMode::Forward(id) => self
                .store
                .message(id)
                .map(|message| message.attachments.clone())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let bcc = match mode {
            ComposeMode::EditDraft(id) => self
                .store
                .message(id)
                .map(|m| m.bcc.join(", "))
                .unwrap_or_default(),
            _ => String::new(),
        };
        let bcc_shown = !bcc.is_empty();
        // Everything above is app-seeded; `is_user_edited` compares against it so
        // autosave does not draft a seeded signature or quote.
        let seed = ComposeSeed {
            to: to.clone(),
            cc: cc.clone(),
            bcc: bcc.clone(),
            subject: subject.clone(),
            body: signed_body.clone(),
            body_html: body_html.clone(),
            attachments: attachments.clone(),
        };
        ComposeState {
            open: true,
            mode: mode.clone(),
            title,
            to,
            cc_shown: !cc.is_empty(),
            cc,
            bcc,
            bcc_shown,
            subject,
            body: signed_body,
            format: ComposeFormat::Plain,
            body_html,
            from_account_id: sending_account
                .map_or_else(|| self.active_account_id.to_owned(), |a| a.id.clone()),
            attachments,
            editing_draft_id,
            in_reply_to,
            references,
            seed,
            review_key: None,
        }
    }

    // Queue a snapshot without publishing Sent. Committing an older pending send can fail if the clock is unavailable.
    pub(crate) fn try_send_compose(
        &mut self,
    ) -> Result<bool, enzui_iced_core::local_time::LocalCivilTimeError> {
        if !self.compose.has_recipient() {
            return Ok(false);
        }
        // An earlier pending send commits first so a second Send never loses it.
        if self.pending_send.is_some() {
            self.try_commit_pending_send()?;
        }
        let restored_draft_id = self.compose.editing_draft_id.take();
        let in_reply_to = self.compose.in_reply_to.clone();
        let references = self.compose.references.clone();
        let id = self.mint_id("sent");
        let from_id = self.compose.from_account_id.clone();
        let from_name = self.sending_name();
        let from_email = self.sending_email();
        let attachments = std::mem::take(&mut self.compose.attachments);
        *self.pending_send = Some(PendingSend {
            id,
            account_id: from_id,
            sender_name: from_name,
            sender_email: from_email,
            attachments,
            to_raw: self.compose.to.clone(),
            cc_raw: self.compose.cc.clone(),
            bcc_raw: self.compose.bcc.clone(),
            bcc_shown: self.compose.bcc_shown,
            subject: self.compose.subject.clone(),
            body: self.compose.body.clone(),
            body_html: self.compose.body_html.clone(),
            in_reply_to,
            references,
            format: self.compose.format,
            mode: self.compose.mode.clone(),
            restored_draft_id,
            review_key: self.compose.review_key.take(),
        });
        *self.compose = ComposeState::default();
        Ok(true)
    }

    // Finalise the pending send after SMTP acceptance, or inside an explicit
    // fixture route: append it to Sent, remove a replaced draft, and clear the
    // durable outbox value. A missing pending send is a no-op.
    //
    // # Errors
    // Returns the clock failure before consuming the pending send.
    pub(crate) fn try_commit_pending_send(
        &mut self,
    ) -> Result<bool, enzui_iced_core::local_time::LocalCivilTimeError> {
        if self.pending_send.is_none() {
            return Ok(false);
        }
        let now = self.clock.now()?;
        let Some(pending) = self.pending_send.take() else {
            return Ok(false);
        };
        let to_list = split_addresses(&pending.to_raw);
        let preview = preview_from_body(&pending.body);
        let sent = Message {
            id: pending.id,
            account_id: pending.account_id,
            location: MessageLocation::Mailbox(MailboxKind::Sent),
            sender_name: pending.sender_name,
            sender_email: pending.sender_email,
            to: to_list,
            subject: pending.subject,
            preview,
            body: pending.body,
            body_html: pending.body_html,
            date: now,
            unread: false,
            flagged: false,
            has_attachment: !pending.attachments.is_empty(),
            attachments: pending.attachments,
            has_remote_content: false,
            has_inline_image: false,
            blocked: false,
            in_reply_to: pending.in_reply_to,
            body_state: crate::model::types::MessageBodyState::Complete,
            references: pending.references,
            snoozed_until: None,
            send_at: None,
            cc: split_addresses(&pending.cc_raw),
            bcc: split_addresses(&pending.bcc_raw),
            unsubscribe: None,
        };
        if let Some(draft_id) = &pending.restored_draft_id {
            self.store.messages.retain(|m| m.id != *draft_id);
        }
        self.store.messages.push(sent);
        self.store.mark_dirty();
        Ok(true)
    }

    pub(crate) fn undo_pending_send(&mut self) -> bool {
        let Some(pending) = self.pending_send.take() else {
            return false;
        };
        *self.compose = ComposeState {
            open: true,
            title: pending.mode.title().to_owned(),
            mode: pending.mode,
            to: pending.to_raw,
            cc_shown: !pending.cc_raw.is_empty(),
            cc: pending.cc_raw,
            bcc: pending.bcc_raw,
            bcc_shown: pending.bcc_shown,
            subject: pending.subject,
            body: pending.body,
            format: pending.format,
            body_html: pending.body_html,
            // Preview visibility is a view-only toggle and is not
            // snapshotted onto `PendingSend`, so an undone send restores the
            // draft with preview hidden — same rule as a fresh compose.
            from_account_id: pending.account_id,
            attachments: pending.attachments,
            editing_draft_id: pending.restored_draft_id,
            in_reply_to: pending.in_reply_to,
            references: pending.references,
            // Restored text is the user's own, so an empty seed keeps it autosaving.
            seed: ComposeSeed::default(),
            review_key: pending.review_key,
        };
        true
    }

    // Save the compose as a draft in the account's Drafts mailbox, updating in
    // place when editing one, and close compose.
    //
    // A blank compose is a no-op. Returns the draft id when one was written.
    //
    // # Errors
    // Returns the clock failure before creating a first draft.
    pub(crate) fn try_save_draft(
        &mut self,
    ) -> Result<Option<String>, enzui_iced_core::local_time::LocalCivilTimeError> {
        if !self.compose.can_save_draft() {
            return Ok(None);
        }
        let preview = preview_from_body(&self.compose.body);
        let to_list = split_addresses(&self.compose.to);
        let in_reply_to = self.compose.in_reply_to.clone();
        // Without the chain, a sent reply could not be threaded by any client.
        let references = self.compose.references.clone();
        let from_id = self.compose.from_account_id.clone();
        let from_name = self.sending_name();
        let from_email = self.sending_email();
        if let Some(existing_id) = self.compose.editing_draft_id.clone()
            && let Some(m) = self.store.messages.iter_mut().find(|m| {
                m.id == existing_id
                    && (m.account_id == from_id || existing_id.starts_with("draft-"))
            })
        {
            m.account_id = from_id;
            m.sender_name = from_name;
            m.sender_email = from_email;
            m.to = to_list;
            m.cc = split_addresses(&self.compose.cc);
            m.bcc = split_addresses(&self.compose.bcc);
            m.subject.clone_from(&self.compose.subject);
            m.preview = preview;
            m.body.clone_from(&self.compose.body);
            m.body_html.clone_from(&self.compose.body_html);
            m.body_state = crate::model::types::MessageBodyState::Complete;
            m.attachments.clone_from(&self.compose.attachments);
            m.has_attachment = !m.attachments.is_empty();
            m.in_reply_to = in_reply_to;
            m.references = references;
            self.store.mark_dirty();
            *self.compose = ComposeState::default();
            return Ok(Some(existing_id));
        }
        let now = self.clock.now()?;
        let id = self.mint_id("draft");
        let attachments = self.compose.attachments.clone();
        let draft = Message {
            id: id.clone(),
            account_id: from_id,
            location: MessageLocation::Mailbox(MailboxKind::Drafts),
            sender_name: from_name,
            sender_email: from_email,
            to: to_list,
            subject: self.compose.subject.clone(),
            preview,
            body: self.compose.body.clone(),
            body_html: self.compose.body_html.clone(),
            date: now,
            unread: false,
            flagged: false,
            has_attachment: !attachments.is_empty(),
            attachments,
            has_remote_content: false,
            has_inline_image: false,
            blocked: false,
            in_reply_to,
            body_state: crate::model::types::MessageBodyState::Complete,
            references,
            snoozed_until: None,
            send_at: None,
            cc: split_addresses(&self.compose.cc),
            bcc: split_addresses(&self.compose.bcc),
            unsubscribe: None,
        };
        self.store.messages.push(draft);
        self.store.mark_dirty();
        *self.compose = ComposeState::default();
        Ok(Some(id))
    }

    pub(crate) fn sync_existing_draft_attachments(&mut self) -> Option<String> {
        let id = self.compose.editing_draft_id.clone()?;
        let message = self
            .store
            .messages
            .iter_mut()
            .find(|message| message.id == id)?;
        message.attachments.clone_from(&self.compose.attachments);
        message.has_attachment = !message.attachments.is_empty();
        self.store.mark_dirty();
        Some(id)
    }

    // Creates or updates a draft without closing compose.
    //
    // Untouched seeded content and edited-but-blank content are no-ops. A
    // first draft records its id so later autosaves update it in place.
    //
    // # Errors
    // Returns the clock failure before creating a first draft.
    pub(crate) fn try_autosave_draft(
        &mut self,
    ) -> Result<bool, enzui_iced_core::local_time::LocalCivilTimeError> {
        if !self.compose.can_save_draft()
            || (!self.compose.is_user_edited() && self.compose.editing_draft_id.is_none())
        {
            return Ok(false);
        }
        let preview = preview_from_body(&self.compose.body);
        let to_list = split_addresses(&self.compose.to);
        let in_reply_to = self.compose.in_reply_to.clone();
        let references = self.compose.references.clone();
        let from_id = self.compose.from_account_id.clone();
        let from_name = self.sending_name();
        let from_email = self.sending_email();

        if let Some(existing_id) = self.compose.editing_draft_id.clone()
            && let Some(m) = self.store.messages.iter_mut().find(|m| {
                m.id == existing_id
                    && (m.account_id == from_id || existing_id.starts_with("draft-"))
            })
        {
            if m.account_id == from_id
                && m.sender_name == from_name
                && m.sender_email == from_email
                && m.to == to_list
                && m.cc == split_addresses(&self.compose.cc)
                && m.bcc == split_addresses(&self.compose.bcc)
                && m.subject == self.compose.subject
                && m.body == self.compose.body
                && m.body_html == self.compose.body_html
                && m.attachments == self.compose.attachments
            {
                return Ok(false);
            }
            m.account_id = from_id;
            m.sender_name = from_name;
            m.sender_email = from_email;
            m.to = to_list;
            m.cc = split_addresses(&self.compose.cc);
            m.bcc = split_addresses(&self.compose.bcc);
            m.subject.clone_from(&self.compose.subject);
            m.preview = preview;
            m.body.clone_from(&self.compose.body);
            m.body_html.clone_from(&self.compose.body_html);
            m.body_state = crate::model::types::MessageBodyState::Complete;
            m.attachments.clone_from(&self.compose.attachments);
            m.has_attachment = !m.attachments.is_empty();
            m.in_reply_to = in_reply_to;
            m.references = references;
            self.store.mark_dirty();
            return Ok(true);
        }

        let now = self.clock.now()?;
        let id = self.mint_id("draft");
        let attachments = self.compose.attachments.clone();
        let draft = Message {
            id: id.clone(),
            account_id: from_id,
            location: MessageLocation::Mailbox(MailboxKind::Drafts),
            sender_name: from_name,
            sender_email: from_email,
            to: to_list,
            subject: self.compose.subject.clone(),
            preview,
            body: self.compose.body.clone(),
            body_html: self.compose.body_html.clone(),
            date: now,
            unread: false,
            flagged: false,
            has_attachment: !attachments.is_empty(),
            attachments,
            has_remote_content: false,
            has_inline_image: false,
            blocked: false,
            in_reply_to,
            body_state: crate::model::types::MessageBodyState::Complete,
            references,
            snoozed_until: None,
            send_at: None,
            cc: split_addresses(&self.compose.cc),
            bcc: split_addresses(&self.compose.bcc),
            unsubscribe: None,
        };
        self.store.messages.push(draft);
        self.store.mark_dirty();
        self.compose.editing_draft_id = Some(id);
        Ok(true)
    }
}

fn blank_seed() -> (
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
) {
    (
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        None,
        None,
    )
}

include!("compose/content.rs");
