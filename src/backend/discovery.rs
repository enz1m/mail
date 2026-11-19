//! Discovery tries domain autoconfig, DNS SRV, ISPDB, then MX provider inference.
//! ISPDB receives the domain; domain autoconfig also receives the address.

use super::config::{AuthMethod, Encryption, ParsedEmail};
use enzui_services::dns::{MxRecord, SrvRecord};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Discovered {
    pub(crate) source: DiscoverySource,
    pub(crate) imap_server: String,
    pub(crate) imap_port: u16,
    pub(crate) imap_encryption: Encryption,
    pub(crate) imap_username: String,
    pub(crate) smtp_server: String,
    pub(crate) smtp_port: u16,
    pub(crate) smtp_encryption: Encryption,
    pub(crate) smtp_username: String,
    pub(crate) auth: AuthMethod,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DiscoverySource {
    DomainAutoconfig,
    ServiceRecords,
    ProviderDirectory,
    MailHost(String),
}

impl DiscoverySource {
    pub(crate) fn describe(&self, domain: &str) -> String {
        let (key, host) = match self {
            Self::DomainAutoconfig => ("mail-discovery-domain", None),
            Self::ServiceRecords => ("mail-discovery-dns", None),
            Self::ProviderDirectory => ("mail-discovery-directory", None),
            Self::MailHost(host) => ("mail-discovery-host", Some(host)),
        };
        crate::l10n::message(
            key,
            &[
                ("domain", domain.to_owned()),
                ("host", host.cloned().unwrap_or_default()),
            ],
        )
    }
}

pub(crate) trait DiscoveryNet {
    // SRV records for a name; empty when there are none or the lookup fails.
    fn srv(&self, name: &str) -> Vec<SrvRecord>;
    // MX records for a domain; empty when there are none or the lookup fails.
    fn mx(&self, domain: &str) -> Vec<MxRecord>;
    // The body of an HTTPS GET, or `None` on any failure.
    fn fetch(&self, url: &str) -> Option<String>;
}

pub(crate) fn discover(email: &ParsedEmail, net: &dyn DiscoveryNet) -> Option<Discovered> {
    let domain = email.domain.to_ascii_lowercase();
    let address = email.as_string();
    let mut provider =
        url::Url::parse(&format!("https://autoconfig.{domain}/mail/config-v1.1.xml")).ok()?;
    provider
        .query_pairs_mut()
        .append_pair("emailaddress", &address);
    let autoconfig = [
        provider.into(),
        format!("https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml"),
    ];
    for url in &autoconfig {
        if let Some(found) = net
            .fetch(url)
            .and_then(|xml| from_autoconfig(&xml, email, DiscoverySource::DomainAutoconfig))
        {
            return Some(found);
        }
    }
    if let Some(found) = from_service_records(&domain, email, net) {
        return Some(found);
    }
    if let Some(found) = provider_directory(&domain, email, net, DiscoverySource::ProviderDirectory)
    {
        return Some(found);
    }
    from_mail_exchangers(&domain, email, net)
}

fn provider_directory(
    domain: &str,
    email: &ParsedEmail,
    net: &dyn DiscoveryNet,
    source: DiscoverySource,
) -> Option<Discovered> {
    let xml = net.fetch(&format!("https://autoconfig.thunderbird.net/v1.1/{domain}"))?;
    from_autoconfig(&xml, email, source)
}

// RFC 6186 / RFC 8314: implicit TLS first, then STARTTLS.
fn from_service_records(
    domain: &str,
    email: &ParsedEmail,
    net: &dyn DiscoveryNet,
) -> Option<Discovered> {
    let first = |service: &str| -> Option<SrvRecord> {
        net.srv(&format!("{service}.{domain}"))
            .into_iter()
            // A target of "." says the service is deliberately not offered.
            .find(|record| !record.target.is_empty() && record.target != "." && record.port != 0)
    };
    let (imap, imap_encryption) = match first("_imaps._tcp") {
        Some(record) => (record, Encryption::Tls),
        None => (first("_imap._tcp")?, Encryption::StartTls),
    };
    let (smtp, smtp_encryption) = match first("_submissions._tcp") {
        Some(record) => (record, Encryption::Tls),
        None => (first("_submission._tcp")?, Encryption::StartTls),
    };
    let address = email.as_string();
    Some(Discovered {
        source: DiscoverySource::ServiceRecords,
        imap_server: imap.target,
        imap_port: imap.port,
        imap_encryption,
        imap_username: address.clone(),
        smtp_server: smtp.target,
        smtp_port: smtp.port,
        smtp_encryption,
        smtp_username: address,
        auth: AuthMethod::Plain,
    })
}

fn from_mail_exchangers(
    domain: &str,
    email: &ParsedEmail,
    net: &dyn DiscoveryNet,
) -> Option<Discovered> {
    let exchanger = net
        .mx(domain)
        .into_iter()
        .next()?
        .exchange
        .to_ascii_lowercase();
    let address = email.as_string();
    let hosted = |name: &str, imap: &str, smtp: &str, smtp_encryption, auth| Discovered {
        source: DiscoverySource::MailHost(name.to_owned()),
        imap_server: imap.to_owned(),
        imap_port: 993,
        imap_encryption: Encryption::Tls,
        imap_username: address.clone(),
        smtp_server: smtp.to_owned(),
        smtp_port: if smtp_encryption == Encryption::Tls {
            465
        } else {
            587
        },
        smtp_encryption,
        smtp_username: address.clone(),
        auth,
    };
    if exchanger.ends_with(".google.com") || exchanger.ends_with(".googlemail.com") {
        return Some(hosted(
            "Google",
            "imap.gmail.com",
            "smtp.gmail.com",
            Encryption::Tls,
            AuthMethod::OAuth2,
        ));
    }
    if exchanger.ends_with(".protection.outlook.com") {
        return Some(hosted(
            "Microsoft",
            "outlook.office365.com",
            "smtp.office365.com",
            Encryption::StartTls,
            AuthMethod::OAuth2,
        ));
    }
    let base = base_domain(&exchanger)?;
    if base == domain {
        return None;
    }
    provider_directory(&base, email, net, DiscoverySource::MailHost(base.clone()))
}

fn base_domain(host: &str) -> Option<String> {
    let labels: Vec<&str> = host.trim_end_matches('.').rsplitn(3, '.').collect();
    (labels.len() >= 2).then(|| format!("{}.{}", labels[1], labels[0]))
}

pub(crate) fn from_autoconfig(
    xml: &str,
    email: &ParsedEmail,
    source: DiscoverySource,
) -> Option<Discovered> {
    let incoming = server_block(xml, "incomingServer", "imap")?;
    let outgoing = server_block(xml, "outgoingServer", "smtp")?;
    let expand = |value: &str| {
        value
            .replace("%EMAILADDRESS%", &email.as_string())
            .replace("%EMAILLOCALPART%", &email.local)
            .replace("%EMAILDOMAIN%", &email.domain)
    };
    let encryption = |block: &str| match tag(block, "socketType").as_deref() {
        Some("SSL") => Encryption::Tls,
        Some("STARTTLS") => Encryption::StartTls,
        Some("plain") => Encryption::None,
        _ => Encryption::Tls,
    };
    // A password works for most accounts; a browser sign-in is chosen only
    // when the provider offers nothing else.
    let methods = all_tags(incoming, "authentication");
    let auth = if !methods.is_empty() && methods.iter().all(|m| m == "OAuth2") {
        AuthMethod::OAuth2
    } else {
        AuthMethod::Plain
    };
    let host = |block: &str| {
        let name = expand(&tag(block, "hostname")?);
        valid_host(&name).then_some(name)
    };
    Some(Discovered {
        source,
        imap_server: host(incoming)?,
        imap_port: tag(incoming, "port")?.parse().ok()?,
        imap_encryption: encryption(incoming),
        imap_username: tag(incoming, "username").map_or_else(|| email.as_string(), |u| expand(&u)),
        smtp_server: host(outgoing)?,
        smtp_port: tag(outgoing, "port")?.parse().ok()?,
        smtp_encryption: encryption(outgoing),
        smtp_username: tag(outgoing, "username").map_or_else(|| email.as_string(), |u| expand(&u)),
        auth,
    })
}

// A host name a server field may hold: letters, digits, hyphens and dots.
fn valid_host(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
}

fn server_block<'a>(xml: &'a str, element: &str, kind: &str) -> Option<&'a str> {
    let open = format!("<{element}");
    let close = format!("</{element}>");
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = rest.get(start + open.len()..)?;
        let head_end = after.find('>')?;
        let head = after.get(..head_end)?;
        let body_and_rest = after.get(head_end + 1..)?;
        let end = body_and_rest.find(&close)?;
        if head.contains(&format!("type=\"{kind}\"")) || head.contains(&format!("type='{kind}'")) {
            return body_and_rest.get(..end);
        }
        rest = body_and_rest.get(end + close.len()..)?;
    }
    None
}

fn tag(block: &str, name: &str) -> Option<String> {
    all_tags(block, name).into_iter().next()
}

fn all_tags(block: &str, name: &str) -> Vec<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let mut out = Vec::new();
    let mut rest = block;
    while let Some(start) = rest.find(&open) {
        let after = rest
            .get(start + open.len()..)
            .expect("matched ASCII tag ends at a UTF-8 boundary");
        let Some(end) = after.find(&close) else { break };
        out.push(
            after
                .get(..end)
                .expect("matched tag starts at a UTF-8 boundary")
                .trim()
                .to_owned(),
        );
        rest = after
            .get(end + close.len()..)
            .expect("matched closing tag ends at a UTF-8 boundary");
    }
    out
}

pub(crate) struct SystemNet {
    resolver: Option<enzui_services::dns::Resolver>,
}

impl SystemNet {
    pub(crate) fn new() -> Self {
        Self {
            resolver: enzui_services::dns::Resolver::system().ok(),
        }
    }
}

impl Default for SystemNet {
    fn default() -> Self {
        Self::new()
    }
}

const MAX_AUTOCONFIG_BYTES: usize = 64 * 1024;

impl DiscoveryNet for SystemNet {
    fn srv(&self, name: &str) -> Vec<SrvRecord> {
        self.resolver
            .as_ref()
            .and_then(|resolver| enzui_services::dns::lookup_srv(resolver, name).ok())
            .unwrap_or_default()
    }

    fn mx(&self, domain: &str) -> Vec<MxRecord> {
        self.resolver
            .as_ref()
            .and_then(|resolver| enzui_services::dns::lookup_mx(resolver, domain).ok())
            .unwrap_or_default()
    }

    fn fetch(&self, url: &str) -> Option<String> {
        fetch_public_https(url)
    }
}

// An HTTPS GET to a public address only: every resolved address must be
// public, and libcurl is pinned to the first so a second resolution cannot
// redirect it. No redirects, short timeouts, a bounded body.
fn fetch_public_https(url: &str) -> Option<String> {
    use std::time::Duration;

    let mut easy = super::public_https::request(url, Duration::from_secs(8))?;
    let mut body = Vec::new();
    {
        let mut transfer = easy.transfer();
        transfer
            .write_function(|data| {
                if body.len() + data.len() > MAX_AUTOCONFIG_BYTES {
                    return Ok(0);
                }
                body.extend_from_slice(data);
                Ok(data.len())
            })
            .ok()?;
        transfer.perform().ok()?;
    }
    (easy.response_code().ok()? == 200).then(|| String::from_utf8_lossy(&body).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Requests(RefCell<Vec<String>>);

    impl DiscoveryNet for Requests {
        fn srv(&self, _: &str) -> Vec<SrvRecord> {
            Vec::new()
        }

        fn mx(&self, _: &str) -> Vec<MxRecord> {
            Vec::new()
        }

        fn fetch(&self, url: &str) -> Option<String> {
            self.0.borrow_mut().push(url.to_owned());
            None
        }
    }

    #[test]
    fn autoconfig_preserves_plus_addressing() {
        let requests = Requests::default();
        let email = super::super::config::parse_email("team+mail@example.invalid").unwrap();
        let _ = discover(&email, &requests);
        let requests = requests.0.into_inner();
        let first = url::Url::parse(&requests[0]).unwrap();
        assert_eq!(
            first.query_pairs().next().unwrap(),
            ("emailaddress".into(), "team+mail@example.invalid".into()),
        );
        assert_eq!(
            requests[2],
            "https://autoconfig.thunderbird.net/v1.1/example.invalid"
        );
    }
}
