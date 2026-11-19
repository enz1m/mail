use super::*;

pub(crate) struct BootConfiguration {
    pub(crate) themes: ThemeIntegration,
    pub(crate) theme_boot: Task<ThemeEvent>,
    pub(crate) demo: Option<String>,
    pub(crate) push_detail: bool,
    pub(crate) shortcut_store: enzui_iced_core::persist::Store<enzui_iced_core::ShortcutOverrides>,
    pub(crate) preferences_store: enzui_iced_core::persist::Store<preferences::Preferences>,
    pub(crate) local_state_store: enzui_iced_core::persist::Store<local_state::LocalState>,
    pub(crate) frames: PersistedFrames,
    pub(crate) seed_fixture_when_absent: bool,
}

impl State {
    pub(crate) fn boot_configured_with_local_state_at(
        config: BootConfiguration,
    ) -> (Self, Task<Message>) {
        let BootConfiguration {
            themes,
            theme_boot,
            demo,
            push_detail,
            shortcut_store,
            preferences_store,
            local_state_store,
            frames,
            seed_fixture_when_absent,
        } = config;
        // A deferred frame is read by its own worker; boot starts from the empty
        // document until a settlement installs the real one.
        let inline_local_state = (!frames.deferred).then(|| local_state_store.load());
        let local_state_load_error = inline_local_state
            .as_ref()
            .and_then(enzui_iced_core::persist::LoadOutcome::user_summary);
        let local_state_retained_for_recovery = local_state_load_error.is_some()
            && local_state::authorize_after_custody(
                &local_state_store,
                frames.local_state.as_deref(),
                None,
            );
        let local_state_absent = matches!(
            &inline_local_state,
            Some(enzui_iced_core::persist::LoadOutcome::Absent { .. })
        );
        let attachment_cleanup_allowed = matches!(
            &inline_local_state,
            Some(
                enzui_iced_core::persist::LoadOutcome::Loaded(_)
                    | enzui_iced_core::persist::LoadOutcome::Absent { .. }
            )
        );
        let mut loaded_local_state = match inline_local_state {
            Some(outcome) => outcome.into_value(),
            None => local_state::LocalState::empty(),
        };
        if seed_fixture_when_absent && local_state_absent {
            loaded_local_state = local_state::LocalState::from_fixture(fixture::seed());
        }
        loaded_local_state.refresh_previews();
        loaded_local_state.recover_ordinals();
        let local_state::LocalState {
            accounts,
            messages,
            today,
            mut pending_send,
            next_message_ordinal: mut next_ordinal,
            mut next_account_ordinal,
            active_account_id,
            send_generation,
            pending_server_mutations,
            next_mutation_ordinal: next_server_mutation_ordinal,
            credential_journal,
        } = loaded_local_state;
        let mut store = MailboxStore::new(accounts, messages, today);
        if seed_fixture_when_absent
            && matches!(demo.as_deref(), Some("html" | "html-switching"))
            && let Some(path) = std::env::var_os("MAIL_APP_HTML_FIXTURE")
            && let Ok(source) = std::fs::read_to_string(path)
            && source.len() <= 512 * 1024
            && let Some(message) = store
                .messages
                .iter_mut()
                .find(|message| message.id == "m-nl-1")
        {
            message.body_html = Some(source.clone());
            message.has_remote_content = true;
            if demo.as_deref() == Some("html-switching") {
                for message in &mut store.messages {
                    if matches!(
                        message.location,
                        model::types::MessageLocation::Mailbox(MailboxKind::Inbox)
                    ) {
                        message.body_html =
                            Some(format!("<h2>Preview {}</h2>{source}", message.id));
                        message.has_remote_content = true;
                    }
                }
            }
        }
        #[cfg(test)]
        let local_clock: Arc<dyn local_clock::LocalClock> =
            Arc::new(local_clock::FixedLocalClock::new(today));
        #[cfg(not(test))]
        let local_clock: Arc<dyn local_clock::LocalClock> = if seed_fixture_when_absent {
            Arc::new(local_clock::FixedLocalClock::new(today))
        } else {
            Arc::new(local_clock::SystemLocalClock)
        };
        #[cfg(test)]
        let attachment_store = attachment_store::AttachmentStore::memory();
        #[cfg(not(test))]
        let attachment_store = if seed_fixture_when_absent {
            attachment_store::AttachmentStore::memory()
        } else {
            attachment_store::AttachmentStore::from_xdg()
        };
        #[cfg(not(test))]
        if seed_fixture_when_absent && local_state_absent {
            fixture::stage_demo_photos(&mut store.messages, &attachment_store);
        }
        let (selection, forced_convo) = match demo.as_deref() {
            Some("junk") => (
                SidebarSelection::AccountMailbox {
                    account_id: "icloud".into(),
                    mailbox: MailboxKind::Junk,
                },
                Some("j-1".to_owned()),
            ),
            Some("html" | "html-switching") => {
                (SidebarSelection::default(), Some("m-nl-1".to_owned()))
            }
            _ => (SidebarSelection::default(), None),
        };
        let selected_conversation = forced_convo.or_else(|| {
            store
                .conversations_for(&selection)
                .first()
                .map(|c| c.id.clone())
        });
        let mut narrow_nav = NarrowNav::default();
        let mut initial_window = Size::new(1400.0, 900.0);
        if push_detail {
            narrow_nav.on_select(true);
            initial_window = Size::new(375.0, 812.0);
        }
        let mut compose = ComposeState::default();
        let today = store.today;
        let fixture_clock = local_clock::FixedLocalClock::new(today);
        match demo.as_deref() {
            Some("compose" | "compose-rich") => {
                let src = store
                    .sorted_messages(&SidebarSelection::default())
                    .first()
                    .map(|m| m.id.clone());
                let mode = match src {
                    Some(id) => ComposeMode::Reply(id),
                    None => ComposeMode::New,
                };
                ComposeController {
                    store: &mut store,
                    compose: &mut compose,
                    pending_send: &mut pending_send,
                    next_ordinal: &mut next_ordinal,
                    active_account_id: &active_account_id,
                    clock: &fixture_clock,
                }
                .try_open(mode)
                .expect("the fixed fixture clock cannot fail");
            }
            Some("autocomplete_field") => {
                compose.open = true;
                compose.mode = ComposeMode::New;
                ComposeMode::New.title().clone_into(&mut compose.title);
                compose.to = "Xiao".into();
                compose.subject = "Sunday plans".into();
            }
            Some("autosaved") => {
                let src = store
                    .sorted_messages(&SidebarSelection::default())
                    .first()
                    .map(|m| m.id.clone());
                let mode = match src {
                    Some(id) => ComposeMode::Reply(id),
                    None => ComposeMode::New,
                };
                let mut ctl = ComposeController {
                    store: &mut store,
                    compose: &mut compose,
                    pending_send: &mut pending_send,
                    next_ordinal: &mut next_ordinal,
                    active_account_id: &active_account_id,
                    clock: &fixture_clock,
                };
                ctl.try_open(mode)
                    .expect("the fixed fixture clock cannot fail");
                ctl.try_autosave_draft()
                    .expect("the fixed fixture clock cannot fail");
            }
            Some("undo") => {
                compose.open = true;
                compose.to = "friend@example.com".into();
                compose.subject = "Lunch this weekend".into();
                compose.body = "Are you free Sunday?".into();
                ComposeController {
                    store: &mut store,
                    compose: &mut compose,
                    pending_send: &mut pending_send,
                    next_ordinal: &mut next_ordinal,
                    active_account_id: &active_account_id,
                    clock: &fixture_clock,
                }
                .try_send_compose()
                .expect("the fixed fixture clock cannot fail");
            }
            _ => {}
        }
        let collapsed_sections_boot: std::collections::HashSet<String> =
            if demo.as_deref() == Some("collapsed") {
                store.accounts.iter().map(|a| a.id.clone()).collect()
            } else {
                std::collections::HashSet::new()
            };

        // `MAIL_APP_DEMO=search-long` seeds one message with pathological sender,
        // subject and preview lengths and opens the popup on a query matching only it.
        if demo.as_deref() == Some("search-long") {
            let id = store.messages.first().map(|m| m.id.clone());
            if let Some(id) = id {
                for m in &mut store.messages {
                    if m.id == id {
                        m.sender_name =
                            "Dr. Alexandra Konstantinopoulos-Whitmore (Deliveries Team, EU)".into();
                        m.subject = "unique-1616 Quarterly platform migration readiness \
                             review and rollout checklist v4 (draft)"
                            .into();
                        m.preview = "Hi all, ahead of Thursday's review please read the \
                             migration checklist in full: the staging cutover window moves to \
                             02:00 UTC, the read-replica lag budget drops to ninety seconds, \
                             and every service owner must confirm their rollback rehearsal \
                             before we freeze the release branch on Friday afternoon at five."
                            .into();
                    }
                }
                store.mark_dirty();
            }
        }

        let mut search_open_boot = false;
        let mut search_query_boot = String::new();
        if demo.as_deref() == Some("search-long") {
            search_open_boot = true;
            "unique-1616".clone_into(&mut search_query_boot);
        }
        if demo.as_deref() == Some("search") {
            search_open_boot = true;
            search_query_boot = std::env::var("MAIL_APP_SEARCH_QUERY")
                .ok()
                .filter(|q| !q.trim().is_empty())
                .unwrap_or_else(|| "re".to_owned());
        }

        let shortcuts_open_boot = demo.as_deref() == Some("shortcuts");

        // The seeded refusal for `MAIL_APP_DEMO=settings-shortcuts-refused`:
        // the exact sentence `RebindRefusal::Reserved` produces for Ctrl+Q.
        let shortcut_feedback_boot: Option<(String, String)> =
            (demo.as_deref() == Some("settings-shortcuts-refused")).then(|| {
                (
                    "mail.archive-selected".to_owned(),
                    enzui_iced_core::RebindRefusal::Reserved {
                        reason: crate::l10n::tr("your desktop quits the application with Ctrl+Q."),
                    }
                    .message(),
                )
            });

        let body_editor = text_editor::Content::with_text(&compose.body);
        let mut undo_send_toasts = ToastHost::new()
            .id_prefix("mail-undo-send")
            .max_visible(2)
            .waiting_budget(WaitingBudget::unbounded().max_waiting(Some(0)));
        let pending_undo_toast = pending_send.as_ref().map(|_| PendingUndoToast {
            id: undo_send_toasts.push(undo_send_toast_spec()),
            generation: send_generation,
        });
        let mut undo_send_expiry = ToastExpirySchedule::default();
        let _ = undo_send_toasts.sync_expiry(&mut undo_send_expiry, std::time::Instant::now());

        // `MAIL_APP_DEMO=settings*` opens Settings at boot: `settings` on Accounts,
        // `settings-privacy` on Advanced, `settings-add` on the Add-Account form with a
        // Gmail address typed so the detection hint shows. The focus lifecycle is
        // seeded from `settings_boot.open` so Tab is scoped on the first frame.
        let mut settings_boot = SettingsState::default();
        match demo.as_deref() {
            Some("settings") => {
                settings_boot.open();
                settings_boot.select_tab(SettingsTab::Accounts);
            }
            Some("settings-general" | "favorites") => {
                settings_boot.open();
                settings_boot.select_tab(SettingsTab::General);
            }
            Some("settings-advanced" | "settings-privacy") => {
                settings_boot.open();
                settings_boot.select_tab(SettingsTab::Advanced);
            }
            Some("settings-add") => {
                settings_boot.open();
                settings_boot.select_tab(SettingsTab::Accounts);
                settings_boot.add_account.begin();
                settings_boot.add_account.email = "someone@gmail.com".into();
            }
            Some("settings-added") => {
                settings_boot.open();
                settings_boot.select_tab(SettingsTab::Accounts);
                settings_boot.add_account.begin();
                settings_boot.add_account.email = "someone@gmail.com".into();
                settings_boot.add_account.continue_to_configure();
                if let Ok(id) = commit_add_account(
                    &mut store,
                    &mut settings_boot.add_account,
                    &mut next_account_ordinal,
                ) {
                    settings_boot.expanded_account = Some(id);
                }
            }
            // `settings-shortcuts-refused` also seeds the reserved refusal, so a capture
            // shows that reserved and invalid chords read differently.
            Some("settings-shortcuts" | "settings-shortcuts-refused") => {
                settings_boot.open();
                settings_boot.select_tab(SettingsTab::Shortcuts);
            }
            Some("settings-config") => {
                settings_boot.open();
                settings_boot.select_tab(SettingsTab::Accounts);
                settings_boot.add_account.begin();
                settings_boot.add_account.email = "someone@gmail.com".into();
                settings_boot.add_account.continue_to_configure();
            }
            _ => {}
        }

        let mut bulk_boot = model::bulk::BulkState::default();
        let row_selection_boot = enzui_iced_core::list_selection::ListSelection::new();
        if demo.as_deref() == Some("bulk-confirm") {
            bulk_boot.pending_empty = Some(MailboxKind::Trash);
            bulk_boot.pending_empty_account = Some("icloud".into());
        }

        let inline_preferences = (!frames.deferred).then(|| preferences_store.load());
        let preferences_load_error = inline_preferences
            .as_ref()
            .and_then(enzui_iced_core::persist::LoadOutcome::user_summary);
        if preferences_load_error.is_some() {
            let _ = local_state::authorize_after_custody(
                &preferences_store,
                frames.preferences.as_deref(),
                None,
            );
        }
        let preferences_absent = matches!(
            &inline_preferences,
            Some(enzui_iced_core::persist::LoadOutcome::Absent { .. })
        );
        let mut loaded_preferences = match inline_preferences {
            Some(outcome) => outcome.into_value(),
            None => preferences::Preferences::defaults(),
        };
        if seed_fixture_when_absent && preferences_absent {
            loaded_preferences = preferences::Preferences::demo_fixture();
        }

        // A composer open at boot must match a reducer open: remembered format and
        // editors carrying the seeded body (the reducer uses `sync_body_editor`).
        if compose.open {
            compose.format = match demo.as_deref() {
                Some("compose-rich") => model::compose::ComposeFormat::Rich,
                _ => loaded_preferences.compose_format,
            };
        }
        let (rich_controller, rich_editor) = match compose.format {
            model::compose::ComposeFormat::Rich | model::compose::ComposeFormat::Html => (
                enzui_iced_widgets::rich_edit::RichEditController::new(
                    enzui_iced_widgets::rich_edit::RichEditDocument::from_plain(&compose.body),
                ),
                text_editor::Content::with_text(&compose.body),
            ),
            model::compose::ComposeFormat::Plain => (
                enzui_iced_widgets::rich_edit::RichEditController::default(),
                text_editor::Content::new(),
            ),
        };
        let privacy_boot = loaded_preferences.privacy_state();
        let mut sidebar_layout_boot = loaded_preferences.sidebar_layout.clone();
        sidebar_layout_boot.prune(&store.accounts);
        if demo.as_deref() == Some("favorites")
            && let Some(folder) = store
                .accounts
                .first()
                .and_then(|a| a.folders.first().map(|f| (a.id.clone(), f.id.clone())))
        {
            sidebar_layout_boot.add_favorite(&SidebarSelection::Folder {
                account_id: folder.0,
                folder_id: folder.1,
            });
            sidebar_layout_boot.remove_favorite(&SidebarSelection::Favorite(
                model::types::FavoriteKind::Drafts,
            ));
            sidebar_layout_boot.set_smart_visible(false);
        }
        let mut rules_boot = loaded_preferences.rules;
        let mut rules_ui_boot = model::rules::RulesUiState {
            // From the highest `rule-N` id, not the length: a set with a hole would mint
            // an existing id and the next save would overwrite a live rule.
            next_id: next_rule_id_seed(&rules_boot),
            ..Default::default()
        };
        match demo.as_deref() {
            Some("rules") => rules_ui_boot.open(),
            Some("rules-edit") => {
                rules_ui_boot.open();
                if let Some(first) = rules_boot.rules.first().cloned() {
                    rules_ui_boot.begin_edit(first);
                    if let Some(d) = rules_ui_boot.draft.as_mut() {
                        while d.rule.conditions.len() < 2 {
                            d.add_condition();
                        }
                        while d.rule.actions.len() < 2 {
                            d.add_action();
                        }
                    }
                }
            }
            Some("rules-delete") => {
                rules_ui_boot.open();
                if let Some(first) = rules_boot.rules.first() {
                    rules_ui_boot.pending_delete = Some(first.id.clone());
                }
            }
            // Many conditions plus an action, which must stay reachable by scrolling.
            Some("rules-edit-many") => {
                rules_ui_boot.open();
                if let Some(first) = rules_boot.rules.first().cloned() {
                    rules_ui_boot.begin_edit(first);
                    if let Some(d) = rules_ui_boot.draft.as_mut() {
                        while d.rule.conditions.len() < 11 {
                            d.add_condition();
                        }
                        while d.rule.actions.is_empty() {
                            d.add_action();
                        }
                    }
                }
            }
            _ => {}
        }
        let _ = &mut rules_boot;

        let context_menu_boot = match demo.as_deref() {
            Some("context") => selected_conversation.as_deref().and_then(|id| {
                let st = mail_actions::ActionState::for_conversation(
                    &store,
                    &selection,
                    &privacy_boot.block_list,
                    id,
                )?;
                let action_target = context_menu::conversation_target(&store, &selection, id)?;
                let target = context_menu::ContextTarget::Conversation { id: id.to_owned() };
                let invoker = context_menu_invoker(&target, &store, &selection);
                Some(context_menu::open(
                    target,
                    context_menu::conversation_items(st, &action_target),
                    iced::Point::new(500.0, 200.0),
                    invoker,
                ))
            }),
            Some("context-sidebar") => {
                context_menu::sidebar_items(&store, &sidebar_layout_boot, &selection).map(|model| {
                    let target = context_menu::ContextTarget::Sidebar {
                        selection: selection.clone(),
                        row_key: crate::view::sidebar_row_key(&selection),
                    };
                    let invoker = context_menu_invoker(&target, &store, &selection);
                    context_menu::open(target, model, iced::Point::new(120.0, 160.0), invoker)
                })
            }
            _ => None,
        }
        .unwrap_or_default();

        // Boot-open presentations must run the same initial-focus operations
        // as reducer opens; constructing lifecycle values alone focuses nothing.
        let compose_focus = None;
        let compose_focus_boot = if compose.open {
            iced::widget::operation::focus(crate::compose_view::compose_body_id())
        } else {
            Task::none()
        };
        let (settings_focus, settings_focus_boot) = modal_focus::boot_focus(
            settings_boot.open,
            crate::settings_view::SETTINGS_SCOPE_KEY,
            Some(crate::view::settings_button_id()),
        );
        let (shortcuts_focus, shortcuts_focus_boot) =
            modal_focus::boot_focus(shortcuts_open_boot, crate::view::SHORTCUTS_SCOPE_KEY, None);
        let mut rule_dialogs = enzui_iced_widgets::modal_host::DialogStack::new();
        let rules_focus_boot = if rules_ui_boot.open {
            rule_dialogs.open(modal_focus::RuleDialog::List, None)
        } else {
            Task::none()
        };
        let rule_editor_focus_boot = if rules_ui_boot.draft.is_some() {
            rule_dialogs.open(modal_focus::RuleDialog::Editor, None)
        } else {
            Task::none()
        };
        let (empty_confirm_focus, empty_confirm_focus_boot) = modal_focus::boot_focus(
            bulk_boot.pending_empty.is_some(),
            crate::bulk_view::EMPTY_CONFIRM_SCOPE_KEY,
            Some(crate::bulk_view::mailbox_actions_invoker_id()),
        );
        let (shortcuts, shortcut_load_error) = if frames.deferred {
            (
                shortcuts::catalog_with_overrides(enzui_iced_core::ShortcutOverrides::default()),
                None,
            )
        } else {
            shortcuts::catalog_from(&shortcut_store)
        };
        if shortcut_load_error.is_some() {
            let _ = local_state::authorize_after_custody(
                &shortcut_store,
                frames.shortcuts.as_deref(),
                None,
            );
        }
        let referenced_attachments = store
            .messages
            .iter()
            .flat_map(|message| message.attachments.iter())
            .chain(compose.attachments.iter())
            .chain(
                pending_send
                    .iter()
                    .flat_map(|pending| pending.attachments.iter()),
            )
            .cloned()
            .collect::<Vec<_>>();
        let local_state_token = local_state::LocalState::token(
            &store,
            0,
            pending_send.as_ref(),
            pending_server_mutations.len(),
            local_state::LocalStateMetadata {
                next_message_ordinal: next_ordinal,
                next_account_ordinal,
                active_account_id: &active_account_id,
                send_generation,
                next_mutation_ordinal: next_server_mutation_ordinal,
                credential_journal: &credential_journal,
            },
        );
        // A refused thread set degrades to a permanently shut-down owner, whose
        // submissions settle as deferred work instead of running on the UI thread.
        let blocking_tasks =
            enzui_iced_core::blocking_task::BlockingTaskOwner::with_defaults("mail-blocking")
                .unwrap_or_else(|_| {
                    enzui_iced_core::blocking_task::BlockingTaskOwner::unavailable(
                        enzui_iced_core::blocking_task::DEFAULT_QUEUE_CAPACITY,
                    )
                });
        // Take the dispatcher before the owner moves into the state struct so
        // the mail backend and the state share one bounded pool.
        let mail_dispatcher = blocking_tasks.dispatcher();
        #[cfg(test)]
        let (credential_store, credential_store_ready, credential_boot) = (
            CredentialStoreHandle(Arc::new(
                enzui_services::credential_store::MemoryCredentialStore::new(),
            )),
            true,
            Task::none(),
        );
        #[cfg(not(test))]
        let (credential_store, credential_store_ready, credential_boot) =
            if seed_fixture_when_absent {
                (
                    CredentialStoreHandle(Arc::new(
                        enzui_services::credential_store::MemoryCredentialStore::new(),
                    )),
                    true,
                    Task::none(),
                )
            } else {
                (
                    CredentialStoreHandle(Arc::new(UnavailableCredentialStore)),
                    false,
                    Task::perform(
                        async {
                            match tokio::time::timeout(
                                Duration::from_secs(3),
                                enzui_services::credential_store::secret_service::SecretServiceTransport::connect_or_unavailable(),
                            )
                            .await
                            {
                                Ok(store) => store,
                                Err(_) => Arc::new(UnavailableCredentialStore)
                                    as Arc<dyn CredentialStore>,
                            }
                        },
                        |store| Message::CredentialStoreReady(CredentialStoreHandle(store)),
                    ),
                )
            };
        let mut state = Self {
            localizer: crate::l10n::current(),
            desktop: desktop::Desktop::default(),
            native_drop: native_drop::NativeDrop::default(),
            privacy_choices: RemoteContentPolicy::ALL
                .into_iter()
                .map(|policy| {
                    enzui_iced_widgets::radio_group::RadioChoice::with_label(
                        policy.radio_label(),
                        policy,
                        crate::l10n::tr(policy.radio_label()),
                    )
                })
                .collect(),
            themes,
            window_size: initial_window,
            fixture_mode: seed_fixture_when_absent,
            store,
            pending_server_mutations,
            next_server_mutation_ordinal,
            local_state_gate: enzui_iced_core::persist::SaveGate::with_baseline(
                local_state_store,
                local_state_token.clone(),
            ),
            local_state_published: local_state_token,
            #[cfg(feature = "center")]
            center: None,
            #[cfg(feature = "center")]
            agent: enzui_iced_agent_app::AgentProcess::default(),
            #[cfg(feature = "center")]
            reviews: enzui_iced_agent::center::review::ReviewBook::new(),
            #[cfg(feature = "center")]
            agent_review: None,
            #[cfg(feature = "center")]
            review_outcomes: crate::center::ReviewOutcomes::default(),
            #[cfg(feature = "center")]
            review_serial: 0,
            #[cfg(feature = "center")]
            waiting_center_calls: Vec::new(),
            sync_adoption_revision: 0,
            local_state_error: None,
            local_state_load_error,
            local_state_retained_for_recovery,
            startup: if frames.deferred {
                StartupLoads {
                    local_state: FrameLoad::deferred(frames.local_state),
                    preferences: FrameLoad::deferred(frames.preferences),
                    shortcuts: FrameLoad::deferred(frames.shortcuts),
                }
            } else {
                StartupLoads {
                    local_state: FrameLoad::inline(attachment_cleanup_allowed),
                    preferences: FrameLoad::inline(true),
                    shortcuts: FrameLoad::inline(true),
                }
            },
            blocking_tasks,
            blocking_generation: 0,
            blocking_generation_exhausted: false,
            persistence_retries: VecDeque::new(),
            attachment_retries: VecDeque::new(),
            mail_busy: false,
            local_clock,
            local_timestamp_failure: None,
            credential_store,
            credential_store_ready,
            credential_inflight: None,
            credential_mutations: credential_mutation::CredentialMutationOwner::new(
                credential_journal,
            ),
            credential_secret: None,
            oauth_sign_in: None,
            oauth_generation: 0,
            oauth_grant: None,
            credential_recovery: None,
            account_removal: None,
            account_edit: None,
            confirm_remove_account: enzui_iced_core::confirm::ConfirmState::new(None),
            mail_backend: Arc::new(backend::curl_backend::CurlMailBackend::with_dispatcher(
                mail_dispatcher,
            )),
            mail_generation: 0,
            mail_intent: None,
            mail_queue: VecDeque::new(),
            mail_cancel: None,
            mail_discard_generation: None,
            mail_retry: None,
            mailbox_sync_retry: None,
            mailbox_sync_generation: 0,
            mailbox_sync_generation_exhausted: false,
            mailbox_sync_retry_at: None,
            mailbox_sync_retry_attempt: 0,
            pending_sync_adoption: None,
            content_sync_inflight: None,
            outgoing_mime: None,
            mail_notice: None,
            close_guard: enzui_iced_core::close_guard::CloseGuard::new(),
            close_generation: 0,
            close_settlement: None,
            close_committed: false,
            selection,
            selected_conversation,
            deferred_read: None,
            row_selection: row_selection_boot,
            narrow_nav,
            nav: enzui_iced_widgets::nav_split::NavSplitState::new(),
            search_query: search_query_boot,
            search_scope: SearchScope::default(),
            search_scope_choices: crate::view::search_scope_choices(),
            search_open: search_open_boot,
            titlebar_overflow_open: false,
            search_highlight: None,
            collapsed_sections: collapsed_sections_boot,
            privacy: enzui_iced_core::persist::Tracked::new(privacy_boot),
            sidebar_layout: enzui_iced_core::persist::Tracked::new(sidebar_layout_boot),
            compose_format: enzui_iced_core::persist::Tracked::new(
                loaded_preferences.compose_format,
            ),
            pending_open_url: None,
            open_url_focus: None,
            confirm_open_url: enzui_iced_core::confirm::ConfirmState::new(None),
            link_open_notice: None,
            attachment_notice: None,
            hidden_notices: Vec::new(),
            attachment_store,
            pending_payload_cleanup: Vec::new(),
            attachment_stage_inflight: 0,
            attachment_picker_generation: 0,
            attachment_cleanup_inflight: 0,
            attachment_cleanup_error: None,
            attachment_reconcile_needed: false,
            attachment_open_generation: 0,
            attachment_open_pending: None,
            attachment_open_held: None,
            attachment_open_busy: false,
            attachment_export_pending: None,
            attachment_opener: link_launcher::LinkOpener::new(),
            link_opener: link_launcher::LinkOpener::new(),
            compose,
            compose_generation: 0,
            draft_save_pending: false,
            body_editor,
            draft_pointer: None,
            draft_bounds: std::cell::Cell::new(None),
            quick_reply: text_editor::Content::new(),
            quick_reply_for: None,
            quick_reply_mode: None,
            html_reader: mail_html::MailHtml::default(),
            remote_images_once: HashSet::new(),
            prefer_plain_text: enzui_iced_core::persist::Tracked::new(
                loaded_preferences.prefer_plain_text,
            ),
            dark_html: enzui_iced_core::persist::Tracked::new(loaded_preferences.dark_html),
            notifications: enzui_iced_core::persist::Tracked::new(loaded_preferences.notifications),
            message_plain_text: HashMap::new(),
            quoted_text_expanded: HashSet::new(),
            older_mailboxes_exhausted: HashSet::new(),
            conversation_scroll: enzui_iced_widgets::scroll_view::ScrollTranslation::new(),
            attachment_reminder_open: false,
            attachment_reminder_accepted: false,
            rich_controller,
            rich_editor,
            pending_send,
            next_ordinal,
            active_account_id,
            send_generation,
            undo_send_toasts,
            undo_send_expiry,
            pending_undo_toast,
            undo_moves: Vec::new(),
            held_mutations: std::collections::BTreeSet::new(),
            unsubscribes: std::collections::HashMap::new(),
            server_search: ServerSearch::default(),
            attachment_previews: std::collections::HashMap::new(),
            previews_for: None,
            server_search_ids: HashSet::new(),
            settings: settings_boot,
            signature_editor: text_editor::Content::new(),
            next_account_ordinal,
            bulk: bulk_boot,
            confirm_empty: enzui_iced_core::confirm::ConfirmState::new(None),
            empty_confirm_focus,
            mailbox_overflow_open: false,
            reader_overflow_target: None,
            compose_overflow_open: false,
            attach_chooser: picker::Picker::new(),
            compose_attach_notice: None,
            shortcuts_open: shortcuts_open_boot,
            shortcuts_focus,
            shortcuts,
            shortcut_armed: None,
            shortcut_feedback: shortcut_feedback_boot,
            shortcut_gate: enzui_iced_core::persist::SaveGate::with_baseline(
                shortcut_store.clone(),
                0,
            ),
            shortcut_store,
            shortcut_revision: 0,
            shortcut_published: 0,
            shortcut_saves_inflight: HashSet::new(),
            shortcut_error_kind: None,
            shortcut_load_error,
            compose_format_menu: enzui_iced_core::menu_model::MenuState::closed(),
            compose_recipient_menu: enzui_iced_core::menu_model::MenuState::closed(),
            discovery_net: (!seed_fixture_when_absent).then(|| {
                std::sync::Arc::new(crate::backend::discovery::SystemNet::new())
                    as std::sync::Arc<dyn crate::backend::discovery::DiscoveryNet + Send + Sync>
            }),
            compose_recipient_model: compose_view::recipient_menu_model(
                &crate::model::compose::ComposeState::default(),
            ),
            compose_format_model: compose_view::compose_format_menu_model(),
            schedule_editor: None,
            sender_menu_state: enzui_iced_core::menu_model::MenuState::closed(),
            sender_menu_closed_state: enzui_iced_core::menu_model::MenuState::closed(),
            sender_menu_target: None,
            sender_menu_model: crate::view::address_action_menu_model(false, false),
            message_selection_clear_generation: 0,
            pending_move_targets: Vec::new(),
            pending_snooze_targets: Vec::new(),
            inbox_watches: std::collections::BTreeMap::new(),
            inbox_watch_stopped: std::collections::HashMap::new(),
            inbox_announce_ready: HashSet::new(),
            new_mail: new_mail_notify::NewMailNotifier::default(),
            snooze_choice: model::snooze::SnoozePreset::Tomorrow,
            message_dialogs: enzui_iced_widgets::modal_host::DialogStack::new(),
            compose_focus,
            settings_focus,
            message_move_destination: MailboxKind::Archive,
            pending_block_sender: None,
            confirm_block_sender: enzui_iced_core::confirm::ConfirmState::new(None),
            rules_count_label: crate::settings_view::rules_count_label(rules_boot.len()),
            rules: enzui_iced_core::persist::Tracked::new(rules_boot),
            rules_ui: rules_ui_boot,
            rule_dialogs,
            // `with_baseline`: the loaded document is already on disk, so the first
            // privacy or rule message must not rewrite an identical file. Both halves
            // start at revision 0, as boot mutates nothing through `get_mut`.
            preferences_gate: enzui_iced_core::persist::SaveGate::with_baseline(
                preferences_store,
                (0, 0, 0, 0, 0, 0, 0),
            ),
            preferences_published: (0, 0, 0, 0, 0, 0, 0),
            preferences_error: None,
            preferences_load_error,
            confirm_delete_rule: enzui_iced_core::confirm::ConfirmState::new(None),
            last_rules_result: None,
            rule_identity_notice: None,
            context_menu_generation: u64::from(context_menu_boot.is_open()),
            context_menu: context_menu_boot,
        };
        #[cfg(test)]
        state.html_reader.refuse_native_for_test();
        // The attachment survey is filesystem work, so it is submitted, not run while
        // building the first frame; a deferred boot submits it once every frame settles.
        let attachment_boot = if state.startup.pending() {
            Task::none()
        } else if attachment_cleanup_allowed {
            state.start_attachment_startup_survey(referenced_attachments)
        } else {
            Task::none()
        };
        let startup_boot = if state.startup.pending() {
            state.begin_startup_loads()
        } else {
            Task::none()
        };
        let credential_recovery_boot = if state.startup.pending() {
            Task::none()
        } else {
            state.continue_credential_mutation()
        };
        (
            state,
            Task::batch([
                theme_boot.map(|event| Message::Theme(Box::new(event))),
                compose_focus_boot,
                settings_focus_boot,
                shortcuts_focus_boot,
                rules_focus_boot,
                rule_editor_focus_boot,
                empty_confirm_focus_boot,
                credential_boot,
                credential_recovery_boot,
                attachment_boot,
                startup_boot,
            ]),
        )
    }
}
