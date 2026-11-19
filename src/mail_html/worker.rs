use super::*;
use enzui_iced_core::media::{Generation, MediaWorker};

pub(super) struct EventSender {
    events: SyncSender<Event>,
    wake: UnixStream,
}

impl EventSender {
    pub(super) fn try_send(&self, event: Event) -> Result<(), mpsc::TrySendError<Event>> {
        self.events.try_send(event)?;
        notify(&self.wake);
        Ok(())
    }
}

pub(in crate::mail_html) struct Worker {
    completed: UnixStream,
    wake: Arc<UnixStream>,
    cancel_input: Arc<AtomicBool>,
    thread: MediaWorker,
}

fn notify(mut socket: &UnixStream) {
    let _ = socket.write(&[1]);
}

impl Worker {
    pub(in crate::mail_html) fn new(
        requests: Receiver<Command>,
        events: SyncSender<Event>,
    ) -> Result<Self, ()> {
        let (read, write) = UnixStream::pair().map_err(|_| ())?;
        read.set_nonblocking(true).map_err(|_| ())?;
        write.set_nonblocking(true).map_err(|_| ())?;
        let wake = Arc::new(write);
        let (completed, outgoing) = UnixStream::pair().map_err(|_| ())?;
        completed.set_nonblocking(true).map_err(|_| ())?;
        outgoing.set_nonblocking(true).map_err(|_| ())?;
        let cancel_input = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::clone(&cancel_input);
        let thread_wake = Arc::clone(&wake);
        let thread = MediaWorker::spawn(Generation::from_raw(1), move |stop, _| {
            // Blitz documents are not Send; their entire lifetime stays on this thread.
            let mut host = NativeHost::new(
                requests,
                EventSender {
                    events,
                    wake: outgoing,
                },
                (read, thread_wake),
            );
            let mut iterations = 0;
            while !stop.should_stop() {
                host.advance(cancelled.swap(false, Ordering::AcqRel));
                iterations += 1;
                if stop.should_stop() {
                    break;
                }
                let Some((read, _)) = &mut host.wake else {
                    break;
                };
                if read.set_nonblocking(false).is_err() {
                    break;
                }
                let mut buffer = [0; 256];
                let result = read.read(&mut buffer);
                if read.set_nonblocking(true).is_err() || !result.is_ok_and(|count| count > 0) {
                    break;
                }
            }
            iterations
        })
        .map_err(|_| ())?;
        Ok(Self {
            completed,
            wake,
            cancel_input,
            thread,
        })
    }

    pub(in crate::mail_html) fn wake(&self) {
        notify(&self.wake);
    }

    pub(in crate::mail_html) fn cancel_input(&self) {
        // A saturated queue must not leave a pressed or focused document behind.
        self.cancel_input.store(true, Ordering::Release);
    }

    pub(in crate::mail_html) fn drain_wake(&mut self) {
        let mut buffer = [0; 256];
        while self
            .completed
            .read(&mut buffer)
            .is_ok_and(|count| count > 0)
        {}
    }

    pub(in crate::mail_html) fn interest(&self) -> enzui_iced_core::native_loop::Interest {
        enzui_iced_core::native_loop::Interest::new().readable(self.completed.as_raw_fd())
    }

    pub(in crate::mail_html) fn is_finished(&self) -> bool {
        self.thread.is_thread_finished()
    }

    pub(in crate::mail_html) fn poll_interval(&self) -> Duration {
        Duration::from_millis(100)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.thread.begin_shutdown();
        self.wake();
    }
}
