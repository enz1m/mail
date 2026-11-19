//! Malformed RFC 2047 words remain visible.
//! Whitespace between adjacent encoded words is omitted as RFC 2047 §6.2 requires.
pub(crate) fn decode_header_text(value: &str) -> String {
    if !value.contains("=?") {
        return value.to_owned();
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    let mut after_word = false;
    while let Some(start) = rest.find("=?") {
        let (before, tail) = rest.split_at(start);
        match decode_word(tail) {
            Some((decoded, used)) => {
                if !(after_word && before.chars().all(char::is_whitespace)) {
                    out.push_str(before);
                }
                out.push_str(&decoded);
                rest = tail
                    .get(used..)
                    .expect("decoded word ends after an ASCII delimiter");
                after_word = true;
            }
            None => {
                out.push_str(before);
                out.push_str("=?");
                rest = tail
                    .strip_prefix("=?")
                    .expect("encoded word prefix was matched");
                after_word = false;
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_word(text: &str) -> Option<(String, usize)> {
    let inner = text.strip_prefix("=?")?;
    let charset_end = inner.find('?')?;
    let charset = inner.get(..charset_end)?;
    let after_charset = inner.get(charset_end + 1..)?;
    let mut encoding_chars = after_charset.chars();
    let encoding = encoding_chars.next()?;
    let after_encoding = after_charset.get(1..)?.strip_prefix('?')?;
    let end = after_encoding.find("?=")?;
    let payload = after_encoding.get(..end)?;
    if payload.contains(char::is_whitespace) {
        return None;
    }
    let bytes = match encoding.to_ascii_uppercase() {
        'B' => decode_base64(payload)?,
        'Q' => decode_q(payload)?,
        _ => return None,
    };
    // `charset*language` (RFC 2231) names the charset before the star.
    let charset = charset.split('*').next().unwrap_or(charset);
    let decoded = decode_charset(charset, &bytes)?;
    let used = 2 + charset_end + 1 + 2 + end + 2;
    Some((decoded, used))
}

fn decode_charset(charset: &str, bytes: &[u8]) -> Option<String> {
    match charset.to_ascii_lowercase().as_str() {
        "utf-8" | "utf8" | "us-ascii" | "ascii" => {
            Some(String::from_utf8_lossy(bytes).into_owned())
        }
        "iso-8859-1" | "latin1" | "iso-8859-15" => {
            Some(bytes.iter().map(|&b| char::from(b)).collect())
        }
        "windows-1252" | "cp1252" => Some(bytes.iter().map(|&b| cp1252(b)).collect()),
        _ => None,
    }
}

// Windows-1252 differs from Latin-1 only in 0x80..0xA0.
fn cp1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}',
        'ž', 'Ÿ',
    ];
    match byte {
        0x80..=0x9F => HIGH[usize::from(byte - 0x80)],
        _ => char::from(byte),
    }
}

fn decode_q(payload: &str) -> Option<Vec<u8>> {
    let bytes = payload.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'_' => {
                out.push(b' ');
                index += 1;
            }
            b'=' => {
                let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                index += 3;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    Some(out)
}

fn decode_base64(payload: &str) -> Option<Vec<u8>> {
    let value = |byte: u8| -> Option<u32> {
        Some(u32::from(match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        }))
    };
    let digits: Vec<u8> = payload.bytes().filter(|byte| *byte != b'=').collect();
    let mut out = Vec::with_capacity(digits.len() * 3 / 4);
    for chunk in digits.chunks(4) {
        let mut acc = 0u32;
        for (index, byte) in chunk.iter().enumerate() {
            acc |= value(*byte)? << (18 - 6 * index);
        }
        let produced = match chunk.len() {
            4 => 3,
            3 => 2,
            2 => 1,
            _ => return None,
        };
        for index in 0..produced {
            out.push((acc >> (16 - 8 * index)) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {}
