use crate::model::privacy::{Tracker, detect_trackers};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum HtmlBlock {
    Paragraph(Vec<InlineRun>),
    Heading {
        level: u8,
        runs: Vec<InlineRun>,
    },
    ListItem {
        marker: ListMarker,
        depth: u8,
        runs: Vec<InlineRun>,
    },
    Blockquote {
        depth: u8,
        runs: Vec<InlineRun>,
    },
    HorizontalRule,
    // A remote image (`<img src="https://…">`) the sanitiser refused. Carries
    // only the alt text; the URL is dropped.
    RemoteImagePlaceholder {
        alt: String,
    },
    // A `<pre>` / `<code>` block. Whitespace is preserved.
    PreformattedText(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum ListMarker {
    Bullet,
    Numbered(u32),
}

// One styled run of inline text, in document order.
//
// `link_href` is `Some` only for `http:`, `https:` or `mailto:` URLs; every
// other scheme is stripped by the sanitiser.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct InlineRun {
    pub(crate) text: String,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) underline: bool,
    pub(crate) link_href: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RenderedHtml {
    pub(crate) blocks: Vec<HtmlBlock>,
    // Every remote resource URL the sanitiser refused to load, kept for the
    // tracker walker and the blocked-resources label; never passed to UI that
    // could fetch it.
    pub(crate) blocked_remote_urls: Vec<String>,
    pub(crate) trackers: Vec<Tracker>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SanitizeOptions {
    // Replaces remote-image placeholders with alt text only. The URL is still
    // dropped, and this option neither authorizes nor performs a fetch.
    pub(crate) allow_remote_images: bool,
}

impl SanitizeOptions {
    // The safe default retains a placeholder and never exposes its URL.
    pub(crate) const SAFE: SanitizeOptions = SanitizeOptions {
        allow_remote_images: false,
    };
}

impl Default for SanitizeOptions {
    fn default() -> Self {
        SanitizeOptions::SAFE
    }
}

pub(crate) fn sanitize_and_render_html(html: &str, opts: SanitizeOptions) -> RenderedHtml {
    let trackers = detect_trackers(html);

    let tokens = tokenize(html);
    let mut builder = BlockBuilder::new(opts);
    for token in tokens {
        builder.consume(token);
    }
    let (blocks, blocked_remote_urls) = builder.finish();

    RenderedHtml {
        blocks,
        blocked_remote_urls,
        trackers,
    }
}

#[derive(Clone, Debug)]
enum Token<'a> {
    Open {
        name: String,
        attrs: &'a str,
        self_closing: bool,
    },
    Close {
        name: String,
    },
    Text(String),
    // A comment, tokenized separately so `<!-- <script>foo</script> -->` cannot
    // leak a script tag.
    Comment,
}

fn tokenize(input: &str) -> Vec<Token<'_>> {
    let mut out: Vec<Token<'_>> = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            if input
                .get(i..)
                .expect("scanner remains on a UTF-8 boundary")
                .starts_with("<!--")
            {
                if let Some(end_rel) = input
                    .get(i + 4..)
                    .expect("ASCII comment prefix preserves byte boundaries")
                    .find("-->")
                {
                    out.push(Token::Comment);
                    i = i + 4 + end_rel + 3;
                    continue;
                } else {
                    out.push(Token::Comment);
                    break;
                }
            }
            if input
                .get(i..)
                .expect("scanner remains on a UTF-8 boundary")
                .starts_with("<!")
                || input
                    .get(i..)
                    .expect("scanner remains on a UTF-8 boundary")
                    .starts_with("<?")
            {
                if let Some(end_rel) = input
                    .get(i..)
                    .expect("scanner remains on a UTF-8 boundary")
                    .find('>')
                {
                    out.push(Token::Comment);
                    i += end_rel + 1;
                    continue;
                } else {
                    break;
                }
            }
            let Some(end_rel) = input
                .get(i + 1..)
                .expect("ASCII tag prefix preserves byte boundaries")
                .find('>')
            else {
                out.push(Token::Text(decode_entities(
                    input.get(i..).expect("scanner remains on a UTF-8 boundary"),
                )));
                break;
            };
            let inner = input
                .get(i + 1..i + 1 + end_rel)
                .expect("tag delimiters are ASCII boundaries");
            i = i + 1 + end_rel + 1;
            if let Some(closed) = inner.strip_prefix('/') {
                let name = tag_name(closed).to_ascii_lowercase();
                if !name.is_empty() {
                    out.push(Token::Close { name });
                }
                continue;
            }
            let (self_closing, body) = if let Some(stripped) = inner.strip_suffix('/') {
                (true, stripped)
            } else {
                (false, inner)
            };
            let body = body.trim();
            let name = tag_name(body).to_ascii_lowercase();
            if name.is_empty() {
                continue;
            }
            let attrs_start = name.len();
            let attrs = body
                .get(attrs_start..)
                .expect("tag name ends at a character boundary");
            out.push(Token::Open {
                name,
                attrs,
                self_closing,
            });
        } else {
            let start = i;
            while i < bytes.len() && bytes[i] != b'<' {
                i += 1;
            }
            let text = input
                .get(start..i)
                .expect("text ends at an ASCII tag delimiter or EOF");
            if !text.is_empty() {
                out.push(Token::Text(decode_entities(text)));
            }
        }
    }
    out
}

pub(crate) fn has_quoted_history(source: &str) -> bool {
    let mut stack: Vec<(String, bool)> = Vec::new();
    let mut quoted = false;
    let mut authored = false;
    for token in tokenize(source) {
        match token {
            Token::Open {
                name,
                attrs,
                self_closing,
            } => {
                let history = (name == "blockquote"
                    && attr(attrs, "type").is_some_and(|v| v == "cite"))
                    || attr(attrs, "class").is_some_and(|classes| {
                        classes
                            .split_ascii_whitespace()
                            .any(|class| matches!(class, "gmail_quote" | "yahoo_quoted"))
                    });
                quoted |= history;
                let hidden = history
                    || matches!(name.as_str(), "head" | "script" | "style")
                    || stack.last().is_some_and(|(_, hidden)| *hidden);
                if !self_closing && !is_void(&name) {
                    stack.push((name, hidden));
                }
            }
            Token::Close { name } => {
                if let Some(index) = stack.iter().rposition(|(tag, _)| tag == &name) {
                    stack.truncate(index);
                }
            }
            Token::Text(text) => {
                authored |=
                    !text.trim().is_empty() && !stack.last().is_some_and(|(_, hidden)| *hidden);
            }
            Token::Comment => {}
        }
    }
    quoted && authored
}

fn tag_name(body: &str) -> &str {
    let mut end = 0;
    for (idx, c) in body.char_indices() {
        if c.is_whitespace() || c == '/' {
            break;
        }
        end = idx + c.len_utf8();
    }
    body.get(..end).expect("tag name uses character indices")
}

// Minimal HTML entity decoder: the five core entities, a few common named
// ones and numeric references. Unknown entities are preserved verbatim.
pub(crate) fn decode_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'&'
            && let Some(end_rel) = input
                .get(i..)
                .expect("scanner remains on a UTF-8 boundary")
                .find(';')
        {
            let end = i + end_rel;
            let raw = input
                .get(i + 1..end)
                .expect("entity delimiters are ASCII boundaries");
            if let Some(decoded) = decode_entity(raw) {
                out.push_str(&decoded);
                i = end + 1;
                continue;
            }
        }
        // Advances one char, which may be multi-byte UTF-8.
        let c_end = next_char_end(input, i);
        out.push_str(
            input
                .get(i..c_end)
                .expect("character indices select one UTF-8 character"),
        );
        i = c_end;
    }
    out
}

fn next_char_end(s: &str, from: usize) -> usize {
    s.get(from..)
        .expect("scanner remains on a UTF-8 boundary")
        .char_indices()
        .nth(1)
        .map_or(s.len(), |(idx, _)| from + idx)
}

fn decode_entity(raw: &str) -> Option<String> {
    if let Some(num) = raw.strip_prefix('#') {
        let (radix, digits) =
            if let Some(hex) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
                (16, hex)
            } else {
                (10, num)
            };
        let code = u32::from_str_radix(digits, radix).ok()?;
        return char::from_u32(code).map(|c| c.to_string());
    }
    Some(match raw {
        "amp" => "&".to_owned(),
        "lt" => "<".to_owned(),
        "gt" => ">".to_owned(),
        "quot" => "\"".to_owned(),
        "apos" | "#39" => "'".to_owned(),
        "nbsp" => "\u{00a0}".to_owned(),
        "mdash" => "\u{2014}".to_owned(),
        "ndash" => "\u{2013}".to_owned(),
        "hellip" => "\u{2026}".to_owned(),
        "copy" => "\u{00a9}".to_owned(),
        "reg" => "\u{00ae}".to_owned(),
        "trade" => "\u{2122}".to_owned(),
        "laquo" => "\u{00ab}".to_owned(),
        "raquo" => "\u{00bb}".to_owned(),
        "lsquo" => "\u{2018}".to_owned(),
        "rsquo" => "\u{2019}".to_owned(),
        "ldquo" => "\u{201c}".to_owned(),
        "rdquo" => "\u{201d}".to_owned(),
        "bull" => "\u{2022}".to_owned(),
        "middot" => "\u{00b7}".to_owned(),
        "zwnj" => "\u{200c}".to_owned(),
        "zwj" => "\u{200d}".to_owned(),
        "shy" => "\u{00ad}".to_owned(),
        "ensp" => "\u{2002}".to_owned(),
        "emsp" => "\u{2003}".to_owned(),
        "thinsp" => "\u{2009}".to_owned(),
        "euro" => "\u{20ac}".to_owned(),
        "pound" => "\u{00a3}".to_owned(),
        "yen" => "\u{00a5}".to_owned(),
        "cent" => "\u{00a2}".to_owned(),
        "sect" => "\u{00a7}".to_owned(),
        "deg" => "\u{00b0}".to_owned(),
        "times" => "\u{00d7}".to_owned(),
        "rarr" => "\u{2192}".to_owned(),
        "larr" => "\u{2190}".to_owned(),
        "sbquo" => "\u{201a}".to_owned(),
        "bdquo" => "\u{201e}".to_owned(),
        _ => return None,
    })
}

fn attr(attrs: &str, name: &str) -> Option<String> {
    let lower = attrs.to_ascii_lowercase();
    let needle = format!(" {}", name.to_ascii_lowercase());
    let mut positions: Vec<usize> = Vec::new();
    if lower.starts_with(&format!("{}=", name.to_ascii_lowercase())) {
        positions.push(0);
    }
    let needle_eq = format!("{needle}=");
    let mut cursor = 0;
    while let Some(rel) = lower.get(cursor..)?.find(&needle_eq) {
        positions.push(cursor + rel + 1);
        cursor = cursor + rel + needle_eq.len();
    }
    let start = *positions.first()?;
    let after_eq = start + name.len() + 1;
    if after_eq > attrs.len() {
        return None;
    }
    let rest = attrs.get(after_eq..)?;
    let first = rest.chars().next()?;
    if first == '"' || first == '\'' {
        let quote = first;
        let after = rest.get(1..)?;
        let end = after.find(quote)?;
        return Some(decode_entities(after.get(..end)?));
    }
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    Some(decode_entities(rest.get(..end)?))
}

// Tags whose entire content, nested tags included, is dropped, so a
// `<script>` block can never leak inline text.
const DROP_TAGS: &[&str] = &[
    "script", "style", "noscript", "iframe", "object", "embed", "form", "input", "textarea",
    "select", "option", "button", "svg", "math", "canvas", "video", "audio", "source", "track",
    "map", "area", "meta", "link", "base", "template", "frame", "frameset", "applet",
];

// HTML void elements. Those also in `DROP_TAGS` must not push a drop scope,
// or every following sibling would be swallowed waiting for a close tag.
const VOID_TAGS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

fn is_void(tag: &str) -> bool {
    VOID_TAGS.contains(&tag)
}

fn inline_style_effect(name: &str) -> Option<InlineStyle> {
    Some(match name {
        "b" | "strong" => InlineStyle::Bold,
        "i" | "em" | "cite" | "dfn" | "var" => InlineStyle::Italic,
        "u" | "ins" => InlineStyle::Underline,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InlineStyle {
    Bold,
    Italic,
    Underline,
}

fn is_safe_href(href: &str) -> bool {
    let trimmed = href.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(authority) = lower
        .strip_prefix("http://")
        .or_else(|| lower.strip_prefix("https://"))
        .map(|rest| rest.split(['/', '?', '#']).next().unwrap_or_default())
    {
        return !authority.is_empty() && !authority.contains('@');
    }
    lower.starts_with("mailto:") || lower.starts_with('#') // Fragment-only links stay within the rendered message.
}

fn is_remote_image_src(src: &str) -> bool {
    let lower = src.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("//")
}

// Holds the open block container and appends inline runs to it. Drop tags push
// a drop depth that swallows every token until the matching close.
struct BlockBuilder {
    opts: SanitizeOptions,
    blocks: Vec<HtmlBlock>,
    blocked_remote_urls: Vec<String>,
    bold: u32,
    italic: u32,
    underline: u32,
    href: Vec<String>,
    drop_depth: u32,
    drop_stack: Vec<String>,
    current: OpenBlock,
    blockquote_depth: u8,
    list_stack: Vec<ListState>,
}

#[derive(Clone, Debug)]
struct ListState {
    marker: ListMarker,
    next_number: u32,
}

#[derive(Clone, Debug)]
enum OpenBlock {
    None,
    Paragraph(Vec<InlineRun>),
    Heading {
        level: u8,
        runs: Vec<InlineRun>,
    },
    ListItem {
        marker: ListMarker,
        depth: u8,
        runs: Vec<InlineRun>,
    },
    Blockquote {
        depth: u8,
        runs: Vec<InlineRun>,
    },
    Pre(String),
}

impl BlockBuilder {
    fn new(opts: SanitizeOptions) -> Self {
        Self {
            opts,
            blocks: Vec::new(),
            blocked_remote_urls: Vec::new(),
            bold: 0,
            italic: 0,
            underline: 0,
            href: Vec::new(),
            drop_depth: 0,
            drop_stack: Vec::new(),
            current: OpenBlock::None,
            blockquote_depth: 0,
            list_stack: Vec::new(),
        }
    }

    fn consume(&mut self, token: Token<'_>) {
        if self.drop_depth > 0 {
            match token {
                Token::Open {
                    name, self_closing, ..
                } => {
                    // Raw-text tags hold no markup, so only their own close matters.
                    let raw_text = self.drop_stack.last().is_some_and(|open| {
                        matches!(open.as_str(), "script" | "style" | "textarea")
                    });
                    if !raw_text && !self_closing && !is_void(&name) {
                        self.drop_depth += 1;
                        self.drop_stack.push(name);
                    }
                }
                Token::Close { name } => {
                    // A close pops back to its own open, so unclosed `<p>`/`<td>`
                    // children cannot keep a hidden subtree open.
                    if let Some(at) = self.drop_stack.iter().rposition(|open| *open == name) {
                        let popped = (self.drop_stack.len() - at) as u32;
                        self.drop_stack.truncate(at);
                        self.drop_depth -= popped;
                    }
                }
                _ => {}
            }
            return;
        }

        match token {
            Token::Comment => {}
            Token::Text(text) => self.on_text(&text),
            Token::Open {
                name,
                attrs,
                self_closing,
            } => {
                self.on_open(&name, attrs, self_closing);
            }
            Token::Close { name } => self.on_close(&name),
        }
    }

    fn on_text(&mut self, raw: &str) {
        if let OpenBlock::Pre(buf) = &mut self.current {
            buf.push_str(raw);
            return;
        }
        // Collapse whitespace runs and drop whitespace-only text between blocks.
        let normalised = collapse_whitespace(raw);
        if normalised.is_empty() {
            return;
        }
        if matches!(self.current, OpenBlock::None) {
            self.current = OpenBlock::Paragraph(Vec::new());
        }
        // An empty href entry marks a suppressed unsafe scheme.
        let link_href = match self.href.last() {
            Some(h) if !h.is_empty() => Some(h.clone()),
            _ => None,
        };
        let run = InlineRun {
            text: normalised,
            bold: self.bold > 0,
            italic: self.italic > 0,
            underline: self.underline > 0,
            link_href,
        };
        self.push_run(run);
    }

    fn push_run(&mut self, run: InlineRun) {
        match &mut self.current {
            OpenBlock::Paragraph(runs) => merge_run(runs, run),
            OpenBlock::Heading { runs, .. } => merge_run(runs, run),
            OpenBlock::ListItem { runs, .. } => merge_run(runs, run),
            OpenBlock::Blockquote { runs, .. } => merge_run(runs, run),
            OpenBlock::Pre(_) | OpenBlock::None => {
                let mut runs = Vec::new();
                merge_run(&mut runs, run);
                self.current = OpenBlock::Paragraph(runs);
            }
        }
    }

    fn on_open(&mut self, name: &str, attrs: &str, self_closing: bool) {
        // Void drop tags never close, so they push no drop scope.
        if DROP_TAGS.contains(&name) || (is_hidden(attrs) && name != "img") {
            if self_closing || is_void(name) {
                return;
            }
            self.drop_depth += 1;
            self.drop_stack.push(name.to_owned());
            return;
        }

        if let Some(style) = inline_style_effect(name) {
            match style {
                InlineStyle::Bold => self.bold += 1,
                InlineStyle::Italic => self.italic += 1,
                InlineStyle::Underline => self.underline += 1,
            }
            return;
        }

        match name {
            "a" => {
                let raw = attr(attrs, "href").unwrap_or_default();
                // Always push a balancing entry; keep the value only for an allowed scheme.
                let safe = if is_safe_href(&raw) {
                    raw
                } else {
                    String::new()
                };
                self.href.push(safe);
            }
            "br" => {
                let run = InlineRun {
                    text: "\n".to_owned(),
                    bold: false,
                    italic: false,
                    underline: false,
                    link_href: None,
                };
                if matches!(self.current, OpenBlock::None) {
                    self.current = OpenBlock::Paragraph(Vec::new());
                }
                self.push_run(run);
            }
            "hr" => {
                self.flush_current();
                self.blocks.push(HtmlBlock::HorizontalRule);
            }
            "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "aside"
            | "nav" | "figure" | "figcaption" | "address" | "details" | "summary" => {
                self.flush_current();
                // Inside a `<blockquote>` every block stays quoted until its close.
                self.current = if self.blockquote_depth > 0 {
                    OpenBlock::Blockquote {
                        depth: self.blockquote_depth - 1,
                        runs: Vec::new(),
                    }
                } else {
                    OpenBlock::Paragraph(Vec::new())
                };
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush_current();
                let level = name.as_bytes()[1] - b'0';
                self.current = OpenBlock::Heading {
                    level: level.clamp(1, 6),
                    runs: Vec::new(),
                };
            }
            "ul" => {
                self.flush_current();
                self.list_stack.push(ListState {
                    marker: ListMarker::Bullet,
                    next_number: 1,
                });
            }
            "ol" => {
                self.flush_current();
                let start = attr(attrs, "start")
                    .and_then(|s| s.trim().parse::<u32>().ok())
                    .unwrap_or(1);
                self.list_stack.push(ListState {
                    marker: ListMarker::Numbered(start),
                    next_number: start,
                });
            }
            "li" => {
                self.flush_current();
                let depth = (self.list_stack.len().saturating_sub(1)) as u8;
                let marker = if let Some(state) = self.list_stack.last_mut() {
                    match state.marker {
                        ListMarker::Bullet => ListMarker::Bullet,
                        ListMarker::Numbered(_) => {
                            let n = state.next_number;
                            state.next_number += 1;
                            ListMarker::Numbered(n)
                        }
                    }
                } else {
                    ListMarker::Bullet
                };
                self.current = OpenBlock::ListItem {
                    marker,
                    depth,
                    runs: Vec::new(),
                };
            }
            "blockquote" => {
                self.flush_current();
                self.blockquote_depth = self.blockquote_depth.saturating_add(1);
                self.current = OpenBlock::Blockquote {
                    depth: self.blockquote_depth - 1,
                    runs: Vec::new(),
                };
            }
            "pre" => {
                self.flush_current();
                self.current = OpenBlock::Pre(String::new());
            }
            "img" => {
                // Remote image handling: the core privacy branch.
                let src = attr(attrs, "src").unwrap_or_default();
                let alt = attr(attrs, "alt").unwrap_or_default();
                if is_remote_image_src(&src) && (is_pixel(attrs) || is_hidden(attrs)) {
                    // A tracking pixel or hidden image has nothing to show; it
                    // still counts as blocked.
                    self.blocked_remote_urls.push(src);
                } else if is_remote_image_src(&src) {
                    if self.opts.allow_remote_images {
                        // This presentation mode retains only alt text; the
                        // source URL never crosses the sanitizer boundary.
                        self.blocked_remote_urls.push(src);
                        if !alt.is_empty() {
                            self.on_text(&alt);
                        }
                    } else {
                        self.blocked_remote_urls.push(src);
                        self.push_placeholder(alt);
                    }
                } else if !src.trim().is_empty() {
                    // Non-remote sources are placeholders too: there is no CID resolver, and
                    // `data:` URIs can carry SVG script payloads.
                    self.push_placeholder(alt);
                }
            }
            "table" | "tbody" | "thead" | "tfoot" | "tr" => {
                if matches!(name, "tr") {
                    self.flush_current();
                }
            }
            "td" | "th" => {
                self.flush_current();
                self.current = OpenBlock::Paragraph(Vec::new());
            }
            _ => {}
        }
        if self_closing && matches!(name, "img" | "br" | "hr") {}
    }

    fn on_close(&mut self, name: &str) {
        if let Some(style) = inline_style_effect(name) {
            match style {
                InlineStyle::Bold => self.bold = self.bold.saturating_sub(1),
                InlineStyle::Italic => self.italic = self.italic.saturating_sub(1),
                InlineStyle::Underline => self.underline = self.underline.saturating_sub(1),
            }
            return;
        }
        match name {
            "a" => {
                self.href.pop();
            }
            "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "aside"
            | "nav" | "figure" | "figcaption" | "address" | "details" | "summary" | "h1" | "h2"
            | "h3" | "h4" | "h5" | "h6" | "li" | "pre" | "td" | "th" => {
                self.flush_current();
            }
            "blockquote" => {
                self.flush_current();
                self.blockquote_depth = self.blockquote_depth.saturating_sub(1);
            }
            "ul" | "ol" => {
                self.flush_current();
                self.list_stack.pop();
            }
            "tr" | "table" | "tbody" | "thead" | "tfoot" => {
                self.flush_current();
            }
            _ => {}
        }
    }

    // Images without alt text are decoration (spacers, icons, logos); the
    // blocked-content banner already counts them, so only alt text gets a chip.
    fn push_placeholder(&mut self, alt: String) {
        let alt = alt.trim().to_owned();
        if alt.is_empty() {
            return;
        }
        self.flush_current();
        // A row of icons (social links, store badges) becomes one chip
        // naming them all rather than a stack of chips.
        if let Some(HtmlBlock::RemoteImagePlaceholder { alt: previous }) = self.blocks.last_mut()
            && previous.len() < 120
        {
            previous.push_str(", ");
            previous.push_str(&alt);
            return;
        }
        self.blocks.push(HtmlBlock::RemoteImagePlaceholder { alt });
    }

    fn flush_current(&mut self) {
        let mut taken = std::mem::replace(&mut self.current, OpenBlock::None);
        if let OpenBlock::Paragraph(runs)
        | OpenBlock::Heading { runs, .. }
        | OpenBlock::ListItem { runs, .. }
        | OpenBlock::Blockquote { runs, .. } = &mut taken
        {
            trim_runs(runs);
        }
        match taken {
            OpenBlock::None => {}
            OpenBlock::Paragraph(runs) => {
                if !runs.is_empty() {
                    self.blocks.push(HtmlBlock::Paragraph(runs));
                }
            }
            OpenBlock::Heading { level, runs } => {
                if !runs.is_empty() {
                    self.blocks.push(HtmlBlock::Heading { level, runs });
                }
            }
            OpenBlock::ListItem {
                marker,
                depth,
                runs,
            } => {
                if !runs.is_empty() {
                    self.blocks.push(HtmlBlock::ListItem {
                        marker,
                        depth,
                        runs,
                    });
                }
            }
            OpenBlock::Blockquote { depth, runs } => {
                if !runs.is_empty() {
                    self.blocks.push(HtmlBlock::Blockquote { depth, runs });
                }
            }
            OpenBlock::Pre(text) => {
                if !text.trim().is_empty() {
                    self.blocks.push(HtmlBlock::PreformattedText(text));
                }
            }
        }
    }

    fn finish(mut self) -> (Vec<HtmlBlock>, Vec<String>) {
        self.flush_current();
        (self.blocks, self.blocked_remote_urls)
    }
}

fn merge_run(runs: &mut Vec<InlineRun>, run: InlineRun) {
    if run.text.is_empty() {
        return;
    }
    if let Some(last) = runs.last_mut()
        && last.bold == run.bold
        && last.italic == run.italic
        && last.underline == run.underline
        && last.link_href == run.link_href
    {
        last.text.push_str(&run.text);
        return;
    }
    runs.push(run);
}

// Collapse ASCII whitespace runs to one space; `<pre>` bypasses this.
fn collapse_whitespace(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_was_space = false;
    for c in input.chars() {
        if c.is_ascii_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(c);
            last_was_space = false;
        }
    }
    out
}

fn is_invisible(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '\u{00ad}' | '\u{034f}' | '\u{200b}' | '\u{200c}' | '\u{2060}' | '\u{feff}'
        )
}

fn trim_runs(runs: &mut Vec<InlineRun>) {
    if runs.iter().all(|run| run.text.chars().all(is_invisible)) {
        runs.clear();
        return;
    }
    while runs
        .first()
        .is_some_and(|run| run.text.chars().all(is_invisible))
    {
        runs.remove(0);
    }
    while runs
        .last()
        .is_some_and(|run| run.text.chars().all(is_invisible))
    {
        runs.pop();
    }
    if let Some(first) = runs.first_mut() {
        first.text = first.text.trim_start_matches(is_invisible).to_owned();
    }
    if let Some(last) = runs.last_mut() {
        last.text = last.text.trim_end_matches(is_invisible).to_owned();
    }
    for run in runs.iter_mut() {
        while run.text.contains("\n ") || run.text.contains(" \n") {
            run.text = run.text.replace("\n ", "\n").replace(" \n", "\n");
        }
    }
}

// Inline style or attribute that keeps an element off screen: preheaders and
// Outlook-only blocks.
fn is_hidden(attrs: &str) -> bool {
    let lower = attrs.to_ascii_lowercase();
    if lower
        .split_whitespace()
        .any(|word| word == "hidden" || word.starts_with("hidden="))
    {
        return true;
    }
    let Some(style) = attr(attrs, "style") else {
        return false;
    };
    let style: String = style
        .to_ascii_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let has = |declaration: &str| {
        style
            .split(';')
            .any(|part| part.trim_end_matches("!important") == declaration)
    };
    has("display:none")
        || has("visibility:hidden")
        || has("mso-hide:all")
        || has("opacity:0")
        || ((has("max-height:0") || has("max-height:0px") || has("height:0") || has("height:0px"))
            && has("overflow:hidden"))
}

fn is_pixel(attrs: &str) -> bool {
    let tiny = |value: Option<String>| {
        value
            .and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok())
            .is_some_and(|px| px <= 2.0)
    };
    tiny(attr(attrs, "width")) || tiny(attr(attrs, "height"))
}

include!("html/plain_text.rs");
