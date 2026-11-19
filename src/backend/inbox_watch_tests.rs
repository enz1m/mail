use super::*;
use crate::backend::{AuthMethod, Encryption, IncomingConfig, MailProtocol, SmtpConfig};
use std::io::{BufRead as _, BufReader};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Instant;

fn loopback_config(port: u16) -> AccountConfig {
    let incoming = IncomingConfig {
        server: "127.0.0.1".to_owned(),
        port,
        username: "reader@example.com".to_owned(),
        encryption: Encryption::None,
    };
    AccountConfig {
        id: "acct-watch".to_owned(),
        credential_owner: "acct-watch".to_owned(),
        display_name: "Watch".to_owned(),
        email: "reader@example.com".to_owned(),
        protocol: MailProtocol::Imap,
        imap: (&incoming).into(),
        incoming,
        smtp: SmtpConfig {
            server: "127.0.0.1".to_owned(),
            port,
            username: "reader@example.com".to_owned(),
            encryption: Encryption::None,
        },
        auth: AuthMethod::Plain,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Script {
    PushOnce,

    RefuseLogin,
}

fn fixture(script: Script) -> (u16, mpsc::Receiver<String>, JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind watch fixture");
    let port = listener.local_addr().expect("fixture address").port();
    let (lines, seen) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("the watch connects");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("bound reads");
        let mut writer = stream.try_clone().expect("clone stream");
        let mut reader = BufReader::new(stream);
        let _ = writer.write_all(b"* OK [CAPABILITY IMAP4rev1 AUTH=PLAIN IDLE] ready\r\n");
        let mut idles = 0;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let line = line.trim_end().to_owned();
            let _ = lines.send(line.clone());
            let (tag, rest) = line.split_once(' ').unwrap_or(("", line.as_str()));
            let upper = rest.to_ascii_uppercase();
            let login = upper.starts_with("LOGIN") || upper.starts_with("AUTHENTICATE");
            if login && script == Script::RefuseLogin {
                let _ = write!(writer, "{tag} NO [AUTHENTICATIONFAILED] denied\r\n");
                continue;
            }
            if upper.starts_with("AUTHENTICATE PLAIN") && upper.split_whitespace().count() == 2 {
                let _ = writer.write_all(b"+ \r\n");
                let mut response = String::new();
                let _ = reader.read_line(&mut response);
            }
            let reply = match upper.as_str() {
                "IDLE" => {
                    let _ = writer.write_all(b"+ idling\r\n");
                    idles += 1;
                    if idles == 1 && script == Script::PushOnce {
                        std::thread::sleep(Duration::from_millis(50));
                        let _ = writer.write_all(b"* 4 EXISTS\r\n");
                    }
                    let mut done = String::new();
                    if reader.read_line(&mut done).unwrap_or(0) == 0 {
                        return;
                    }
                    let _ = lines.send(done.trim_end().to_owned());
                    String::new()
                }
                "CAPABILITY" => "* CAPABILITY IMAP4rev1 AUTH=PLAIN IDLE\r\n".to_owned(),
                _ if upper.starts_with("EXAMINE") => "* 3 EXISTS\r\n".to_owned(),
                _ => String::new(),
            };
            let _ = write!(writer, "{reply}{tag} OK done\r\n");
        }
    });
    (port, seen, server)
}

fn secret() -> Secret {
    Secret::new(b"generated-password".to_vec())
}

fn next_event(watch: &InboxWatch, budget: Duration) -> Option<InboxWatchEvent> {
    use iced::futures::StreamExt as _;
    use std::task::{Context, Poll};
    let mut slot = watch
        .receiver
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let receiver = slot.as_mut()?;
    let waker = iced::futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);
    let deadline = Instant::now() + budget;
    loop {
        match receiver.poll_next_unpin(&mut cx) {
            Poll::Ready(event) => return event,
            Poll::Pending if Instant::now() >= deadline => return None,
            Poll::Pending => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

fn wait_for_line(seen: &mpsc::Receiver<String>, wanted: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match seen.recv_timeout(left) {
            Ok(line) if line == wanted || line.ends_with(&format!(" {wanted}")) => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    false
}

#[test]
fn inbox_watch_reports_changes_or_auth_refusal_and_releases_its_idle() {
    for (script, refused) in [(Script::PushOnce, false), (Script::RefuseLogin, true)] {
        let (port, seen, server) = fixture(script);
        let watch = InboxWatch::spawn(loopback_config(port), secret()).expect("watch");
        let event = next_event(&watch, Duration::from_secs(20));
        if refused {
            assert!(matches!(event, Some(InboxWatchEvent::Ended(failure))
                if failure.kind == MailFailureKind::Authentication));
        } else {
            assert!(matches!(event, Some(InboxWatchEvent::Changed)));
            assert!(wait_for_line(&seen, "DONE"));
            assert!(wait_for_line(&seen, "IDLE"));
        }
        let dropped = Instant::now();
        drop(watch);
        if !refused {
            assert!(wait_for_line(&seen, "DONE"));
            assert!(dropped.elapsed() < Duration::from_secs(5));
        }
        server.join().expect("fixture exits");
    }
}
