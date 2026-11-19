use crate::{Message, State};
use enzui_iced_core::civil_date::{Date, DateBounds};
use enzui_iced_core::modal::DismissCause;
use enzui_iced_core::time_of_day::{HourCycle, TimeFormat, TimeOfDay};
use enzui_iced_widgets::date_picker::{DatePickerEvent, DatePickerState};
use enzui_iced_widgets::modal_host::{ModalFocusLifecycle, ModalHostEvent};
use enzui_iced_widgets::time_field::TimeFieldEvent;
use iced::Task;

pub(crate) struct ScheduleEditor {
    pub(crate) date: Date,
    pub(crate) picker: DatePickerState,
    pub(crate) time: TimeOfDay,
    pub(crate) draft: String,
    pub(crate) error: Option<String>,
    pub(crate) message_id: Option<String>,
    pub(crate) focus: ModalFocusLifecycle,
    pub(crate) allow_missing_attachment: bool,
}

pub(crate) fn time_format() -> TimeFormat {
    crate::l10n::current()
        .time_format()
        .unwrap_or_else(|_| TimeFormat::new(HourCycle::H23))
}

impl State {
    pub(crate) fn open_schedule(
        &mut self,
        at: crate::model::types::MessageDate,
        message_id: Option<String>,
    ) -> Task<Message> {
        let Some(date) = i32::try_from(at.year)
            .ok()
            .and_then(|year| Date::new(year, at.month, at.day))
        else {
            return Task::none();
        };
        let Some(time) = TimeOfDay::hm(at.minute / 60, at.minute % 60) else {
            return Task::none();
        };
        let (focus, operation) =
            ModalFocusLifecycle::open::<Message>("mail-schedule-presentation", None);
        self.schedule_editor = Some(ScheduleEditor {
            date,
            picker: DatePickerState::new(
                date,
                crate::l10n::current()
                    .locale_for(enzui_iced_core::localization::LocaleCategory::Time)
                    .first_weekday()
                    .unwrap_or(enzui_iced_core::civil_date::Weekday::Monday),
                DateBounds::unbounded(),
            ),
            time,
            draft: time_format().format(time),
            error: None,
            message_id,
            focus,
            allow_missing_attachment: false,
        });
        iced::advanced::widget::operate(operation)
    }

    pub(crate) fn schedule_event(&mut self, event: ModalHostEvent) -> Task<Message> {
        let Some(editor) = self.schedule_editor.as_ref() else {
            return Task::none();
        };
        let outcome = editor.focus.reduce::<Message>(event);
        if outcome.dismissed().is_some() {
            self.schedule_editor = None;
        }
        outcome.into_task()
    }

    pub(crate) fn schedule_date(&mut self, event: DatePickerEvent) -> Task<Message> {
        let Some(editor) = self.schedule_editor.as_mut() else {
            return Task::none();
        };
        let focus = match event {
            DatePickerEvent::TriggerPressed => editor
                .picker
                .toggle(Some(editor.date), Some(editor.picker.trigger_id())),
            DatePickerEvent::Calendar(event) => {
                let (selected, focus) = editor.picker.apply_calendar_event_and_close(event);
                if let Some(date) = selected {
                    editor.date = date;
                    editor.error = None;
                }
                focus
            }
            DatePickerEvent::Dismissed => editor.picker.close(),
        };
        focus.map_or_else(Task::none, iced::widget::operation::focus)
    }

    pub(crate) fn schedule_time(&mut self, event: TimeFieldEvent) {
        let Some(editor) = self.schedule_editor.as_mut() else {
            return;
        };
        match event {
            TimeFieldEvent::Drafted(draft) => {
                editor.draft = draft;
                editor.error = None;
            }
            TimeFieldEvent::Committed(time) => {
                editor.time = time;
                editor.draft = time_format().format(time);
                editor.error = None;
            }
            TimeFieldEvent::Refused(_) => {
                editor.error = Some(crate::l10n::message(
                    "mail-valid-time-example",
                    &[(
                        "time",
                        time_format().format(TimeOfDay::hm(9, 30).expect("valid example time")),
                    )],
                ));
            }
        }
    }

    pub(crate) fn confirm_schedule(&mut self) -> Task<Message> {
        let Some(editor) = self.schedule_editor.as_mut() else {
            return Task::none();
        };
        let Ok(time) = time_format().parse(&editor.draft) else {
            editor.error = Some(crate::l10n::message(
                "mail-valid-time-example",
                &[(
                    "time",
                    time_format().format(TimeOfDay::hm(9, 30).expect("valid example time")),
                )],
            ));
            return Task::none();
        };
        let Ok(year) = u32::try_from(editor.date.year()) else {
            editor.error = Some(crate::l10n::tr("Choose a date and time in the future."));
            return Task::none();
        };
        let at = crate::model::types::MessageDate {
            year,
            month: editor.date.month(),
            day: editor.date.day(),
            minute: time.hour() * 60 + time.minute(),
        };
        let Ok(now) = self.local_clock.now() else {
            editor.error = Some(crate::l10n::tr(
                "Could not read the local clock. Try again.",
            ));
            return Task::none();
        };
        if at.year < 1 || crate::model::snooze::minutes_until(now, at) <= 0 {
            editor.error = Some(crate::l10n::tr("Choose a date and time in the future."));
            return Task::none();
        }
        if editor.message_id.is_none()
            && self.compose.mentions_missing_attachment()
            && !editor.allow_missing_attachment
        {
            editor.error = Some(crate::l10n::tr(
                "The message mentions an attachment, but no file is attached.",
            ));
            return Task::none();
        }
        let existing = editor.message_id.clone();
        if existing.is_none()
            && let Some(error) = self.compose_attachment_failure()
        {
            if let Some(editor) = self.schedule_editor.as_mut() {
                editor.error = Some(error.user_message().to_owned());
            }
            return Task::none();
        }
        let task = if let Some(id) = existing {
            let Some(message) = self
                .store
                .messages
                .iter_mut()
                .find(|m| m.id == id && m.send_at.is_some())
            else {
                return Task::none();
            };
            message.send_at = Some(at);
            self.store.mark_dirty();
            Task::none()
        } else {
            if !self.compose.open || !self.compose.can_send() {
                return Task::none();
            }
            self.schedule_send_at(at)
        };
        if self.compose.open
            && self
                .schedule_editor
                .as_ref()
                .is_some_and(|editor| editor.message_id.is_none())
        {
            return task;
        }
        let dismiss = self.schedule_event(ModalHostEvent::Dismissed(DismissCause::DefaultAction));
        Task::batch([task, dismiss])
    }
}

pub(crate) fn with_schedule<'a>(
    state: &'a State,
    base: iced::Element<'a, Message>,
) -> iced::Element<'a, Message> {
    use enzui_iced_widgets::button::{ButtonKind, ButtonSize};
    use enzui_iced_widgets::date_picker::enz_date_picker;
    use enzui_iced_widgets::modal_host::enz_modal_host;
    use enzui_iced_widgets::text::TextSize;
    use enzui_iced_widgets::time_field::enz_time_field;
    use enzui_iced_widgets::{enz_button, enz_modal_surface, enz_text};
    use iced::widget::{Space, column, row};
    use iced::{Element, Length};
    let Some(editor) = state.schedule_editor.as_ref() else {
        return base;
    };
    let sp = state.themes.theme().spacing;
    let title = if editor.message_id.is_some() {
        "Change send time"
    } else {
        "Schedule send"
    };
    let mut content = column![
        enz_text(crate::l10n::tr(title)).size(TextSize::Title3),
        enz_text(crate::l10n::tr("Local time. Mail must be running and connected to send; otherwise it sends when available.")).size(TextSize::Footnote),
        enzui_iced_widgets::enz_combo_box(crate::l10n::tr("Quick choices"), std::iter::once(crate::l10n::tr("Custom")).chain(crate::model::snooze::SnoozePreset::ALL.into_iter().map(|p| crate::l10n::tr(p.label()))).collect(), None, Message::ScheduleChoice)
            .id(iced::advanced::widget::Id::new("mail-schedule-choice"))
            .selected_index(Some(schedule_choice_index(state, editor)))
            .on_select_index(|index| Message::ScheduleChoice(index.checked_sub(1).and_then(|index| crate::model::snooze::SnoozePreset::ALL.get(index)).map_or_else(|| "Custom".to_owned(), |preset| preset.label().to_owned()))),
        enz_date_picker(Some(editor.date), &editor.picker, Message::ScheduleDate).label(crate::l10n::tr("Date")).width(Length::Fill),
        enz_time_field(&editor.draft, editor.time, Message::ScheduleTime).format(time_format()).label(crate::l10n::tr("Time")),
    ].spacing(sp.md());
    if editor.message_id.is_none() && state.compose.mentions_missing_attachment() {
        content = content.push(enzui_iced_widgets::enz_checkbox(
            editor.allow_missing_attachment,
            crate::l10n::tr("Schedule without an attachment"),
            Message::ScheduleAllowMissingAttachment,
        ));
    }
    if let Some(error) = &editor.error {
        content = content.push(
            enz_text(error)
                .size(TextSize::Footnote)
                .color(state.themes.theme().palette.error),
        );
    }
    content = content.push(
        row![
            Space::new().width(Length::Fill),
            enz_button(
                crate::l10n::tr("Cancel"),
                Message::ScheduleDialog(ModalHostEvent::Dismissed(DismissCause::CloseButton))
            )
            .kind(ButtonKind::Quiet)
            .size(ButtonSize::Sm),
            enz_button(crate::l10n::tr("Schedule"), Message::ConfirmSchedule)
                .id(iced::advanced::widget::Id::new("mail-schedule-confirm"))
                .kind(ButtonKind::Primary)
                .size(ButtonSize::Sm),
        ]
        .mirrored(crate::l10n::rtl())
        .spacing(sp.sm()),
    );
    let dialog: Element<Message> = enz_modal_surface(content.into())
        .width(Length::Fixed(
            state
                .themes
                .theme()
                .sizing
                .dialog_width_sm
                .min((state.window_size.width - 24.0).max(240.0)),
        ))
        .into();
    enz_modal_host(base, Some(dialog), Message::ScheduleDialog)
        .label(crate::l10n::tr(title))
        .scoped_to(&editor.focus)
        .into()
}

fn schedule_choice_index(state: &State, editor: &ScheduleEditor) -> usize {
    let label = schedule_choice_label(state, editor);
    crate::model::snooze::SnoozePreset::ALL
        .iter()
        .position(|preset| preset.label() == label)
        .map_or(0, |index| index + 1)
}

fn schedule_choice_label(state: &State, editor: &ScheduleEditor) -> String {
    if let Ok(now) = state.local_clock.now() {
        for preset in crate::model::snooze::SnoozePreset::ALL {
            let at = preset.until(now);
            if editor.date.year() == at.year as i32
                && editor.date.month() == at.month
                && editor.date.day() == at.day
                && editor.draft
                    == time_format().format(
                        TimeOfDay::hm(at.minute / 60, at.minute % 60).unwrap_or(editor.time),
                    )
            {
                return preset.label().to_owned();
            }
        }
    }
    "Custom".into()
}
