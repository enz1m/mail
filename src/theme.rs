//! Theme source and colour helpers.
//!
//! Every colour comes from the framework `Palette` via the runtime
//! [`EnzTheme`], never from app-local literals.
use enzui_iced_theme::{ContrastFloor, EnzTheme};
use iced::Color;

pub(crate) fn sidebar_background(theme: &EnzTheme) -> Color {
    theme.palette.surface_alt
}

pub(crate) fn list_background(theme: &EnzTheme) -> Color {
    theme
        .palette
        .background
        .mix(theme.palette.surface_alt, 0.45)
}

pub(crate) fn list_separator(theme: &EnzTheme) -> Color {
    theme.palette.border.scale_alpha(0.35)
}

pub(crate) fn reader_background(theme: &EnzTheme) -> Color {
    theme.palette.background
}

pub(crate) fn divider_color(theme: &EnzTheme) -> Color {
    theme.palette.border
}

pub(crate) fn text_primary(theme: &EnzTheme) -> Color {
    theme.palette.text
}

pub(crate) fn text_secondary(theme: &EnzTheme) -> Color {
    theme.palette.text_muted
}

// Backdrop-resolved tertiary ink for metadata painted on a known surface.
//
// `text_subtle` cannot clear the 4.5:1 body-text floor on every selected,
// alternate or elevated surface, so this delegates to the palette's
// contrast-aware promotion.
pub(crate) fn text_tertiary_on(theme: &EnzTheme, backdrop: Color) -> Color {
    theme.palette.text_subtle_on(backdrop)
}

// Meaningful graphics use the 3:1 non-text contrast floor rather than the body-text floor.
pub(crate) fn graphic_tertiary_on(theme: &EnzTheme, backdrop: Color) -> Color {
    theme.palette.subtle_on(backdrop, ContrastFloor::NonText)
}

pub(crate) fn accent(theme: &EnzTheme) -> Color {
    theme.palette.accent
}

pub(crate) fn subtle_overlay(theme: &EnzTheme) -> Color {
    theme.palette.surface_hover
}

pub(crate) fn flag_accent(theme: &EnzTheme) -> Color {
    theme.palette.warning
}

pub(crate) fn subtle_border(theme: &EnzTheme) -> Color {
    theme.palette.border
}
