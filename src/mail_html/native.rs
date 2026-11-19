use super::*;
use anyrender::{ImageRenderer, PaintScene};
use blitz_dom::{Document, DocumentConfig};
use blitz_html::{HtmlDocument, HtmlProvider};
use blitz_traits::{
    events::{
        BlitzPointerEvent, BlitzPointerId, MouseEventButton, MouseEventButtons, Point,
        PointerCoords, PointerDetails, UiEvent,
    },
    navigation::{NavigationOptions, NavigationProvider},
    shell::{ColorScheme, ShellProvider, Viewport},
};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[path = "worker.rs"]
mod worker;
use worker::EventSender;
pub(super) use worker::Worker;

#[path = "resources.rs"]
mod resources;

struct Shell {
    wake: Arc<UnixStream>,
    revision: AtomicU64,
    owner: std::thread::ThreadId,
    cursor: std::sync::Mutex<Option<String>>,
    links: std::sync::Mutex<Vec<String>>,
}

impl ShellProvider for Shell {
    fn request_redraw(&self) {
        if std::thread::current().id() != self.owner {
            self.revision.fetch_add(1, Ordering::Release);
        }
        let mut wake = &*self.wake;
        let _ = wake.write(&[1]);
    }
    fn set_cursor(&self, icon: Option<cursor_icon::CursorIcon>) {
        *self
            .cursor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = icon.map(|icon| icon.to_string());
    }
}

impl NavigationProvider for Shell {
    fn navigate_to(&self, options: NavigationOptions) {
        if matches!(options.url.scheme(), "https" | "http" | "mailto") {
            let mut links = self
                .links
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if links.len() < 8 {
                links.push(options.url.to_string());
            }
        }
        self.request_redraw();
    }
}

pub(super) struct Page {
    id: String,
    token: u64,
    pub(super) view: ViewId,
    pub(super) doc: HtmlDocument,
    renderer: anyrender_vello_cpu::VelloCpuImageRenderer,
    shell: Arc<Shell>,
    cancelled: Arc<AtomicBool>,
    width: u32,
    height: u32,
    raster_scale: u32,
    revision: u64,
    dirty: bool,
    focused: bool,
    pressed: bool,
    sequence: u64,
    palette: Option<DarkPalette>,
    last_frame: Option<Arc<Rgba8Frame>>,
    last_cursor: Option<String>,
    pending: Option<Event>,
}

impl Page {
    fn begin(
        request: RenderRequest,
        router: &mut ViewRouter,
        wake: &Arc<UnixStream>,
        slot: u64,
    ) -> Result<Self, RenderFailure> {
        if request.source.len() > 4 * 1024 * 1024 {
            return Err("HTML document exceeds the display budget".into());
        }
        let (html, allowed) = approved_document(&request.source, request.images_allowed);
        let shell = Arc::new(Shell {
            wake: Arc::clone(wake),
            revision: AtomicU64::new(1),
            owner: std::thread::current().id(),
            cursor: std::sync::Mutex::new(None),
            links: std::sync::Mutex::new(Vec::new()),
        });
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut doc = HtmlDocument::from_html(
            &html,
            DocumentConfig {
                viewport: Some(Viewport::new(
                    request.width * 2,
                    480,
                    2.0,
                    if request.dark_palette.is_some() {
                        ColorScheme::Dark
                    } else {
                        ColorScheme::Light
                    },
                )),
                html_parser_provider: Some(Arc::new(HtmlProvider)),
                style_threading: blitz_dom::StyleThreading::Sequential,
                net_provider: Some(Arc::new(resources::Images::new(
                    allowed,
                    Arc::clone(&cancelled),
                ))),
                shell_provider: Some(Arc::clone(&shell) as _),
                navigation_provider: Some(Arc::clone(&shell) as _),
                ..Default::default()
            },
        );
        if doc
            .query_selector_all("*")
            .map_or(true, |nodes| nodes.len() > 16384)
        {
            return Err("HTML document has too many elements".into());
        }
        doc.resolve(0.0);
        normalize_stacked_tables(&mut doc);
        if let Some(palette) = request.dark_palette {
            palette.apply(&mut doc);
        }
        doc.resolve(0.0);
        let height = document_height(&doc, request.width)?;
        let raster_scale = raster_scale(request.width, height);
        doc.set_viewport(Viewport::new(
            request.width * raster_scale,
            height * raster_scale,
            raster_scale as f32,
            if request.dark_palette.is_some() {
                ColorScheme::Dark
            } else {
                ColorScheme::Light
            },
        ));
        doc.resolve(0.0);
        let view = router.create(slot);
        let revision = shell.revision.load(Ordering::Acquire);
        Ok(Self {
            id: request.id,
            token: request.token,
            view,
            doc,
            renderer: anyrender_vello_cpu::VelloCpuImageRenderer::new(
                request.width * raster_scale,
                height * raster_scale,
            ),
            shell,
            cancelled,
            width: request.width,
            height,
            raster_scale,
            revision,
            dirty: true,
            focused: false,
            pressed: false,
            sequence: 0,
            palette: request.dark_palette,
            last_frame: None,
            last_cursor: None,
            pending: None,
        })
    }

    fn input(&mut self, input: InputEvent, outgoing: &EventSender) {
        use enzui_webkit::input::{KeyCode, Modifiers, PointerButton};
        match input {
            InputEvent::FocusGained { .. } => self.focused = true,
            InputEvent::FocusLost { .. } => {
                self.focused = false;
                self.pressed = false;
            }
            InputEvent::PointerPressed {
                at,
                button: PointerButton::Primary,
                modifiers,
                ..
            } => {
                self.pressed = true;
                self.doc
                    .handle_ui_event(UiEvent::PointerDown(pointer(at.x, at.y, modifiers, true)));
                self.dirty = true;
            }
            InputEvent::PointerReleased {
                at,
                button: PointerButton::Primary,
                modifiers,
                ..
            } => {
                self.doc.handle_ui_event(UiEvent::PointerUp(pointer(
                    at.x,
                    at.y,
                    modifiers,
                    self.pressed,
                )));
                self.pressed = false;
                self.dirty = true;
            }
            InputEvent::PointerMoved { at, modifiers, .. } => {
                let before = self.doc.get_hover_node_id();
                self.doc.handle_ui_event(UiEvent::PointerMove(pointer(
                    at.x,
                    at.y,
                    modifiers,
                    self.pressed,
                )));
                self.dirty |= self.pressed || before != self.doc.get_hover_node_id();
            }
            InputEvent::KeyPressed {
                key: KeyCode::Character(key),
                modifiers,
                ..
            } if self.focused
                && modifiers.contains(Modifiers::CTRL)
                && key.eq_ignore_ascii_case("c") =>
            {
                if let Some(text) = self
                    .doc
                    .get_selected_text()
                    .filter(|text| !text.is_empty() && text.len() <= 1024 * 1024)
                {
                    let _ = outgoing.try_send(Event::Copied(self.view, self.token, text));
                }
            }
            InputEvent::KeyPressed {
                key: KeyCode::Character(key),
                modifiers,
                ..
            } if self.focused
                && modifiers.contains(Modifiers::CTRL)
                && key.eq_ignore_ascii_case("a") =>
            {
                let ranges = self
                    .doc
                    .query_selector_all("*")
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|id| {
                        self.doc
                            .get_node(id)
                            .and_then(|node| node.element_data())
                            .and_then(|e| e.inline_layout_data.as_ref())
                            .map(|layout| (id, layout.text.len()))
                    })
                    .filter(|(_, len)| *len > 0)
                    .collect::<Vec<_>>();
                if let (Some((first, _)), Some((last, len))) = (ranges.first(), ranges.last()) {
                    self.doc.set_text_selection(*first, 0, *last, *len);
                }
                self.dirty = true;
            }
            InputEvent::KeyPressed {
                key: KeyCode::Named(enzui_webkit::input::NamedKey::Tab),
                modifiers,
                ..
            } if self.focused => {
                if modifiers.contains(Modifiers::SHIFT) {
                    self.doc.focus_prev_node();
                } else {
                    self.doc.focus_next_node();
                }
                self.dirty = true;
            }
            InputEvent::KeyPressed {
                key: KeyCode::Named(enzui_webkit::input::NamedKey::Enter),
                ..
            } if self.focused => {
                if let Some(node) = self
                    .doc
                    .get_focussed_node_id()
                    .and_then(|id| self.doc.get_node(id))
                    && super::tag(node, "a")
                    && let Some(url) = node.attr("href".into())
                {
                    let _ = outgoing.try_send(Event::Link(self.view, self.token, url.to_owned()));
                }
            }
            InputEvent::WheelScrolledAt {
                at,
                delta_x,
                delta_y,
                unit,
                modifiers,
                ..
            } => {
                use blitz_traits::events::{BlitzWheelDelta, BlitzWheelEvent};
                let point = pointer(at.x, at.y, modifiers, self.pressed);
                let delta = if matches!(unit, enzui_webkit::input::ScrollUnit::Lines) {
                    BlitzWheelDelta::Lines(f64::from(delta_x), f64::from(delta_y))
                } else {
                    BlitzWheelDelta::Pixels(f64::from(delta_x), f64::from(delta_y))
                };
                self.doc.handle_ui_event(UiEvent::Wheel(BlitzWheelEvent {
                    delta,
                    coords: point.coords,
                    buttons: point.buttons,
                    mods: point.mods,
                    element: Point::default(),
                }));
                self.dirty = true;
            }
            InputEvent::KeyPressed {
                key: KeyCode::Named(enzui_webkit::input::NamedKey::Escape),
                ..
            } => {
                self.doc.clear_text_selection();
                self.pressed = false;
                self.dirty = true;
            }
            _ => {}
        }
        let mut links = self
            .shell
            .links
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for url in links.drain(..) {
            let _ = outgoing.try_send(Event::Link(self.view, self.token, url));
        }
        self.shell.request_redraw();
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

pub(super) struct NativeHost {
    pages: Vec<Page>,
    router: ViewRouter,
    requests: Receiver<Command>,
    events: EventSender,
    wake: Option<(UnixStream, Arc<UnixStream>)>,
}

impl NativeHost {
    fn new(
        requests: Receiver<Command>,
        events: EventSender,
        wake: (UnixStream, Arc<UnixStream>),
    ) -> Self {
        Self {
            pages: Vec::new(),
            router: ViewRouter::new(),
            requests,
            events,
            wake: Some(wake),
        }
    }
    pub(super) fn input(&mut self, view: ViewId, input: InputEvent) {
        if self.router.accepts(view)
            && let Some(page) = self.pages.iter_mut().find(|page| page.view == view)
        {
            page.input(input, &self.events);
        }
    }
    fn advance(&mut self, cancel_input: bool) {
        if let Some((read, _)) = &mut self.wake {
            let mut buffer = [0; 256];
            while read.read(&mut buffer).is_ok_and(|n| n > 0) {}
        }
        let commands: Vec<_> = self.requests.try_iter().take(32).collect();
        if cancel_input {
            for page in &mut self.pages {
                page.focused = false;
                page.pressed = false;
            }
        }
        let mut commands = commands;
        // Replaced render requests must not parse or paint obsolete documents.
        for index in (0..commands.len()).rev() {
            if let Command::Render(request) = &commands[index]
                && commands[index + 1..]
                    .iter()
                    .any(|later| matches!(later, Command::Render(later) if later.id == request.id))
            {
                commands.remove(index);
            }
        }
        #[cfg(test)]
        let mut fences = Vec::new();
        for command in commands {
            match command {
                #[cfg(test)]
                Command::Fence(fence) => fences.push(fence),
                Command::Stop => {
                    self.pages.clear();
                    return;
                }
                Command::Retain(ids) => self.pages.retain(|page| ids.contains(&page.id)),
                Command::Input(view, input) => {
                    if !cancel_input {
                        self.input(view, input);
                    }
                }
                Command::Render(request) => {
                    self.pages.retain(|page| page.id != request.id);
                    while self.pages.len() >= 4 {
                        self.pages.remove(0);
                    }
                    let slot = (1..=4)
                        .find(|slot| self.pages.iter().all(|p| p.view.slot() != *slot))
                        .unwrap_or(1);
                    let id = request.id.clone();
                    let token = request.token;
                    let page = self
                        .wake
                        .as_ref()
                        .ok_or(RenderFailure::Startup)
                        .and_then(|(_, wake)| Page::begin(request, &mut self.router, wake, slot));
                    match page {
                        Ok(page) => self.pages.push(page),
                        Err(error) => {
                            if let RenderFailure::Document(message) = error {
                                eprintln!("mail-app: {message}");
                            }
                            let _ = self.events.try_send(Event::Failed { id, token });
                        }
                    }
                }
            }
        }
        for page in &mut self.pages {
            if let Some(event) = page.pending.take()
                && let Err(mpsc::TrySendError::Full(event)) = self.events.try_send(event)
            {
                page.pending = Some(event);
                continue;
            }
            let name = page
                .shell
                .cursor
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if page.last_cursor != name
                && self
                    .events
                    .try_send(Event::Cursor {
                        id: page.id.clone(),
                        token: page.token,
                        view: page.view,
                        name: name.clone(),
                    })
                    .is_ok()
            {
                page.last_cursor = name;
            }
            let revision = page.shell.revision.load(Ordering::Acquire);
            if !page.dirty && revision == page.revision {
                continue;
            }
            page.revision = revision;
            page.doc.resolve(0.0);
            let Ok(height) = document_height(&page.doc, page.width) else {
                continue;
            };
            let raster_scale = raster_scale(page.width, height);
            if height != page.height || raster_scale != page.raster_scale {
                page.height = height;
                page.doc.set_viewport(Viewport::new(
                    page.width * raster_scale,
                    height * raster_scale,
                    raster_scale as f32,
                    if page.palette.is_some() {
                        ColorScheme::Dark
                    } else {
                        ColorScheme::Light
                    },
                ));
                page.doc.resolve(0.0);
                page.raster_scale = raster_scale;
                page.renderer
                    .resize(page.width * raster_scale, page.height * raster_scale);
            }
            let mut pixels = Vec::new();
            page.renderer.render_to_vec(
                |scene| {
                    scene.reset();
                    blitz_paint::paint_scene(
                        scene,
                        &mut page.doc,
                        f64::from(page.raster_scale),
                        page.width * page.raster_scale,
                        page.height * page.raster_scale,
                        0,
                        0,
                    );
                },
                &mut pixels,
            );
            // The CPU painter uses premultiplied RGBA; iced's image input is straight RGBA.
            for pixel in pixels.as_chunks_mut::<4>().0 {
                if pixel[3] > 0 && pixel[3] < 255 {
                    for c in 0..3 {
                        pixel[c] =
                            ((u16::from(pixel[c]) * 255) / u16::from(pixel[3])).min(255) as u8;
                    }
                }
            }
            page.sequence = page.sequence.wrapping_add(1);
            let Ok(frame) = Rgba8Frame::new(
                enzui_iced_core::media::Generation::from_raw(page.token),
                enzui_iced_core::media::FrameSeq(page.sequence),
                page.width * page.raster_scale,
                page.height * page.raster_scale,
                pixels,
            ) else {
                continue;
            };
            let frame = Arc::new(frame);
            if page
                .last_frame
                .as_ref()
                .is_none_or(|old| !same_pixels(old, &frame))
            {
                let mut delivery = enzui_webkit::page_delivery::PageA11yDelivery::new();
                let _ = delivery.offer(&self.router, page.view, accessible_document(&page.doc));
                if let Err(mpsc::TrySendError::Full(event)) = self.events.try_send(Event::Frame {
                    id: page.id.clone(),
                    token: page.token,
                    view: page.view,
                    frame: Arc::clone(&frame),
                    accessibility: Some(Arc::new(delivery)),
                    router: Arc::new(self.router.clone()),
                }) {
                    page.pending = Some(event);
                }
                page.last_frame = Some(frame);
            }
            page.dirty = false;
        }
        #[cfg(test)]
        for fence in fences {
            let _ = fence.try_send(());
        }
    }
}

// Supersampling avoids resampling already-antialiased text at fractional UI positions.
fn raster_scale(width: u32, height: u32) -> u32 {
    if u64::from(width) * u64::from(height) * 16 <= 64 * 1024 * 1024 {
        2
    } else {
        1
    }
}

fn document_height(doc: &HtmlDocument, width: u32) -> Result<u32, RenderFailure> {
    let size = doc.root_element().final_layout().size.height;
    if !size.is_finite() || size > MAX_DOCUMENT_HEIGHT.min(64 * 1024 * 1024 / 4 / width) as f32 {
        return Err("HTML document exceeds display budget".into());
    }
    Ok((size.ceil() as u32).max(120))
}

fn pointer(
    x: f32,
    y: f32,
    modifiers: enzui_webkit::input::Modifiers,
    pressed: bool,
) -> BlitzPointerEvent {
    let mut mods = keyboard_types::Modifiers::empty();
    if modifiers.contains(enzui_webkit::input::Modifiers::SHIFT) {
        mods |= keyboard_types::Modifiers::SHIFT;
    }
    if modifiers.contains(enzui_webkit::input::Modifiers::CTRL) {
        mods |= keyboard_types::Modifiers::CONTROL;
    }
    if modifiers.contains(enzui_webkit::input::Modifiers::ALT) {
        mods |= keyboard_types::Modifiers::ALT;
    }
    BlitzPointerEvent {
        id: BlitzPointerId::Mouse,
        is_primary: true,
        coords: PointerCoords {
            page_x: x,
            page_y: y,
            client_x: x,
            client_y: y,
            screen_x: x,
            screen_y: y,
        },
        button: MouseEventButton::Main,
        buttons: if pressed {
            MouseEventButtons::from(MouseEventButton::Main)
        } else {
            MouseEventButtons::empty()
        },
        mods,
        details: PointerDetails::default(),
        element: Point::default(),
        active_pointers: Default::default(),
    }
}

// Blitz's table collector omits blockified cells; a fully stacked presentation
// table is equivalent to its table/row-group/row wrappers participating in block flow.
fn normalize_stacked_tables(doc: &mut HtmlDocument) {
    let tables = doc.query_selector_all("table").unwrap_or_default();
    for table in tables.into_iter().rev() {
        let cells = doc
            .query_selector_all("td,th")
            .unwrap_or_default()
            .into_iter()
            .filter(|id| {
                let mut parent = doc.get_node(*id).and_then(|n| n.parent);
                while let Some(id) = parent {
                    if doc.get_node(id).is_some_and(|n| super::tag(n, "table")) {
                        return id == table;
                    }
                    parent = doc.get_node(id).and_then(|n| n.parent);
                }
                false
            })
            .collect::<Vec<_>>();
        if cells.is_empty()
            || !cells.iter().all(|id| {
                doc.get_node(*id)
                    .is_some_and(|node| node.primary_styles().is_some())
                    && doc.resolved_style_value(*id, "display") == "block"
            })
        {
            continue;
        }
        let mut wrappers = vec![table];
        for cell in &cells {
            let mut parent = doc.get_node(*cell).and_then(|n| n.parent);
            while let Some(id) = parent {
                if id == table {
                    break;
                }
                if !wrappers.contains(&id) {
                    wrappers.push(id);
                }
                parent = doc.get_node(id).and_then(|n| n.parent);
            }
        }
        for id in wrappers {
            doc.set_style_property(id, "display", "block");
        }
        for cell in cells {
            doc.set_style_property(cell, "display", "flow-root");
        }
    }
    doc.resolve(0.0);
}

struct AccessibleNode {
    role: u32,
    name: String,
    children: Vec<Self>,
}
impl enzui_webkit::page_a11y::AccessibleSource for AccessibleNode {
    fn atspi_role(&self) -> u32 {
        self.role
    }
    fn accessible_name(&self) -> &str {
        &self.name
    }
    fn accessible_children(&self) -> &[Self] {
        &self.children
    }
    fn accessible_address(&self) -> Option<enzui_webkit::page_a11y::PageNodeAddress> {
        None
    }
    fn accessible_actions(&self) -> &[String] {
        &[]
    }
}
fn accessible_document(doc: &HtmlDocument) -> enzui_webkit::page_a11y::PageA11y {
    use enzui_webkit::page_a11y::{PageA11y, atspi};
    let mut children = Vec::new();
    for id in doc
        .query_selector_all("h1,h2,h3,h4,h5,h6,p,a,li,blockquote")
        .unwrap_or_default()
        .into_iter()
        .take(256)
    {
        if doc
            .get_node(id)
            .is_none_or(|node| node.primary_styles().is_none())
            || doc.resolved_style_value(id, "display") == "none"
        {
            continue;
        }
        if let Some(node) = doc.get_node(id) {
            let name = node.text_content();
            if name.trim().is_empty() {
                continue;
            }
            let role = if super::tag(node, "a") {
                atspi::LINK
            } else if (1..=6).any(|n| super::tag(node, &format!("h{n}"))) {
                atspi::HEADING
            } else {
                atspi::STATIC
            };
            children.push(AccessibleNode {
                role,
                name,
                children: Vec::new(),
            });
        }
    }
    PageA11y::from_source(&AccessibleNode {
        role: atspi::PANEL,
        name: "HTML message".into(),
        children,
    })
}
