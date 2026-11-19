//! The OAuth sign-in end to end against a local provider and a local
//! IMAP server that accepts only XOAUTH2.

use super::test_support::{Provider, browser_returns, get};
use super::*;
use crate::backend::{AuthMethod, Encryption, IncomingConfig, MailProtocol, SmtpConfig};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;

fn base64_decode(text: &str) -> Vec<u8> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => panic!("not base64: {c}"),
    };
    let digits: Vec<u8> = text.bytes().filter(|&c| c != b'=').map(value).collect();
    let mut out = Vec::new();
    for chunk in digits.chunks(4) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &v)| n | (u32::from(v) << (18 - 6 * i)));
        for i in 0..chunk.len().saturating_sub(1) {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    out
}

fn xoauth2_imap() -> (u16, mpsc::Receiver<Vec<String>>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind IMAP");
    let port = listener.local_addr().expect("address").port();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let mut writer = stream.try_clone().expect("clone");
        let mut reader = BufReader::new(stream);
        writer
            .write_all(b"* OK [CAPABILITY IMAP4rev1 AUTH=XOAUTH2] ready\r\n")
            .ok();
        let mut auth = Vec::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let command = line.trim_end().to_owned();
            let tag = command.split_whitespace().next().unwrap_or("A0").to_owned();
            let upper = command.to_ascii_uppercase();
            if upper.contains(" CAPABILITY") {
                let _ = write!(
                    writer,
                    "* CAPABILITY IMAP4rev1 AUTH=XOAUTH2\r\n{tag} OK\r\n"
                );
            } else if upper.contains(" AUTHENTICATE XOAUTH2") {
                let encoded = match command.split_whitespace().nth(3) {
                    Some(initial) => initial.to_owned(),
                    None => {
                        writer.write_all(b"+ \r\n").ok();
                        let mut response = String::new();
                        reader.read_line(&mut response).ok();
                        response.trim_end().to_owned()
                    }
                };
                auth.push(String::from_utf8_lossy(&base64_decode(&encoded)).into_owned());
                let _ = write!(writer, "{tag} OK authenticated\r\n");
            } else if upper.contains(" SELECT ") {
                let _ = write!(
                    writer,
                    "* 0 EXISTS\r\n* OK [UIDVALIDITY 1] valid\r\n{tag} OK selected\r\n"
                );
            } else if upper.contains(" NOOP") {
                let _ = write!(writer, "{tag} OK noop\r\n");
                break;
            } else if upper.contains(" LOGIN ") || upper.contains("AUTHENTICATE PLAIN") {
                auth.push("PASSWORD LOGIN".into());
                let _ = write!(writer, "{tag} NO password login is not offered\r\n");
            } else {
                let _ = write!(writer, "{tag} BAD unexpected\r\n");
            }
        }
        sender.send(auth).ok();
    });
    (port, receiver)
}

fn oauth_config(port: u16) -> AccountConfig {
    let incoming = IncomingConfig {
        server: "127.0.0.1".to_owned(),
        port,
        username: "person@example.com".to_owned(),
        encryption: Encryption::None,
    };
    AccountConfig {
        id: "acct-oauth".to_owned(),
        credential_owner: "acct-oauth".to_owned(),
        display_name: "OAuth".to_owned(),
        email: "person@example.com".to_owned(),
        protocol: MailProtocol::Imap,
        imap: (&incoming).into(),
        incoming,
        smtp: SmtpConfig {
            server: "127.0.0.1".to_owned(),
            port,
            username: "person@example.com".to_owned(),
            encryption: Encryption::None,
        },
        auth: AuthMethod::OAuth2,
    }
}

fn imap_noop(config: &AccountConfig, secret: &Secret) -> MailResult<Vec<u8>> {
    let url = super::super::transport::imap_endpoint_url(
        &config.incoming.server,
        config.incoming.port,
        config.incoming.encryption,
        "INBOX",
    )?;
    super::super::transport::perform_imap(
        config,
        secret,
        &AtomicBool::new(false),
        &url,
        Some("NOOP"),
        super::super::MAX_CONTROL_RESPONSE_BYTES,
    )
}

#[test]
fn a_browser_sign_in_signs_imap_in_with_xoauth2_and_refreshes() {
    forget_tokens();
    let provider = Provider::start("CODE-1");
    let start = begin_sign_in(provider.client(), "person@example.com").expect("begin");
    let url = start.authorization_url.clone();
    let query = super::test_support::form(url.split_once('?').expect("query").1);
    assert_eq!(get(&query, "login_hint"), Some("person@example.com"));
    let browser = std::thread::spawn(move || browser_returns(&url, "CODE-1"));
    let grant = start
        .sign_in
        .finish(Duration::from_secs(20))
        .expect("sign-in completes");
    browser.join().expect("browser");
    assert!(is_grant(&grant));
    let text = String::from_utf8(grant.expose_secret().to_vec()).expect("utf8");
    assert!(text.contains("refresh_token=RT1"));
    assert!(!text.contains("AT1"));

    let (port, transcript) = xoauth2_imap();
    imap_noop(&oauth_config(port), &grant).expect("XOAUTH2 IMAP");
    let auth = transcript
        .recv_timeout(Duration::from_secs(5))
        .expect("transcript");
    assert_eq!(
        auth,
        ["user=person@example.com\u{1}auth=Bearer AT1\u{1}\u{1}"]
    );

    forget_tokens();
    let (port, transcript) = xoauth2_imap();
    imap_noop(&oauth_config(port), &grant).expect("refreshed XOAUTH2 IMAP");
    let auth = transcript
        .recv_timeout(Duration::from_secs(5))
        .expect("transcript");
    assert_eq!(
        auth,
        ["user=person@example.com\u{1}auth=Bearer AT2\u{1}\u{1}"]
    );

    let requests = provider.requests.lock().expect("lock").clone();
    assert_eq!(requests.len(), 2, "one exchange, one refresh: {requests:?}");
    assert_eq!(get(&requests[0], "grant_type"), Some("authorization_code"));
    assert_eq!(get(&requests[0], "client_secret"), Some("desktop-secret"));
    assert_eq!(get(&requests[1], "grant_type"), Some("refresh_token"));
}

#[test]
fn a_sign_in_that_never_returns_is_cancellable() {
    let provider = Provider::start("CODE-3");
    let start = begin_sign_in(provider.client(), "").expect("begin");
    let cancel = start.cancel.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        cancel.cancel();
    });
    let began = Instant::now();
    assert_eq!(
        start.sign_in.finish(Duration::from_secs(60)).unwrap_err(),
        SignInError::Cancelled
    );
    canceller.join().expect("cancel");
    assert!(began.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_grant_round_trips_and_a_tampered_one_is_refused() {
    let grant = Grant {
        provider: OAuthProvider::Microsoft,
        client_id: "client".into(),
        client_secret: Some(SecretText::new("s=e=c".into())),
        token_endpoint: "https://login.microsoftonline.com/common/oauth2/v2.0/token".into(),
        refresh_token: SecretText::new("0.AAA-refresh".into()),
    };
    let decoded = Grant::decode(&grant.encode()).expect("round trip");
    assert_eq!(decoded.provider, OAuthProvider::Microsoft);
    assert_eq!(
        decoded
            .client_secret
            .as_ref()
            .map(SecretText::expose_secret),
        Some("s=e=c")
    );
    assert_eq!(decoded.refresh_token.expose_secret(), "0.AAA-refresh");
    for bad in [
        "enzui-mail-oauth/2\nprovider=google\nclient_id=c\ntoken_endpoint=https://x\nrefresh_token=r\n",
        "enzui-mail-oauth/1\nprovider=yahoo\nclient_id=c\ntoken_endpoint=https://x\nrefresh_token=r\n",
        "enzui-mail-oauth/1\nprovider=google\nclient_id=c\ntoken_endpoint=https://x\n",
        "enzui-mail-oauth/1\nprovider=google\nclient_id=c\nclient_id=d\ntoken_endpoint=https://x\nrefresh_token=r\n",
        "hunter2",
    ] {
        let secret = Secret::new(bad.as_bytes().to_vec());
        assert!(Grant::decode(&secret).is_err(), "{bad:?}");
    }
    assert!(!is_grant(&Secret::new(b"hunter2".to_vec())));
}
