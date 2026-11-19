//! Leaving a mailing list: the RFC 8058 one-click POST and `mailto:` routes.

use std::time::Duration;

// The body RFC 8058 requires of a one-click unsubscribe request.
const ONE_CLICK_BODY: &[u8] = b"List-Unsubscribe=One-Click";
const ONE_CLICK_TIMEOUT: Duration = Duration::from_secs(20);

pub(crate) fn one_click(url: &str) -> Result<(), String> {
    let mut easy = super::public_https::request(url, ONE_CLICK_TIMEOUT)
        .ok_or_else(|| "The list did not offer a reachable public HTTPS link.".to_owned())?;
    let failed = |_| "The list could not be reached.".to_owned();
    easy.post(true).map_err(failed)?;
    easy.post_fields_copy(ONE_CLICK_BODY).map_err(failed)?;
    let mut headers = curl::easy::List::new();
    headers
        .append("Content-Type: application/x-www-form-urlencoded")
        .map_err(failed)?;
    easy.http_headers(headers).map_err(failed)?;
    easy.write_function(|data| Ok(data.len())).map_err(failed)?;
    easy.perform().map_err(failed)?;
    match easy.response_code().map_err(failed)? {
        200..=299 => Ok(()),
        code => Err(format!("The list answered with status {code}.")),
    }
}

// A `mailto:` route split into its address, subject and body (RFC 6068).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct MailtoRoute {
    pub(crate) to: String,
    pub(crate) subject: String,
    pub(crate) body: String,
}

pub(crate) fn parse_mailto(uri: &str) -> Option<MailtoRoute> {
    let rest = uri.strip_prefix("mailto:")?;
    let (to, query) = rest.split_once('?').unwrap_or((rest, ""));
    let to = percent_decode(to)?;
    if !to.contains('@') {
        return None;
    }
    let mut route = MailtoRoute {
        to,
        ..MailtoRoute::default()
    };
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = percent_decode(value)?;
        if name.eq_ignore_ascii_case("subject") {
            route.subject = value;
        } else if name.eq_ignore_ascii_case("body") {
            route.body = value;
        }
    }
    Some(route)
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_click_requires_a_public_https_destination_without_url_credentials() {
        for destination in [
            "http://list.example/u",
            "https://127.0.0.1/u",
            "https://[::1]/u",
            "https://user@list.example/u",
        ] {
            assert!(one_click(destination).is_err());
        }
    }
}
