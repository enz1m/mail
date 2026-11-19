//! Attachment receipt ownership until private staging completes.

use enzui_iced_widgets::file_drop::{DragAction, FileDrop};
use iced::Task;

use crate::{AttachmentRetry, Message, State};

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Received(u64, FileDrop),
}

#[derive(Default)]
pub(crate) struct NativeDrop {
    pending: Option<(u64, FileDrop)>,
}

impl NativeDrop {
    pub(crate) fn take_pending(&mut self, generation: u64) -> Option<FileDrop> {
        if self
            .pending
            .as_ref()
            .is_some_and(|(current, _)| *current == generation)
        {
            self.pending.take().map(|(_, receipt)| receipt)
        } else {
            None
        }
    }
}

impl State {
    pub(crate) fn accepts_attachment_drop(&self) -> bool {
        self.compose.open
            && !self.close_committed
            && self.close_settlement.is_none()
            && self.attachment_stage_inflight == 0
            && !self.attach_chooser.is_pending()
    }

    pub(crate) fn native_drop_event(&mut self, event: Event) -> Task<Message> {
        let Event::Received(generation, receipt) = event;
        if !self.accepts_attachment_drop()
            || generation != self.attachment_picker_generation
            || receipt
                .action()
                .is_some_and(|action| action != DragAction::Copy)
            || receipt.paths().is_empty()
        {
            return Task::none();
        }
        let paths = receipt.paths().to_vec();
        // Only current-generation private staging may acknowledge source custody.
        self.native_drop.pending = Some((generation, receipt));
        self.attachment_stage_inflight = self.attachment_stage_inflight.saturating_add(1);
        self.stage_attachments_task(AttachmentRetry::Stage {
            generation,
            paths,
            existing_count: self.compose.attachments.len(),
            existing_bytes: self
                .compose
                .attachments
                .iter()
                .fold(0_u64, |total, attachment| {
                    total.saturating_add(attachment.size)
                }),
        })
    }
}

pub(crate) fn attachment_source<'a>(
    content: iced::Element<'a, crate::Message>,
    store: &crate::attachment_store::AttachmentStore,
    attachment: &crate::model::types::Attachment,
) -> iced::Element<'a, crate::Message> {
    if attachment.payload_ref().is_none() {
        return content;
    }
    let store = store.clone();
    let source_key = (attachment.payload_ref(), attachment.name.clone());
    let attachment = attachment.clone();
    enzui_iced_widgets::file_drag::enz_file_drag_source_with(content, move |cancel| {
        if cancel.is_terminal() {
            return Err("Drag cancelled".into());
        }
        let directory = tempfile::Builder::new()
            .prefix("enz-mail-drag-")
            .tempdir()
            .map_err(|error| error.to_string())?;
        let path = directory
            .path()
            .join(crate::attachment_store::bounded_display_name(
                &attachment.name,
            ));
        store
            .export(&attachment, &path)
            .map_err(|error| format!("{error:?}"))?;
        if cancel.is_terminal() {
            return Err("Drag cancelled".into());
        }
        Ok(enzui_iced_widgets::file_drag::PreparedFiles::owned(
            vec![path],
            directory,
        ))
    })
    .key(source_key)
    .allows_captured_press(true)
    .into()
}
