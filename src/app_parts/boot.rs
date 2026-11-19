use super::*;

#[path = "boot_configured.rs"]
mod boot_configured;

impl State {
    pub(crate) fn boot() -> (Self, Task<Message>) {
        let (themes, theme_boot) = ThemeIntegration::boot_for(
            enzui_iced_core::app_data::AppId::new(DESKTOP_APP_ID)
                .expect("BUG: DESKTOP_APP_ID must be a valid application identity"),
        );
        let demo = std::env::var("MAIL_APP_DEMO").ok();
        let push_detail = std::env::var("MAIL_APP_PUSH_DETAIL").as_deref() == Ok("1");
        #[cfg_attr(
            not(feature = "center"),
            allow(
                unused_mut,
                reason = "The center feature installs its worker on this state."
            )
        )]
        let (mut state, task) = Self::boot_configured(
            themes,
            theme_boot,
            demo,
            push_detail,
            shortcuts::file_store(),
            preferences::file_store(),
        );
        state.new_mail.enable();
        #[cfg(feature = "center")]
        state.start_center();
        (state, task)
    }

    #[cfg(test)]
    pub(crate) fn boot_for_test() -> (Self, Task<Message>) {
        // An in-memory override store, so no test reads or writes the developer's
        // real shortcut file.
        Self::boot_for_test_with_preferences(preferences::memory_store())
    }

    #[cfg(test)]
    pub(crate) fn boot_for_test_with_preferences(
        preferences_store: enzui_iced_core::persist::Store<preferences::Preferences>,
    ) -> (Self, Task<Message>) {
        Self::boot_for_test_with_stores(shortcuts::memory_store(), preferences_store)
    }

    #[cfg(test)]
    pub(crate) fn boot_for_test_with_stores(
        shortcut_store: enzui_iced_core::persist::Store<enzui_iced_core::ShortcutOverrides>,
        preferences_store: enzui_iced_core::persist::Store<preferences::Preferences>,
    ) -> (Self, Task<Message>) {
        Self::boot_configured(
            test_theme_integration(EnzTheme::dark()),
            Task::none(),
            None,
            false,
            shortcut_store,
            preferences_store,
        )
    }

    #[cfg(test)]
    pub(crate) fn boot_for_test_with_local_state(
        local_state_store: enzui_iced_core::persist::Store<local_state::LocalState>,
        seed_fixture_when_absent: bool,
    ) -> (Self, Task<Message>) {
        Self::boot_configured_with_local_state(
            test_theme_integration(EnzTheme::dark()),
            Task::none(),
            None,
            false,
            shortcuts::memory_store(),
            preferences::memory_store(),
            local_state_store,
            seed_fixture_when_absent,
        )
    }

    pub(crate) fn boot_configured(
        themes: ThemeIntegration,
        theme_boot: Task<ThemeEvent>,
        demo: Option<String>,
        push_detail: bool,
        shortcut_store: enzui_iced_core::persist::Store<enzui_iced_core::ShortcutOverrides>,
        preferences_store: enzui_iced_core::persist::Store<preferences::Preferences>,
    ) -> (Self, Task<Message>) {
        let fixture_mode = cfg!(test) || demo.is_some();
        let (shortcut_store, preferences_store) = if demo.is_some() {
            (shortcuts::memory_store(), preferences::memory_store())
        } else {
            (shortcut_store, preferences_store)
        };
        let (local_state_store, local_state_path) = if fixture_mode {
            (local_state::memory_store(), None)
        } else {
            local_state::file_store_with_path()
        };
        // Only a production boot names real frames; fixture and test boots carry no
        // path, so custody never touches a developer's files and reads stay inline.
        let frames = if fixture_mode {
            PersistedFrames::default()
        } else {
            PersistedFrames {
                local_state: local_state_path,
                preferences: preferences::file_store_path(),
                shortcuts: shortcuts::file_store_path(),
                deferred: true,
            }
        };
        Self::boot_configured_with_local_state_at(boot_configured::BootConfiguration {
            themes,
            theme_boot,
            demo,
            push_detail,
            shortcut_store,
            preferences_store,
            local_state_store,
            frames,
            seed_fixture_when_absent: fixture_mode,
        })
    }

    #[cfg(test)]
    #[allow(
        clippy::too_many_arguments,
        reason = "test fixture mirrors the independent boot stores and options"
    )]
    pub(crate) fn boot_configured_with_local_state(
        themes: ThemeIntegration,
        theme_boot: Task<ThemeEvent>,
        demo: Option<String>,
        push_detail: bool,
        shortcut_store: enzui_iced_core::persist::Store<enzui_iced_core::ShortcutOverrides>,
        preferences_store: enzui_iced_core::persist::Store<preferences::Preferences>,
        local_state_store: enzui_iced_core::persist::Store<local_state::LocalState>,
        seed_fixture_when_absent: bool,
    ) -> (Self, Task<Message>) {
        Self::boot_configured_with_local_state_at(boot_configured::BootConfiguration {
            themes,
            theme_boot,
            demo,
            push_detail,
            shortcut_store,
            preferences_store,
            local_state_store,
            frames: PersistedFrames::default(),
            seed_fixture_when_absent,
        })
    }
}
