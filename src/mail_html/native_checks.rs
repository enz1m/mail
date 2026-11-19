use super::*;
use enzui_webkit::input::{Modifiers, PointerButton, ViewPoint};

fn ready(reader: &mut MailHtml, id: &str, source: &str, width: u32) -> Arc<Rgba8Frame> {
    reader.prepare(id, source, false, width);
    reader.report_visibility(id.into(), true, width);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        reader.prepare(id, source, false, width);
        reader.drain();
        let pane = &reader.panes[id];
        assert!(!pane.failed);
        if let Some(frame) = &pane.frame
            && pane.width == width
            && frame.generation().raw() == pane.token
        {
            return Arc::clone(frame);
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn settle(reader: &mut MailHtml) {
    let (sent, received) = mpsc::sync_channel(1);
    assert!(
        reader
            .command
            .as_ref()
            .unwrap()
            .try_send(Command::Fence(sent))
            .is_ok()
    );
    reader.wake_native();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while received.try_recv().is_err() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn color_pixels(frame: &Rgba8Frame, color: [u8; 3]) -> usize {
    frame
        .pixels()
        .as_chunks::<4>().0.iter()
        .filter(|p| p[..3] == color)
        .count()
}

pub(crate) fn native_dark_html_adapts_important_colors_and_preserves_image_paint() {
    let source = "<div style='display:none'><p>Hidden preview</p><table><tr><td>Hidden cell</td></tr></table></div><div style='background:white!important;color:black!important;height:200px'><div style='height:30px;color:transparent!important'>Hidden preheader</div><h1>Readable newsletter</h1><img src='https://example.invalid/blocked.png' width='80' height='40' style='background:rgb(220,30,60)!important'></div>";
    let mut reader = MailHtml::default();
    let light = ready(&mut reader, "dark", source, 360);
    let theme = enzui_iced_theme::EnzTheme::dark();
    reader.set_dark_palette(Some(DarkPalette::from_theme(&theme)));
    let dark = ready(&mut reader, "dark", source, 360);
    let bg = [
        theme.palette.background.r,
        theme.palette.background.g,
        theme.palette.background.b,
    ]
    .map(|v| (v * 255.).round() as u8);
    assert!(color_pixels(&light, [255; 3]) > 10000);
    assert!(color_pixels(&dark, bg) > 10000);
    assert!(color_pixels(&light, [220, 30, 60]) > 80 * 35);
    assert_eq!(
        color_pixels(&light, [220, 30, 60]),
        color_pixels(&dark, [220, 30, 60])
    );
    assert!(
        dark.pixels()[..dark.width() as usize * 56 * 4]
            .as_chunks::<4>().0.iter()
            .all(|p| p[..3] == bg)
    );
}

pub(crate) fn native_html_paints_css_and_reserves_blocked_image_dimensions_without_ui_work() {
    let mut reader = MailHtml::default();
    let source = "<style>@media(max-width:480px){td{display:block;width:100%;box-sizing:border-box}}</style><table style='width:100%'><tr><td style='height:70px;background:rgb(255,0,0)'>First column</td><td style='height:70px;background:rgb(0,150,0)'>Second column</td></tr></table><p>Content below the table</p><img src='https://example.invalid/private.png' width='180' height='90'>";
    let started = std::time::Instant::now();
    let first = ready(&mut reader, "css", source, 360);
    assert!(color_pixels(&first, [255, 0, 0]) > 360 * 60);
    assert!(color_pixels(&first, [0, 150, 0]) > 360 * 60);
    assert!(color_pixels(&first, [233, 233, 237]) >= 180 * 85);
    assert!(
        first.pixels()[..first.width() as usize * 280 * 4]
            .as_chunks::<4>().0.iter()
            .filter(|p| p[0] < 80 && p[1] < 80 && p[2] < 80 && p[3] > 0)
            .count()
            > 300
    );
    let token = reader.next_token;
    for width in [360, 361, 359, 360] {
        reader.prepare("css", source, false, width);
        assert_eq!(reader.next_token, token);
    }
    assert!(Arc::ptr_eq(
        reader.panes["css"].frame.as_ref().unwrap(),
        &first
    ));
    eprintln!("Blitz cold styled document: {:?}", started.elapsed());
    reader.prepare("css", source, false, 640);
    assert!(Arc::ptr_eq(
        reader.panes["css"].frame.as_ref().unwrap(),
        &first
    ));
    let wider = ready(&mut reader, "css", source, 640);
    assert_eq!(wider.width(), 1280);
    assert_eq!(reader.panes["css"].height, wider.height() as f32 / 2.0);
    let floated = ready(
        &mut reader,
        "floats",
        "<div style='display:flow-root'><div style='float:left;width:0;height:200px'></div><p style='margin:0;font:20px sans-serif'>Visible hero heading</p></div>",
        360,
    );
    assert!(
        floated.pixels()[..floated.width() as usize * 70 * 4]
            .as_chunks::<4>().0.iter()
            .any(|p| p[0] < 80 && p[1] < 80 && p[2] < 80)
    );
    assert!(color_pixels(&wider, [255, 0, 0]) > 10000);
    assert!(color_pixels(&wider, [0, 150, 0]) > 10000);
    let compact = ready(&mut reader, "css", source, 360);
    assert_eq!(compact.width(), 720);
    assert!(color_pixels(&compact, [255, 0, 0]) > 360 * 60);
    assert!(color_pixels(&compact, [0, 150, 0]) > 360 * 60);
}

fn press(reader: &mut MailHtml, view: ViewId, x: f32, y: f32) {
    reader.input(
        view,
        InputEvent::PointerPressed {
            view,
            at: ViewPoint { x, y },
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        },
    );
}
fn release(reader: &mut MailHtml, view: ViewId, x: f32, y: f32) {
    reader.input(
        view,
        InputEvent::PointerReleased {
            view,
            at: ViewPoint { x, y },
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        },
    );
}
fn copy(reader: &mut MailHtml, view: ViewId) -> Vec<String> {
    reader.input(
        view,
        InputEvent::KeyPressed {
            view,
            key: enzui_webkit::KeyCode::Character("c".into()),
            modifiers: Modifiers::CTRL,
        },
    );
    settle(reader);
    reader.drain().1
}

pub(crate) fn native_pointer_drag_selects_html_text() {
    let mut reader = MailHtml::default();
    ready(
        &mut reader,
        "select",
        "<style>body{padding:0!important}</style><div style='font-size:20px;line-height:32px'>Alpha beta gamma delta.</div><p style='margin:0;line-height:30px'><a href='https://example.invalid/link'>Open project</a></p>",
        360,
    );
    let view = reader.panes["select"].view.unwrap();
    reader.input(
        view,
        InputEvent::FocusGained {
            view,
            modifiers: Modifiers::NONE,
        },
    );
    press(&mut reader, view, 1., 12.);
    reader.input(
        view,
        InputEvent::PointerMoved {
            view,
            at: ViewPoint { x: 170., y: 12. },
            modifiers: Modifiers::NONE,
        },
    );
    release(&mut reader, view, 170., 12.);
    let selected = copy(&mut reader, view);
    assert_eq!(selected.len(), 1);
    assert!(
        selected[0].contains("beta"),
        "selection follows real pointer hit testing: {selected:?}"
    );
    reader.input(
        view,
        InputEvent::FocusLost {
            view,
            modifiers: Modifiers::NONE,
        },
    );
    assert!(copy(&mut reader, view).is_empty());
    reader.input(
        view,
        InputEvent::PointerMoved {
            view,
            at: ViewPoint { x: 10., y: 45. },
            modifiers: Modifiers::NONE,
        },
    );
    settle(&mut reader);
    reader.drain();
    assert_eq!(reader.panes["select"].cursor.as_deref(), Some("pointer"));
    press(&mut reader, view, 10., 45.);
    release(&mut reader, view, 10., 45.);
    settle(&mut reader);
    assert_eq!(
        reader.drain().0,
        vec!["https://example.invalid/link".to_owned()]
    );
}

pub(crate) fn native_worker_publishes_selection_highlight_after_pointer_input() {
    let mut reader = MailHtml::default();
    let source = "<style>body{padding:0!important}</style><p style='font:20px sans-serif;line-height:32px;margin:0'>Select these words in a message.</p>";
    let before = ready(&mut reader, "message", source, 360);
    let view = reader.panes["message"].view.unwrap();
    reader.input(
        view,
        InputEvent::FocusGained {
            view,
            modifiers: Modifiers::NONE,
        },
    );
    press(&mut reader, view, 1., 12.);
    reader.input(
        view,
        InputEvent::PointerMoved {
            view,
            at: ViewPoint { x: 180., y: 12. },
            modifiers: Modifiers::NONE,
        },
    );
    release(&mut reader, view, 180., 12.);
    settle(&mut reader);
    reader.drain();
    let after = reader.panes["message"].frame.as_ref().unwrap();
    assert_ne!(before.pixels(), after.pixels());
    ready(&mut reader, "message", "<p>Replacement document</p>", 360);
    assert_ne!(reader.panes["message"].view, Some(view));
    assert!(copy(&mut reader, view).is_empty());
    assert!(reader.panes["message"].accessibility.is_some());
}
