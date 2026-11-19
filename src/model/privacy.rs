//! Pure tracker detection and remote-content policy decisions.
//! The HTML loader enforces these decisions after sanitization.

// Sender policy is enforced by the HTML resource loader.
//
// Sender blocks take precedence in every mode, while sender allowances can
// override `AlwaysBlock`. `AskPerSender` is the block-by-default mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(crate) enum RemoteContentPolicy {
    AlwaysBlock,
    NeverBlock,
    #[default]
    AskPerSender,
}

impl RemoteContentPolicy {
    pub(crate) const ALL: [RemoteContentPolicy; 3] = [
        RemoteContentPolicy::AlwaysBlock,
        RemoteContentPolicy::AskPerSender,
        RemoteContentPolicy::NeverBlock,
    ];

    pub(crate) fn radio_label(self) -> &'static str {
        match self {
            RemoteContentPolicy::AlwaysBlock => "Always block (strip images)",
            RemoteContentPolicy::AskPerSender => "Ask per sender (recommended)",
            RemoteContentPolicy::NeverBlock => "Allow by default",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum RemoteContentDecision {
    Blocked(BlockedReason),
    Allowed(AllowedReason),
    // Global policy strips remote content unless the sender is allowlisted.
    Stripped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum BlockedReason {
    Policy,
    Blocklist,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum AllowedReason {
    Policy,
    Allowlist,
}

// Composes policy and sender rules without performing a fetch.
pub(crate) fn remote_content_decision(
    policy: RemoteContentPolicy,
    allowlist: &[String],
    blocklist: &[String],
    sender_email: &str,
    message_has_remote_content: bool,
) -> RemoteContentDecision {
    if !message_has_remote_content {
        return RemoteContentDecision::Allowed(AllowedReason::Policy);
    }

    if sender_in_list(sender_email, blocklist) {
        return RemoteContentDecision::Blocked(BlockedReason::Blocklist);
    }

    if sender_in_list(sender_email, allowlist) {
        return RemoteContentDecision::Allowed(AllowedReason::Allowlist);
    }

    match policy {
        RemoteContentPolicy::NeverBlock => RemoteContentDecision::Allowed(AllowedReason::Policy),
        RemoteContentPolicy::AskPerSender => RemoteContentDecision::Blocked(BlockedReason::Policy),
        RemoteContentPolicy::AlwaysBlock => RemoteContentDecision::Stripped,
    }
}

fn sender_in_list(sender_email: &str, list: &[String]) -> bool {
    let trimmed = sender_email.trim();
    if trimmed.is_empty() {
        return false;
    }
    list.iter().any(|s| s.eq_ignore_ascii_case(trimmed))
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct Tracker {
    pub(crate) kind: TrackerKind,
    pub(crate) source: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum TrackerKind {
    OnePixelImage,
    KnownTrackerDomain,
    UtmBeacon,
}

// A short static list of common tracker and analytics domains, matched
// case-insensitively on the URL's domain suffix (`sub.example.com` matches
// `example.com`).
const KNOWN_TRACKER_DOMAINS: &[&str] = &[
    "doubleclick.net",
    "google-analytics.com",
    "googletagmanager.com",
    "mailchimp.com",
    "list-manage.com",
    "mkt.com",
    "mktoresp.com",
    "sendgrid.net",
    "sendgrid.com",
    "hubspotlinks.com",
    "hubspot.com",
    "mailtrack.io",
    "mailmodo.com",
    "sparkpost.com",
    "brevo.com",
    "convertkit.com",
    "email.marketingcloud.com",
    "e.customeriomail.com",
    "postmarkapp.com",
    "constantcontact.com",
    "ct.sendgrid.net",
    "trk.mail.com",
    "click.mail.com",
    "beacon-2.newrelic.com",
];

// Detect likely trackers in an HTML or plain-text body, one entry per
// `(kind, source)` in document order.
//
// Inspects `<img>` tags (including 1×1 `width` / `height`) and every raw URL.
// Character-based, matching tracker shapes rather than parsing HTML.
pub(crate) fn detect_trackers(body: &str) -> Vec<Tracker> {
    let mut out: Vec<Tracker> = Vec::new();

    for tag in find_img_tags(body) {
        let src = attr_value(tag, "src").unwrap_or_default();
        if src.is_empty() {
            continue;
        }
        let w = parse_dim_attr(tag, "width");
        let h = parse_dim_attr(tag, "height");
        if is_one_pixel_dims(w, h) {
            push_unique(
                &mut out,
                Tracker {
                    kind: TrackerKind::OnePixelImage,
                    source: src.to_owned(),
                },
            );
        }
        // An image's URL also goes through the domain and UTM heuristics, so a
        // `utm_source` image trips without 1×1 attributes.
        classify_url(src, &mut out);
    }

    for url in find_urls(body) {
        classify_url(url, &mut out);
    }

    out
}

fn push_unique(out: &mut Vec<Tracker>, tracker: Tracker) {
    if !out.iter().any(|t| t == &tracker) {
        out.push(tracker);
    }
}

fn classify_url(url: &str, out: &mut Vec<Tracker>) {
    if let Some(host) = url_host(url) {
        let host_lower = host.to_ascii_lowercase();
        if KNOWN_TRACKER_DOMAINS
            .iter()
            .any(|d| host_lower == *d || host_lower.ends_with(&format!(".{d}")))
        {
            push_unique(
                out,
                Tracker {
                    kind: TrackerKind::KnownTrackerDomain,
                    source: url.to_owned(),
                },
            );
        }
    }
    if has_beacon_query(url) {
        push_unique(
            out,
            Tracker {
                kind: TrackerKind::UtmBeacon,
                source: url.to_owned(),
            },
        );
    }
}

fn is_one_pixel_dims(w: Option<u32>, h: Option<u32>) -> bool {
    matches!((w, h), (Some(1), Some(1)) | (Some(0), Some(0)))
}

fn parse_dim_attr(tag: &str, name: &str) -> Option<u32> {
    let v = attr_value(tag, name)?;
    let trimmed = v.trim().trim_end_matches("px");
    trimmed.parse::<u32>().ok()
}

fn find_img_tags(body: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let lower = body.to_ascii_lowercase();
    let mut i = 0;
    while let Some(start_rel) = lower
        .get(i..)
        .expect("ASCII tag delimiter preserves byte boundaries")
        .find("<img")
    {
        let start = i + start_rel;
        let after = start + 4;
        if let Some(end_rel) = body
            .get(after..)
            .expect("ASCII image prefix preserves byte boundaries")
            .find('>')
        {
            let end = after + end_rel;
            out.push(
                body.get(start..=end)
                    .expect("image tag delimiters are ASCII boundaries"),
            );
            i = end + 1;
        } else {
            break;
        }
    }
    out
}

fn attr_value<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_ascii_lowercase();
    let needle = format!("{}=", name.to_ascii_lowercase());
    let start_rel = lower.find(&needle)?;
    let after_eq = start_rel + needle.len();
    let rest = tag.get(after_eq..)?;
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .unwrap_or(rest.len());
        return rest.get(..end);
    }
    let after_quote = rest.get(1..)?;
    let end = after_quote.find(quote)?;
    after_quote.get(..end)
}

fn find_urls(body: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut cursor = 0;
    while cursor < body.len() {
        let rest = body
            .get(cursor..)
            .expect("URL delimiter preserves byte boundaries");
        let http = rest.find("http://");
        let https = rest.find("https://");
        let rel = match (http, https) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => break,
        };
        let start = cursor + rel;
        let sub = body
            .get(start..)
            .expect("ASCII URL prefix starts at a UTF-8 boundary");
        let end_rel = sub
            .find(|c: char| {
                c.is_whitespace() || c == '"' || c == '\'' || c == '<' || c == '>' || c == ')'
            })
            .unwrap_or(sub.len());
        let mut end = start + end_rel;
        while end > start {
            let last = body.as_bytes()[end - 1];
            if last == b',' || last == b'.' || last == b';' || last == b']' || last == b')' {
                end -= 1;
            } else {
                break;
            }
        }
        out.push(
            body.get(start..end)
                .expect("URL punctuation delimiters preserve UTF-8 boundaries"),
        );
        cursor = end.max(start + 1);
    }
    out
}

fn url_host(url: &str) -> Option<&str> {
    let after_scheme = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let host_end = after_scheme
        .find(['/', '?', '#', ':'])
        .unwrap_or(after_scheme.len());
    after_scheme.get(..host_end)
}

fn has_beacon_query(url: &str) -> bool {
    let Some(idx) = url.find('?') else {
        return false;
    };
    let query = url
        .get(idx + 1..)
        .expect("query delimiter is an ASCII boundary");
    for pair in query.split('&') {
        let key = pair.split('=').next().unwrap_or("").to_ascii_lowercase();
        if key.starts_with("utm_") {
            return true;
        }
        if matches_beacon_key(&key) {
            return true;
        }
    }
    false
}

fn matches_beacon_key(key: &str) -> bool {
    matches!(
        key,
        "beacon"
            | "open"
            | "opens"
            | "pixel"
            | "track"
            | "tracker"
            | "tracking"
            | "mid"
            | "subscriber"
            | "subscriber_id"
            | "recipient"
            | "recipient_id"
            | "campaign"
            | "campaign_id"
            | "email_id"
            | "eid"
    )
}
