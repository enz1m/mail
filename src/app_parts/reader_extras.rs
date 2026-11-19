use super::*;

impl State {
    pub(crate) fn reader_html_source<'a>(
        &self,
        message: &'a model::types::Message,
        quotes: &'a model::quoted::QuotedBody,
    ) -> Option<&'a str> {
        if !self.quoted_text_expanded.contains(&message.id)
            && let Some(source) = quotes.html_collapsed.as_deref()
        {
            return Some(source);
        }
        message.body_html.as_deref()
    }

    pub(crate) fn effective_html_dark_palette(&self) -> Option<crate::mail_html::DarkPalette> {
        (*self.dark_html.get() && self.themes.theme().palette.is_dark())
            .then(|| crate::mail_html::DarkPalette::from_theme(self.themes.theme()))
    }
    pub(crate) fn shows_plain_text(&self, id: &str) -> bool {
        self.message_plain_text
            .get(id)
            .copied()
            .unwrap_or_else(|| *self.prefer_plain_text.get())
    }
    pub(crate) fn html_images_allowed(&self, message: &model::types::Message) -> bool {
        !self.privacy.proxy_remote_content
            && !crate::mail_actions::sender_is_blocked(
                &self.privacy.block_list,
                &message.sender_email,
            )
            && (self.remote_images_once.contains(&message.id)
                || matches!(
                    model::privacy::remote_content_decision(
                        self.privacy.policy,
                        &self.privacy.allow_list,
                        &self.privacy.block_list,
                        &message.sender_email,
                        true,
                    ),
                    model::privacy::RemoteContentDecision::Allowed(_)
                ))
    }

    pub(crate) fn html_reader_active(&self) -> bool {
        !self.compose.open
            && (self.mode() != LayoutMode::Narrow
                || self.narrow_nav.pane() == model::layout::NarrowPane::Detail)
            && self
                .selected_conversation
                .as_deref()
                .is_some_and(|conversation| {
                    self.message_ids_of(conversation).iter().any(|id| {
                        !self.shows_plain_text(id)
                            && self
                                .store
                                .message(id)
                                .is_some_and(|message| message.body_html.is_some())
                    })
                })
    }

    pub(crate) fn update_html_reader(&mut self) -> Task<Message> {
        if !self.html_reader_active() {
            self.html_reader.retain(&[]);
            return Task::none();
        }
        let palette = self.effective_html_dark_palette();
        self.html_reader.set_dark_palette(palette);
        let ids = self
            .selected_conversation
            .as_deref()
            .map(|id| self.message_ids_of(id))
            .unwrap_or_default();
        for id in &ids {
            if self.shows_plain_text(id)
                && let Some((visible, _)) = self.html_reader.visible.get_mut(id)
            {
                *visible = false;
            }
        }
        let mut requests: Vec<_> = ids
            .iter()
            .filter(|id| !self.shows_plain_text(id))
            .filter_map(|id| {
                self.html_reader
                    .visible_width(id)
                    .map(|width| (id.clone(), width))
            })
            .take(4)
            .collect();
        let width = requests.first().map_or_else(
            || {
                let sidebar = if self.mode() == LayoutMode::Wide
                    && self.nav.preference
                        != enzui_iced_widgets::nav_split::SidebarPreference::Hidden
                {
                    220.0
                } else {
                    0.0
                };
                let list = match self.mode() {
                    LayoutMode::Wide => 340.0,
                    LayoutMode::Medium => 320.0,
                    LayoutMode::Narrow => 0.0,
                };
                (self.window_size.width - sidebar - list - 28.0).max(240.0) as u32
            },
            |(_, width)| *width,
        );
        let conversations = self.store.conversations_shared(&self.selection);
        let index = conversations
            .iter()
            .position(|conversation| Some(&conversation.id) == self.selected_conversation.as_ref())
            .unwrap_or(0);
        let neighbors = std::iter::once(index)
            .chain((index + 1)..conversations.len())
            .chain((0..index).rev());
        for index in neighbors.take(8) {
            if requests.len() >= 2 {
                break;
            }
            if let Some(message) = conversations
                .get(index)
                .and_then(|conversation| conversation.message_ids.last())
                .and_then(|id| self.store.message(id))
                && message.body_html.is_some()
                && !self.shows_plain_text(&message.id)
                && !requests.iter().any(|(id, _)| id == &message.id)
            {
                requests.push((message.id.clone(), width));
            }
        }
        for (id, width) in requests {
            if let Some(message) = self.store.message(&id)
                && !self.shows_plain_text(&message.id)
            {
                let quotes = self.store.quoted_body(message);
                if let Some(source) = self.reader_html_source(message, &quotes) {
                    let allowed = self.html_images_allowed(message);
                    self.html_reader.prepare(&id, source, allowed, width);
                }
            }
        }
        self.html_reader.retain(&ids);
        let (links, copies) = self.html_reader.drain();
        let mut tasks = links
            .into_iter()
            .map(|url| self.update(Message::RequestOpenUrl(url)))
            .collect::<Vec<_>>();
        tasks.extend(
            self.html_reader
                .allocation_requests()
                .into_iter()
                .map(|request| {
                    iced::widget::image::allocate(request.handle.clone())
                        .map(move |result| Message::HtmlAllocated(request.clone(), result))
                }),
        );
        tasks.extend(
            copies
                .into_iter()
                .map(|text| iced::clipboard::write(text).map(|_| Message::ClipboardWritten)),
        );
        Task::batch(tasks)
    }

    // Start reading the open conversation's previewable attachments, once per
    // conversation and store generation, and forget previews of any other.
    pub(crate) fn request_attachment_previews(&mut self) -> Task<Message> {
        let key = self
            .selected_conversation
            .clone()
            .map(|conversation| (conversation, self.store.generation()));
        if key == self.previews_for {
            return Task::none();
        }
        self.previews_for.clone_from(&key);
        let wanted: Vec<model::types::Attachment> = key
            .map(|(conversation, _)| self.message_ids_of(&conversation))
            .unwrap_or_default()
            .iter()
            .filter_map(|id| self.store.message(id))
            .flat_map(|message| message.attachments.iter())
            .filter(|attachment| {
                attachment.payload_id.is_some() && preview_kind(attachment).is_some()
            })
            .cloned()
            .collect();
        let keys: HashSet<String> = wanted
            .iter()
            .filter_map(|attachment| attachment.payload_id.as_ref())
            .map(|id| id.as_str().to_owned())
            .collect();
        self.attachment_previews.retain(|key, _| keys.contains(key));
        let mut tasks = Vec::new();
        for attachment in wanted {
            let Some(key) = attachment
                .payload_id
                .as_ref()
                .map(|id| id.as_str().to_owned())
            else {
                continue;
            };
            if self.attachment_previews.contains_key(&key) {
                continue;
            }
            self.attachment_previews
                .insert(key.clone(), AttachmentPreview::Loading);
            let store = self.attachment_store.clone();
            tasks.push(Task::perform(
                self.submit_blocking(move || read_preview(&store, &attachment)),
                move |settlement| {
                    let payload = settlement.into_result().unwrap_or_default();
                    Message::AttachmentPreviewLoaded(key, payload)
                },
            ));
        }
        Task::batch(tasks)
    }

    pub(crate) fn quick_reply_text(&self) -> Option<String> {
        if self.quick_reply_for != self.selected_conversation {
            return None;
        }
        let text = self.quick_reply.text();
        let text = text.trim_end();
        (!text.trim().is_empty()).then(|| text.to_owned())
    }

    pub(crate) fn save_inline_reply(&mut self) -> Task<Message> {
        if self.compose.open || self.pending_send.is_some() {
            return Task::none();
        }
        let Some(text) = self.quick_reply_text() else {
            return Task::none();
        };
        let Some(mode) = self.quick_reply_mode.clone() else {
            return Task::none();
        };
        if !self.open_compose(mode) {
            return Task::none();
        }
        self.compose_generation = self.compose_generation.wrapping_add(1);
        self.compose.body = format!("{text}\n\n{}", self.compose.body);
        self.quick_reply = iced::widget::text_editor::Content::new();
        self.quick_reply_for = None;
        self.quick_reply_mode = None;
        if let Some(id) = self.save_compose_draft() {
            self.queue_server_mutation(local_state::ServerMutationKind::DraftUpserted, vec![id]);
            self.reconcile_selection();
            // Click-away saving must not restore focus over the clicked control.
            return Task::none();
        }
        self.sync_body_editor();
        self.arm_compose_focus()
    }

    pub(crate) fn quick_reply(&mut self, send: bool) -> Task<Message> {
        if self.compose.open {
            return Task::none();
        }
        let text = self.quick_reply_text();
        if send && text.is_none() {
            return Task::none();
        }
        let Some(target) = crate::view::action_target_for_breadth(self) else {
            return Task::none();
        };
        let anchor = target.anchor_message_id().to_owned();
        let mode = self
            .quick_reply_mode
            .clone()
            .unwrap_or(ComposeMode::Reply(anchor));
        let previous_mode = self.quick_reply_mode.take();
        let open = self.update(Message::OpenCompose(mode));
        if !self.compose.open {
            self.quick_reply_mode = previous_mode;
            return open;
        }
        if let Some(text) = text {
            self.compose.body = format!("{text}\n\n{}", self.compose.body);
            self.sync_body_editor();
            self.position_compose_caret(text.len());
        }
        self.quick_reply = iced::widget::text_editor::Content::new();
        self.quick_reply_for = None;
        self.quick_reply_mode = None;
        if !send {
            return open;
        }
        let sent = self.update(Message::SendCompose);
        Task::batch([open, sent])
    }

    pub(crate) fn unsubscribe(&mut self, message_id: String) -> Task<Message> {
        let Some(message) = self.store.message(&message_id) else {
            return Task::none();
        };
        let Some(route) = message.unsubscribe.clone() else {
            return Task::none();
        };
        let sender = message.sender_email.to_ascii_lowercase();
        if matches!(
            self.unsubscribes.get(&sender),
            Some(UnsubscribeProgress::Sending | UnsubscribeProgress::Done)
        ) {
            return Task::none();
        }
        if route.one_click
            && let Some(url) = route.https().map(str::to_owned)
        {
            if self.fixture_mode {
                self.unsubscribes.insert(sender, UnsubscribeProgress::Done);
                return Task::none();
            }
            self.unsubscribes
                .insert(sender.clone(), UnsubscribeProgress::Sending);
            return Task::perform(
                self.submit_blocking(move || crate::backend::unsubscribe::one_click(&url)),
                move |settlement| {
                    let result = match settlement.into_result() {
                        Ok(result) => result,
                        Err(_) => Err(crate::l10n::tr(
                            "Background work is unavailable until Mail restarts.",
                        )),
                    };
                    Message::UnsubscribeFinished(sender, result)
                },
            );
        }
        if let Some(url) = route.https() {
            return self.update(Message::RequestOpenUrl(url.to_owned()));
        }
        let Some(mailto) = route.mailto() else {
            return Task::none();
        };
        self.desktop
            .enqueue(enzui_iced_core::activation::ApplicationIntent::Open(vec![
                mailto.to_owned(),
            ]));
        self.desktop_pending()
    }
}

pub(crate) fn preview_kind(attachment: &model::types::Attachment) -> Option<PreviewKind> {
    let mime = attachment.mime.to_ascii_lowercase();
    if matches!(
        mime.as_str(),
        "image/png" | "image/jpeg" | "image/jpg" | "image/gif" | "image/webp" | "image/bmp"
    ) {
        Some(PreviewKind::Image)
    } else if mime == "text/plain" || mime == "text/csv" {
        Some(PreviewKind::Text)
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PreviewKind {
    Image,
    Text,
}

const PREVIEW_IMAGE_MAX_BYTES: u64 = 10 * 1024 * 1024;
const PREVIEW_IMAGE_BOUNDS: (u32, u32) = (720, 360);
const PREVIEW_TEXT_CHARS: usize = 600;

pub(crate) fn read_preview(
    store: &attachment_store::AttachmentStore,
    attachment: &model::types::Attachment,
) -> Option<PreviewPayload> {
    let kind = preview_kind(attachment)?;
    if kind == PreviewKind::Image && attachment.size > PREVIEW_IMAGE_MAX_BYTES {
        return None;
    }
    let bytes = store.read(attachment).ok()?;
    match kind {
        PreviewKind::Image => enzui_iced_resources::decode_static_resized(
            &bytes,
            &enzui_iced_core::decode_limits::DecodeLimits::DEFAULT,
            PREVIEW_IMAGE_BOUNDS.0,
            PREVIEW_IMAGE_BOUNDS.1,
            || false,
        )
        .ok()
        .map(|decoded| PreviewPayload::Image(decoded.handle)),
        PreviewKind::Text => {
            let text = String::from_utf8_lossy(&bytes);
            let snippet: String = text.chars().take(PREVIEW_TEXT_CHARS).collect();
            (!snippet.trim().is_empty()).then_some(PreviewPayload::Text(snippet))
        }
    }
}
