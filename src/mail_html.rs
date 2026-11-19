use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::Duration;

use enzui_iced_core::media::Rgba8Frame;
use enzui_webkit::InputEvent;
use enzui_webkit::web::{ViewId, ViewRouter};

const PLACEHOLDER: &str =
    "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";
const MAX_DOCUMENT_HEIGHT: u32 = 16_384;

fn tag(node: &blitz_dom::Node, value: &str) -> bool {
    node.data
        .is_element_with_tag_name(&html5ever::LocalName::from(value))
}

fn fluid_dimension(value: &str) -> bool {
    value
        .trim()
        .trim_end_matches("px")
        .trim()
        .parse::<f32>()
        .is_ok_and(|width| width >= 480.0)
}

fn fluid_layout_style(value: &str) -> String {
    value
        .split(';')
        .map(|declaration| {
            let Some((property, dimension)) = declaration.split_once(':') else {
                return declaration.to_owned();
            };
            let size = dimension.split('!').next().unwrap_or(dimension).trim();
            if matches!(
                property.trim().to_ascii_lowercase().as_str(),
                "width" | "min-width" | "max-width"
            ) && fluid_dimension(size)
            {
                format!(
                    "{property}:100%{}",
                    if dimension.contains('!') {
                        "!important"
                    } else {
                        ""
                    }
                )
            } else {
                declaration.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(";")
}

#[path = "mail_html/dark_mode.rs"]
mod dark_mode;
#[path = "mail_html/native.rs"]
mod native;
#[path = "mail_html/resource_policy.rs"]
mod resource_policy;
pub(crate) use dark_mode::DarkPalette;

fn same_pixels(left: &Rgba8Frame, right: &Rgba8Frame) -> bool {
    left.width() == right.width()
        && left.height() == right.height()
        && left.pixels() == right.pixels()
}

fn painted_frame(frame: &Rgba8Frame) -> bool {
    frame
        .pixels()
        .as_chunks::<4>()
        .0
        .iter()
        .any(|pixel| pixel[3] != 0)
}

#[cfg(test)]
pub(crate) fn document(source: &str, images_allowed: bool) -> String {
    approved_document(source, images_allowed).0
}

fn approved_document(source: &str, images_allowed: bool) -> (String, HashSet<String>) {
    let resources = Arc::new(std::sync::Mutex::new(HashSet::new()));
    let permissions = Arc::clone(&resources);
    let mut clean = ammonia::Builder::default();
    clean
        .add_tags([
            "style", "table", "tbody", "thead", "tfoot", "tr", "td", "th", "center", "font",
        ])
        .add_tag_attributes("blockquote", ["type"])
        .add_generic_attributes([
            "style",
            "class",
            "id",
            "width",
            "height",
            "align",
            "valign",
            "bgcolor",
            "border",
            "cellpadding",
            "cellspacing",
            "color",
            "size",
            "face",
        ])
        .clean_content_tags(HashSet::from([
            "script", "iframe", "object", "embed", "form", "noscript",
        ]))
        .url_schemes(HashSet::from(["https", "http", "mailto", "data"]))
        .attribute_filter(move |element, attribute, value| {
            if matches!(element, "table" | "td" | "div") {
                if attribute == "width" && fluid_dimension(value) {
                    return Some("100%".into());
                }
                if attribute == "style" {
                    return Some(fluid_layout_style(value).into());
                }
            }
            if element == "img" && attribute == "src" {
                if images_allowed && value.starts_with("https://") {
                    permissions
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(
                            url::Url::parse(value)
                                .map_or_else(|_| value.to_owned(), |url| url.to_string()),
                        );
                    return Some(value.into());
                }
                return Some(PLACEHOLDER.into());
            }
            if attribute == "href"
                && !["https://", "http://", "mailto:"]
                    .iter()
                    .any(|prefix| value.starts_with(prefix))
            {
                return None;
            }
            Some(value.into())
        });
    let body = clean.clean(source);
    let image_sources = if images_allowed {
        "data: https:"
    } else {
        "data:"
    };
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src {image_sources}; script-src 'none'; form-action 'none'; base-uri 'none'\"><style>html{{background:white;color:#202124}}body{{margin:0;padding:16px 20px;font:15px system-ui;overflow-wrap:anywhere}}img{{max-width:100%;object-fit:contain}}img[src^='data:image/gif']{{background:#e9e9ed}}table{{max-width:100%!important;box-sizing:border-box}}a{{color:#1264c4}}</style></head><body>{body}</body></html>"
    );
    let mut resources = resources
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if images_allowed {
        resources.extend(resource_policy::background_urls(&html));
    }
    (html, resources)
}

struct RenderRequest {
    id: String,
    source: String,
    images_allowed: bool,
    dark_palette: Option<DarkPalette>,
    width: u32,
    token: u64,
}

enum Command {
    Render(RenderRequest),
    Retain(Vec<String>),
    Input(ViewId, InputEvent),
    #[cfg(test)]
    #[allow(
        dead_code,
        reason = "Only the isolated HTML renderer runner sends worker fences."
    )]
    Fence(SyncSender<()>),
    Stop,
}

enum Event {
    Cursor {
        id: String,
        token: u64,
        view: ViewId,
        name: Option<String>,
    },
    Frame {
        id: String,
        token: u64,
        view: ViewId,
        frame: Arc<Rgba8Frame>,
        accessibility: Option<Arc<enzui_webkit::page_delivery::PageA11yDelivery>>,
        router: Arc<ViewRouter>,
    },
    Failed {
        id: String,
        token: u64,
    },
    Link(ViewId, u64, String),
    Copied(ViewId, u64, String),
}

#[derive(Clone)]
pub(crate) struct PresentationRequest {
    id: String,
    token: u64,
    view: ViewId,
    frame: Arc<Rgba8Frame>,
    pub(crate) handle: iced::widget::image::Handle,
    logical_width: u32,
}

#[derive(Clone)]
pub(crate) struct PresentedHtml {
    pub(crate) logical_width: u32,
    pub(crate) view: ViewId,
    pub(crate) frame: Arc<Rgba8Frame>,
    pub(crate) allocation: iced::widget::image::Allocation,
}

pub(crate) struct HtmlPane {
    pub(crate) presented: Option<PresentedHtml>,
    uploading: bool,
    pub(crate) cursor: Option<String>,
    pub(crate) view: Option<ViewId>,
    pub(crate) frame: Option<Arc<Rgba8Frame>>,
    pub(crate) frame_handle: Option<iced::widget::image::Handle>,
    pub(crate) failed: bool,
    pub(crate) height: f32,
    pub(crate) accessibility: Option<Arc<enzui_webkit::page_delivery::PageA11yDelivery>>,
    pub(crate) router: Option<Arc<ViewRouter>>,
    token: u64,
    needs_render: bool,
    source: String,
    images_allowed: bool,
    dark_palette: Option<DarkPalette>,
    width: u32,
    resize_candidate: Option<(u32, std::time::Instant)>,
}

impl HtmlPane {
    pub(crate) fn can_input(&self, shown: &PresentedHtml) -> bool {
        self.router.is_some()
            && self.view == Some(shown.view)
            && !self.failed
            && !self.needs_render
            && self.resize_candidate.is_none()
            && self.token == shown.frame.generation().raw()
    }

    pub(crate) fn display_matches(&self, source: &str, images_allowed: bool) -> bool {
        #[cfg(test)]
        if self.source.is_empty() {
            return !self.images_allowed || images_allowed;
        }
        self.source == source && (!self.images_allowed || images_allowed)
    }

    pub(crate) fn matches(&self, source: &str, images_allowed: bool) -> bool {
        #[cfg(test)]
        if self.source.is_empty() {
            return self.images_allowed == images_allowed;
        }
        self.source == source && self.images_allowed == images_allowed
    }
}

#[derive(Default)]
pub(crate) struct MailHtml {
    command: Option<SyncSender<Command>>,
    events: Option<Receiver<Event>>,
    native: Option<native::Worker>,
    pub(crate) panes: HashMap<String, HtmlPane>,
    pub(crate) visible: HashMap<String, (bool, u32)>,
    retained: Vec<String>,
    next_token: u64,
    watcher: Option<enzui_iced_core::native_loop::NativeLoop>,
    pub(crate) revision: u64,
    recent: Vec<String>,
    last_presented: Option<String>,
    dark_palette: Option<DarkPalette>,
}

impl MailHtml {
    pub(crate) fn set_dark_palette(&mut self, palette: Option<DarkPalette>) {
        self.dark_palette = palette;
    }

    pub(crate) fn prepare(&mut self, id: &str, source: &str, images_allowed: bool, width: u32) {
        let width = width.clamp(240, 1400);
        self.recent.retain(|cached| cached != id);
        self.recent.insert(0, id.to_owned());
        // Failed content stays on the safe fallback across viewport and theme churn.
        if self
            .panes
            .get(id)
            .is_some_and(|pane| pane.failed && pane.matches(source, images_allowed))
        {
            return;
        }
        if let Some(pane) = self.panes.get_mut(id)
            && !pane.needs_render
            && pane.matches(source, images_allowed)
            && pane.dark_palette == self.dark_palette
        {
            // Fractional shell geometry can round either way across modal transitions.
            if pane.width.abs_diff(width) <= 1 {
                pane.resize_candidate = None;
                return;
            }
            if pane.frame.is_some() {
                let (candidate, since) = pane
                    .resize_candidate
                    .get_or_insert_with(|| (width, std::time::Instant::now()));
                if *candidate != width {
                    *candidate = width;
                    *since = std::time::Instant::now();
                }
                if since.elapsed() < Duration::from_millis(160) {
                    return;
                }
            }
        }
        self.start();
        self.next_token = self.next_token.wrapping_add(1);
        let token = self.next_token;
        let frame = self
            .panes
            .get(id)
            .filter(|pane| pane.display_matches(source, images_allowed))
            .and_then(|pane| pane.frame.clone());
        let view = self
            .panes
            .get(id)
            .filter(|pane| pane.display_matches(source, images_allowed))
            .and_then(|pane| pane.view);
        self.panes.insert(
            id.to_owned(),
            HtmlPane {
                presented: self
                    .panes
                    .get(id)
                    .filter(|pane| pane.display_matches(source, images_allowed))
                    .and_then(|pane| pane.presented.clone()),
                uploading: false,
                cursor: None,
                view,
                frame,
                frame_handle: self
                    .panes
                    .get(id)
                    .filter(|pane| pane.display_matches(source, images_allowed))
                    .and_then(|pane| pane.frame_handle.clone()),
                failed: false,
                height: self.panes.get(id).map_or(480.0, |pane| pane.height),
                accessibility: None,
                router: None,
                token,
                needs_render: false,
                source: source.to_owned(),
                images_allowed,
                dark_palette: self.dark_palette,
                width,
                resize_candidate: None,
            },
        );
        self.sync_native_pages();
        let request = RenderRequest {
            id: id.to_owned(),
            source: source.to_owned(),
            images_allowed,
            dark_palette: self.dark_palette,
            width,
            token,
        };
        let admission = self
            .command
            .as_ref()
            .map(|command| command.try_send(Command::Render(request)));
        self.wake_native();
        if let Some(pane) = self.panes.get_mut(id) {
            match admission {
                Some(Ok(())) => {}
                Some(Err(mpsc::TrySendError::Full(_))) => {
                    pane.needs_render = true;
                }
                Some(Err(mpsc::TrySendError::Disconnected(_))) | None => {
                    pane.failed = true;
                }
            }
        }
    }

    pub(crate) fn report_visibility(&mut self, id: String, visible: bool, width: u32) {
        if visible {
            if self
                .panes
                .get(&id)
                .is_some_and(|pane| pane.presented.is_some() && pane.router.is_some())
            {
                self.last_presented = Some(id.clone());
            }
            self.recent.retain(|cached| cached != &id);
            self.recent.insert(0, id.clone());
        }
        self.visible.insert(id.clone(), (visible, width));
        if visible
            && !self.retained.contains(&id)
            && let Some(pane) = self.panes.get_mut(&id)
        {
            pane.needs_render = true;
        }
    }

    pub(crate) fn visible_width(&self, id: &str) -> Option<u32> {
        self.visible
            .get(id)
            .filter(|(visible, _)| *visible)
            .map(|(_, width)| *width)
    }

    pub(crate) fn previous_pane(
        &self,
        id: &str,
        mut allowed: impl FnMut(&str, &HtmlPane) -> bool,
    ) -> Option<&HtmlPane> {
        let previous = self
            .last_presented
            .as_ref()
            .filter(|previous| previous.as_str() != id)?;
        self.panes
            .get(previous)
            .filter(|pane| pane.frame.is_some() && allowed(previous, pane))
    }

    pub(crate) fn input(&mut self, view: ViewId, event: InputEvent) {
        if let Some(command) = &self.command
            && let Err(mpsc::TrySendError::Full(_)) = command.try_send(Command::Input(view, event))
            && let Some(native) = &self.native
        {
            native.cancel_input();
        }
        self.wake_native();
        self.arm_native();
    }

    pub(crate) fn subscription(
        &self,
    ) -> Option<iced::Subscription<enzui_iced_core::native_loop::NativeReady>> {
        self.watcher
            .as_ref()
            .map(|watcher| watcher.subscription(std::convert::identity))
    }

    pub(crate) fn stop_watcher(&mut self) {
        self.watcher = None;
    }

    fn arm_native(&self) {
        if let (Some(watcher), Some(native)) = (&self.watcher, &self.native) {
            let mut interest = native.interest();
            if let Some(deadline) = self
                .panes
                .iter()
                .filter(|(id, _)| self.visible.get(*id).is_some_and(|(visible, _)| *visible))
                .filter_map(|(_, pane)| {
                    pane.resize_candidate
                        .map(|(_, since)| since + Duration::from_millis(160))
                })
                .min()
            {
                interest = interest.deadline(deadline);
            }
            watcher.arm(interest);
        }
    }

    pub(crate) fn poll_interval(&self) -> Duration {
        self.native
            .as_ref()
            .map_or(Duration::from_millis(80), native::Worker::poll_interval)
    }

    pub(crate) fn retain(&mut self, ids: &[String]) {
        self.visible.retain(|id, _| ids.contains(id));
        self.sync_native_pages();
        self.trim_frames();
        while self.panes.len() > 24 {
            let Some(id) = self
                .recent
                .iter()
                .rev()
                .find(|id| !self.retained.contains(id) && !ids.contains(id))
                .cloned()
            else {
                break;
            };
            self.panes.remove(&id);
            self.recent.retain(|cached| cached != &id);
        }
    }

    fn sync_native_pages(&mut self) {
        let mut retained: Vec<_> = self
            .recent
            .iter()
            .filter(|id| self.visible_width(id).is_some())
            .take(4)
            .cloned()
            .collect();
        let capacity = retained.len().max(2);
        for id in &self.recent {
            if retained.len() == capacity {
                break;
            }
            if !retained.contains(id) && self.panes.get(id).is_some_and(|pane| !pane.failed) {
                retained.push(id.clone());
            }
        }
        retained.sort();
        if self.retained != retained
            && let Some(command) = &self.command
            && command.try_send(Command::Retain(retained.clone())).is_ok()
        {
            for id in &self.retained {
                if !retained.contains(id)
                    && let Some(pane) = self.panes.get_mut(id)
                {
                    pane.needs_render = true;
                }
            }
            self.retained = retained;
            self.wake_native();
        }
    }

    pub(crate) fn remove_deleted(&mut self, mut exists: impl FnMut(&str) -> bool) {
        self.panes.retain(|id, _| exists(id));
        self.recent.retain(|id| self.panes.contains_key(id));
        self.visible.retain(|id, _| self.panes.contains_key(id));
        self.sync_native_pages();
    }

    fn trim_frames(&mut self) {
        let mut bytes: usize = self
            .panes
            .values()
            .filter_map(|pane| pane.frame.as_ref())
            .map(|frame| frame.pixels().len())
            .sum();
        for id in self.recent.iter().rev() {
            if bytes <= 64 * 1024 * 1024 {
                break;
            }
            if self.last_presented.as_ref() != Some(id)
                && !self.visible.get(id).is_some_and(|(visible, _)| *visible)
                && let Some(pane) = self.panes.get_mut(id)
            {
                if let Some(frame) = pane.frame.take() {
                    bytes = bytes.saturating_sub(frame.pixels().len());
                }
                pane.frame_handle = None;
                pane.presented = None;
                pane.view = None;
                pane.needs_render = true;
            }
        }
    }

    pub(crate) fn allocation_requests(&mut self) -> Vec<PresentationRequest> {
        self.panes
            .iter_mut()
            .filter_map(|(id, pane)| {
                let frame = pane.frame.as_ref()?;
                let view = pane.view?;
                if pane.uploading
                    || pane.failed
                    || pane.token != frame.generation().raw()
                    || pane
                        .presented
                        .as_ref()
                        .is_some_and(|shown| Arc::ptr_eq(&shown.frame, frame))
                {
                    return None;
                }
                let handle = pane.frame_handle.clone()?;
                pane.uploading = true;
                Some(PresentationRequest {
                    id: id.clone(),
                    logical_width: pane.width,
                    token: pane.token,
                    view,
                    frame: Arc::clone(frame),
                    handle,
                })
            })
            .collect()
    }

    pub(crate) fn allocated(
        &mut self,
        request: PresentationRequest,
        result: Result<iced::widget::image::Allocation, iced::widget::image::Error>,
    ) {
        if let Some(pane) = self.panes.get_mut(&request.id)
            && pane.token == request.token
            && pane.view == Some(request.view)
        {
            pane.uploading = false;
            match result {
                Ok(allocation) if allocation.handle() == &request.handle => {
                    pane.presented = Some(PresentedHtml {
                        logical_width: request.logical_width,
                        view: request.view,
                        frame: request.frame,
                        allocation,
                    });
                    if self
                        .visible
                        .get(&request.id)
                        .is_some_and(|(visible, _)| *visible)
                    {
                        self.last_presented = Some(request.id);
                    }
                }
                _ => {
                    pane.failed = true;
                    pane.presented = None;
                }
            }
        }
    }

    pub(crate) fn drain(&mut self) -> (Vec<String>, Vec<String>) {
        if let Some(native) = &mut self.native {
            native.drain_wake();
        }
        let mut received = false;
        let mut links = Vec::new();
        let mut copies = Vec::new();
        if let Some(events) = &self.events {
            while let Ok(event) = events.try_recv() {
                received = true;
                match event {
                    Event::Cursor {
                        id,
                        token,
                        view,
                        name,
                    } => {
                        if let Some(pane) = self.panes.get_mut(&id)
                            && pane.token == token
                            && pane.view == Some(view)
                        {
                            pane.cursor = name;
                            self.revision = self.revision.wrapping_add(1);
                        }
                    }
                    Event::Frame {
                        id,
                        token,
                        view,
                        frame,
                        accessibility,
                        router,
                    } => {
                        if let Some(pane) = self.panes.get_mut(&id)
                            && pane.token == token
                            && painted_frame(&frame)
                        {
                            pane.height =
                                frame.height() as f32 * pane.width as f32 / frame.width() as f32;
                            let same = pane
                                .frame
                                .as_ref()
                                .is_some_and(|current| same_pixels(current, &frame));
                            if !same {
                                pane.frame_handle =
                                    Some(enzui_iced_webview::web_frame_handle(&frame));
                            }
                            if !same || pane.view != Some(view) {
                                self.revision = self.revision.wrapping_add(1);
                                pane.frame = Some(frame);
                            }
                            if pane.view != Some(view) {
                                pane.accessibility = None;
                            }
                            pane.view = Some(view);
                            pane.failed = false;
                            if accessibility.is_some() {
                                pane.accessibility = accessibility;
                            }
                            pane.router = Some(router);
                        }
                    }
                    Event::Failed { id, token } => {
                        if let Some(pane) = self.panes.get_mut(&id)
                            && pane.token == token
                        {
                            pane.failed = true;
                            pane.presented = None;
                            self.revision = self.revision.wrapping_add(1);
                        }
                    }
                    Event::Link(view, token, url)
                        if self.panes.iter().any(|(id, pane)| {
                            pane.view == Some(view)
                                && pane.token == token
                                && pane.router.is_some()
                                && !pane.needs_render
                                && pane.resize_candidate.is_none()
                                && self.visible.get(id).is_some_and(|(visible, _)| *visible)
                        }) =>
                    {
                        links.push(url);
                    }
                    Event::Copied(view, token, text)
                        if self.panes.iter().any(|(id, pane)| {
                            pane.view == Some(view)
                                && pane.token == token
                                && pane.router.is_some()
                                && !pane.needs_render
                                && pane.resize_candidate.is_none()
                                && self.visible.get(id).is_some_and(|(visible, _)| *visible)
                        }) =>
                    {
                        copies.push(text);
                    }
                    Event::Link(..) | Event::Copied(..) => {}
                }
            }
        }
        if self
            .native
            .as_ref()
            .is_some_and(native::Worker::is_finished)
        {
            self.command = None;
            self.events = None;
            self.native = None;
            self.watcher = None;
            for id in &self.recent {
                if let Some(pane) = self.panes.get_mut(id) {
                    pane.failed = true;
                    pane.presented = None;
                    pane.router = None;
                }
            }
            self.revision = self.revision.wrapping_add(1);
        }
        self.trim_frames();
        if received {
            self.wake_native();
        }
        self.arm_native();
        (links, copies)
    }

    fn wake_native(&self) {
        if let Some(native) = &self.native {
            native.wake();
        }
    }

    fn start(&mut self) {
        if self.command.is_some() {
            return;
        }
        let (command, requests) = mpsc::sync_channel(32);
        let (events, completed) = mpsc::sync_channel(8);
        let Ok(native) = native::Worker::new(requests, events) else {
            return;
        };
        self.native = Some(native);
        self.watcher = enzui_iced_core::native_loop::NativeLoop::new().ok();
        self.command = Some(command);
        self.events = Some(completed);
    }
}

impl Drop for MailHtml {
    fn drop(&mut self) {
        self.watcher = None;
        if let Some(command) = self.command.take() {
            let _ = command.try_send(Command::Stop);
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum RenderFailure {
    Startup,
    Document(&'static str),
}

impl From<&'static str> for RenderFailure {
    fn from(message: &'static str) -> Self {
        Self::Document(message)
    }
}

#[cfg(test)]
impl MailHtml {
    pub(crate) fn refuse_native_for_test(&mut self) {
        self.command = Some(mpsc::sync_channel(1).0);
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn styled_mail_preserves_layout_and_blocks_active_content_and_images() {
        use super::*;
        let html = document(
            r#"<style>.card { padding:24px; background:#edf8f4 }</style><table class="card" width="600" style="margin:auto"><tr><td><img src="https://example.com/banner.png" width="320" height="140" style="border-radius:8px"><a href="https://example.com/open">Open</a><script>alert('bad')</script></td></tr></table>"#,
            false,
        );
        assert!(html.contains("<table") && html.contains("width=\"100%\""));
        assert!(html.contains("<style>.card"));
        assert!(html.contains("class=\"card\""));
        assert!(html.contains("padding:24px") && html.contains("border-radius:8px"));
        assert!(html.contains("width=\"320\"") && html.contains("height=\"140\""));
        assert!(html.contains(PLACEHOLDER));
        assert!(!html.contains("https://example.com/banner.png"));
        assert!(!html.contains("alert('bad')"));
        assert!(html.contains("https://example.com/open"));
        assert!(html.contains("img-src data:;"));
        let allowed = document(
            "<img src=\"https://example.com/banner.png\" width=\"320\">",
            true,
        );
        assert!(allowed.contains("https://example.com/banner.png"));
        assert!(allowed.contains("img-src data: https:"));
    }

    #[test]
    fn retired_html_frame_and_link_events_cannot_replace_the_visible_generation() {
        use super::*;
        let (events, incoming) = mpsc::sync_channel(8);
        let mut router = ViewRouter::new();
        let retired = router.create(1);
        let live = router.create(1);
        let mut reader = MailHtml::default();
        reader.events = Some(incoming);
        reader.panes.insert(
            "message".into(),
            HtmlPane {
                presented: None,
                uploading: false,
                cursor: None,
                view: Some(live),
                frame: None,
                frame_handle: None,
                failed: false,
                height: 480.0,
                accessibility: None,
                router: Some(Arc::new(router.clone())),
                token: 2,
                needs_render: false,
                source: "current".into(),
                images_allowed: false,
                dark_palette: None,
                width: 320,
                resize_candidate: None,
            },
        );
        reader.retained.push("message".into());
        reader.report_visibility("message".into(), true, 320);
        events
            .send(Event::Failed {
                id: "message".into(),
                token: 1,
            })
            .unwrap();
        events
            .send(Event::Link(retired, 1, "https://example.com/stale".into()))
            .unwrap();
        events
            .send(Event::Link(live, 2, "https://example.com/live".into()))
            .unwrap();
        let (links, copies) = reader.drain();
        assert!(!reader.panes["message"].failed);
        assert_eq!(links, vec!["https://example.com/live"]);
        assert!(copies.is_empty());

        let (commands, requests) = mpsc::sync_channel(8);
        reader.command = Some(commands);
        events
            .send(Event::Failed {
                id: "message".into(),
                token: 2,
            })
            .unwrap();
        let _ = reader.drain();
        for width in [320, 640, 400] {
            reader.report_visibility("message".into(), true, width);
            reader.retain(&[]);
            while requests.try_recv().is_ok() {}
            reader.prepare("message", "current", false, width);
            assert!(requests.try_recv().is_err());
            assert!(reader.panes["message"].failed);
        }
        reader.prepare("message", "changed", false, 400);
        assert!(
            requests
                .try_iter()
                .any(|command| matches!(command, Command::Render(_)))
        );
        assert!(!reader.panes["message"].failed);
        events
            .send(Event::Link(
                live,
                2,
                "https://example.com/superseded".into(),
            ))
            .unwrap();
        assert!(reader.drain().0.is_empty());

        let (commands, requests) = mpsc::sync_channel(1);
        commands.send(Command::Stop).unwrap();
        reader.command = Some(commands);
        reader.prepare("message", "queued", false, 400);
        assert!(reader.panes["message"].needs_render);
        assert!(!reader.panes["message"].failed);
        let _ = requests.recv().unwrap();
        reader.prepare("message", "queued", false, 400);
        assert!(matches!(requests.recv().unwrap(), Command::Render(_)));

        let frame = Arc::new(
            Rgba8Frame::new(
                enzui_iced_core::media::Generation::from_raw(reader.next_token),
                enzui_iced_core::frame_plan::FrameSeq(1),
                2,
                2,
                vec![255; 16],
            )
            .unwrap(),
        );
        events
            .send(Event::Frame {
                id: "message".into(),
                token: reader.next_token,
                view: live,
                frame: Arc::clone(&frame),
                accessibility: None,
                router: Arc::new(router),
            })
            .unwrap();
        let _ = reader.drain();
        reader.report_visibility("message".into(), true, 400);
        reader.prepare("next", "another", false, 400);
        reader.last_presented = Some("message".into());
        let retained = reader
            .previous_pane("next", |id, _| id == "message")
            .unwrap();
        assert!(Arc::ptr_eq(retained.frame.as_ref().unwrap(), &frame));
        assert!(reader.previous_pane("next", |_, _| false).is_none());
        assert!(reader.previous_pane("message", |_, _| true).is_none());
        let request = reader
            .allocation_requests()
            .into_iter()
            .find(|request| request.id == "message")
            .unwrap();
        assert!(reader.panes["message"].presented.is_none());
        reader.prepare("message", "replacement", false, 400);
        reader.allocated(request, Err(iced::widget::image::Error::OutOfMemory));
        assert!(!reader.panes["message"].failed);
        assert!(!reader.panes["message"].uploading);
    }

    #[test]
    fn cached_remote_pixels_are_hidden_when_permission_is_revoked() {
        use super::*;
        let mut reader = MailHtml::default();
        let (commands, incoming) = mpsc::sync_channel(32);
        reader.command = Some(commands);
        reader.prepare("message", "<p>Same document</p>", true, 360);
        let pane = reader.panes.get_mut("message").unwrap();
        pane.frame = Some(Arc::new(
            Rgba8Frame::new(
                enzui_iced_core::media::Generation::from_raw(1),
                enzui_iced_core::media::FrameSeq(1),
                2,
                1,
                vec![255; 8],
            )
            .unwrap(),
        ));
        assert!(!pane.display_matches("<p>Same document</p>", false));
        reader.prepare("message", "<p>Same document</p>", false, 360);
        assert!(reader.panes["message"].frame.is_none());
        assert!(
            incoming.try_iter().any(
                |command| matches!(command, Command::Render(request) if !request.images_allowed)
            )
        );
    }
}
