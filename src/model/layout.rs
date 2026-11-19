//! Responsive mail navigation keeps the sidebar in an overlay outside the wide layout.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum LayoutMode {
    Wide,
    Medium,
    Narrow,
}

// Width below which the sidebar collapses (Wide to Medium).
//
// Reads
// [`SizingTokens::layout_switch_wide`](enzui_iced_theme::SizingTokens::layout_switch_wide);
// a `fn` because `SizingTokens::default()` is not `const`.
pub(crate) fn wide_min_width() -> f32 {
    enzui_iced_theme::SizingTokens::default().layout_switch_wide
}

// Width below which the app drops to single-column master-detail (Medium to
// Narrow), where list and reader stop fitting side by side.
//
// No `BreakpointScale` rung matches this app-specific width, so it stays a
// named literal.
pub(crate) const NARROW_MAX_WIDTH: f32 = 700.0;

pub(crate) fn layout_mode(width: f32) -> LayoutMode {
    if !width.is_finite() || width < NARROW_MAX_WIDTH {
        LayoutMode::Narrow
    } else if width < wide_min_width() {
        LayoutMode::Medium
    } else {
        LayoutMode::Wide
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum NarrowPane {
    List,
    Detail,
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct NarrowNav {
    pushed: bool,
}

impl NarrowNav {
    pub(crate) fn pane(self) -> NarrowPane {
        if self.pushed {
            NarrowPane::Detail
        } else {
            NarrowPane::List
        }
    }

    pub(crate) fn on_select(&mut self, has_selection: bool) {
        if has_selection {
            self.pushed = true;
        }
    }

    pub(crate) fn on_back(&mut self) {
        self.pushed = false;
    }

    // Another sidebar row was picked; resets to the list so the reader does not
    // stay on a stale message.
    pub(crate) fn on_sidebar_change(&mut self) {
        self.pushed = false;
    }
}
