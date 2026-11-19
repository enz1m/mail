use crate::{Message, State};
use enzui_iced_core::modal::DismissCause;
use enzui_iced_widgets::modal_host::{ModalFocusLifecycle, ModalHostEvent};
use enzui_iced_widgets::settings_dialog::SettingsDialogEvent;
use iced::Task;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuleDialog {
    List,
    Editor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MessageDialog {
    Move,
    Snooze,
}

pub(crate) fn boot_focus(
    open: bool,
    scope_key: &str,
    invoker: Option<iced::advanced::widget::Id>,
) -> (Option<ModalFocusLifecycle>, Task<Message>) {
    if !open {
        return (None, Task::none());
    }
    let (lifecycle, operation) = ModalFocusLifecycle::open::<Message>(scope_key, invoker);
    (Some(lifecycle), iced::advanced::widget::operate(operation))
}

impl State {
    pub(crate) fn open_rules_focus(&mut self) -> Task<Message> {
        self.rules_ui.open();
        self.rule_dialogs.open(
            RuleDialog::List,
            Some(crate::settings_view::rules_invoker_id()),
        )
    }

    pub(crate) fn open_rule_editor_focus(
        &mut self,
        invoker: iced::advanced::widget::Id,
    ) -> Task<Message> {
        self.rule_dialogs.open(RuleDialog::Editor, Some(invoker))
    }

    fn forget_rule_dialogs(&mut self, closed: Vec<RuleDialog>) {
        for key in closed {
            match key {
                RuleDialog::List => self.rules_ui.close(),
                RuleDialog::Editor => self.rules_ui.cancel_edit(),
            }
            self.last_rules_result = None;
            self.rule_identity_notice = None;
        }
    }

    pub(crate) fn dismiss_rules_focus(&mut self, cause: DismissCause) -> Task<Message> {
        let (closed, focus) = self.rule_dialogs.dismiss(&RuleDialog::List, cause);
        self.forget_rule_dialogs(closed);
        focus
    }

    pub(crate) fn save_rule_from_editor(&mut self) -> Task<Message> {
        let Some(draft) = self.rules_ui.draft.clone() else {
            return Task::none();
        };
        let mut updated = self.rules.get().clone();
        if updated.commit_draft(draft).is_err() {
            self.rule_identity_notice = Some(crate::RULE_ID_CONFLICT_NOTICE);
            return Task::none();
        }
        self.rules.set(updated);
        self.rules_ui.draft = None;
        self.last_rules_result = None;
        self.rule_identity_notice = None;
        self.refresh_rules_count();
        let save = self.save_preferences(crate::preferences::SaveOrigin::Rules);
        let restore = self.dismiss_rule_editor_focus(DismissCause::CloseButton);
        Task::batch([save, restore])
    }

    pub(crate) fn dismiss_rule_editor_focus(&mut self, cause: DismissCause) -> Task<Message> {
        let (closed, focus) = self.rule_dialogs.dismiss(&RuleDialog::Editor, cause);
        self.forget_rule_dialogs(closed);
        focus
    }

    pub(crate) fn reduce_rules_host(
        &mut self,
        event: enzui_iced_widgets::modal_host::DialogEvent<RuleDialog>,
    ) -> Task<Message> {
        let (closed, focus) = self.rule_dialogs.update(event);
        self.forget_rule_dialogs(closed);
        focus
    }

    pub(crate) fn arm_compose_focus(&mut self) -> Task<Message> {
        self.compose_focus = None;
        iced::widget::operation::focus(crate::compose_view::compose_body_id())
    }

    // Tear down lifecycles whose host disappeared without a dismissal event,
    // without restoring an invoker.
    pub(crate) fn close_missing_modal_hosts(&mut self) -> Option<Task<Message>> {
        let mut closes = Vec::new();
        if !self.compose.open
            && let Some(lifecycle) = self.compose_focus.take()
        {
            closes.push(lifecycle.close_for_host::<Message>().into_task());
        }
        if self.pending_move_targets.is_empty() {
            self.message_dialogs.close(&MessageDialog::Move);
        }
        if self.pending_snooze_targets.is_empty() {
            self.message_dialogs.close(&MessageDialog::Snooze);
        }
        if !self.settings.open
            && let Some(lifecycle) = self.settings_focus.take()
        {
            closes.push(lifecycle.close_for_host::<Message>().into_task());
        }
        if !self.shortcuts_open
            && let Some(lifecycle) = self.shortcuts_focus.take()
        {
            closes.push(lifecycle.close_for_host::<Message>().into_task());
        }
        if !self.rules_ui.open {
            self.rule_dialogs.close(&RuleDialog::List);
        } else if self.rules_ui.draft.is_none() {
            self.rule_dialogs.close(&RuleDialog::Editor);
        }
        let policy = if self.rules_ui.pending_delete.is_some() {
            enzui_iced_widgets::modal_host::alert_dismiss_policy()
        } else {
            enzui_iced_widgets::modal_host::modal_dismiss_policy()
        };
        self.rule_dialogs.set_policy(&RuleDialog::List, policy);
        self.rule_dialogs.set_policy(&RuleDialog::Editor, policy);
        if self.bulk.pending_empty.is_none()
            && let Some(lifecycle) = self.empty_confirm_focus.take()
        {
            closes.push(lifecycle.close_for_host::<Message>().into_task());
        }
        (!closes.is_empty()).then(|| Task::batch(closes))
    }

    pub(crate) fn toggle_shortcuts_focus(&mut self) -> Task<Message> {
        if self.shortcuts_open {
            let (lifecycle, op) =
                ModalFocusLifecycle::open::<Message>(crate::view::SHORTCUTS_SCOPE_KEY, None);
            self.shortcuts_focus = Some(lifecycle);
            iced::advanced::widget::operate(op)
        } else {
            self.shortcuts_focus
                .take()
                .map_or_else(Task::none, |lifecycle| {
                    lifecycle
                        .reduce::<Message>(ModalHostEvent::Dismissed(DismissCause::CloseButton))
                        .into_task()
                })
        }
    }

    pub(crate) fn reduce_shortcuts_host(
        &mut self,
        event: enzui_iced_widgets::shortcuts_dialog::ShortcutsDialogEvent,
    ) -> Task<Message> {
        use enzui_iced_widgets::shortcuts_dialog::ShortcutsDialogEvent;
        let host = match event {
            ShortcutsDialogEvent::FocusNext => ModalHostEvent::FocusNext,
            ShortcutsDialogEvent::FocusPrevious => ModalHostEvent::FocusPrevious,
            ShortcutsDialogEvent::Dismissed(cause) => ModalHostEvent::Dismissed(cause),
        };
        let Some(lifecycle) = self.shortcuts_focus.as_ref() else {
            return Task::none();
        };
        let outcome = lifecycle.reduce::<Message>(host);
        if outcome.dismissed().is_some() {
            self.shortcuts_open = false;
            self.shortcuts_focus = None;
        }
        outcome.into_task()
    }

    // Compose closes from Cancel, Send or Save Draft still reduce a real
    // Dismissed event through the lifecycle, so teardown cannot be bypassed.
    pub(crate) fn dismiss_compose_focus(&mut self, cause: DismissCause) -> Task<Message> {
        self.attach_chooser
            .cancel(enzui_iced_widgets::file_chooser::CancelReason::Application);
        let Some(lifecycle) = self.compose_focus.take() else {
            return Task::none();
        };
        lifecycle
            .reduce::<Message>(ModalHostEvent::Dismissed(cause))
            .into_task()
    }

    pub(crate) fn arm_empty_confirm_focus(&mut self) -> Task<Message> {
        let (lifecycle, open_op) = ModalFocusLifecycle::open::<Message>(
            crate::bulk_view::EMPTY_CONFIRM_SCOPE_KEY,
            Some(crate::bulk_view::mailbox_actions_invoker_id()),
        );
        self.empty_confirm_focus = Some(lifecycle);
        iced::advanced::widget::operate(open_op)
    }

    pub(crate) fn reduce_empty_confirm_host(&mut self, event: ModalHostEvent) -> Task<Message> {
        let Some(lifecycle) = self.empty_confirm_focus.as_ref() else {
            return Task::none();
        };
        let outcome = lifecycle.reduce::<Message>(event);
        if outcome.dismissed().is_some() {
            self.empty_confirm_focus = None;
        }
        outcome.into_task()
    }

    pub(crate) fn arm_move_dialog_focus(&mut self) -> Task<Message> {
        self.message_dialogs.open(MessageDialog::Move, None)
    }

    pub(crate) fn dismiss_move_dialog_focus(&mut self, cause: DismissCause) -> Task<Message> {
        self.message_dialogs.dismiss(&MessageDialog::Move, cause).1
    }

    pub(crate) fn arm_snooze_dialog_focus(&mut self) -> Task<Message> {
        self.message_dialogs.open(MessageDialog::Snooze, None)
    }

    pub(crate) fn dismiss_snooze_dialog_focus(&mut self, cause: DismissCause) -> Task<Message> {
        self.message_dialogs
            .dismiss(&MessageDialog::Snooze, cause)
            .1
    }

    pub(crate) fn reduce_message_dialog(
        &mut self,
        event: enzui_iced_widgets::modal_host::DialogEvent<MessageDialog>,
    ) -> Task<Message> {
        let (closed, focus) = self.message_dialogs.update(event);
        for key in closed {
            match key {
                MessageDialog::Move => self.pending_move_targets.clear(),
                MessageDialog::Snooze => self.pending_snooze_targets.clear(),
            }
        }
        focus
    }

    pub(crate) fn arm_settings_focus(&mut self) -> Task<Message> {
        let (lifecycle, open_op) = ModalFocusLifecycle::open::<Message>(
            crate::settings_view::SETTINGS_SCOPE_KEY,
            Some(if self.titlebar_overflow_open {
                crate::view::shell::titlebar_overflow_id()
            } else {
                crate::view::settings_button_id()
            }),
        );
        self.settings_focus = Some(lifecycle);
        iced::advanced::widget::operate(open_op)
    }

    pub(crate) fn dismiss_settings_focus(&mut self, cause: DismissCause) -> Task<Message> {
        if !self.settings.open {
            return Task::none();
        }
        let Some(lifecycle) = self.settings_focus.take() else {
            return Task::none();
        };
        if self.account_edit.is_none() {
            self.settings.edit_account.reset();
        }
        self.settings.close();
        // The Settings scrim has no lasting focus target, so its pointer-driven
        // dismissals restore the sole invoker rather than retaining modal focus.
        let restore_cause = if cause.is_pointer_ambient() {
            DismissCause::CloseButton
        } else {
            cause
        };
        lifecycle
            .reduce::<Message>(ModalHostEvent::Dismissed(restore_cause))
            .into_task()
    }

    pub(crate) fn reduce_settings_host(&mut self, event: ModalHostEvent) -> Task<Message> {
        if let ModalHostEvent::Dismissed(cause) = event {
            return self.dismiss_settings_focus(cause);
        }
        let Some(lifecycle) = self.settings_focus.as_ref() else {
            return Task::none();
        };
        lifecycle.reduce::<Message>(event).into_task()
    }

    pub(crate) fn reduce_settings_dialog_event(
        &mut self,
        event: SettingsDialogEvent,
    ) -> Task<Message> {
        let host_event = match event {
            SettingsDialogEvent::FocusNext => ModalHostEvent::FocusNext,
            SettingsDialogEvent::FocusPrevious => ModalHostEvent::FocusPrevious,
            SettingsDialogEvent::Dismissed(cause) => ModalHostEvent::Dismissed(cause),
        };
        self.reduce_settings_host(host_event)
    }
}
