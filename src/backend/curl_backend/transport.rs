//! Transport: IMAP execution, SMTP submission, endpoints, and failure
//! mapping.

use super::types::{MailFailure, MailFailureKind, MailResult, SendRequest, SmtpWireState};
use super::{
    AccountConfig, AuthMethod, CONNECT_TIMEOUT, Encryption, LOW_SPEED_BYTES_PER_SECOND,
    LOW_SPEED_TIME, MAX_CONTROL_RESPONSE_BYTES, MAX_CREDENTIAL_BYTES, MAX_PROTOCOL_HEADER_BYTES,
    MAX_SMTP_MESSAGE_BYTES, MailProtocol, TOTAL_TIMEOUT,
};
use crate::model::types::MailboxKind;
use curl::easy::{Easy, ReadError};
use enzui_services::credential_store::Secret;
use enzui_services::mail_transport::{
    Activity, MailTransport, MailTransportError, MailTransportErrorKind, RecipientList,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) fn perform_imap(
    config: &AccountConfig,
    secret: &Secret,
    cancel: &AtomicBool,
    url: &str,
    command: Option<&str>,
    limit: usize,
) -> MailResult<Vec<u8>> {
    perform_imap_on(
        &mut Easy::new(),
        config,
        secret,
        cancel,
        url,
        command,
        limit,
    )
}

pub(super) fn perform_imap_on(
    easy: &mut Easy,
    config: &AccountConfig,
    secret: &Secret,
    cancel: &AtomicBool,
    url: &str,
    command: Option<&str>,
    limit: usize,
) -> MailResult<Vec<u8>> {
    let credential = Credential::for_account(config, secret)?;
    configure_common(easy, url, &config.incoming.username, &credential)?;
    if let Some(command) = command {
        easy.custom_request(command).map_err(curl_failure)?;
    }
    let mut response = Vec::new();
    let mut response_too_large = false;
    let mut header_bytes = 0usize;
    let mut headers_too_large = false;
    let perform = {
        let mut transfer = easy.transfer();
        transfer
            .write_function(|data| {
                let next = response.len().saturating_add(data.len());
                if next > limit {
                    response_too_large = true;
                    Ok(0)
                } else {
                    response.extend_from_slice(data);
                    Ok(data.len())
                }
            })
            .map_err(curl_failure)?;
        transfer
            .header_function(|data| {
                header_bytes = header_bytes.saturating_add(data.len());
                headers_too_large = header_bytes > MAX_PROTOCOL_HEADER_BYTES;
                !headers_too_large
            })
            .map_err(curl_failure)?;
        transfer
            .progress_function(|_, _, _, _| !cancel.load(Ordering::Relaxed))
            .map_err(curl_failure)?;
        transfer.perform()
    };
    if response_too_large || headers_too_large {
        return Err(MailFailure::new(
            MailFailureKind::ResponseTooLarge,
            "IMAP response exceeded configured limit",
        ));
    }
    perform.map_err(|error| transfer_failure(error, cancel))?;
    Ok(response)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SmtpEndpoint {
    pub(super) url: String,
    pub(super) require_ssl: bool,
}

pub(super) fn smtp_endpoint(config: &AccountConfig) -> MailResult<SmtpEndpoint> {
    let host = &config.smtp.server;
    let port = config.smtp.port;
    if !valid_host(host) || port == 0 {
        return Err(MailFailure::new(
            MailFailureKind::Configuration,
            "invalid SMTP server endpoint",
        ));
    }
    let (scheme, require_ssl) = match config.smtp.encryption {
        Encryption::Tls => ("smtps", true),
        Encryption::StartTls => ("smtp", true),
        Encryption::None if cfg!(test) && is_loopback(host) => ("smtp", false),
        Encryption::None => {
            return Err(MailFailure::new(
                MailFailureKind::Configuration,
                "plaintext SMTP transport is disabled",
            ));
        }
    };
    Ok(SmtpEndpoint {
        url: format!("{scheme}://{host}:{port}"),
        require_ssl,
    })
}

pub(super) fn validate_submission_config(config: &AccountConfig) -> MailResult<()> {
    if config.protocol != MailProtocol::Imap {
        return Err(MailFailure::new(
            MailFailureKind::Configuration,
            "only SMTP submission for IMAP accounts is supported",
        ));
    }
    validate_username(&config.smtp.username)?;
    let _ = smtp_endpoint(config)?;
    Ok(())
}

pub(super) fn submit_message(
    request: SendRequest,
    cancel: Arc<AtomicBool>,
    smtp_wire: Arc<SmtpWireState>,
) -> MailResult<()> {
    if request.mime.len() > MAX_SMTP_MESSAGE_BYTES {
        return Err(MailFailure::new(
            MailFailureKind::MessageTooLarge,
            "SMTP upload exceeded configured limit",
        ));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(MailFailure::new(
            MailFailureKind::Cancelled,
            "operation cancelled before SMTP upload",
        ));
    }
    let endpoint = smtp_endpoint(&request.config)?;
    let mut sender = RecipientList::new();
    sender
        .add(&request.envelope_from)
        .map_err(mail_transport_failure)?;
    let mut recipients = RecipientList::new();
    for recipient in &request.recipients {
        recipients.add(recipient).map_err(mail_transport_failure)?;
    }
    drop(sender);
    let credential = Credential::for_account(&request.config, &request.secret)?;
    let mut easy = Easy::new();
    configure_common(
        &mut easy,
        &endpoint.url,
        &request.config.smtp.username,
        &credential,
    )?;
    easy.upload(true).map_err(curl_failure)?;
    easy.in_filesize(u64::try_from(request.mime.len()).map_err(|_| {
        MailFailure::new(
            MailFailureKind::MessageTooLarge,
            "SMTP upload length cannot be represented",
        )
    })?)
    .map_err(curl_failure)?;
    let mut offset = 0usize;
    let mime = request.mime;
    // libcurl requests upload bytes only after SMTP accepts the DATA command.
    let wire_flag = Arc::clone(&smtp_wire);
    easy.read_function(move |buffer| {
        if !wire_flag.enter_wire() {
            return Err(ReadError::Abort);
        }
        let remaining = &mime[offset..];
        let count = remaining.len().min(buffer.len());
        buffer[..count].copy_from_slice(&remaining[..count]);
        offset += count;
        Ok(count)
    })
    .map_err(curl_failure)?;
    let progress_cancel = Arc::clone(&cancel);
    easy.progress_function(move |_, _, _, _| !progress_cancel.load(Ordering::Relaxed))
        .map_err(curl_failure)?;

    let mut transport = MailTransport::new(&mut easy);
    if endpoint.require_ssl {
        transport
            .set_require_ssl()
            .map_err(mail_transport_failure)?;
    }
    transport
        .set_mail_from(&request.envelope_from)
        .map_err(mail_transport_failure)?;
    transport
        .set_recipients(recipients)
        .map_err(|error| mail_transport_failure(error.source))?;
    transport
        .perform()
        .map_err(|error| smtp_transport_failure(error, &cancel, &smtp_wire))
}

// How a connection signs in: the account password, or an OAuth
// access token refreshed from the stored grant.
enum Credential<'a> {
    Password(&'a str),
    Bearer(enzui_services::oauth::SecretText),
}

impl<'a> Credential<'a> {
    fn for_account(config: &AccountConfig, secret: &'a Secret) -> MailResult<Self> {
        if config.auth == AuthMethod::OAuth2 {
            if !super::oauth::is_grant(secret) {
                return Err(MailFailure::new(
                    MailFailureKind::Authentication,
                    "the account has no OAuth sign-in; sign in again",
                ));
            }
            return super::oauth::access_token(config, secret).map(Self::Bearer);
        }
        secret_text(secret).map(Self::Password)
    }
}

fn configure_common(
    easy: &mut Easy,
    url: &str,
    username: &str,
    credential: &Credential<'_>,
) -> MailResult<()> {
    easy.url(url).map_err(curl_failure)?;
    easy.proxy("").map_err(curl_failure)?;
    match credential {
        Credential::Password(password) => {
            easy.username(username).map_err(curl_failure)?;
            easy.password(password).map_err(curl_failure)?;
        }
        Credential::Bearer(token) => {
            enzui_services::mail_transport::set_oauth2_bearer(easy, username, token)
                .map_err(mail_transport_failure)?;
        }
    }
    easy.connect_timeout(CONNECT_TIMEOUT)
        .map_err(curl_failure)?;
    easy.timeout(TOTAL_TIMEOUT).map_err(curl_failure)?;
    easy.low_speed_limit(LOW_SPEED_BYTES_PER_SECOND)
        .map_err(curl_failure)?;
    easy.low_speed_time(LOW_SPEED_TIME).map_err(curl_failure)?;
    easy.progress(true).map_err(curl_failure)?;
    easy.ssl_verify_host(true).map_err(curl_failure)?;
    easy.ssl_verify_peer(true).map_err(curl_failure)?;
    easy.verbose(false).map_err(curl_failure)?;
    Ok(())
}

pub(super) fn mailbox_identity_token(mailbox: MailboxKind) -> &'static str {
    match mailbox {
        MailboxKind::Inbox => "inbox",
        MailboxKind::Drafts => "drafts",
        MailboxKind::Sent => "sent",
        MailboxKind::Junk => "junk",
        MailboxKind::Trash => "trash",
        MailboxKind::Archive => "archive",
    }
}

pub(super) fn imap_endpoint_url(
    host: &str,
    port: u16,
    encryption: Encryption,
    path: &str,
) -> MailResult<String> {
    if !valid_host(host) || port == 0 {
        return Err(MailFailure::new(
            MailFailureKind::Configuration,
            "invalid IMAP server endpoint",
        ));
    }
    let scheme = match encryption {
        Encryption::Tls => "imaps",
        Encryption::None if cfg!(test) && is_loopback(host) => "imap",
        Encryption::StartTls => {
            return Err(MailFailure::new(
                MailFailureKind::Unavailable,
                "curl 0.4.50 lacks a safe CURLOPT_USE_SSL binding",
            ));
        }
        Encryption::None => {
            return Err(MailFailure::new(
                MailFailureKind::Configuration,
                "plaintext IMAP transport is disabled",
            ));
        }
    };
    if path.is_empty() {
        Ok(format!("{scheme}://{host}:{port}/"))
    } else {
        Ok(format!("{scheme}://{host}:{port}/{path}"))
    }
}

pub(crate) fn validate_supported_config(config: &AccountConfig) -> MailResult<()> {
    if config.protocol != MailProtocol::Imap {
        return Err(MailFailure::new(
            MailFailureKind::Configuration,
            "only IMAP accounts are supported",
        ));
    }
    validate_username(&config.incoming.username)?;
    imap_endpoint_url(
        &config.incoming.server,
        config.incoming.port,
        config.incoming.encryption,
        "INBOX",
    )?;
    Ok(())
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
        && !host.starts_with('.')
        && !host.ends_with('.')
}

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || host.starts_with("127.")
}

fn validate_username(username: &str) -> MailResult<()> {
    if username.is_empty()
        || username.len() > 320
        || username.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(MailFailure::new(
            MailFailureKind::Configuration,
            "invalid mail username",
        ));
    }
    Ok(())
}

fn secret_text(secret: &Secret) -> MailResult<&str> {
    if secret.expose_secret().is_empty() || secret.expose_secret().len() > MAX_CREDENTIAL_BYTES {
        return Err(MailFailure::new(
            MailFailureKind::Authentication,
            "credential length outside configured limit",
        ));
    }
    std::str::from_utf8(secret.expose_secret()).map_err(|_| {
        MailFailure::new(
            MailFailureKind::Authentication,
            "credential is not valid UTF-8",
        )
    })
}

fn curl_failure_kind(error: &curl::Error) -> MailFailureKind {
    if error.is_login_denied() || error.is_remote_access_denied() {
        MailFailureKind::Authentication
    } else if error.is_operation_timedout() {
        MailFailureKind::Timeout
    } else if error.is_ssl_connect_error()
        || error.is_peer_failed_verification()
        || error.is_ssl_certproblem()
        || error.is_ssl_cacert()
        || error.is_ssl_cacert_badfile()
        || error.is_use_ssl_failed()
    {
        MailFailureKind::Tls
    } else if error.is_unsupported_protocol() || error.is_url_malformed() {
        MailFailureKind::Configuration
    } else {
        MailFailureKind::Network
    }
}

pub(super) fn curl_failure(error: curl::Error) -> MailFailure {
    let kind = curl_failure_kind(&error);
    MailFailure::new(kind, format!("libcurl code {}", error.code()))
}

fn mail_transport_failure(error: MailTransportError) -> MailFailure {
    let kind = match error.kind() {
        MailTransportErrorKind::EmptyRecipientList
        | MailTransportErrorKind::RecipientLimitReached
        | MailTransportErrorKind::AddressTooLong
        | MailTransportErrorKind::ControlCharInAddress => MailFailureKind::Envelope,
        MailTransportErrorKind::RecipientAppendFailed => MailFailureKind::Worker,
        MailTransportErrorKind::InvalidCredential => MailFailureKind::Authentication,
        MailTransportErrorKind::CurlSetopt { .. } => MailFailureKind::Unavailable,
        MailTransportErrorKind::Perform(error) => curl_failure_kind(error),
        MailTransportErrorKind::NoActiveSocket => MailFailureKind::Network,
        MailTransportErrorKind::Poll(_) => MailFailureKind::Worker,
    };
    MailFailure::new(kind, error.to_string())
}

fn smtp_transport_failure(
    error: MailTransportError,
    cancel: &AtomicBool,
    smtp_wire: &SmtpWireState,
) -> MailFailure {
    let entered = smtp_wire.entered();
    let cancelled = cancel.load(Ordering::Relaxed);
    if let MailTransportErrorKind::Perform(curl_error) = error.kind() {
        if curl_error.is_aborted_by_callback() {
            if entered {
                return MailFailure::new(
                    MailFailureKind::DeliveryUncertain,
                    "SMTP transfer aborted after DATA phase",
                );
            }
            if cancelled {
                return MailFailure::new(MailFailureKind::Cancelled, "operation cancelled");
            }
        } else if entered {
            let kind = curl_failure_kind(curl_error);
            if matches!(kind, MailFailureKind::Timeout | MailFailureKind::Network) {
                return MailFailure::new(
                    MailFailureKind::DeliveryUncertain,
                    format!("libcurl code {} after SMTP DATA phase", curl_error.code()),
                );
            }
        }
    }
    mail_transport_failure(error)
}

fn transfer_failure(error: curl::Error, cancel: &AtomicBool) -> MailFailure {
    if error.is_aborted_by_callback() && cancel.load(Ordering::Relaxed) {
        MailFailure::new(MailFailureKind::Cancelled, "operation cancelled")
    } else {
        curl_failure(error)
    }
}

// libcurl owns TLS/login; the CONNECT_ONLY session handles tagged commands and literals.
pub(super) struct ImapSession<'a> {
    easy: Easy,
    cancel: &'a AtomicBool,
    next_tag: u32,
    pending: Vec<u8>,
    deadline: std::time::Instant,
}

impl<'a> ImapSession<'a> {
    pub(super) fn open(
        config: &AccountConfig,
        secret: &Secret,
        cancel: &'a AtomicBool,
        url: &str,
    ) -> MailResult<Self> {
        let credential = Credential::for_account(config, secret)?;
        let mut easy = Easy::new();
        configure_common(&mut easy, url, &config.incoming.username, &credential)?;
        easy.connect_only(true).map_err(curl_failure)?;
        easy.perform()
            .map_err(|error| transfer_failure(error, cancel))?;
        Ok(Self {
            easy,
            cancel,
            next_tag: 1,
            pending: Vec::new(),
            deadline: std::time::Instant::now() + TOTAL_TIMEOUT,
        })
    }

    // Run `command` and return its untagged responses, at most `limit`
    // bytes. A `NO` or `BAD` completion is a protocol failure.
    pub(super) fn run(&mut self, command: &str, limit: usize) -> MailResult<Vec<u8>> {
        let tag = format!("E{}", self.next_tag);
        self.next_tag = self.next_tag.wrapping_add(1);
        self.send_all(format!("{tag} {command}\r\n").as_bytes())?;
        let tagged = format!("{tag} ");
        let mut response = Vec::new();
        loop {
            let remaining = limit.saturating_sub(response.len());
            let line = self.read_response_line(remaining)?;
            if let Some(status) = line.strip_prefix(tagged.as_bytes()) {
                if status.starts_with(b"OK") {
                    return Ok(response);
                }
                let verb = command.split_whitespace().next().unwrap_or("command");
                return Err(MailFailure::new(
                    MailFailureKind::Protocol,
                    format!("IMAP {verb} was refused"),
                ));
            }
            response.extend_from_slice(&line);
        }
    }

    // Wait in `IDLE` for up to `max` until the selected mailbox changes or
    // `wake` becomes readable, without waking in between.
    //
    // A new, expunged or re-flagged message is [`IdleOutcome::Changed`]; a
    // quiet `max` ends the `IDLE` so the caller can renew it before the
    // server's inactivity timeout.
    pub(super) fn idle(
        &mut self,
        wake: &std::os::unix::net::UnixStream,
        max: std::time::Duration,
    ) -> MailResult<IdleOutcome> {
        self.deadline = std::time::Instant::now() + TOTAL_TIMEOUT;
        let tag = format!("E{}", self.next_tag);
        self.next_tag = self.next_tag.wrapping_add(1);
        self.send_all(format!("{tag} IDLE\r\n").as_bytes())?;
        let tagged = format!("{tag} ");
        let mut changed = false;
        loop {
            let line = self.read_response_line(MAX_CONTROL_RESPONSE_BYTES)?;
            if line.starts_with(b"+") {
                break;
            }
            if line.starts_with(tagged.as_bytes()) {
                return Err(MailFailure::new(
                    MailFailureKind::Unavailable,
                    "IMAP IDLE was refused",
                ));
            }
            changed |= reports_change(&line)?;
        }
        let until = std::time::Instant::now() + max;
        let outcome = loop {
            while let Some(end) = self.pending.windows(2).position(|pair| pair == b"\r\n") {
                let line: Vec<u8> = self.pending.drain(..end + 2).collect();
                changed |= reports_change(&line)?;
            }
            if changed {
                break IdleOutcome::Changed;
            }
            if self.pending.len() > MAX_CONTROL_RESPONSE_BYTES {
                return Err(response_too_large());
            }
            let mut buf = [0u8; 16 * 1024];
            match self.easy.recv(&mut buf) {
                Ok(0) => return Err(server_closed()),
                Ok(read) => {
                    self.pending.extend_from_slice(&buf[..read]);
                    continue;
                }
                Err(error) if error.is_again() => {}
                Err(error) => return Err(transfer_failure(error, self.cancel)),
            }
            let remaining = until.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break IdleOutcome::Quiet;
            }
            match enzui_services::mail_transport::wait_for_activity(&self.easy, wake, remaining)
                .map_err(mail_transport_failure)?
            {
                Activity::Readable => {}
                Activity::TimedOut => break IdleOutcome::Quiet,
                Activity::Woken => {
                    let _ = self.easy.send(b"DONE\r\n");
                    return Ok(IdleOutcome::Stopped);
                }
            }
        };
        self.deadline = std::time::Instant::now() + TOTAL_TIMEOUT;
        self.send_all(b"DONE\r\n")?;
        loop {
            let line = self.read_response_line(MAX_CONTROL_RESPONSE_BYTES)?;
            if let Some(status) = line.strip_prefix(tagged.as_bytes()) {
                if !status.starts_with(b"OK") {
                    return Err(MailFailure::new(
                        MailFailureKind::Protocol,
                        "IMAP IDLE ended with a refusal",
                    ));
                }
                return Ok(outcome);
            }
        }
    }

    fn send_all(&mut self, mut data: &[u8]) -> MailResult<()> {
        while !data.is_empty() {
            match self.easy.send(data) {
                Ok(sent) => data = &data[sent..],
                Err(error) if error.is_again() => self.wait()?,
                Err(error) => return Err(transfer_failure(error, self.cancel)),
            }
        }
        Ok(())
    }

    // One response line with every literal it announces, so a body that
    // happens to contain a tag-like line can never end the command early.
    fn read_response_line(&mut self, limit: usize) -> MailResult<Vec<u8>> {
        let mut line = Vec::new();
        loop {
            let end = loop {
                if let Some(at) = self.pending.windows(2).position(|pair| pair == b"\r\n") {
                    break at + 2;
                }
                self.fill(limit.saturating_sub(line.len()))?;
            };
            let chunk: Vec<u8> = self.pending.drain(..end).collect();
            line.extend_from_slice(&chunk);
            let Some(size) = literal_size(&chunk[..chunk.len() - 2]) else {
                return Ok(line);
            };
            if size > limit.saturating_sub(line.len()) {
                return Err(response_too_large());
            }
            while self.pending.len() < size {
                self.fill(limit.saturating_sub(line.len()))?;
            }
            line.extend(self.pending.drain(..size));
        }
    }

    fn fill(&mut self, limit: usize) -> MailResult<()> {
        if self.pending.len() > limit {
            return Err(response_too_large());
        }
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match self.easy.recv(&mut buf) {
                Ok(0) => {
                    return Err(MailFailure::new(
                        MailFailureKind::Protocol,
                        "IMAP server closed the connection",
                    ));
                }
                Ok(read) => {
                    self.pending.extend_from_slice(&buf[..read]);
                    return Ok(());
                }
                Err(error) if error.is_again() => self.wait()?,
                Err(error) => return Err(transfer_failure(error, self.cancel)),
            }
        }
    }

    // The socket is non-blocking under `CONNECT_ONLY`; a short sleep keeps the
    // wait cheap without a poller, and the cancel flag and the overall
    // deadline still bound it.
    fn wait(&self) -> MailResult<()> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(MailFailure::new(
                MailFailureKind::Cancelled,
                "operation cancelled during IMAP sync",
            ));
        }
        if std::time::Instant::now() > self.deadline {
            return Err(MailFailure::new(
                MailFailureKind::Timeout,
                "IMAP sync exceeded its deadline",
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
        Ok(())
    }
}

impl Drop for ImapSession<'_> {
    fn drop(&mut self) {
        let tag = format!("E{}", self.next_tag);
        let _ = self.easy.send(format!("{tag} LOGOUT\r\n").as_bytes());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IdleOutcome {
    Changed,
    Quiet,
    Stopped,
}

pub(super) fn reports_change(line: &[u8]) -> MailResult<bool> {
    let line = line.strip_suffix(b"\r\n").unwrap_or(line);
    let Some(rest) = line.strip_prefix(b"* ") else {
        return Ok(false);
    };
    let upper = rest.to_ascii_uppercase();
    if upper.starts_with(b"BYE") {
        return Err(server_closed());
    }
    let mut words = upper.split(|byte| *byte == b' ');
    let numbered = words
        .next()
        .is_some_and(|word| !word.is_empty() && word.iter().all(u8::is_ascii_digit));
    Ok(numbered && matches!(words.next(), Some(b"EXISTS" | b"EXPUNGE" | b"FETCH")))
}

fn server_closed() -> MailFailure {
    MailFailure::new(
        MailFailureKind::Network,
        "IMAP server closed the connection",
    )
}

fn literal_size(line: &[u8]) -> Option<usize> {
    let line = line.strip_suffix(b"}")?;
    let line = line.strip_suffix(b"+").unwrap_or(line);
    let open = line.iter().rposition(|byte| *byte == b'{')?;
    let digits = &line[open + 1..];
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse().ok()
}

fn response_too_large() -> MailFailure {
    MailFailure::new(
        MailFailureKind::ResponseTooLarge,
        "IMAP response exceeded configured limit",
    )
}

pub(super) fn uid_set(uids: &[u32]) -> String {
    let mut sorted = uids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut parts = Vec::new();
    let mut index = 0;
    while index < sorted.len() {
        let start = sorted[index];
        let mut end = start;
        while index + 1 < sorted.len() && sorted[index + 1] == end + 1 {
            index += 1;
            end = sorted[index];
        }
        parts.push(if start == end {
            start.to_string()
        } else {
            format!("{start}:{end}")
        });
        index += 1;
    }
    parts.join(",")
}
