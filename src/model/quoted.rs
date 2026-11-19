use super::types::Message;

#[derive(Clone, Debug, Default)]
pub(crate) struct QuotedBody {
    pub(crate) plain_end: Option<usize>,
    pub(crate) html_collapsed: Option<String>,
}

impl QuotedBody {
    pub(crate) fn of(message: &Message) -> Self {
        if !super::thread::looks_like_reply(message) {
            return Self::default();
        }
        Self {
            plain_end: quoted_suffix(&message.body),
            html_collapsed: message.body_html.as_deref().and_then(|source| {
                super::html::has_quoted_history(source).then(|| {
                    // Presentation only; the original HTML still passes through the resource sanitizer.
                    format!("{source}<style>.gmail_quote,.yahoo_quoted,blockquote[type=\"cite\"]{{display:none!important}}</style>")
                })
            }),
        }
    }
}

pub(crate) fn quoted_suffix(body: &str) -> Option<usize> {
    let mut suffix = None;
    let mut offset = body.len();
    for line in body.split_inclusive('\n').rev() {
        offset -= line.len();
        if line.trim().is_empty() {
            continue;
        }
        if line.trim_start().starts_with('>') {
            suffix = Some(offset);
        } else {
            if suffix.is_some()
                && line.trim_start().starts_with("On ")
                && line.trim_end().ends_with("wrote:")
            {
                suffix = Some(offset);
            }
            break;
        }
    }
    suffix.filter(|end| {
        body.get(..*end)
            .is_some_and(|prefix| !prefix.trim().is_empty())
    })
}
