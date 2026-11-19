use crate::Message;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector};

pub(crate) struct HtmlViewport<'a> {
    child: Element<'a, Message>,
    id: String,
}

impl<'a> HtmlViewport<'a> {
    pub(crate) fn new(child: Element<'a, Message>, id: String) -> Self {
        Self { child, id }
    }
}

#[derive(Default)]
struct ViewportState {
    reported: Option<(String, bool, u32)>,
}

impl Widget<Message, Theme, Renderer> for HtmlViewport<'_> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ViewportState>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(ViewportState::default())
    }
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }
    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }
    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if matches!(event, Event::Mouse(mouse::Event::WheelScrolled { .. })) {
            return;
        }
        if matches!(
            event,
            Event::Window(iced::window::Event::RedrawRequested(_))
        ) {
            let bounds = layout.bounds();
            let visible = bounds
                .intersection(viewport)
                .is_some_and(|area| area.width > 1.0 && area.height > 1.0);
            let report = (
                self.id.clone(),
                visible,
                bounds.width.round().max(1.0) as u32,
            );
            let state = tree.state.downcast_mut::<ViewportState>();
            if state.reported.as_ref() != Some(&report) {
                state.reported = Some(report.clone());
                shell.publish(Message::HtmlViewports(vec![report]));
            }
        }
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a> From<HtmlViewport<'a>> for Element<'a, Message> {
    fn from(boundary: HtmlViewport<'a>) -> Self {
        Element::new(boundary)
    }
}
