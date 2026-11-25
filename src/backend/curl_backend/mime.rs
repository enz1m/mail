//! MIME parsing: messages, entities, headers, transfer decoding, and
//! dates.

use super::{
    MAX_MESSAGE_BODY_BYTES, MAX_MIME_DEPTH, MAX_MIME_HEADER_BYTES, MAX_MIME_HEADER_COUNT,
    MAX_MIME_PARTS,
};
use crate::model::types::{ListUnsubscribe, MessageBodyState, MessageDate};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub(super) struct DecodedAttachment {
    pub(super) part: String,
    pub(super) name: String,
    pub(super) mime: String,
    pub(super) bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(super) struct ParsedMessageContent {
    pub(super) body: String,
    pub(super) preview: String,
    pub(super) body_html: Option<String>,
    pub(super) body_state: MessageBodyState,
    pub(super) selected_part: String,
    pub(super) attachments: Vec<DecodedAttachment>,
    pub(super) has_remote_content: bool,
    pub(super) has_inline_image: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MimeParseFailure {
    Bounds,
    Headers,
    Multipart,
    TransferEncoding,
}

#[derive(Clone, Debug)]
struct MimeLeaf {
    part: String,
    media_type: String,
    disposition: String,
    filename: Option<String>,
    content_id: Option<String>,
    charset: Option<String>,
    bytes: Vec<u8>,
}

pub(super) fn parse_mime_message(raw: &[u8]) -> Result<ParsedMessageContent, MimeParseFailure> {
    let mut leaves = Vec::new();
    let mut part_count = 0_usize;
    parse_mime_entity(raw, "1", 0, &mut part_count, &mut leaves)?;

    let mut plain = None;
    let mut html = None;
    let mut attachments = Vec::new();
    let mut has_inline_image = false;
    for leaf in leaves {
        let is_attachment = leaf.disposition == "attachment" || leaf.filename.is_some();
        let is_inline_image = leaf.media_type.starts_with("image/") && leaf.content_id.is_some();
        if is_attachment || is_inline_image {
            if attachments.len() >= crate::attachment_store::MAX_ATTACHMENT_COUNT {
                return Err(MimeParseFailure::Bounds);
            }
            has_inline_image |= is_inline_image;
            attachments.push(DecodedAttachment {
                part: leaf.part,
                name: leaf
                    .filename
                    .unwrap_or_else(|| format!("attachment-{}", attachments.len() + 1)),
                mime: leaf.media_type,
                bytes: leaf.bytes,
            });
        } else if leaf.media_type == "text/plain" && plain.is_none() {
            let text = decode_text(&leaf.bytes, leaf.charset.as_deref());
            plain = Some((leaf.part, text));
        } else if leaf.media_type == "text/html" && html.is_none() {
            let source = decode_text(&leaf.bytes, leaf.charset.as_deref());
            html = Some((leaf.part, source));
        }
    }

    if let Some((selected_part, source)) = html {
        let rendered = crate::model::html::sanitize_and_render_html(
            &source,
            crate::model::html::SanitizeOptions::SAFE,
        );
        return Ok(ParsedMessageContent {
            preview: plain
                .as_ref()
                .filter(|(_, text)| !text.trim().is_empty())
                .map_or_else(
                    || crate::model::html::blocks_to_preview(&rendered.blocks),
                    |(_, text)| crate::model::compose::preview_from_body(text),
                ),
            body: plain.map_or_else(String::new, |(_, text)| text),
            body_html: Some(source),
            body_state: MessageBodyState::Complete,
            selected_part,
            attachments,
            has_remote_content: !rendered.blocked_remote_urls.is_empty(),
            has_inline_image,
        });
    }
    if let Some((selected_part, body)) = plain {
        return Ok(ParsedMessageContent {
            preview: crate::model::compose::preview_from_body(&body),
            body,
            body_html: None,
            body_state: MessageBodyState::Complete,
            selected_part,
            attachments,
            has_remote_content: false,
            has_inline_image,
        });
    }
    Ok(ParsedMessageContent {
        body: String::new(),
        preview: String::new(),
        body_html: None,
        body_state: MessageBodyState::Complete,
        selected_part: "BODY.NONE".to_owned(),
        attachments,
        has_remote_content: false,
        has_inline_image,
    })
}

fn parse_mime_entity(
    raw: &[u8],
    part: &str,
    depth: usize,
    part_count: &mut usize,
    leaves: &mut Vec<MimeLeaf>,
) -> Result<(), MimeParseFailure> {
    if depth > MAX_MIME_DEPTH {
        return Err(MimeParseFailure::Bounds);
    }
    *part_count = part_count.checked_add(1).ok_or(MimeParseFailure::Bounds)?;
    if *part_count > MAX_MIME_PARTS {
        return Err(MimeParseFailure::Bounds);
    }
    let (header_bytes, body) = split_mime_head(raw)?;
    let headers = parse_mime_headers(header_bytes)?;
    let content_type = headers
        .get("content-type")
        .map_or("text/plain", String::as_str);
    let (media_type, content_parameters) = parse_header_value(content_type)?;
    if media_type.is_empty()
        || !media_type
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/!#$&-^_.+".contains(&byte))
    {
        return Err(MimeParseFailure::Headers);
    }
    let transfer = headers.get("content-transfer-encoding").map_or_else(
        || "7bit".to_owned(),
        |value| value.trim().to_ascii_lowercase(),
    );
    if media_type.starts_with("multipart/") {
        if !matches!(transfer.as_str(), "7bit" | "8bit" | "binary") {
            return Err(MimeParseFailure::TransferEncoding);
        }
        let boundary = content_parameters
            .iter()
            .find(|(name, _)| name == "boundary")
            .map(|(_, value)| value.as_str())
            .filter(|value| !value.is_empty() && value.len() <= 70)
            .ok_or(MimeParseFailure::Multipart)?;
        let children = split_multipart(body, boundary.as_bytes())?;
        for (index, child) in children.into_iter().enumerate() {
            let child_number = index.checked_add(1).ok_or(MimeParseFailure::Bounds)?;
            let child_part = format!("{part}.{child_number}");
            parse_mime_entity(child, &child_part, depth + 1, part_count, leaves)?;
        }
        return Ok(());
    }

    let disposition_value = headers
        .get("content-disposition")
        .map_or("", String::as_str);
    let (disposition, disposition_parameters) = parse_header_value(disposition_value)?;
    if !disposition.is_empty()
        && !disposition
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(MimeParseFailure::Headers);
    }
    let filename = parameter_value(&disposition_parameters, "filename*")
        .and_then(decode_extended_filename)
        .or_else(|| {
            parameter_value(&disposition_parameters, "filename")
                .or_else(|| parameter_value(&content_parameters, "name"))
                .map(crate::model::header_text::decode_header_text)
        })
        .map(|name| crate::attachment_store::bounded_display_name(&name));
    let content_id = headers
        .get("content-id")
        .map(|value| {
            value
                .trim()
                .trim_matches(|character| matches!(character, '<' | '>'))
                .to_owned()
        })
        .filter(|value| !value.is_empty() && value.len() <= 998);
    let charset = parameter_value(&content_parameters, "charset")
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    if charset.as_ref().is_some_and(|value| value.len() > 64) {
        return Err(MimeParseFailure::Bounds);
    }
    let attachment_like = disposition == "attachment"
        || filename.is_some()
        || (media_type.starts_with("image/") && content_id.is_some());
    let decoded_limit = if attachment_like {
        usize::try_from(crate::attachment_store::MAX_ATTACHMENT_BYTES)
            .map_err(|_| MimeParseFailure::Bounds)?
    } else {
        MAX_MESSAGE_BODY_BYTES as usize
    };
    let bytes = decode_transfer(body, &transfer, decoded_limit)?;
    leaves.push(MimeLeaf {
        part: part.to_owned(),
        media_type,
        disposition,
        filename,
        content_id,
        charset,
        bytes,
    });
    Ok(())
}

fn decode_text(bytes: &[u8], charset: Option<&str>) -> String {
    charset
        .and_then(|label| crate::model::header_text::decode_charset(label, bytes))
        .unwrap_or_else(|| String::from_utf8_lossy(bytes).into_owned())
}

fn split_mime_head(raw: &[u8]) -> Result<(&[u8], &[u8]), MimeParseFailure> {
    let separator = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| (index, 4))
        .or_else(|| {
            raw.windows(2)
                .position(|window| window == b"\n\n")
                .map(|index| (index, 2))
        })
        .ok_or(MimeParseFailure::Headers)?;
    if separator.0 > MAX_MIME_HEADER_BYTES {
        return Err(MimeParseFailure::Bounds);
    }
    Ok((&raw[..separator.0], &raw[separator.0 + separator.1..]))
}

fn parse_mime_headers(raw: &[u8]) -> Result<BTreeMap<String, String>, MimeParseFailure> {
    if raw.len() > MAX_MIME_HEADER_BYTES {
        return Err(MimeParseFailure::Bounds);
    }
    let text = String::from_utf8_lossy(raw);
    let mut headers = BTreeMap::new();
    let mut current_name = None::<String>;
    let mut current_value = String::new();
    let mut count = 0_usize;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.len() > 8 * 1024 {
            return Err(MimeParseFailure::Bounds);
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            if current_name.is_none() {
                continue;
            }
            if !current_value.is_empty() {
                current_value.push(' ');
            }
            current_value.push_str(line.trim());
            continue;
        }
        if let Some(name) = current_name.take() {
            insert_mime_header(&mut headers, name, std::mem::take(&mut current_value))?;
        }
        count = count.checked_add(1).ok_or(MimeParseFailure::Bounds)?;
        if count > MAX_MIME_HEADER_COUNT {
            return Err(MimeParseFailure::Bounds);
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim_end_matches([' ', '\t']);
        if name.is_empty() || name.len() > 128 || !name.bytes().all(|byte| byte.is_ascii_graphic())
        {
            continue;
        }
        current_name = Some(name.to_ascii_lowercase());
        current_value.push_str(value.trim());
    }
    if let Some(name) = current_name {
        insert_mime_header(&mut headers, name, current_value)?;
    }
    Ok(headers)
}

// The message's unsubscribe routes, read from its top-level headers.
//
// Only `mailto:` and `https:` routes are kept, at most four; anything else a
// list advertises is not something Mail can act on safely. Headers that do not
// parse yield `None` rather than failing the message.
pub(super) fn list_unsubscribe(raw: &[u8]) -> Option<ListUnsubscribe> {
    let (head, _) = split_mime_head(raw).ok()?;
    // Lenient on purpose: a stray 8-bit byte elsewhere in a list mail's
    // header must not hide its unsubscribe route.
    let head = String::from_utf8_lossy(head);
    let mut value = None::<String>;
    let mut post = None::<String>;
    let mut current = None::<&mut Option<String>>;
    for line in head.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with([' ', '\t']) {
            if let Some(Some(folded)) = current.as_deref_mut() {
                folded.push(' ');
                folded.push_str(line.trim());
            }
            continue;
        }
        current = None;
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let slot = if name.eq_ignore_ascii_case("list-unsubscribe") {
            &mut value
        } else if name.eq_ignore_ascii_case("list-unsubscribe-post") {
            &mut post
        } else {
            continue;
        };
        if slot.is_none() {
            *slot = Some(rest.trim().to_owned());
            current = Some(slot);
        }
    }
    let uris: Vec<String> = value?
        .split(',')
        .filter_map(|part| {
            let uri = part.trim().strip_prefix('<')?.strip_suffix('>')?.trim();
            let scheme_ok = uri.starts_with("https://") || uri.starts_with("mailto:");
            (scheme_ok && uri.len() <= 2048 && !uri.chars().any(char::is_whitespace))
                .then(|| uri.to_owned())
        })
        .take(4)
        .collect();
    if uris.is_empty() {
        return None;
    }
    let one_click = post.is_some_and(|post| {
        post.trim()
            .eq_ignore_ascii_case("List-Unsubscribe=One-Click")
    });
    Some(ListUnsubscribe { uris, one_click })
}

fn insert_mime_header(
    headers: &mut BTreeMap<String, String>,
    name: String,
    value: String,
) -> Result<(), MimeParseFailure> {
    let authority_bearing = matches!(
        name.as_str(),
        "content-type" | "content-transfer-encoding" | "content-disposition" | "content-id"
    );
    if authority_bearing && headers.contains_key(&name) {
        return Err(MimeParseFailure::Headers);
    }
    headers.entry(name).or_insert(value);
    Ok(())
}

fn parse_header_value(value: &str) -> Result<(String, Vec<(String, String)>), MimeParseFailure> {
    let mut pieces = split_parameters(value)?.into_iter();
    let main = pieces
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if main.len() > crate::attachment_store::MAX_ATTACHMENT_MIME_BYTES {
        return Err(MimeParseFailure::Bounds);
    }
    let mut parameters = Vec::new();
    for piece in pieces {
        let Some((name, value)) = piece.split_once('=') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() || name.contains(['"', ' ', '\t']) {
            continue;
        }
        if name.len() > 64 {
            return Err(MimeParseFailure::Bounds);
        }
        let value = unquote(value.trim());
        if value.len() > 1024 {
            return Err(MimeParseFailure::Bounds);
        }
        parameters.push((name, value));
    }
    Ok((main, parameters))
}

// Splits at `;` outside quoted strings and drops empty parameters.
fn split_parameters(value: &str) -> Result<Vec<&str>, MimeParseFailure> {
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        match character {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ';' if !quoted => {
                pieces.push(&value[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if quoted {
        return Err(MimeParseFailure::Headers);
    }
    pieces.push(&value[start..]);
    let mut pieces = pieces.into_iter();
    let main = pieces.next().unwrap_or_default();
    Ok(std::iter::once(main)
        .chain(pieces.filter(|piece| !piece.trim().is_empty()))
        .collect())
}

fn unquote(value: &str) -> String {
    let Some(inner) = value.strip_prefix('"') else {
        return value.to_owned();
    };
    let mut out = String::with_capacity(inner.len());
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' => out.extend(characters.next()),
            '"' => break,
            _ => out.push(character),
        }
    }
    out
}

fn parameter_value<'a>(parameters: &'a [(String, String)], name: &str) -> Option<&'a str> {
    parameters
        .iter()
        .find(|(candidate, _)| candidate == name)
        .map(|(_, value)| value.as_str())
}

fn split_multipart<'a>(body: &'a [u8], boundary: &[u8]) -> Result<Vec<&'a [u8]>, MimeParseFailure> {
    let mut marker = Vec::with_capacity(boundary.len().saturating_add(2));
    marker.extend_from_slice(b"--");
    marker.extend_from_slice(boundary);
    let mut offsets = Vec::new();
    let mut cursor = 0_usize;
    let mut closed = false;
    while cursor < body.len() {
        let line_end = body[cursor..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(body.len(), |relative| cursor + relative + 1);
        let mut line = &body[cursor..line_end];
        line = line.strip_suffix(b"\n").unwrap_or(line);
        line = line.strip_suffix(b"\r").unwrap_or(line);
        if line == marker {
            offsets.push((cursor, line_end, false));
        } else if line.starts_with(&marker) && &line[marker.len()..] == b"--" {
            offsets.push((cursor, line_end, true));
            closed = true;
            break;
        }
        cursor = line_end;
    }
    if !closed || offsets.len() < 2 || offsets.last().is_none_or(|entry| !entry.2) {
        return Err(MimeParseFailure::Multipart);
    }
    let mut parts = Vec::new();
    for pair in offsets.windows(2) {
        if pair[0].2 {
            return Err(MimeParseFailure::Multipart);
        }
        let mut end = pair[1].0;
        while end > pair[0].1 && matches!(body[end - 1], b'\r' | b'\n') {
            end -= 1;
        }
        if end <= pair[0].1 {
            return Err(MimeParseFailure::Multipart);
        }
        parts.push(&body[pair[0].1..end]);
    }
    Ok(parts)
}

pub(super) fn decode_transfer(
    bytes: &[u8],
    encoding: &str,
    limit: usize,
) -> Result<Vec<u8>, MimeParseFailure> {
    match encoding {
        "7bit" | "8bit" | "binary" => {
            if bytes.len() > limit {
                return Err(MimeParseFailure::Bounds);
            }
            Ok(bytes.to_vec())
        }
        "base64" => decode_base64(bytes, limit),
        "quoted-printable" => decode_quoted_printable(bytes, limit),
        _ => Err(MimeParseFailure::TransferEncoding),
    }
}

fn decode_base64(input: &[u8], limit: usize) -> Result<Vec<u8>, MimeParseFailure> {
    let encoded = input
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<_>>();
    if encoded.len() % 4 != 0 {
        return Err(MimeParseFailure::TransferEncoding);
    }
    let capacity = encoded
        .len()
        .checked_div(4)
        .and_then(|groups| groups.checked_mul(3))
        .ok_or(MimeParseFailure::Bounds)?;
    if capacity > limit.saturating_add(2) {
        return Err(MimeParseFailure::Bounds);
    }
    let mut out = Vec::with_capacity(capacity.min(limit));
    for (index, chunk) in encoded.as_chunks::<4>().0.iter().enumerate() {
        let final_chunk = index + 1 == encoded.len() / 4;
        let a = base64_value(chunk[0]).ok_or(MimeParseFailure::TransferEncoding)?;
        let b = base64_value(chunk[1]).ok_or(MimeParseFailure::TransferEncoding)?;
        let c = if chunk[2] == b'=' {
            if !final_chunk || chunk[3] != b'=' {
                return Err(MimeParseFailure::TransferEncoding);
            }
            None
        } else {
            Some(base64_value(chunk[2]).ok_or(MimeParseFailure::TransferEncoding)?)
        };
        let d = if chunk[3] == b'=' {
            if !final_chunk {
                return Err(MimeParseFailure::TransferEncoding);
            }
            None
        } else {
            Some(base64_value(chunk[3]).ok_or(MimeParseFailure::TransferEncoding)?)
        };
        push_decoded(&mut out, (a << 2) | (b >> 4), limit)?;
        if let Some(c) = c {
            push_decoded(&mut out, (b << 4) | (c >> 2), limit)?;
            if let Some(d) = d {
                push_decoded(&mut out, (c << 6) | d, limit)?;
            }
        } else if d.is_some() {
            return Err(MimeParseFailure::TransferEncoding);
        }
    }
    Ok(out)
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn decode_quoted_printable(input: &[u8], limit: usize) -> Result<Vec<u8>, MimeParseFailure> {
    let mut out = Vec::with_capacity(input.len().min(limit));
    let mut index = 0_usize;
    while index < input.len() {
        if input[index] != b'=' {
            push_decoded(&mut out, input[index], limit)?;
            index += 1;
            continue;
        }
        if input.get(index + 1..index + 3) == Some(b"\r\n") {
            index += 3;
            continue;
        }
        if input.get(index + 1) == Some(&b'\n') {
            index += 2;
            continue;
        }
        if input[index + 1..]
            .iter()
            .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        {
            break;
        }
        let pair = input
            .get(index + 1..index + 3)
            .ok_or(MimeParseFailure::TransferEncoding)?;
        let high = hex_value(pair[0]).ok_or(MimeParseFailure::TransferEncoding)?;
        let low = hex_value(pair[1]).ok_or(MimeParseFailure::TransferEncoding)?;
        push_decoded(&mut out, (high << 4) | low, limit)?;
        index += 3;
    }
    Ok(out)
}

fn push_decoded(out: &mut Vec<u8>, byte: u8, limit: usize) -> Result<(), MimeParseFailure> {
    if out.len() >= limit {
        return Err(MimeParseFailure::Bounds);
    }
    out.push(byte);
    Ok(())
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn decode_extended_filename(value: &str) -> Option<String> {
    let mut fields = value.splitn(3, '\'');
    let charset = fields.next()?;
    let _language = fields.next()?;
    let bytes = percent_decode(fields.next()?)?;
    Some(decode_text(&bytes, Some(charset)))
}

fn percent_decode(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0_usize;
    while index < bytes.len() {
        let byte = if bytes[index] == b'%' {
            let pair = bytes.get(index + 1..index + 3)?;
            index += 3;
            (hex_value(pair[0])? << 4) | hex_value(pair[1])?
        } else {
            let byte = bytes[index];
            index += 1;
            byte
        };
        push_decoded(
            &mut out,
            byte,
            crate::attachment_store::MAX_ATTACHMENT_NAME_BYTES,
        )
        .ok()?;
    }
    Some(out)
}

pub(super) fn address_parts(address: &imap_proto::Address<'_>) -> (String, String) {
    let name = address
        .name
        .as_deref()
        .map(|value| crate::model::header_text::decode_header_text(&String::from_utf8_lossy(value)))
        .unwrap_or_default();
    let mailbox = address
        .mailbox
        .as_deref()
        .map(|value| String::from_utf8_lossy(value).into_owned())
        .unwrap_or_default();
    let host = address
        .host
        .as_deref()
        .map(|value| String::from_utf8_lossy(value).into_owned())
        .unwrap_or_default();
    let email = if mailbox.is_empty() || host.is_empty() {
        String::new()
    } else {
        format!("{mailbox}@{host}")
    };
    (name, email)
}

pub(super) fn parse_internal_date(value: &str) -> Option<MessageDate> {
    let value = value.trim_matches('"');
    let mut parts = value.split_whitespace();
    let date = parts.next()?;
    let time = parts.next()?;
    let mut date_parts = date.split('-');
    let day = date_parts.next()?.parse().ok()?;
    let month = match date_parts.next()?.to_ascii_lowercase().as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    };
    let year: u32 = date_parts.next()?.parse().ok()?;
    let mut time_parts = time.split(':');
    let hour: u32 = time_parts.next()?.parse().ok()?;
    let minute: u32 = time_parts.next()?.parse().ok()?;
    if !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    let seconds: u32 = time_parts.next()?.parse().ok()?;
    if seconds > 59 {
        return None;
    }
    let offset = parts.next()?;
    if offset.len() != 5
        || !matches!(offset.as_bytes()[0], b'+' | b'-')
        || !offset.as_bytes().get(1..)?.iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    let offset_hour: i64 = offset.get(1..3)?.parse().ok()?;
    let offset_minute: i64 = offset.get(3..5)?.parse().ok()?;
    if offset_hour > 23 || offset_minute > 59 {
        return None;
    }
    let offset_seconds =
        (offset_hour * 3600 + offset_minute * 60) * if offset.starts_with('-') { -1 } else { 1 };
    let date = enzui_iced_core::civil_date::Date::new(i32::try_from(year).ok()?, month, day)?;
    let unix_seconds = i64::from(date.to_days()) * 86400
        + i64::from(hour) * 3600
        + i64::from(minute) * 60
        + i64::from(seconds)
        - offset_seconds;
    let delta = std::time::Duration::from_secs(unix_seconds.unsigned_abs());
    let instant = if unix_seconds < 0 {
        std::time::UNIX_EPOCH.checked_sub(delta)?
    } else {
        std::time::UNIX_EPOCH.checked_add(delta)?
    };
    let local = enzui_iced_core::local_time::local_civil_time_at(instant).ok()?;
    Some(MessageDate {
        year: u32::try_from(local.date.year()).ok()?,
        month: local.date.month(),
        day: local.date.day(),
        minute: u32::from(local.minute),
    })
}

#[cfg(test)]
mod list_unsubscribe_tests {
    use super::*;

    #[test]
    fn only_https_and_mailto_routes_are_kept() {
        let raw = b"List-Unsubscribe: <http://list.example/u>, <javascript:alert(1)>\r\n\r\n";
        assert_eq!(list_unsubscribe(raw), None);
        let raw = b"List-Unsubscribe: <https://list.example/u>\r\n\r\n";
        assert_eq!(
            list_unsubscribe(raw).map(|route| route.one_click),
            Some(false)
        );
    }
}
