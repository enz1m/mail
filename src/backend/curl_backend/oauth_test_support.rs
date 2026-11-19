//! Test support: a local OAuth provider and the browser's return.

use super::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

pub(crate) fn form(body: &str) -> Vec<(String, String)> {
    let decode = |text: &str| {
        let bytes = text.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'%' => {
                    out.push(
                        u8::from_str_radix(
                            text.get(i + 1..i + 3)
                                .expect("fixture percent escape contains two ASCII digits"),
                            16,
                        )
                        .expect("hex"),
                    );
                    i += 3;
                }
                b'+' => {
                    out.push(b' ');
                    i += 1;
                }
                b => {
                    out.push(b);
                    i += 1;
                }
            }
        }
        String::from_utf8(out).expect("utf8")
    };
    body.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(name), decode(value))
        })
        .collect()
}

pub(crate) fn get<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

pub(crate) type Recorded = Arc<std::sync::Mutex<Vec<Vec<(String, String)>>>>;

pub(crate) struct Provider {
    pub(crate) port: u16,
    pub(crate) requests: Recorded,
}

impl Provider {
    pub(crate) fn start(expected_code: &'static str) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind provider");
        let port = listener.local_addr().expect("address").port();
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
                let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(value) = line
                        .to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(str::to_owned)
                    {
                        length = value.parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; length];
                reader.read_exact(&mut body).ok();
                let params = form(&String::from_utf8_lossy(&body));
                let reply = match get(&params, "grant_type") {
                    Some("authorization_code")
                        if get(&params, "code") == Some(expected_code)
                            && get(&params, "code_verifier").is_some_and(|v| v.len() == 43) =>
                    {
                        r#"{"access_token":"AT1","expires_in":3600,"refresh_token":"RT1","token_type":"Bearer"}"#
                    }
                    Some("refresh_token") if get(&params, "refresh_token") == Some("RT1") => {
                        r#"{"access_token":"AT2","expires_in":3600,"token_type":"Bearer"}"#
                    }
                    _ => r#"{"error":"invalid_grant","error_description":"Bad Request"}"#,
                };
                recorded.lock().expect("lock").push(params);
                let status = if reply.contains("invalid_grant") {
                    "400 Bad Request"
                } else {
                    "200 OK"
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        Self { port, requests }
    }

    pub(crate) fn client(&self) -> OAuthClient {
        OAuthClient::with_endpoints(
            OAuthProvider::Google,
            &format!("http://127.0.0.1:{}/authorize", self.port),
            &format!("http://127.0.0.1:{}/token", self.port),
            "test-client.apps.example",
            Some(SecretText::new("desktop-secret".into())),
        )
        .expect("client")
    }
}

pub(crate) fn browser_returns(authorization_url: &str, code: &str) {
    let query = form(authorization_url.split_once('?').expect("query").1);
    let redirect = get(&query, "redirect_uri")
        .expect("redirect_uri")
        .to_owned();
    let state = get(&query, "state").expect("state").to_owned();
    assert_eq!(get(&query, "access_type"), Some("offline"));
    assert_eq!(get(&query, "scope"), Some("https://mail.google.com/"));
    let address = redirect
        .trim_start_matches("http://")
        .split('/')
        .next()
        .expect("address")
        .to_owned();
    let path = redirect
        .get(redirect.find("/oauth2").expect("path")..)
        .expect("matched callback path is a UTF-8 boundary");
    let mut stream = TcpStream::connect(&address).expect("connect loopback");
    write!(
        stream,
        "GET {path}?code={code}&state={state} HTTP/1.1\r\nHost: {address}\r\n\r\n"
    )
    .expect("request");
    let mut reply = String::new();
    stream.read_to_string(&mut reply).ok();
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
}
