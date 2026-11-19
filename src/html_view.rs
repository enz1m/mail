//! Non-networking fallback for sanitized HTML mail.
//!
//! Remote images remain inert placeholders. Links publish a confirmation request
//! before the desktop opener can launch them.

use iced::widget::Space;
use iced::{Alignment, Element, Padding};

use crate::Message as Msg;
use crate::model::html::{HtmlBlock, InlineRun, ListMarker};
use crate::theme as t;

use enzui_iced_theme::{EnzTheme, FontRole};
use enzui_iced_widgets::block_list::{EnzBlock, enz_block_list};
use enzui_iced_widgets::enz_text::{TextSize, TextTone, enz_text};
use enzui_iced_widgets::icon::enz_icon;
use enzui_iced_widgets::rich_text::{RichSpan, RichText, SpanStyle};
use enzui_iced_widgets::surface::enz_surface;

pub(crate) fn html_blocks_view(blocks: Vec<HtmlBlock>, enz: &EnzTheme) -> Element<'static, Msg> {
    if blocks.is_empty() {
        return Space::new().into();
    }

    let ty = &enz.typography;

    let heading_sizes: [f32; 6] = [
        ty.large_title_size,
        ty.title_size,
        ty.title3_size,
        ty.section_size,
        ty.section_size,
        ty.section_size,
    ];

    let mut segments: Vec<Segment> = Vec::new();
    let mut current_enz: Vec<EnzBlock<String>> = Vec::new();

    for block in blocks {
        match block {
            HtmlBlock::RemoteImagePlaceholder { alt } => {
                if !current_enz.is_empty() {
                    segments.push(Segment::BlockList(std::mem::take(&mut current_enz)));
                }
                segments.push(Segment::HandRolled(remote_image_chip(alt, enz)));
            }
            other => {
                current_enz.push(html_to_enz_block(other, enz));
            }
        }
    }
    if !current_enz.is_empty() {
        segments.push(Segment::BlockList(current_enz));
    }

    if segments.len() == 1
        && let Segment::BlockList(enz_blocks) = segments.remove(0)
    {
        return configured_block_list(enz_blocks, heading_sizes, enz).into_element();
    }

    let mut items: Vec<Element<Msg>> = Vec::new();
    for seg in segments {
        match seg {
            Segment::BlockList(enz_blocks) => {
                items.push(configured_block_list(enz_blocks, heading_sizes, enz).into_element());
            }
            Segment::HandRolled(el) => {
                items.push(el);
            }
        }
    }
    iced::widget::column(items).spacing(enz.spacing.sm()).into()
}

fn configured_block_list(
    enz_blocks: Vec<EnzBlock<String>>,
    heading_sizes: [f32; 6],
    enz: &EnzTheme,
) -> enzui_iced_widgets::block_list::EnzBlockList<'static, String, Msg> {
    enz_block_list(enz_blocks)
        .heading_sizes(heading_sizes)
        .block_spacing(enz.spacing.sm())
        .on_link(Msg::RequestOpenUrl)
        .font_role(FontRole::Document)
        .font_from_theme(enz)
}

enum Segment {
    BlockList(Vec<EnzBlock<String>>),
    HandRolled(Element<'static, Msg>),
}

// Convert a single `HtmlBlock` to an `EnzBlock`.
//
// # Panics
//
// On `RemoteImagePlaceholder`, which is handled separately.
fn html_to_enz_block(block: HtmlBlock, _enz: &EnzTheme) -> EnzBlock<String> {
    match block {
        HtmlBlock::Paragraph(runs) => {
            EnzBlock::paragraph("").with_inline(inline_runs_to_rich_text(runs))
        }
        HtmlBlock::Heading { level, runs } => {
            EnzBlock::heading("", level).with_inline(inline_runs_to_rich_text(runs))
        }
        HtmlBlock::ListItem {
            marker,
            depth,
            runs,
        } => match marker {
            ListMarker::Bullet => {
                EnzBlock::list_item("", depth).with_inline(inline_runs_to_rich_text(runs))
            }
            ListMarker::Numbered(n) => {
                EnzBlock::ordered_item("", depth, n).with_inline(inline_runs_to_rich_text(runs))
            }
        },
        HtmlBlock::Blockquote { depth, runs } => {
            let mut b = EnzBlock::blockquote("");
            b.level = depth;
            b.with_inline(inline_runs_to_rich_text(runs))
        }
        // Rule and code blocks have no inline runs; the empty `RichText<String>` only
        // unifies the segment's payload type.
        HtmlBlock::HorizontalRule => EnzBlock::rule().with_inline(RichText::new(Vec::new())),
        HtmlBlock::PreformattedText(s) => EnzBlock::code(s).with_inline(RichText::new(Vec::new())),
        HtmlBlock::RemoteImagePlaceholder { .. } => {
            unreachable!("RemoteImagePlaceholder is handled separately in html_blocks_view")
        }
    }
}

// Convert `InlineRun`s into a `RichText<String>`, preserving bold, italic and
// underline.
//
// Links carry the sanitised href as payload and render accent-toned and
// underlined; the reducer owns confirmation and launching.
fn inline_runs_to_rich_text(runs: Vec<InlineRun>) -> RichText<String> {
    let spans: Vec<RichSpan<String>> = runs
        .into_iter()
        .map(|r| {
            let linked = r.link_href.is_some();
            let style = SpanStyle {
                bold: r.bold,
                italic: r.italic,
                underline: r.underline || linked,
                tone: if linked { Some(TextTone::Accent) } else { None },
                ..SpanStyle::default()
            };
            match r.link_href {
                Some(href) => {
                    let mut span = RichSpan::link(r.text, href);
                    span.style = style;
                    span
                }
                None => RichSpan::styled(r.text, style),
            }
        })
        .collect();
    RichText::new(spans)
}

// A compact chip standing in for a blocked remote image, showing the alt text
// or a generic label. Carries no URL.
fn remote_image_chip(alt: String, enz: &EnzTheme) -> Element<'static, Msg> {
    let label = if alt.trim().is_empty() {
        crate::l10n::tr("Remote image blocked")
    } else {
        crate::l10n::message("mail-remote-image-alt", &[("alt", alt)])
    };
    let chip_bg = t::subtle_overlay(enz);
    let chip_border = t::subtle_border(enz);
    enz_surface(
        iced::widget::row![
            enz_icon("image").size(13.0).tint(t::text_secondary(enz)),
            enz_text(label)
                .size(TextSize::Footnote)
                .color(t::text_secondary(enz)),
        ]
        .mirrored(crate::l10n::rtl())
        .align_y(Alignment::Center)
        .spacing(enz.spacing.sm()),
    )
    .fill(chip_bg)
    .radius(enz.radius.step(enz.shape.row))
    .border_width(1.0)
    .border_color(chip_border)
    .padding(Padding::from([enz.spacing.space3, enz.spacing.space5]))
    .fill_width()
    .into()
}
