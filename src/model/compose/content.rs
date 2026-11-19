// Prefixes a subject unless its trimmed text already starts with the prefix,
// compared case-insensitively on Unicode scalar boundaries.
pub(crate) fn add_subject_prefix(subject: &str, prefix: &str) -> String {
    let trimmed = subject.trim_start();
    let head: String = trimmed.chars().take(prefix.chars().count()).collect();
    if head.eq_ignore_ascii_case(prefix) {
        return subject.to_owned();
    }
    format!("{prefix} {}", subject.trim())
}

pub(crate) fn format_address(name: &str, email: &str) -> String {
    if name.is_empty() {
        email.to_owned()
    } else {
        format!("{name} <{email}>")
    }
}

pub(crate) fn split_addresses(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

pub(crate) fn email_token(addr: &str) -> &str {
    if let Some((_, tail)) = addr.rsplit_once('<')
        && let Some((email, _)) = tail.split_once('>')
    {
        return email.trim();
    }
    addr.trim()
}

pub(crate) fn reply_all_cc(originals: &[String], exclude: &[&str]) -> String {
    let mut seen = std::collections::HashSet::new();
    originals
        .iter()
        .filter(|addr| {
            let tok = email_token(addr);
            !exclude
                .iter()
                .any(|ex| !ex.is_empty() && tok.eq_ignore_ascii_case(email_token(ex)))
        })
        .filter(|addr| seen.insert(email_token(addr).to_ascii_lowercase()))
        .cloned()
        .collect::<Vec<_>>()
        .join(", ")
}

fn original_body(m: &Message) -> std::borrow::Cow<'_, str> {
    if !m.body.trim().is_empty() {
        return std::borrow::Cow::Borrowed(&m.body);
    }
    match &m.body_html {
        Some(source) => {
            let rendered = crate::model::html::sanitize_and_render_html(
                source,
                crate::model::html::SanitizeOptions::SAFE,
            );
            std::borrow::Cow::Owned(crate::model::html::blocks_to_plain_text(&rendered.blocks))
        }
        None => std::borrow::Cow::Borrowed(&m.body),
    }
}

// Quote the plain-text body, falling back to safe HTML text when absent.
pub(crate) fn quote_body(m: &Message) -> String {
    let header = format!("On {}, {} wrote:", m.date.detail_label(), m.sender_name);
    let quoted: String = original_body(m)
        .lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("\n\n{header}\n{quoted}\n")
}

pub(crate) fn forward_body(m: &Message) -> String {
    let to = m.to.join(", ");
    format!(
        "\n\n--- Forwarded message ---\nFrom: {} <{}>\nDate: {}\nSubject: {}\nTo: {}\n\n{}",
        m.sender_name,
        m.sender_email,
        m.date.detail_label(),
        m.subject,
        to,
        original_body(m),
    )
}

pub(crate) const PREVIEW_MAX_CHARS: usize = 320;

pub(crate) fn preview_from_body(body: &str) -> String {
    body.split_whitespace()
        .flat_map(|word| word.chars().chain(std::iter::once(' ')))
        .take(PREVIEW_MAX_CHARS)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

// RFC 3676 §4.3 signature delimiter ("-- " then newline).
pub(crate) const SIGNATURE_DELIMITER: &str = "-- \n";

pub(crate) fn compose_body_with_signature(base: &str, signature: &str, mode: &ComposeMode) -> String {
    if signature.trim().is_empty() {
        return base.to_owned();
    }
    if matches!(mode, ComposeMode::EditDraft(_)) {
        return base.to_owned();
    }
    let delimited = format!("{SIGNATURE_DELIMITER}{signature}");
    if base.contains(&delimited) {
        return base.to_owned();
    }
    match mode {
        ComposeMode::New => {
            if base.is_empty() {
                format!("\n\n{SIGNATURE_DELIMITER}{signature}")
            } else {
                format!("{base}\n\n{SIGNATURE_DELIMITER}{signature}")
            }
        }
        ComposeMode::Reply(_) | ComposeMode::ReplyAll(_) | ComposeMode::Forward(_) => {
            if let Some(rest) = base.strip_prefix("\n\n") {
                format!("\n\n{SIGNATURE_DELIMITER}{signature}\n\n{rest}")
            } else {
                format!("\n\n{SIGNATURE_DELIMITER}{signature}\n\n{base}")
            }
        }
        ComposeMode::EditDraft(_) => base.to_owned(),
    }
}

pub(crate) fn compose_body_with_account_signature(
    base: &str,
    account: Option<&Account>,
    mode: &ComposeMode,
) -> String {
    let Some(acc) = account else {
        return base.to_owned();
    };
    if !acc.append_signature {
        return base.to_owned();
    }
    compose_body_with_signature(base, &acc.signature, mode)
}
