use enzui_iced_core::local_time::LocalCivilTimeError;

use crate::model::types::MessageDate;

pub(crate) trait LocalClock: Send + Sync {
    fn now(&self) -> Result<MessageDate, LocalCivilTimeError>;
}

#[cfg(not(test))]
#[derive(Debug, Default)]
pub(crate) struct SystemLocalClock;

#[derive(Debug)]
pub(crate) struct FixedLocalClock(MessageDate);

impl FixedLocalClock {
    pub(crate) fn new(now: MessageDate) -> Self {
        Self(now)
    }
}

impl LocalClock for FixedLocalClock {
    fn now(&self) -> Result<MessageDate, LocalCivilTimeError> {
        Ok(self.0)
    }
}

#[cfg(not(test))]
impl LocalClock for SystemLocalClock {
    fn now(&self) -> Result<MessageDate, LocalCivilTimeError> {
        message_date(enzui_iced_core::local_time::current_local_civil_time()?)
    }
}

#[cfg(not(test))]
fn message_date(
    local: enzui_iced_core::local_time::LocalCivilTime,
) -> Result<MessageDate, LocalCivilTimeError> {
    Ok(MessageDate {
        year: u32::try_from(local.date.year())
            .map_err(|_| LocalCivilTimeError::LocalTimeOutOfRange)?,
        month: local.date.month(),
        day: local.date.day(),
        minute: u32::from(local.minute),
    })
}
