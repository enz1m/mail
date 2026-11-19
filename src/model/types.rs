use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsStr;

use super::html;
use super::html::RenderedHtml;
use super::privacy::RemoteContentPolicy;
use super::thread::{self, Conversation};

#[path = "types/mailbox_kinds.rs"]
mod mailbox_kinds;

pub(crate) use mailbox_kinds::{FavoriteKind, MailboxKind, SidebarSelection};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Account {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) email: String,
    pub(crate) mailboxes: Vec<MailboxKind>,
    pub(crate) folders: Vec<Folder>,
    // Server and authentication configuration. `None` identifies an explicit
    // fixture account that cannot be passed to the production backend.
    pub(crate) config: Option<crate::backend::AccountConfig>,
    pub(crate) signature: String,
    pub(crate) append_signature: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Folder {
    pub(crate) id: String,
    pub(crate) name: String,
}

// Opaque identifier for bytes in the app-owned attachment store.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct AttachmentPayloadId(String);

impl AttachmentPayloadId {
    pub(crate) fn parse(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then_some(Self(value.to_ascii_lowercase()))
    }

    pub(crate) fn from_bytes(bytes: [u8; 16]) -> Self {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut value = String::with_capacity(32);
        for byte in bytes {
            value.push(char::from(HEX[usize::from(byte >> 4)]));
            value.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        Self(value)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn file_name(&self) -> &OsStr {
        OsStr::new(&self.0)
    }

    pub(crate) fn from_file_name(value: &str) -> Option<Self> {
        Self::parse(value)
    }

    pub(crate) fn is_valid(&self) -> bool {
        self.0.len() == 32 && self.0.bytes().all(|byte| byte.is_ascii_hexdigit())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct AttachmentPayloadIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
    pub(crate) size: u64,
    pub(crate) mtime_secs: i64,
    pub(crate) mtime_nanos: i64,
    pub(crate) ctime_secs: i64,
    pub(crate) ctime_nanos: i64,
}

impl AttachmentPayloadIdentity {
    pub(crate) fn is_valid(self) -> bool {
        self.inode != 0
            && self.mtime_nanos >= 0
            && self.mtime_nanos < 1_000_000_000
            && self.ctime_nanos >= 0
            && self.ctime_nanos < 1_000_000_000
    }
}

// Random store key paired with the exact descriptor identity it must denote.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct AttachmentPayloadRef {
    pub(crate) id: AttachmentPayloadId,
    pub(crate) identity: AttachmentPayloadIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Attachment {
    pub(crate) name: String,
    pub(crate) mime: String,
    pub(crate) size: u64,
    // `None` denotes a metadata-only record whose bytes are unavailable.
    pub(crate) payload_id: Option<AttachmentPayloadId>,
    pub(crate) payload_identity: Option<AttachmentPayloadIdentity>,
}

impl Attachment {
    pub(crate) fn payload_ref(&self) -> Option<AttachmentPayloadRef> {
        let id = self.payload_id.clone()?;
        let identity = self.payload_identity?;
        (id.is_valid() && identity.is_valid() && identity.size == self.size)
            .then_some(AttachmentPayloadRef { id, identity })
    }

    pub(crate) fn size_label(&self) -> String {
        format_size(self.size)
    }

    pub(crate) fn icon(&self) -> &'static str {
        let mime = self.mime.as_str();
        match mime {
            "application/pdf" => "file-text",
            "text/csv"
            | "application/vnd.ms-excel"
            | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
                "file-spreadsheet"
            }
            "application/zip" | "application/x-tar" | "application/x-gzip" => "file-archive",
            _ => match mime.split('/').next().unwrap_or("") {
                "image" => "file-image",
                "video" => "file-video-camera",
                "audio" => "file-music",
                "text" => "file-text",
                _ => "file",
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum MessageBodyState {
    Complete,
    Incomplete {
        retained_bytes: u32,
        limit_bytes: u32,
    },
    Malformed,
}

impl MessageBodyState {
    pub(crate) fn is_complete(self) -> bool {
        matches!(self, Self::Complete)
    }

    pub(crate) fn disclosure(self) -> String {
        match self {
            Self::Complete => String::new(),
            Self::Incomplete {
                retained_bytes,
                limit_bytes,
            } => format!(
                "Message body incomplete — {retained_bytes} bytes retained from a {limit_bytes}-byte bounded fetch."
            ),
            Self::Malformed => {
                "Message body unavailable — the MIME content could not be decoded safely."
                    .to_owned()
            }
        }
    }
}

// What a subject reads as: `(no subject)` when it is blank, as other
// clients show it, so a row or reader never shows an empty line.
pub(crate) fn shown_subject(subject: &str) -> std::borrow::Cow<'_, str> {
    if subject.trim().is_empty() {
        std::borrow::Cow::Owned(crate::l10n::current().text("mail-no-subject"))
    } else {
        std::borrow::Cow::Borrowed(subject)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Message {
    pub(crate) id: String,
    pub(crate) account_id: String,
    pub(crate) location: MessageLocation,
    pub(crate) sender_name: String,
    pub(crate) sender_email: String,
    pub(crate) to: Vec<String>,
    pub(crate) cc: Vec<String>,
    pub(crate) bcc: Vec<String>,
    pub(crate) subject: String,
    pub(crate) preview: String,
    pub(crate) body: String,
    pub(crate) body_html: Option<String>,
    pub(crate) date: MessageDate,
    pub(crate) unread: bool,
    pub(crate) flagged: bool,
    pub(crate) has_attachment: bool,
    pub(crate) attachments: Vec<Attachment>,
    pub(crate) has_remote_content: bool,
    pub(crate) has_inline_image: bool,
    pub(crate) blocked: bool,
    pub(crate) in_reply_to: Option<String>,
    // Completeness of the selected MIME body; incomplete or malformed input
    // is rendered as a disclosure state instead of as an empty plain body.
    pub(crate) body_state: MessageBodyState,
    // RFC 5322 `References:`: the full ancestry of this message, oldest first,
    // excluding its own id; `in_reply_to` is normally the last entry.
    //
    // Threading uses the chain, so a reply whose direct parent is not in the
    // current mailbox keeps its thread; see [`super::thread`].
    pub(crate) references: Vec<String>,
    pub(crate) unsubscribe: Option<ListUnsubscribe>,
    // Hidden until this local time, then back in its mailbox unread.
    pub(crate) snoozed_until: Option<MessageDate>,
    pub(crate) send_at: Option<MessageDate>,
}

// A mailing list's unsubscribe routes: the `mailto:` and `https:` URIs of
// `List-Unsubscribe` (RFC 2369), in the sender's order, and whether
// `List-Unsubscribe-Post` allows a one-click POST to the `https:` one (RFC 8058).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ListUnsubscribe {
    pub(crate) uris: Vec<String>,
    pub(crate) one_click: bool,
}

impl ListUnsubscribe {
    pub(crate) fn https(&self) -> Option<&str> {
        self.uris
            .iter()
            .map(String::as_str)
            .find(|uri| uri.starts_with("https://"))
    }

    pub(crate) fn mailto(&self) -> Option<&str> {
        self.uris
            .iter()
            .map(String::as_str)
            .find(|uri| uri.starts_with("mailto:"))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub(crate) enum MessageLocation {
    Mailbox(MailboxKind),
    Folder(String),
}

// A calendar day, kept as plain fields rather than a chrono type.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub(crate) struct MessageDate {
    pub(crate) year: u32,
    pub(crate) month: u32,
    pub(crate) day: u32,
    pub(crate) minute: u32,
}

impl MessageDate {
    pub(crate) fn ymd(year: u32, month: u32, day: u32) -> Self {
        Self {
            year,
            month,
            day,
            minute: 0,
        }
    }

    pub(crate) fn at(year: u32, month: u32, day: u32, hour: u32, minute: u32) -> Self {
        Self {
            year,
            month,
            day,
            minute: hour * 60 + minute,
        }
    }

    fn civil_date(self) -> Option<enzui_iced_core::civil_date::Date> {
        i32::try_from(self.year)
            .ok()
            .and_then(|year| enzui_iced_core::civil_date::Date::new(year, self.month, self.day))
    }

    fn localized_time(self) -> String {
        let locale =
            crate::l10n::current().locale_for(enzui_iced_core::localization::LocaleCategory::Time);
        enzui_iced_core::time_of_day::TimeOfDay::hm(self.minute / 60, self.minute % 60)
            .and_then(|time| locale.format_time(time, false).ok())
            .unwrap_or_else(|| format!("{:02}:{:02}", self.minute / 60, self.minute % 60))
    }

    pub(crate) fn relative_label(self, today: Self) -> String {
        match self
            .civil_date()
            .zip(today.civil_date())
            .map(|(date, now)| date.days_until(now))
        {
            Some(0) => crate::l10n::relative_time("mail-today-time", self.localized_time()),
            Some(1) => crate::l10n::relative_time("mail-yesterday-time", self.localized_time()),
            _ => self.medium_label(),
        }
    }

    pub(crate) fn short_label(self) -> String {
        self.civil_date()
            .and_then(|date| {
                crate::l10n::current()
                    .locale_for(enzui_iced_core::localization::LocaleCategory::Time)
                    .format_month_day(date)
                    .ok()
            })
            .unwrap_or_else(|| format!("{}-{:02}-{:02}", self.year, self.month, self.day))
    }

    pub(crate) fn detail_label(self) -> String {
        self.localized_date_time(true)
    }

    pub(crate) fn medium_label(self) -> String {
        self.localized_date_time(self.minute != 0)
    }

    fn localized_date_time(self, include_time: bool) -> String {
        let date = self
            .civil_date()
            .and_then(|date| {
                crate::l10n::current()
                    .locale_for(enzui_iced_core::localization::LocaleCategory::Time)
                    .format_date(date, enzui_iced_core::localization::DateStyle::Medium)
                    .ok()
            })
            .unwrap_or_else(|| format!("{}-{:02}-{:02}", self.year, self.month, self.day));
        if include_time {
            crate::l10n::date_time(date, self.localized_time())
        } else {
            date
        }
    }
}

// Derived caches are invalidated by the store generation, which also governs persistence.
#[derive(Clone, Debug, Default)]
struct Derived {
    // The `generation` these caches were built against. A mismatch means the
    // store mutated and everything below is discarded before use.
    built_at: u64,
    search: HashMap<String, String>,
    unread: Option<HashMap<SidebarSelection, usize>>,
    convos: Option<(SidebarSelection, std::rc::Rc<Vec<Conversation>>)>,
    html: HashMap<(String, bool), RenderedHtml>,
    quotes: HashMap<String, std::rc::Rc<super::quoted::QuotedBody>>,
}

impl Derived {
    fn reset(&mut self, generation: u64) {
        self.built_at = generation;
        self.search.clear();
        self.unread = None;
        self.convos = None;
        self.html.clear();
        self.quotes.clear();
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MailboxStore {
    pub(crate) accounts: Vec<Account>,
    pub(crate) messages: Vec<Message>,
    pub(crate) today: MessageDate,
    // Bumped on durable account or message mutations. Derived caches and the
    // local-state persistence gate both use this generation.
    generation: u64,
    derived: RefCell<Derived>,
}

// The uncached, lowercased searchable plaintext of a message body: the
// sanitised block text for HTML (never raw markup), else the body.
pub(crate) fn searchable_body_lower(m: &Message) -> String {
    if !m.body_state.is_complete() {
        return String::new();
    }
    let base = match &m.body_html {
        Some(html_src) => {
            let rendered = html::sanitize_and_render_html(html_src, html::SanitizeOptions::SAFE);
            html::blocks_to_plain_text(&rendered.blocks)
        }
        None => m.body.clone(),
    };
    base.to_ascii_lowercase()
}

// Product sidebar rows cached in the shared unread-count pass.
// Compatibility-only smart mailboxes are evaluated on demand when a
// persisted layout requests them.
fn contributing_selections(m: &Message, today: MessageDate) -> Vec<SidebarSelection> {
    if m.send_at.is_some() && matches!(m.location, MessageLocation::Mailbox(MailboxKind::Drafts)) {
        return vec![SidebarSelection::SmartMailbox(
            super::sidebar_layout::SmartMailboxKind::Scheduled,
        )];
    }
    if m.snoozed_until.is_some()
        && !matches!(
            m.location,
            MessageLocation::Mailbox(MailboxKind::Trash | MailboxKind::Junk)
        )
    {
        return vec![SidebarSelection::SmartMailbox(
            super::sidebar_layout::SmartMailboxKind::Snoozed,
        )];
    }
    let mut v: Vec<SidebarSelection> = Vec::with_capacity(4);
    match &m.location {
        MessageLocation::Mailbox(kind) => {
            v.push(SidebarSelection::AccountMailbox {
                account_id: m.account_id.clone(),
                mailbox: *kind,
            });
            match kind {
                MailboxKind::Inbox => v.push(SidebarSelection::Favorite(FavoriteKind::Inbox)),
                MailboxKind::Drafts => v.push(SidebarSelection::Favorite(FavoriteKind::Drafts)),
                MailboxKind::Sent => v.push(SidebarSelection::Favorite(FavoriteKind::Sent)),
                _ => {}
            }
        }
        MessageLocation::Folder(folder_id) => v.push(SidebarSelection::Folder {
            account_id: m.account_id.clone(),
            folder_id: folder_id.clone(),
        }),
    }
    if m.flagged {
        v.push(SidebarSelection::Favorite(FavoriteKind::Flagged));
    }
    for kind in super::sidebar_layout::SmartMailboxKind::BUILT_INS {
        if kind.matches(m, today) {
            v.push(SidebarSelection::SmartMailbox(kind));
        }
    }
    v
}

impl MailboxStore {
    pub(crate) fn new(accounts: Vec<Account>, messages: Vec<Message>, today: MessageDate) -> Self {
        Self {
            accounts,
            messages,
            today,
            generation: 0,
            derived: RefCell::new(Derived::default()),
        }
    }

    // Invalidates derived message caches and advances durable-state tracking.
    pub(crate) fn mark_dirty(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    fn messages_for(&self, selection: &SidebarSelection) -> Vec<&Message> {
        self.messages
            .iter()
            .filter(|m| selection_matches(selection, m, self.today))
            .collect()
    }

    pub(crate) fn message_count(&self, selection: &SidebarSelection) -> usize {
        self.messages_for(selection).len()
    }

    // Maintained unread count for a sidebar row.
    //
    // Product rows share one memoized store pass. A compatibility-only smart
    // mailbox retained by persisted state is scanned only when displayed.
    pub(crate) fn unread_count(&self, selection: &SidebarSelection) -> usize {
        let mut d = self.derived.borrow_mut();
        if d.built_at != self.generation {
            d.reset(self.generation);
        }
        if d.unread.is_none() {
            let mut counts: HashMap<SidebarSelection, usize> = HashMap::new();
            for m in &self.messages {
                if !m.unread {
                    continue;
                }
                for sel in contributing_selections(m, self.today) {
                    *counts.entry(sel).or_insert(0) += 1;
                }
            }
            d.unread = Some(counts);
        }
        let cached = d
            .unread
            .as_ref()
            .and_then(|counts| counts.get(selection))
            .copied();
        if let Some(count) = cached {
            return count;
        }
        let compatibility_smart_mailbox = matches!(
            selection,
            SidebarSelection::SmartMailbox(kind)
                if !super::sidebar_layout::SmartMailboxKind::BUILT_INS.contains(kind)
        );
        drop(d);
        if compatibility_smart_mailbox {
            self.unread_count_uncached(selection)
        } else {
            0
        }
    }

    pub(crate) fn unread_count_uncached(&self, selection: &SidebarSelection) -> usize {
        self.messages_for(selection)
            .into_iter()
            .filter(|m| m.unread)
            .count()
    }

    pub(crate) fn body_matches_query(&self, m: &Message, q_lower: &str) -> bool {
        let mut d = self.derived.borrow_mut();
        if d.built_at != self.generation {
            d.reset(self.generation);
        }
        d.search
            .entry(m.id.clone())
            .or_insert_with(|| searchable_body_lower(m))
            .contains(q_lower)
    }

    pub(crate) fn render_html_cached(
        &self,
        id: &str,
        source: &str,
        allow_remote_images: bool,
    ) -> RenderedHtml {
        let mut d = self.derived.borrow_mut();
        if d.built_at != self.generation {
            d.reset(self.generation);
        }
        d.html
            .entry((id.to_owned(), allow_remote_images))
            .or_insert_with(|| {
                html::sanitize_and_render_html(
                    source,
                    html::SanitizeOptions {
                        allow_remote_images,
                    },
                )
            })
            .clone()
    }

    pub(crate) fn quoted_body(&self, message: &Message) -> std::rc::Rc<super::quoted::QuotedBody> {
        let mut d = self.derived.borrow_mut();
        if d.built_at != self.generation {
            d.reset(self.generation);
        }
        std::rc::Rc::clone(
            d.quotes
                .entry(message.id.clone())
                .or_insert_with(|| std::rc::Rc::new(super::quoted::QuotedBody::of(message))),
        )
    }

    pub(crate) fn sorted_messages(&self, selection: &SidebarSelection) -> Vec<&Message> {
        let mut list = self.messages_for(selection);
        list.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| a.id.cmp(&b.id)));
        list
    }

    pub(crate) fn conversations_for(&self, selection: &SidebarSelection) -> Vec<Conversation> {
        self.conversations_shared(selection).as_ref().clone()
    }

    // Shares the cached conversation model without cloning every row.
    pub(crate) fn conversations_shared(
        &self,
        selection: &SidebarSelection,
    ) -> std::rc::Rc<Vec<Conversation>> {
        {
            let mut d = self.derived.borrow_mut();
            if d.built_at != self.generation {
                d.reset(self.generation);
            }
            if let Some((sel, convos)) = &d.convos
                && sel == selection
            {
                return std::rc::Rc::clone(convos);
            }
        }
        let convos = std::rc::Rc::new(self.conversations_for_uncached(selection));
        let mut d = self.derived.borrow_mut();
        d.convos = Some((selection.clone(), std::rc::Rc::clone(&convos)));
        convos
    }

    pub(crate) fn conversations_for_uncached(
        &self,
        selection: &SidebarSelection,
    ) -> Vec<Conversation> {
        let mut messages = self.sorted_messages(selection);
        let anchors: std::collections::HashSet<_> =
            messages.iter().map(|m| m.id.as_str()).collect();
        if matches!(
            selection,
            SidebarSelection::Favorite(FavoriteKind::Inbox | FavoriteKind::Flagged)
                | SidebarSelection::AccountMailbox {
                    mailbox: MailboxKind::Inbox | MailboxKind::Archive,
                    ..
                }
                | SidebarSelection::Folder { .. }
        ) {
            let accounts: std::collections::HashSet<_> =
                messages.iter().map(|m| m.account_id.as_str()).collect();
            // Sent copies supply thread context, never independent rows in an incoming folder.
            messages.extend(self.messages.iter().filter(|m| {
                m.location == MessageLocation::Mailbox(MailboxKind::Sent)
                    && accounts.contains(m.account_id.as_str())
                    && !anchors.contains(m.id.as_str())
            }));
        }
        let mut convos = thread::thread_messages(&messages);
        convos.retain(|c| c.message_ids.iter().any(|id| anchors.contains(id.as_str())));
        // In Sent and Drafts every sender is the account owner, so participants come
        // from `Message::to` and the row reads "To: Alejandro".
        if outgoing_mailbox(selection) {
            for convo in &mut convos {
                let mut names: Vec<String> = Vec::new();
                let mut seen_lower: Vec<String> = Vec::new();
                for mid in &convo.message_ids {
                    let Some(m) = self.message(mid) else { continue };
                    for addr in &m.to {
                        let display = display_name_of(addr);
                        if display.is_empty() {
                            continue;
                        }
                        let key = display.to_ascii_lowercase();
                        if !seen_lower.iter().any(|s| s == &key) {
                            seen_lower.push(key);
                            names.push(display);
                        }
                    }
                }
                convo.participants = names;
            }
        }
        convos
    }

    pub(crate) fn account(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    pub(crate) fn message(&self, id: &str) -> Option<&Message> {
        self.messages.iter().find(|m| m.id == id)
    }
}

// Whether a sidebar row is Sent or Drafts, where rows and reader show
// recipients because the user sent every message.
pub(crate) fn outgoing_mailbox(selection: &SidebarSelection) -> bool {
    matches!(
        selection,
        SidebarSelection::Favorite(FavoriteKind::Sent | FavoriteKind::Drafts)
            | SidebarSelection::AccountMailbox {
                mailbox: MailboxKind::Sent | MailboxKind::Drafts,
                ..
            }
    )
}

// Extract the display portion of an RFC 5322 address string.
//
// * `"Alejandro <alejandro@example.com>"` → `"Alejandro"`
// * `"alejandro@example.com"` → `"alejandro@example.com"` (no phrase, so the
//   raw address)
pub(crate) fn display_name_of(addr: &str) -> String {
    if let Some((phrase, _)) = addr.split_once('<') {
        let phrase = phrase.trim().trim_matches('"').trim();
        if !phrase.is_empty() {
            return phrase.to_owned();
        }
    }
    addr.trim().to_owned()
}

pub(crate) fn selection_matches(sel: &SidebarSelection, m: &Message, today: MessageDate) -> bool {
    // A scheduled draft waits apart from Drafts until it sends.
    if m.send_at.is_some() && matches!(m.location, MessageLocation::Mailbox(MailboxKind::Drafts)) {
        return *sel
            == SidebarSelection::SmartMailbox(super::sidebar_layout::SmartMailboxKind::Scheduled);
    }
    // A snoozed message is away from every mailbox until it wakes.
    if m.snoozed_until.is_some()
        && !matches!(
            m.location,
            MessageLocation::Mailbox(MailboxKind::Trash | MailboxKind::Junk)
        )
    {
        return *sel
            == SidebarSelection::SmartMailbox(super::sidebar_layout::SmartMailboxKind::Snoozed);
    }
    match sel {
        SidebarSelection::Favorite(FavoriteKind::Inbox) => {
            matches!(m.location, MessageLocation::Mailbox(MailboxKind::Inbox))
        }
        SidebarSelection::Favorite(FavoriteKind::Flagged) => m.flagged,
        SidebarSelection::Favorite(FavoriteKind::Drafts) => {
            matches!(m.location, MessageLocation::Mailbox(MailboxKind::Drafts))
        }
        SidebarSelection::Favorite(FavoriteKind::Sent) => {
            matches!(m.location, MessageLocation::Mailbox(MailboxKind::Sent))
        }
        SidebarSelection::SmartMailbox(kind) => kind.matches(m, today),
        SidebarSelection::AccountMailbox {
            account_id,
            mailbox,
        } => {
            &m.account_id == account_id
                && matches!(&m.location, MessageLocation::Mailbox(k) if k == mailbox)
        }
        SidebarSelection::Folder {
            account_id,
            folder_id,
        } => {
            &m.account_id == account_id
                && matches!(&m.location, MessageLocation::Folder(id) if id == folder_id)
        }
    }
}

// The privacy state the reading pane consults to decide, per message,
// whether remote content loads; edited in Settings → Advanced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PrivacyState {
    pub(crate) policy: RemoteContentPolicy,
    pub(crate) allow_list: Vec<String>,
    pub(crate) block_list: Vec<String>,
    // "Hide my IP address (privacy proxy)": fetch remote images through a
    // privacy proxy. Off by default, since it trades one third party for another.
    pub(crate) proxy_remote_content: bool,
    // "Never automatically send read receipts". On by default: a read receipt
    // leaks an opened signal.
    pub(crate) auto_decline_read_receipts: bool,
}

impl Default for PrivacyState {
    fn default() -> Self {
        Self {
            policy: RemoteContentPolicy::AskPerSender,
            allow_list: Vec::new(),
            block_list: Vec::new(),
            proxy_remote_content: false,
            auto_decline_read_receipts: true,
        }
    }
}

impl PrivacyState {
    pub(crate) fn always_allow_sender(&mut self, sender_email: &str) {
        let addr = sender_email.trim().to_ascii_lowercase();
        if addr.is_empty() {
            return;
        }
        if !self
            .allow_list
            .iter()
            .any(|s| s.eq_ignore_ascii_case(&addr))
        {
            self.allow_list.push(addr);
        }
    }

    // Settings → Advanced → Blocklisted senders → Add: trims, lowercases and
    // de-duplicates.
    //
    // The blocklist wins over the allowlist, so the address also leaves the
    // allowlist. Returns `true` iff the list changed.
    pub(crate) fn block_sender(&mut self, sender_email: &str) -> bool {
        let addr = sender_email.trim().to_ascii_lowercase();
        if addr.is_empty() {
            return false;
        }
        self.allow_list.retain(|s| !s.eq_ignore_ascii_case(&addr));
        if self
            .block_list
            .iter()
            .any(|s| s.eq_ignore_ascii_case(&addr))
        {
            return false;
        }
        self.block_list.push(addr);
        true
    }

    // Settings → Advanced → Allowlisted senders → Add. Mirror of
    // [`Self::block_sender`]: an allowed sender must leave the blocklist, or
    // the blocklist's precedence would make the new allow entry inert.
    // Returns `true` iff the list changed.
    pub(crate) fn allow_sender(&mut self, sender_email: &str) -> bool {
        let addr = sender_email.trim().to_ascii_lowercase();
        if addr.is_empty() {
            return false;
        }
        self.block_list.retain(|s| !s.eq_ignore_ascii_case(&addr));
        if self
            .allow_list
            .iter()
            .any(|s| s.eq_ignore_ascii_case(&addr))
        {
            return false;
        }
        self.allow_list.push(addr);
        true
    }

    pub(crate) fn remove_allowed(&mut self, sender_email: &str) {
        self.allow_list
            .retain(|s| !s.eq_ignore_ascii_case(sender_email));
    }

    pub(crate) fn remove_blocked(&mut self, sender_email: &str) {
        self.block_list
            .retain(|s| !s.eq_ignore_ascii_case(sender_email));
    }
}

pub(crate) fn selection_display_label(sel: &SidebarSelection, accounts: &[Account]) -> String {
    match sel {
        SidebarSelection::Favorite(f) => crate::l10n::tr(f.label()),
        SidebarSelection::SmartMailbox(kind) => crate::l10n::tr(kind.label()),
        SidebarSelection::AccountMailbox { mailbox, .. } => crate::l10n::tr(mailbox.label()),
        SidebarSelection::Folder {
            account_id,
            folder_id,
        } => accounts
            .iter()
            .find(|account| &account.id == account_id)
            .and_then(|account| {
                account
                    .folders
                    .iter()
                    .find(|folder| &folder.id == folder_id)
            })
            .map_or_else(|| folder_id.clone(), |folder| folder.name.clone()),
    }
}

pub(crate) fn mailbox_summary_label(
    selection: &SidebarSelection,
    store: &MailboxStore,
    _accounts: &[Account],
) -> String {
    let label = selection_display_label(selection, &store.accounts);
    let mut arguments = enzui_iced_core::localization::Arguments::new();
    arguments.set("label", label);
    arguments.set("count", store.message_count(selection));
    arguments.set("unread", store.unread_count(selection));
    crate::l10n::current().text_with("mail-mailbox-summary", &arguments)
}

pub(crate) fn format_size(bytes: u64) -> String {
    enzui_iced_widgets::file_browser::format_bytes(bytes)
}
