//! Optional approved-agent worker sharing the desktop state and persistence.
//! Preparing a message presents a composer; only human confirmation sends mail.

use std::sync::Arc;
use std::time::{Duration, Instant};

use enzui_iced_agent::center::background::Disposition;
use enzui_iced_agent::center::task_host::{TaskHost, TaskHostError};
use enzui_iced_agent_app::{
    AgentProcess, ApplicationIntent, BackgroundApp, BackgroundExit, DEFAULT_IDLE_EXIT, LaunchMode,
};

use super::*;

pub(crate) use enzui_iced_agent_app::DesktopClaim;

pub(crate) const IDLE_EXIT: Duration = DEFAULT_IDLE_EXIT;

pub(crate) fn launch_mode() -> Result<LaunchMode, String> {
    LaunchMode::from_args(std::env::args_os().skip(1)).map_err(|error| error.to_string())
}

pub(crate) fn export_manifest() -> std::process::ExitCode {
    enzui_iced_agent_app::print_manifest(&crate::center::registry(), crate::center::metadata())
}

pub(crate) fn claim_for_gui(
    intent: ApplicationIntent,
) -> Result<DesktopClaim, enzui_iced_agent_app::ApplicationServiceError> {
    enzui_iced_agent_app::claim_desktop(crate::center::CENTER_APP_ID, intent)
}

pub(crate) fn run_background() -> Result<Option<State>, String> {
    let Some(service) = enzui_iced_agent_app::claim_background(crate::center::CENTER_APP_ID)?
    else {
        return Ok(None);
    };
    let tasks =
        Arc::new(TaskHost::spawn().map_err(|error| format!("task host did not start: {error}"))?);
    // The ordinary boot; its theme task waits for a window that may never open.
    // Without a connector the worker still serves its desktop launch and refuses
    // agents.
    let (mut state, boot) = State::boot();
    state.agent = AgentProcess::windowless(Some(service), Arc::clone(&tasks));
    state.serve_desktop_search();
    match enzui_iced_agent_app::run_background(Background(state), boot, &tasks, IDLE_EXIT)
        .map_err(|error| error.to_string())?
    {
        BackgroundExit::Present(Background(state)) => Ok(Some(state)),
        BackgroundExit::Idle(Background(mut state))
        | BackgroundExit::Exit(Background(mut state)) => {
            state.shutdown_center();
            Ok(None)
        }
    }
}

impl State {
    pub(crate) fn present_task(&mut self) -> Task<Message> {
        let (_launch, deferred) = self.agent.begin_presenting();
        let focus = if self.compose.open {
            self.arm_compose_focus()
        } else {
            Task::none()
        };
        Task::batch([deferred, focus, self.desktop_pending()])
    }

    pub(crate) fn present_for_review(&mut self) -> Task<Message> {
        if self.agent.is_windowless() {
            self.agent.request_present(ApplicationIntent::Activate);
            Task::none()
        } else {
            iced::window::latest().and_then(iced::window::gain_focus)
        }
    }

    pub(crate) fn application_wake(&mut self) -> Task<Message> {
        for call in self.agent.take_application_calls() {
            self.desktop.enqueue_call(call);
        }
        for activation in self.agent.take_searches() {
            self.desktop.enqueue_search_activation(activation);
        }
        self.desktop_pending()
    }

    pub(crate) fn agent_subscription(&self) -> Subscription<Message> {
        self.agent
            .subscription(|_| Message::ApplicationWake, Message::BackgroundTaskRefused)
    }

    pub(crate) fn center_deadline(&mut self) -> Task<Message> {
        if let Some(host) = self.center.as_mut() {
            let ended = host.poll_ended();
            self.waiting_center_calls
                .retain(|(call, _)| !ended.iter().any(|ended| ended.call == call.reference()));
        }
        Task::none()
    }
}

struct Background(State);

impl std::fmt::Debug for Background {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Background").field(&self.0.agent).finish()
    }
}

impl BackgroundApp for Background {
    type Message = Message;

    fn update(&mut self, message: Message) -> Task<Message> {
        self.0.update(message)
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([self.0.center_subscription(), self.0.agent_subscription()])
    }

    fn disposition(&self) -> Disposition {
        let state = &self.0;
        enzui_iced_agent_app::disposition(
            state.center.as_ref(),
            &state.agent,
            state.startup.pending(),
        )
    }

    fn wake_at(&self) -> Option<Instant> {
        self.0
            .center
            .as_ref()
            .and_then(enzui_iced_agent_app::deadline_instant)
    }

    fn wake(&mut self) -> Task<Message> {
        self.0.center_deadline()
    }

    fn refused(&mut self, error: TaskHostError) -> Task<Message> {
        self.0
            .update(Message::BackgroundTaskRefused(error.to_string()))
    }
}
