//! Consent-scoped, bounded image retrieval without cookies or account credentials.

use blitz_traits::net::{Bytes, NetHandler, NetProvider, Request};
use enzui_iced_core::url_policy::{AddressVerdict, UrlPolicy};
use std::collections::HashSet;
use std::io::Cursor;
use std::net::ToSocketAddrs;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};
use std::time::Duration;

const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const DOCUMENT_BYTES: usize = 24 * 1024 * 1024;
struct Job {
    address: String,
    handler: Box<dyn NetHandler>,
    cancelled: Arc<AtomicBool>,
    remaining: Arc<AtomicUsize>,
}

impl Job {
    fn run(self) {
        if let Some((bytes, decoded_bytes)) = fetch_image(&self.address, &self.cancelled)
            && !self.cancelled.load(Ordering::Acquire)
            && self
                .remaining
                .try_update(Ordering::AcqRel, Ordering::Acquire, |available| {
                    available.checked_sub(decoded_bytes)
                })
                .is_ok()
        {
            self.handler.bytes(self.address, Bytes::from(bytes));
        } else {
            self.refuse();
        }
    }
    fn refuse(self) {
        self.handler.bytes(self.address, Bytes::new());
    }
}

pub(super) struct Images {
    pub allowed: HashSet<String>,
    pub cancelled: Arc<AtomicBool>,
    remaining: Arc<AtomicUsize>,
}

impl Images {
    pub(super) fn new(allowed: HashSet<String>, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            allowed,
            cancelled,
            remaining: Arc::new(AtomicUsize::new(DOCUMENT_BYTES)),
        }
    }
}

impl NetProvider for Images {
    fn fetch(&self, _: usize, request: Request, handler: Box<dyn NetHandler>) {
        let address = request.url.as_str().to_owned();
        if address == super::PLACEHOLDER {
            handler.bytes(address, Bytes::new());
            return;
        }
        if !self.allowed.contains(&address) || self.cancelled.load(Ordering::Acquire) {
            handler.bytes(address, Bytes::new());
            return;
        }
        let job = Job {
            address,
            handler,
            cancelled: Arc::clone(&self.cancelled),
            remaining: Arc::clone(&self.remaining),
        };
        if let Err(error) = workers().try_send(job) {
            match error {
                mpsc::TrySendError::Full(job) | mpsc::TrySendError::Disconnected(job) => {
                    job.refuse();
                }
            }
        }
    }
}

fn workers() -> &'static mpsc::SyncSender<Job> {
    static POOL: OnceLock<mpsc::SyncSender<Job>> = OnceLock::new();
    POOL.get_or_init(|| {
        let (send, receive) = mpsc::sync_channel::<Job>(32);
        let receive = Arc::new(std::sync::Mutex::new(receive));
        for _ in 0..4 {
            let incoming = Arc::clone(&receive);
            let _ = std::thread::Builder::new()
                .name("mail-image".into())
                .spawn(move || {
                    loop {
                        let job = incoming
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .recv();
                        match job {
                            Ok(job) => job.run(),
                            Err(_) => break,
                        }
                    }
                });
        }
        send
    })
}

fn fetch_image(address: &str, cancelled: &AtomicBool) -> Option<(Vec<u8>, usize)> {
    let policy = UrlPolicy::DEFAULT;
    let mut address = url::Url::parse(address).ok()?;
    for hop in 0..=3 {
        if cancelled.load(Ordering::Acquire) {
            return None;
        }
        policy.check_url(address.as_str()).ok()?;
        let host = address.host_str()?;
        let port = address.port_or_known_default()?;
        let ip = (host, port)
            .to_socket_addrs()
            .ok()?
            .find(|a| matches!(policy.check_address(a.ip()), AddressVerdict::Allow { .. }))?
            .ip();
        let mut pinned = curl::easy::List::new();
        let literal = if ip.is_ipv6() {
            format!("[{ip}]")
        } else {
            ip.to_string()
        };
        pinned.append(&format!("{host}:{port}:{literal}")).ok()?;
        let mut curl = curl::easy::Easy::new();
        curl.url(address.as_str()).ok()?;
        curl.proxy("").ok()?;
        curl.resolve(pinned).ok()?;
        curl.follow_location(false).ok()?;
        curl.connect_timeout(Duration::from_secs(3)).ok()?;
        curl.timeout(Duration::from_secs(10)).ok()?;
        curl.progress(true).ok()?;
        let mut bytes = Vec::new();
        let mut location = None;
        {
            let mut transfer = curl.transfer();
            transfer
                .progress_function(|_, _, _, _| !cancelled.load(Ordering::Acquire))
                .ok()?;
            transfer
                .write_function(|part| {
                    if bytes.len().saturating_add(part.len()) > MAX_IMAGE_BYTES {
                        return Ok(0);
                    }
                    bytes.extend_from_slice(part);
                    Ok(part.len())
                })
                .ok()?;
            transfer
                .header_function(|header| {
                    if let Ok(header) = std::str::from_utf8(header)
                        && let Some((name, value)) = header.split_once(':')
                        && name.eq_ignore_ascii_case("location")
                    {
                        location = Some(value.trim().to_owned());
                    }
                    true
                })
                .ok()?;
            transfer.perform().ok()?;
        }
        let status = curl.response_code().ok()?;
        if (300..400).contains(&status) {
            if hop == 3 {
                return None;
            }
            let next = address.join(location.as_deref()?).ok()?;
            policy
                .check_redirect(address.as_str(), next.as_str(), hop + 1)
                .ok()?;
            address = next;
            continue;
        }
        if !(200..300).contains(&status) {
            return None;
        }
        let reader = image::ImageReader::new(Cursor::new(&bytes))
            .with_guessed_format()
            .ok()?;
        let (width, height) = reader.into_dimensions().ok()?;
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 4_194_304 {
            return None;
        }
        return Some((bytes, width as usize * height as usize * 4));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Reply(mpsc::Sender<Bytes>);
    impl NetHandler for Reply {
        fn bytes(self: Box<Self>, _: String, bytes: Bytes) {
            let _ = self.0.send(bytes);
        }
    }
    #[test]
    fn image_requests_require_current_consent_and_public_https_destinations() {
        for address in [
            "file:///etc/passwd",
            "http://example.invalid/image.png",
            "https://127.0.0.1/image.png",
            "https://[::1]/image.png",
            "https://169.254.169.254/image.png",
        ] {
            assert!(
                fetch_image(address, &AtomicBool::new(false)).is_none(),
                "refused before connection: {address}"
            );
        }
        let url = "https://example.invalid/private-image.png";
        for (allowed, cancelled) in [(false, false), (true, true)] {
            let images = Images::new(
                if allowed {
                    HashSet::from([url.into()])
                } else {
                    HashSet::new()
                },
                Arc::new(AtomicBool::new(cancelled)),
            );
            let (send, receive) = mpsc::channel();
            images.fetch(1, Request::get(url.parse().unwrap()), Box::new(Reply(send)));
            assert!(
                receive
                    .recv_timeout(Duration::from_secs(1))
                    .unwrap()
                    .is_empty()
            );
        }
    }
}
