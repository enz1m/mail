use std::net::{IpAddr, ToSocketAddrs};
use std::time::Duration;

use curl::easy::{Easy, List};
use enzui_iced_core::url_policy::{AddressVerdict, UrlPolicy};
use url::{Host, Url};

pub(super) fn request(raw_url: &str, timeout: Duration) -> Option<Easy> {
    let policy = UrlPolicy::DEFAULT;
    policy.check_url(raw_url).ok()?;
    let url = Url::parse(raw_url).ok()?;
    let port = url.port_or_known_default()?;
    let is_public = |ip| matches!(policy.check_address(ip), AddressVerdict::Allow { .. });
    let mut easy = Easy::new();
    match url.host()? {
        Host::Domain(host) => {
            let mut addresses = (host, port).to_socket_addrs().ok()?;
            let first = addresses.next()?.ip();
            if !is_public(first) || !addresses.all(|address| is_public(address.ip())) {
                return None;
            }
            let ip = match first {
                IpAddr::V4(ip) => ip.to_string(),
                IpAddr::V6(ip) => format!("[{ip}]"),
            };
            // Keep libcurl on the address checked above even if DNS changes.
            let mut pin = List::new();
            pin.append(&format!("{host}:{port}:{ip}")).ok()?;
            easy.resolve(pin).ok()?;
        }
        Host::Ipv4(ip) if is_public(ip.into()) => {}
        Host::Ipv6(ip) if is_public(ip.into()) => {}
        _ => return None,
    }
    easy.url(url.as_str()).ok()?;
    easy.proxy("").ok()?;
    easy.follow_location(false).ok()?;
    easy.connect_timeout(Duration::from_secs(4)).ok()?;
    easy.timeout(timeout).ok()?;
    Some(easy)
}
