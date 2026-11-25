//! Curl backend tests: fetch parsing, MIME, failure mapping, and
//! transport behavior.

use std::fmt::Write as _;

use super::*;
use crate::model::types::{MailboxKind, MessageBodyState};
use enzui_services::credential_store::Secret;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Instant;

pub(super) fn loopback_config(port: u16) -> AccountConfig {
    let incoming = super::super::IncomingConfig {
        server: "127.0.0.1".to_owned(),
        port,
        username: "sender@example.com".to_owned(),
        encryption: Encryption::None,
    };
    AccountConfig {
        id: "acct-loopback".to_owned(),
        credential_owner: "acct-loopback".to_owned(),
        display_name: "Loopback".to_owned(),
        email: "sender@example.com".to_owned(),
        protocol: MailProtocol::Imap,
        imap: (&incoming).into(),
        incoming,
        smtp: super::super::SmtpConfig {
            server: "127.0.0.1".to_owned(),
            port,
            username: "sender@example.com".to_owned(),
            encryption: Encryption::None,
        },
        auth: AuthMethod::Plain,
    }
}

fn fetch_response(uid: u32, raw: &[u8]) -> Vec<u8> {
    let prefix = format!(
        "* 1 FETCH (UID {uid} FLAGS (\\Seen) INTERNALDATE \"15-Sep-2026 13:42:00 +0000\" ENVELOPE (\"Tue, 15 Sep 2026 13:42:00 +0000\" \"Subject\" ((\"Sender\" NIL \"sender\" \"example.com\")) ((\"Sender\" NIL \"sender\" \"example.com\")) ((\"Sender\" NIL \"sender\" \"example.com\")) ((NIL NIL \"reader\" \"example.com\")) NIL NIL NIL \"<m1@example.com>\") BODY[] {{{}}}\r\n",
        raw.len()
    );
    let mut response = prefix.into_bytes();
    response.extend_from_slice(raw);
    response.extend_from_slice(b")\r\nA3 OK fetch\r\n");
    response
}

#[test]
fn mail_backend_capped_body_is_typed_incomplete_with_exact_fetch_identity() {
    let status = b"* STATUS INBOX (UIDVALIDITY 9123)\r\nA1 OK status\r\n";
    assert_eq!(parse_uid_validity(status), Ok(9123));
    let search = b"* SEARCH 7 2 9\r\nA2 OK search\r\n";
    assert_eq!(parse_search_uids(search), Ok(vec![7, 2, 9]));
    let raw = b"Content-Type: text/plain; charset=utf-8\r\n\r\nhello";
    let fetch = fetch_response(7, raw);
    let messages = parse_fetch_messages(44, "acct-4", MailboxKind::Inbox, 9123, &[7], &fetch)
        .expect("bounded response parses");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].message.id, "imap-acct-4-9123-7");
    assert_eq!(messages[0].message.body, "hello");
    assert!(!messages[0].message.unread);
    assert_eq!(messages[0].body_completion.uid_validity, 9123);
    assert_eq!(messages[0].body_completion.uid, 7);
    assert_eq!(messages[0].body_completion.sync_generation, 44);

    let oversized = vec![b'x'; MAX_MESSAGE_BODY_BYTES as usize + 1];
    let html = fetch_response(7, b"Content-Type: text/html; charset=utf-8\r\n\r\n<img src='https://example.invalid/logo.png' alt='Logo'><p>Hello &amp; welcome</p><hr><p>Second paragraph</p>");
    let html = parse_fetch_messages(45, "acct-4", MailboxKind::Inbox, 9123, &[7], &html).unwrap();
    assert_eq!(html[0].message.preview, "Hello & welcome Second paragraph");
    assert!(html[0].message.body_html.is_some());
    let capped = fetch_response(7, &oversized);
    let capped = parse_fetch_messages(45, "acct-4", MailboxKind::Inbox, 9123, &[7], &capped)
        .expect("the body cap is a message state, not a mailbox failure");
    assert_eq!(
        capped[0].message.body_state,
        MessageBodyState::Incomplete {
            retained_bytes: 0,
            limit_bytes: MAX_MESSAGE_BODY_BYTES,
        }
    );
    assert_eq!(
        bounded_sync_coverage(SyncCoverage::Complete, 1, &capped),
        SyncCoverage::Capped {
            completed: 0,
            total: 1,
        }
    );
    assert!(capped[0].message.body.is_empty());
    assert!(capped[0].message.body_html.is_none());
    assert!(
        capped[0]
            .message
            .body_state
            .disclosure()
            .starts_with("Message body incomplete")
    );
}

#[test]
fn mail_backend_hostile_mime_isolated_and_filename_encoding_is_bounded_before_staging() {
    assert!(matches!(
        parse_mime_message(
            b"Content-Type: multipart/mixed; boundary=unterminated\r\n\r\n--unterminated\r\nContent-Type: text/plain\r\n\r\nbody"
        ),
        Err(MimeParseFailure::Multipart)
    ));
    assert!(matches!(
        parse_mime_message(b"Content-Type: text/plain\r\nContent-Type: text/html\r\n\r\nbody"),
        Err(MimeParseFailure::Headers)
    ));
    assert!(matches!(
        decode_transfer(b"%%%not-base64%%%", "base64", 1024),
        Err(MimeParseFailure::TransferEncoding)
    ));
    let malformed = b"Content-Type: multipart/mixed; boundary=bad\r\n\r\n--bad\r\nContent-Type: text/plain\r\n\r\nvisible\r\n--bad\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=bad.bin\r\nContent-Transfer-Encoding: base64\r\n\r\n%%%not-base64%%%\r\n--bad--\r\n";
    let hostile_name = b"Content-Type: multipart/mixed; boundary=good\r\n\r\n--good\r\nContent-Type: text/plain\r\n\r\nsecond message\r\n--good\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename*=UTF-8''..%2Fsecret%00.txt\r\nContent-Transfer-Encoding: base64\r\n\r\naGVsbG8=\r\n--good--\r\n";
    let mut fetch = fetch_response(7, malformed);
    let marker = b"UID 7 FLAGS";
    let marker_at = fetch
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("generated FETCH UID marker");
    fetch.splice(
        marker_at..marker_at + marker.len(),
        b"UID 7 UID 7 FLAGS".iter().copied(),
    );
    fetch.extend(fetch_response(8, hostile_name));
    let messages = parse_fetch_messages(91, "acct-mime", MailboxKind::Inbox, 55, &[7, 8], &fetch)
        .expect("one malformed MIME entity must not poison another fetched UID");
    assert_eq!(messages[0].message.body_state, MessageBodyState::Malformed);
    assert!(messages[0].message.attachments.is_empty());
    assert_eq!(messages[1].message.body, "second message");
    assert_eq!(messages[1].message.attachments.len(), 1);
    let attachment = &messages[1].message.attachments[0];
    assert_eq!(attachment.name, ".._secret_.txt");
    assert!(
        attachment
            .name
            .chars()
            .all(|character| !matches!(character, '/' | '\\' | '\0'))
    );
    assert!(attachment.payload_id.is_none());
    assert_eq!(
        messages[1].attachment_completions[0].decoded_bytes,
        b"hello"
    );
    assert_eq!(messages[1].attachment_completions[0].identity.part, "1.2");
    assert_eq!(
        messages[1].attachment_completions[0]
            .identity
            .sync_generation,
        91
    );
}

#[test]
fn real_world_mime_with_legacy_charsets_and_loose_headers_parses_completely() {
    let mut raw = b"Received: from mx.example.net\r\n".to_vec();
    raw.extend_from_slice(b"X-Spam_score: 0.1\r\nX-Spam_bar: /\r\n");
    raw.extend_from_slice(b"Subject: Caf\xe9 r\xe9sum\xe9\r\n");
    raw.extend_from_slice(b"MIME-Version: 1.0\r\n");
    raw.extend_from_slice(b"Content-Type: multipart/mixed; boundary=\"outer;part\";\r\n\r\n");
    raw.extend_from_slice(b"--outer;part\r\n");
    raw.extend_from_slice(b"Content-Type: text/plain; charset=\"windows-1252\";\r\n");
    raw.extend_from_slice(b"Content-Transfer-Encoding: quoted-printable\r\n\r\n");
    raw.extend_from_slice(b"Total =80 12 =96 =93final=94\r\n");
    raw.extend_from_slice(b"--outer;part\r\n");
    raw.extend_from_slice(b"Content-Type: text/html; charset=ISO-8859-1\r\n");
    raw.extend_from_slice(b"Content-Transfer-Encoding: 8bit\r\n\r\n");
    raw.extend_from_slice(b"<p>Gr\xfc\xdfe aus K\xf6ln</p>\r\n");
    raw.extend_from_slice(b"--outer;part\r\n");
    raw.extend_from_slice(b"Content-Type: application/pdf; name=\"invoice.pdf\"\r\n");
    raw.extend_from_slice(b"Content-Disposition: attachment; filename*=iso-8859-1''re%E7u.pdf\r\n");
    raw.extend_from_slice(b"Content-Transfer-Encoding: base64\r\n\r\naGVsbG8=\r\n");
    raw.extend_from_slice(b"--outer;part--\r\n");

    let parsed = parse_mime_message(&raw).expect("real-world MIME parses");
    assert_eq!(parsed.body_state, MessageBodyState::Complete);
    assert_eq!(
        parsed.body.trim_end(),
        "Total \u{20ac} 12 \u{2013} \u{201c}final\u{201d}"
    );
    assert!(
        parsed
            .body_html
            .as_deref()
            .is_some_and(|html| html.contains("Gr\u{fc}\u{df}e aus K\u{f6}ln"))
    );
    assert_eq!(parsed.attachments.len(), 1);
    assert_eq!(parsed.attachments[0].name, "re\u{e7}u.pdf");
    assert_eq!(parsed.attachments[0].bytes, b"hello");

    let single = b"Content-Type: text/plain; charset=iso-8859-1;;\r\n\r\nna\xefve\r\n";
    assert_eq!(
        parse_mime_message(single)
            .expect("single part parses")
            .body
            .trim_end(),
        "na\u{ef}ve"
    );
    assert!(matches!(
        parse_mime_message(
            b"Content-Type: text/plain\r\nContent-Type: text/html; charset=windows-1252\r\n\r\nbody"
        ),
        Err(MimeParseFailure::Headers)
    ));
}

#[test]
fn mail_backend_production_transport_gates_plaintext_starttls_and_runtime_protocols() {
    assert_eq!(
        imap_endpoint_url("mail.example.com", 993, Encryption::Tls, "INBOX").expect("TLS endpoint"),
        "imaps://mail.example.com:993/INBOX"
    );
    assert_eq!(
        imap_endpoint_url("mail.example.com", 143, Encryption::StartTls, "INBOX")
            .unwrap_err()
            .kind,
        MailFailureKind::Unavailable
    );
    assert_eq!(
        imap_endpoint_url("mail.example.com", 143, Encryption::None, "INBOX")
            .unwrap_err()
            .kind,
        MailFailureKind::Configuration
    );

    let mut config = loopback_config(993);
    config.incoming.encryption = Encryption::Tls;
    config.imap.encryption = Encryption::Tls;
    let missing_imaps = CurlMailBackend::with_protocols(&["imap", "smtp", "smtps"]);
    assert_eq!(
        missing_imaps
            .validate_configuration(&config)
            .unwrap_err()
            .kind,
        MailFailureKind::Unavailable
    );
    CurlMailBackend::with_protocols(&["imaps"])
        .validate_configuration(&config)
        .expect("configured implicit-TLS IMAP protocol is available");
    config.incoming.encryption = Encryption::StartTls;
    assert_eq!(
        CurlMailBackend::with_protocols(&["imap"])
            .validate_configuration(&config)
            .unwrap_err()
            .kind,
        MailFailureKind::Unavailable
    );

    config.smtp.server = "mail.example.com".to_owned();
    config.smtp.port = 465;
    config.smtp.encryption = Encryption::Tls;
    assert_eq!(
        smtp_endpoint(&config).expect("SMTPS endpoint"),
        SmtpEndpoint {
            url: "smtps://mail.example.com:465".to_owned(),
            require_ssl: true,
        }
    );
    CurlMailBackend::with_protocols(&["smtps"])
        .validate_submission(&config)
        .expect("implicit TLS SMTP is available");

    config.smtp.port = 587;
    config.smtp.encryption = Encryption::StartTls;
    assert_eq!(
        smtp_endpoint(&config).expect("STARTTLS endpoint"),
        SmtpEndpoint {
            url: "smtp://mail.example.com:587".to_owned(),
            require_ssl: true,
        }
    );
    CurlMailBackend::with_protocols(&["smtp"])
        .validate_submission(&config)
        .expect("required STARTTLS SMTP is available");

    config.smtp.encryption = Encryption::None;
    assert_eq!(
        smtp_endpoint(&config).unwrap_err().kind,
        MailFailureKind::Configuration
    );
}

#[test]
fn mail_backend_post_data_cancel_waits_for_uncertain_transport_settlement() {
    let (dispatcher, _owner) = super::internal_owner();
    let (entered_tx, entered_rx) = mpsc::channel();
    let operation = super::submit_smtp_operation(&dispatcher, 78, move |cancel, smtp_wire| {
        assert!(smtp_wire.enter_wire());
        entered_tx.send(()).expect("publish SMTP wire entry");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cancel.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(MailFailure::new(
            MailFailureKind::DeliveryUncertain,
            "generated post-DATA cancellation",
        ))
    });
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("SMTP closure must enter the wire before cancellation");
    operation.cancel_handle().cancel();
    let (generation, result) = iced_test::futures::futures::executor::block_on(operation.finish());
    assert_eq!(generation, 78);
    assert_eq!(
        result
            .expect_err("post-DATA cancel must remain uncertain")
            .kind,
        MailFailureKind::DeliveryUncertain
    );
}

#[test]
fn mail_backend_parser_rejects_oversized_or_malformed_responses() {
    let oversized = vec![b'x'; MAX_FETCH_RESPONSE_BYTES + 1];
    assert_eq!(
        parse_responses(&oversized).unwrap_err().kind,
        MailFailureKind::ResponseTooLarge
    );
    assert_eq!(
        parse_search_uids(b"not an IMAP response\r\n")
            .unwrap_err()
            .kind,
        MailFailureKind::Protocol
    );
}

fn mailbox_fixture(count: u32) -> (u16, std::sync::Arc<AtomicBool>, JoinHandle<Vec<String>>) {
    mailbox_fixture_with(count, "Content-Type: text/plain\r\n\r\nbody")
}

fn mailbox_fixture_with(
    count: u32,
    raw: &'static str,
) -> (u16, std::sync::Arc<AtomicBool>, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mailbox fixture");
    let port = listener.local_addr().expect("fixture address").port();
    listener.set_nonblocking(true).expect("non-blocking accept");
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let stopped = std::sync::Arc::clone(&stop);
    let worker = std::thread::spawn(move || {
        let mut commands = Vec::new();
        while !stopped.load(Ordering::Relaxed) {
            let stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(error) => panic!("mailbox fixture accept failed: {error}"),
            };
            stream.set_nonblocking(false).expect("blocking stream");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("bound reads");
            let mut writer = stream.try_clone().expect("clone stream");
            let mut reader = BufReader::new(stream);
            writer
                .write_all(b"* OK [CAPABILITY IMAP4rev1 AUTH=PLAIN] ready\r\n")
                .expect("greeting");
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let command = line.trim_end_matches(['\r', '\n']).to_owned();
                let (tag, rest) = command.split_once(' ').unwrap_or(("A0", ""));
                let upper = rest.to_ascii_uppercase();
                let reply = if upper.starts_with("CAPABILITY") {
                    "* CAPABILITY IMAP4rev1 AUTH=PLAIN\r\n".to_owned()
                } else if upper.starts_with("AUTHENTICATE PLAIN") {
                    if upper.split_whitespace().count() == 2 {
                        writer.write_all(b"+ \r\n").expect("auth challenge");
                        let mut response = String::new();
                        let _ = reader.read_line(&mut response);
                    }
                    String::new()
                } else if upper.starts_with("LOGIN") {
                    String::new()
                } else if upper.starts_with("SELECT") || upper.starts_with("EXAMINE") {
                    format!("* {count} EXISTS\r\n* OK [UIDVALIDITY 7] valid\r\n")
                } else if upper.starts_with("STATUS") {
                    format!("* STATUS INBOX (MESSAGES {count} UIDVALIDITY 7)\r\n")
                } else if upper.starts_with("UID SEARCH") {
                    let range = upper
                        .strip_prefix("UID SEARCH ")
                        .and_then(|set| set.split_once(':'));
                    let first = range
                        .and_then(|(first, _)| first.parse::<u32>().ok())
                        .unwrap_or(1);
                    let last = range
                        .and_then(|(_, last)| last.parse::<u32>().ok())
                        .unwrap_or(count);
                    let uids = (first..=last)
                        .map(|sequence| (sequence * 2).to_string())
                        .collect::<Vec<_>>()
                        .join(" ");
                    format!("* SEARCH {uids}\r\n")
                } else if upper.starts_with("UID FETCH") {
                    let set = rest.split_whitespace().nth(2).unwrap_or_default();
                    let mut out = String::new();
                    for uid in set.split(',').filter_map(|uid| uid.parse::<u32>().ok()) {
                        writeln!(&mut out, "* {} FETCH (UID {uid} FLAGS (\\Seen) INTERNALDATE \"15-Sep-2026 13:42:00 +0000\" ENVELOPE (\"Tue, 15 Sep 2026 13:42:00 +0000\" \"Subject\" ((\"Sender\" NIL \"sender\" \"example.com\")) ((\"Sender\" NIL \"sender\" \"example.com\")) ((\"Sender\" NIL \"sender\" \"example.com\")) ((NIL NIL \"reader\" \"example.com\")) NIL NIL NIL \"<m{uid}@example.com>\") BODY[] {{{}}}\r\n{raw})\r",
                            uid / 2,
                            raw.len()).expect("formatting an owned String succeeds");
                    }
                    out
                } else if upper.starts_with("LOGOUT") {
                    let _ = write!(writer, "* BYE\r\n{tag} OK logout\r\n");
                    break;
                } else {
                    format!("{tag} BAD unexpected\r\n")
                };
                commands.push(format!("FULL {upper}"));
                commands.push(
                    rest.split_whitespace()
                        .take(2)
                        .collect::<Vec<_>>()
                        .join(" "),
                );
                let _ = write!(writer, "{reply}{tag} OK done\r\n");
            }
        }
        commands
    });
    (port, stop, worker)
}

#[test]
fn server_search_and_older_pages_fetch_only_bounded_missing_messages() {
    for (population, known, query, first, last) in [
        (
            500,
            (451..=500).map(|uid| (7, uid * 2)).collect::<Vec<_>>(),
            "TEXT \"invoice\"",
            802,
            900,
        ),
        (100_000, Vec::new(), "X-MAIL-OLDER 199602", 199502, 199600),
    ] {
        let (port, stop, server) = mailbox_fixture(population);
        let batch = super::operations::search_mailbox(
            6,
            &loopback_config(port),
            &Secret::new(b"generated-password".to_vec()),
            &AtomicBool::new(false),
            MailboxKind::Inbox,
            &known,
            query,
        );
        stop.store(true, Ordering::Relaxed);
        let commands = server.join().unwrap();
        let batch = batch.unwrap();
        assert_eq!(batch.messages.len(), super::operations::SERVER_SEARCH_LIMIT);
        assert_eq!(batch.messages.first().unwrap().body_completion.uid, first);
        assert_eq!(batch.messages.last().unwrap().body_completion.uid, last);
        assert_eq!(
            commands
                .iter()
                .filter(|command| command.starts_with("FULL UID FETCH")
                    && command.contains("BODY.PEEK"))
                .count(),
            1
        );
        if population == 100_000 {
            assert!(
                !commands
                    .iter()
                    .any(|command| command == "FULL UID SEARCH ALL")
            );
        }
    }
}

#[test]
fn imap_dates_apply_the_server_offset_before_local_calendar_formatting() {
    assert!(super::mime::parse_internal_date("31-Dec-2025 22:30:00 +0000").is_some());
    assert_eq!(
        super::mime::parse_internal_date("01-Jan-2026 00:30:00 +0200"),
        super::mime::parse_internal_date("31-Dec-2025 22:30:00 +0000")
    );
    assert!(super::mime::parse_internal_date("31-Feb-2026 00:30:00 +0000").is_none());
    assert!(super::mime::parse_internal_date("01-Jan-2026 00:30:00 +0261").is_none());
}
