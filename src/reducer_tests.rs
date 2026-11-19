use super::*;
use crate::mail_actions::{ActionTarget, MailAction};
use crate::model::settings::account_from_config;
use model::types::{FavoriteKind, MailboxKind, MessageDate, MessageLocation, SidebarSelection};

fn boot_state() -> State {
    State::boot_for_test().0
}

fn production_state_for_mail_backend() -> State {
    State::boot_for_test_with_local_state(crate::local_state::memory_store(), false).0
}

fn fetched_message(
    message: model::types::Message,
    sync_generation: u64,
) -> backend::curl_backend::FetchedMessage {
    let target = State::imap_server_target(&message).expect("exact generated IMAP identity");
    let root = backend::curl_backend::ImapContentIdentity {
        account_id: target.account_id,
        mailbox: target.mailbox,
        uid_validity: target.uid_validity,
        uid: target.uid,
        part: "1".to_owned(),
        sync_generation,
    };
    let attachment_completions = message
        .attachments
        .iter()
        .enumerate()
        .map(
            |(index, attachment)| backend::curl_backend::AttachmentCompletion {
                identity: backend::curl_backend::ImapContentIdentity {
                    part: format!("1.{}", index + 1),
                    ..root.clone()
                },
                payload_id: attachment.payload_id.clone(),
                payload_identity: attachment.payload_identity,
                decoded_bytes: Vec::new(),
            },
        )
        .collect();
    backend::curl_backend::FetchedMessage {
        message,
        body_completion: root,
        attachment_completions,
    }
}

fn retag_sync_batch(batch: &mut backend::curl_backend::SyncBatch, generation: u64) {
    batch.sync_generation = generation;
    for fetched in &mut batch.messages {
        fetched.body_completion.sync_generation = generation;
        for completion in &mut fetched.attachment_completions {
            completion.identity.sync_generation = generation;
        }
    }
}

#[derive(Default)]
struct SuccessfulMailBackend {
    calls: std::sync::Mutex<Vec<&'static str>>,

    expected_secret: std::sync::Mutex<Option<Vec<u8>>>,
    sync_batches: std::sync::Mutex<std::collections::VecDeque<backend::curl_backend::SyncBatch>>,

    search_batches: std::sync::Mutex<std::collections::VecDeque<backend::curl_backend::SyncBatch>>,
    searched: std::sync::Mutex<Vec<String>>,
}

impl SuccessfulMailBackend {
    fn assert_secret(&self, secret: &Secret) {
        let expected = self
            .expected_secret
            .lock()
            .expect("expected secret lock")
            .clone()
            .unwrap_or_else(|| b"generated-password".to_vec());
        assert_eq!(secret.expose_secret(), expected.as_slice());
    }
}

impl backend::curl_backend::MailBackend for SuccessfulMailBackend {
    fn validate_configuration(
        &self,
        _config: &backend::AccountConfig,
    ) -> backend::curl_backend::MailResult<()> {
        Ok(())
    }

    fn validate_submission(
        &self,
        _config: &backend::AccountConfig,
    ) -> backend::curl_backend::MailResult<()> {
        Ok(())
    }

    fn probe(
        &self,
        generation: u64,
        request: backend::curl_backend::SyncRequest,
    ) -> backend::curl_backend::MailOperation {
        self.assert_secret(&request.secret);
        self.calls.lock().expect("mail calls lock").push("probe");
        backend::curl_backend::ready_operation(
            generation,
            Ok(backend::curl_backend::MailOperationOutput::Authenticated),
        )
    }

    fn sync_mailbox(
        &self,
        generation: u64,
        request: backend::curl_backend::SyncRequest,
        _mailbox: MailboxKind,
    ) -> backend::curl_backend::MailOperation {
        self.assert_secret(&request.secret);
        self.calls.lock().expect("mail calls lock").push("sync");
        let mut batch = self
            .sync_batches
            .lock()
            .expect("sync batches lock")
            .pop_front()
            .unwrap_or_else(|| backend::curl_backend::SyncBatch {
                older_than_uid: None,
                retained: Vec::new(),
                account_id: request.config.id,
                mailbox: MailboxKind::Inbox,
                uid_validity: 1,
                sync_generation: generation,
                coverage: backend::curl_backend::SyncCoverage::Complete,
                messages: Vec::new(),
            });
        retag_sync_batch(&mut batch, generation);
        backend::curl_backend::ready_operation(
            generation,
            Ok(backend::curl_backend::MailOperationOutput::Inbox(batch)),
        )
    }

    fn mutate(
        &self,
        generation: u64,
        request: backend::curl_backend::MutationRequest,
    ) -> backend::curl_backend::MailOperation {
        self.calls.lock().expect("mail calls lock").push("mutate");
        backend::curl_backend::ready_operation(
            generation,
            Ok(backend::curl_backend::MailOperationOutput::Mutation(
                backend::curl_backend::MutationResult {
                    mutation_id: request.mutation.id,
                    disposition: backend::curl_backend::MutationDisposition::Applied,
                    missing_target_ids: Vec::new(),
                },
            )),
        )
    }

    fn submit(
        &self,
        generation: u64,
        request: backend::curl_backend::SendRequest,
    ) -> backend::curl_backend::MailOperation {
        assert_eq!(request.secret.expose_secret(), b"generated-password");
        assert_eq!(request.envelope_from, "sender@example.com");
        assert_eq!(request.recipients, vec!["recipient@example.com".to_owned()]);
        let subject = b"Subject: delivered\r\n";
        assert!(
            request
                .mime
                .windows(subject.len())
                .any(|bytes| bytes == subject)
        );
        self.calls.lock().expect("mail calls lock").push("submit");
        backend::curl_backend::ready_operation(
            generation,
            Ok(backend::curl_backend::MailOperationOutput::Submitted),
        )
    }

    fn search_mailbox(
        &self,
        generation: u64,
        request: backend::curl_backend::SyncRequest,
        mailbox: MailboxKind,
        criteria: String,
    ) -> backend::curl_backend::MailOperation {
        self.assert_secret(&request.secret);
        self.calls.lock().expect("mail calls lock").push("search");
        self.searched.lock().expect("searched lock").push(criteria);
        let mut batch = self
            .search_batches
            .lock()
            .expect("search batches lock")
            .pop_front()
            .unwrap_or_else(|| backend::curl_backend::SyncBatch {
                older_than_uid: None,
                retained: Vec::new(),
                account_id: request.config.id,
                mailbox,
                uid_validity: 1,
                sync_generation: generation,
                coverage: backend::curl_backend::SyncCoverage::Complete,
                messages: Vec::new(),
            });
        retag_sync_batch(&mut batch, generation);
        backend::curl_backend::ready_operation(
            generation,
            Ok(backend::curl_backend::MailOperationOutput::Search(batch)),
        )
    }
    fn watch_inbox(
        &self,
        _config: &backend::AccountConfig,
        secret: Secret,
    ) -> Option<backend::inbox_watch::InboxWatch> {
        self.assert_secret(&secret);
        None
    }
}

#[derive(Default)]
struct RejectingEditMailBackend;

impl backend::curl_backend::MailBackend for RejectingEditMailBackend {
    fn validate_configuration(
        &self,
        _config: &backend::AccountConfig,
    ) -> backend::curl_backend::MailResult<()> {
        Ok(())
    }

    fn validate_submission(
        &self,
        _config: &backend::AccountConfig,
    ) -> backend::curl_backend::MailResult<()> {
        Err(backend::curl_backend::MailFailure::for_test(
            backend::curl_backend::MailFailureKind::Unavailable,
        ))
    }

    fn probe(
        &self,
        generation: u64,
        _request: backend::curl_backend::SyncRequest,
    ) -> backend::curl_backend::MailOperation {
        backend::curl_backend::ready_operation(
            generation,
            Err(backend::curl_backend::MailFailure::for_test(
                backend::curl_backend::MailFailureKind::Authentication,
            )),
        )
    }

    fn sync_mailbox(
        &self,
        generation: u64,
        request: backend::curl_backend::SyncRequest,
        _mailbox: MailboxKind,
    ) -> backend::curl_backend::MailOperation {
        backend::curl_backend::ready_operation(
            generation,
            Ok(backend::curl_backend::MailOperationOutput::Inbox(
                backend::curl_backend::SyncBatch {
                    older_than_uid: None,
                    retained: Vec::new(),
                    account_id: request.config.id,
                    mailbox: MailboxKind::Inbox,
                    uid_validity: 1,
                    sync_generation: generation,
                    coverage: backend::curl_backend::SyncCoverage::Complete,
                    messages: Vec::new(),
                },
            )),
        )
    }

    fn mutate(
        &self,
        generation: u64,
        _request: backend::curl_backend::MutationRequest,
    ) -> backend::curl_backend::MailOperation {
        backend::curl_backend::ready_operation(
            generation,
            Err(backend::curl_backend::MailFailure::for_test(
                backend::curl_backend::MailFailureKind::Unavailable,
            )),
        )
    }

    fn submit(
        &self,
        generation: u64,
        _request: backend::curl_backend::SendRequest,
    ) -> backend::curl_backend::MailOperation {
        backend::curl_backend::ready_operation(
            generation,
            Err(backend::curl_backend::MailFailure::for_test(
                backend::curl_backend::MailFailureKind::Unavailable,
            )),
        )
    }

    fn search_mailbox(
        &self,
        generation: u64,
        _request: backend::curl_backend::SyncRequest,
        _mailbox: MailboxKind,
        _criteria: String,
    ) -> backend::curl_backend::MailOperation {
        backend::curl_backend::ready_operation(
            generation,
            Err(backend::curl_backend::MailFailure::for_test(
                backend::curl_backend::MailFailureKind::Unavailable,
            )),
        )
    }
}

#[derive(Default)]
struct RetryMailBackend {
    submits: std::sync::atomic::AtomicUsize,
}

impl backend::curl_backend::MailBackend for RetryMailBackend {
    fn validate_configuration(
        &self,
        _config: &backend::AccountConfig,
    ) -> backend::curl_backend::MailResult<()> {
        Ok(())
    }

    fn validate_submission(
        &self,
        _config: &backend::AccountConfig,
    ) -> backend::curl_backend::MailResult<()> {
        Ok(())
    }

    fn probe(
        &self,
        generation: u64,
        _request: backend::curl_backend::SyncRequest,
    ) -> backend::curl_backend::MailOperation {
        backend::curl_backend::ready_operation(
            generation,
            Ok(backend::curl_backend::MailOperationOutput::Authenticated),
        )
    }

    fn sync_mailbox(
        &self,
        generation: u64,
        request: backend::curl_backend::SyncRequest,
        _mailbox: MailboxKind,
    ) -> backend::curl_backend::MailOperation {
        backend::curl_backend::ready_operation(
            generation,
            Ok(backend::curl_backend::MailOperationOutput::Inbox(
                backend::curl_backend::SyncBatch {
                    older_than_uid: None,
                    retained: Vec::new(),
                    account_id: request.config.id,
                    mailbox: MailboxKind::Inbox,
                    uid_validity: 1,
                    sync_generation: generation,
                    coverage: backend::curl_backend::SyncCoverage::Complete,
                    messages: Vec::new(),
                },
            )),
        )
    }

    fn mutate(
        &self,
        generation: u64,
        _request: backend::curl_backend::MutationRequest,
    ) -> backend::curl_backend::MailOperation {
        backend::curl_backend::ready_operation(
            generation,
            Err(backend::curl_backend::MailFailure::for_test(
                backend::curl_backend::MailFailureKind::Unavailable,
            )),
        )
    }

    fn submit(
        &self,
        generation: u64,
        _request: backend::curl_backend::SendRequest,
    ) -> backend::curl_backend::MailOperation {
        let attempt = self
            .submits
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let result = if attempt == 0 {
            Err(backend::curl_backend::MailFailure::for_test(
                backend::curl_backend::MailFailureKind::Network,
            ))
        } else {
            Ok(backend::curl_backend::MailOperationOutput::Submitted)
        };
        backend::curl_backend::ready_operation(generation, result)
    }

    fn search_mailbox(
        &self,
        generation: u64,
        _request: backend::curl_backend::SyncRequest,
        _mailbox: MailboxKind,
        _criteria: String,
    ) -> backend::curl_backend::MailOperation {
        backend::curl_backend::ready_operation(
            generation,
            Err(backend::curl_backend::MailFailure::for_test(
                backend::curl_backend::MailFailureKind::Unavailable,
            )),
        )
    }
}

fn loopback_account_config(id: &str) -> backend::AccountConfig {
    let incoming = backend::IncomingConfig {
        server: "127.0.0.1".to_owned(),
        port: 1143,
        username: "sender@example.com".to_owned(),
        encryption: backend::Encryption::None,
    };
    backend::AccountConfig {
        id: id.to_owned(),
        credential_owner: id.to_owned(),
        display_name: "Generated account".to_owned(),
        email: "sender@example.com".to_owned(),
        protocol: backend::MailProtocol::Imap,
        imap: (&incoming).into(),
        incoming,
        smtp: backend::SmtpConfig {
            server: "127.0.0.1".to_owned(),
            port: 1025,
            username: "sender@example.com".to_owned(),
            encryption: backend::Encryption::None,
        },
        auth: backend::AuthMethod::Plain,
    }
}

fn prepare_generated_account_add(state: &mut State) {
    let config = loopback_account_config("unused");
    let _ = state.update(Message::BeginAddAccount);
    state.settings.add_account.draft = crate::backend::config::AccountConfigDraft {
        display_name: config.display_name,
        email: config.email,
        protocol: config.protocol,
        imap_server: config.incoming.server,
        imap_port: config.incoming.port.to_string(),
        imap_username: config.incoming.username,
        imap_encryption: config.incoming.encryption,
        smtp_server: config.smtp.server,
        smtp_port: config.smtp.port.to_string(),
        smtp_username: config.smtp.username,
        smtp_encryption: config.smtp.encryption,
        auth: config.auth,
    };
    state.settings.add_account.password = "generated-password".to_owned();
}

#[derive(Debug)]
struct SmtpCapture {
    commands: Vec<String>,
    data: Vec<u8>,
}

fn generated_smtp_server(
    accept_message: bool,
) -> (
    u16,
    std::sync::mpsc::Receiver<SmtpCapture>,
    std::thread::JoinHandle<()>,
) {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind generated SMTP fixture");
    let port = listener.local_addr().expect("SMTP fixture address").port();
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        listener.set_nonblocking(true).expect("bound SMTP accept");
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("SMTP fixture accept failed: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("bound SMTP reads");
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .expect("bound SMTP writes");
        let mut writer = stream.try_clone().expect("clone SMTP stream");
        let mut reader = BufReader::new(stream);
        writer
            .write_all(b"220 localhost ESMTP\r\n")
            .expect("SMTP greeting");
        let mut capture = SmtpCapture {
            commands: Vec::new(),
            data: Vec::new(),
        };
        for _ in 0..32 {
            let mut bytes = Vec::new();
            assert!(reader.read_until(b'\n', &mut bytes).expect("SMTP command") > 0);
            let line = String::from_utf8_lossy(&bytes)
                .trim_end_matches(['\r', '\n'])
                .to_owned();
            let upper = line.to_ascii_uppercase();
            if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                capture.commands.push(line);
                writer
                    .write_all(b"250-localhost\r\n250 AUTH PLAIN\r\n")
                    .expect("SMTP capabilities");
            } else if upper.starts_with("AUTH PLAIN") {
                capture.commands.push("AUTH PLAIN <redacted>".to_owned());
                if line.split_whitespace().count() == 2 {
                    writer.write_all(b"334 \r\n").expect("SMTP auth challenge");
                    let mut response = Vec::new();
                    assert!(
                        reader
                            .read_until(b'\n', &mut response)
                            .expect("SMTP auth response")
                            > 0
                    );
                }
                writer
                    .write_all(b"235 authenticated\r\n")
                    .expect("SMTP auth");
            } else if upper.starts_with("MAIL FROM:") || upper.starts_with("RCPT TO:") {
                capture.commands.push(line);
                writer.write_all(b"250 OK\r\n").expect("SMTP envelope");
            } else if upper == "DATA" {
                capture.commands.push(line);
                writer.write_all(b"354 send data\r\n").expect("SMTP DATA");
                loop {
                    let mut data_line = Vec::new();
                    assert!(
                        reader
                            .read_until(b'\n', &mut data_line)
                            .expect("SMTP message data")
                            > 0
                    );
                    if data_line == b".\r\n" || data_line == b".\n" {
                        break;
                    }
                    assert!(
                        capture.data.len().saturating_add(data_line.len())
                            <= backend::curl_backend::MAX_SMTP_MESSAGE_BYTES
                    );
                    capture.data.extend_from_slice(&data_line);
                }
                if accept_message {
                    writer.write_all(b"250 accepted\r\n").expect("SMTP accept");
                } else {
                    writer
                        .write_all(b"550 generated rejection\r\n")
                        .expect("SMTP reject");
                    break;
                }
            } else if upper == "QUIT" {
                capture.commands.push(line);
                writer.write_all(b"221 bye\r\n").expect("SMTP quit");
                break;
            } else {
                panic!("unexpected SMTP command: {line}");
            }
        }
        sender.send(capture).expect("publish SMTP capture");
    });
    (port, receiver, worker)
}

pub(crate) fn apply_task(state: &mut State, task: iced::Task<Message>) -> usize {
    apply_task_with_local_failures(state, task, false)
}

fn apply_close_task(state: &mut State, task: iced::Task<Message>) -> usize {
    let mut closes = apply_task(state, task);
    let give_up = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while closes == 0 && state.close_settlement.is_some() && std::time::Instant::now() < give_up {
        std::thread::sleep(std::time::Duration::from_millis(20));
        let tick = state.advance_close_settlement(std::time::Instant::now());
        closes += apply_task(state, tick);
    }
    closes
}

fn apply_task_allowing_local_failure(state: &mut State, task: iced::Task<Message>) -> usize {
    apply_task_with_local_failures(state, task, true)
}

fn apply_task_wave(state: &mut State, task: iced::Task<Message>) -> Vec<iced::Task<Message>> {
    let mut followups = Vec::new();
    if let Some(mut stream) = iced_test::runtime::task::into_stream(task) {
        loop {
            use iced_test::futures::futures::StreamExt;
            let Some(action) = iced_test::futures::futures::executor::block_on(stream.next())
            else {
                break;
            };
            if let iced_test::runtime::Action::Output(message) = action {
                followups.push(state.update(message));
            }
        }
    }
    followups
}

fn settle_credential_journal(
    state: &mut State,
    commit: iced::Task<Message>,
) -> iced::Task<Message> {
    assert!(matches!(
        state.account_edit.as_ref().map(|edit| &edit.phase),
        Some(AccountEditPhase::AwaitingJournal(_))
    ));
    let followups = apply_task_wave(state, commit);
    assert!(!matches!(
        state.account_edit.as_ref().map(|edit| &edit.phase),
        Some(AccountEditPhase::AwaitingJournal(_))
    ));
    iced::Task::batch(followups)
}

fn apply_task_with_local_failures(
    state: &mut State,
    task: iced::Task<Message>,
    allow_local_failure: bool,
) -> usize {
    let mut pending = std::collections::VecDeque::from([task]);
    let mut closes = 0;
    for _ in 0..32 {
        let Some(task) = pending.pop_front() else {
            return closes;
        };
        if let Some(mut stream) = iced_test::runtime::task::into_stream(task) {
            loop {
                use iced_test::futures::futures::StreamExt;
                let Some(action) = iced_test::futures::futures::executor::block_on(stream.next())
                else {
                    break;
                };
                match action {
                    iced_test::runtime::Action::Output(message) => {
                        if !allow_local_failure
                            && let Message::LocalStateSaved(_, Err(error)) = &message
                        {
                            panic!("local-state write failed: {error}");
                        }
                        pending.push_back(state.update(message));
                    }
                    iced_test::runtime::Action::Window(
                        iced_test::runtime::window::Action::Close(_),
                    ) => closes += 1,
                    _ => {}
                }
            }
        }
    }
    panic!("task chain exceeded the bounded 32-step fixture limit");
}

#[derive(Debug)]
struct ManualLocalClock {
    current:
        std::sync::Mutex<Result<MessageDate, enzui_iced_core::local_time::LocalCivilTimeError>>,
}

impl ManualLocalClock {
    fn new(now: Result<MessageDate, enzui_iced_core::local_time::LocalCivilTimeError>) -> Self {
        Self {
            current: std::sync::Mutex::new(now),
        }
    }

    fn set(&self, now: Result<MessageDate, enzui_iced_core::local_time::LocalCivilTimeError>) {
        *self.current.lock().expect("manual clock lock") = now;
    }
}

impl crate::local_clock::LocalClock for ManualLocalClock {
    fn now(&self) -> Result<MessageDate, enzui_iced_core::local_time::LocalCivilTimeError> {
        *self.current.lock().map_err(|_| {
            enzui_iced_core::local_time::LocalCivilTimeError::TimezoneConversionFailed
        })?
    }
}

#[derive(Debug)]
struct PausedCloseWrite {
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

#[derive(Clone, Debug, Default)]
struct CloseRefusingBackend {
    saves: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pause: std::sync::Arc<std::sync::Mutex<Option<PausedCloseWrite>>>,
}

impl enzui_iced_core::persist::PersistBackend for CloseRefusingBackend {
    fn load_raw(&self, _format_id: &str, _version: u32) -> enzui_iced_core::persist::BackendLoad {
        enzui_iced_core::persist::BackendLoad::Absent
    }

    fn save_raw(
        &self,
        _format_id: &str,
        _version: u32,
        _body: &str,
    ) -> Result<(), enzui_iced_core::persist::PersistError> {
        self.saves
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let pause = self
            .pause
            .lock()
            .map_err(|_| enzui_iced_core::persist::PersistError::new("fixture pause lock failed"))?
            .take();
        if let Some(pause) = pause {
            pause.entered.send(()).map_err(|_| {
                enzui_iced_core::persist::PersistError::new("fixture pause channel closed")
            })?;
            let _ = pause
                .release
                .recv_timeout(std::time::Duration::from_secs(5));
        }
        Err(enzui_iced_core::persist::PersistError::new(
            "close fixture refusal",
        ))
    }
}

fn expire_pending_send(s: &mut State) {
    let (deadline, _) = s.undo_send_expiry.armed().expect("undo wake is armed");
    let _ = s.update(Message::Toast(ToastEvent::Tick(deadline)));
}

fn sent_count(s: &State) -> usize {
    s.store
        .messages
        .iter()
        .filter(|m| matches!(m.location, MessageLocation::Mailbox(MailboxKind::Sent)))
        .count()
}

#[path = "reducer_tests/account_edit_tests.rs"]
mod account_edit_tests;
#[path = "reducer_tests/account_outbox_tests.rs"]
mod account_outbox_tests;
#[path = "reducer_tests/account_tests.rs"]
mod account_tests;

#[path = "reducer_tests/close_tests.rs"]
mod close_tests;
#[path = "reducer_tests/compose_tests.rs"]
mod compose_tests;
#[path = "reducer_tests/inbox_push_tests.rs"]
mod inbox_push_tests;
#[path = "reducer_tests/quick_reply_tests.rs"]
mod quick_reply_tests;
#[path = "reducer_tests/send_later_tests.rs"]
mod send_later_tests;
#[path = "reducer_tests/send_tests.rs"]
mod send_tests;
#[path = "reducer_tests/server_search_tests.rs"]
mod server_search_tests;
#[path = "reducer_tests/sync_engine_tests.rs"]
mod sync_engine_tests;
#[path = "reducer_tests/sync_tests.rs"]
mod sync_tests;
#[path = "reducer_tests/undo_move_tests.rs"]
mod undo_move_tests;

mod mailbox_tests;
