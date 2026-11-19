use std::fmt::Write as _;

pub(crate) fn runs_to_text(runs: &[InlineRun]) -> String {
    let mut out = String::new();
    for r in runs {
        out.push_str(&r.text);
    }
    out
}

pub(crate) fn blocks_to_plain_text(blocks: &[HtmlBlock]) -> String {
    let mut out = String::new();
    for (idx, block) in blocks.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        match block {
            HtmlBlock::Paragraph(runs) => out.push_str(&runs_to_text(runs)),
            HtmlBlock::Heading { runs, .. } => out.push_str(&runs_to_text(runs)),
            HtmlBlock::ListItem {
                marker,
                depth,
                runs,
            } => {
                for _ in 0..*depth {
                    out.push_str("  ");
                }
                match marker {
                    ListMarker::Bullet => out.push_str("• "),
                    ListMarker::Numbered(n) => {
                        write!(&mut out, "{n}. ").expect("formatting an owned String succeeds");
                    }
                }
                out.push_str(&runs_to_text(runs));
            }
            HtmlBlock::Blockquote { depth, runs } => {
                for _ in 0..=*depth {
                    out.push_str("> ");
                }
                out.push_str(&runs_to_text(runs));
            }
            HtmlBlock::HorizontalRule => out.push('—'),
            HtmlBlock::RemoteImagePlaceholder { alt } => {
                if alt.is_empty() {
                    out.push_str("[remote image blocked]");
                } else {
                    write!(&mut out, "[remote image: {alt}]")
                        .expect("formatting an owned String succeeds");
                }
            }
            HtmlBlock::PreformattedText(text) => out.push_str(text),
        }
    }
    out
}

pub(crate) fn blocks_to_preview(blocks: &[HtmlBlock]) -> String {
    let mut text = String::new();
    for block in blocks {
        match block {
            HtmlBlock::Paragraph(runs)
            | HtmlBlock::Heading { runs, .. }
            | HtmlBlock::ListItem { runs, .. }
            | HtmlBlock::Blockquote { runs, .. } => {
                for run in runs {
                    text.push_str(&run.text);
                }
            }
            HtmlBlock::PreformattedText(value) => text.push_str(value),
            HtmlBlock::RemoteImagePlaceholder { .. } | HtmlBlock::HorizontalRule => continue,
        }
        text.push(' ');
    }
    crate::model::compose::preview_from_body(&text)
}
