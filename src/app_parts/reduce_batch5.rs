use super::*;

impl State {
    pub(crate) fn reduce_batch5(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::SaveAttachment(attachment) => {
                if self.attachment_export_pending.is_some() {
                    self.attachment_notice = Some(
                        "An attachment is still being saved. Try again when it finishes."
                            .to_owned(),
                    );
                    return Task::none();
                }
                self.attachment_notice = None;
                let target = picker::PickerTarget::SaveAttachment(attachment);
                return self.request_attachment_picker(target);
            }
            Message::AttachmentBusy(retry) => {
                self.retain_attachment_retry(retry);
            }
            Message::AttachmentOpenPortalTick(wake) => match self.attachment_opener.pump(wake) {
                link_launcher::WakeVerdict::Finished(outcome) => {
                    return Task::done(Message::AttachmentOpenFinished(outcome));
                }
                link_launcher::WakeVerdict::Pending | link_launcher::WakeVerdict::Stale => {}
            },
            Message::AttachmentOpenFinished(outcome) => {
                let opened = self.attachment_open_held.take();
                let held_verdict = opened.as_ref().map_or(
                    Err(attachment_store::AttachmentFailure::OpenUncertain),
                    |opened| opened.verify_path(),
                );
                if let Err(error) = held_verdict {
                    if let Some(opened) = opened.as_ref() {
                        self.attachment_store
                            .retain_cleanup_authority(&opened.payload_ref());
                        self.attachment_reconcile_needed = true;
                    }
                    self.attachment_notice = Some(crate::l10n::message(
                        "mail-notice-attachment-maintenance-is-required",
                        &[("value1", (error.user_message()).to_owned())],
                    ));
                    return Task::none();
                }
                self.attachment_notice = match outcome {
                    enzui_iced_core::portal::PortalOutcome::Completed(()) => Some(crate::l10n::tr(
                        "Attachment handed to the selected application.",
                    )),
                    enzui_iced_core::portal::PortalOutcome::Cancelled(_) => None,
                    other => attachment_store::open_failure_text(&other),
                };
            }
            Message::OpenAttachmentMenu {
                message_id,
                name,
                at,
            } => {
                let items = context_menu::attachment_items(&message_id, &name);
                let target = context_menu::ContextTarget::Attachment { message_id, name };
                if let Some(invoker) = context_menu_invoker(&target, &self.store, &self.selection) {
                    self.replace_context_menu(context_menu::open(target, items, at, Some(invoker)));
                }
            }
            Message::DismissAttachmentNotice => {
                self.attachment_notice = None;
            }
            Message::OpenMessageSelectionMenu(context) => {
                let items = context_menu::message_selection_items(&context.selection.text);
                self.replace_context_menu(context_menu::with_invocation(
                    context_menu::ContextTarget::MessageSelection {
                        text: context.selection.text,
                    },
                    items,
                    context.invocation,
                    context.focus_origin,
                ));
            }
            Message::ClearMessageSelection => {
                self.message_selection_clear_generation =
                    self.message_selection_clear_generation.wrapping_add(1);
            }
            Message::ClipboardWritten => {}
            Message::SetMessageMoveDestination(label) => {
                if let Some(kind) = model::rules::ACTION_MAILBOXES
                    .iter()
                    .copied()
                    .find(|kind| kind.label() == label)
                {
                    self.message_move_destination = kind;
                }
            }
            Message::ConfirmMessageMove => {
                let ids = std::mem::take(&mut self.pending_move_targets);
                if !ids.is_empty() {
                    self.perform_move(&ids, self.message_move_destination);
                    return self.dismiss_move_dialog_focus(DismissCause::DefaultAction);
                }
            }
            Message::CancelMessageMove => {
                self.pending_move_targets.clear();
                return self.dismiss_move_dialog_focus(DismissCause::CloseButton);
            }
            Message::PickerDialogEvent(event) => return self.reduce_message_dialog(event),
            Message::ConfirmBlockSender(event) => {
                use enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent;
                if matches!(
                    event,
                    ConfirmDialogEvent::FocusNext | ConfirmDialogEvent::FocusPrevious
                ) {
                    if let Some(op) = event.focus_operation::<Message>(true) {
                        return iced::advanced::widget::operate(op);
                    }
                    return Task::none();
                }
                let ConfirmDialogEvent::Resolved(cause) = event else {
                    return Task::none();
                };
                let Some(addr) = self.pending_block_sender.take() else {
                    return Task::none();
                };
                if enzui_iced_core::confirm::ConfirmDecision::from_cause(cause).is_confirmed() {
                    self.privacy.get_mut().block_sender(&addr);
                    return self.save_preferences(preferences::SaveOrigin::Privacy);
                }
            }

            Message::OpenConversationMenu(id, at) => {
                // A context action uses the clicked row or its explicit multi-selection, never an unrelated reader target.
                let inside_selection = self.row_selection.len() > 1
                    && self
                        .row_selection
                        .contains(&enzui_iced_core::drag::ItemId::new(id.clone()));
                let resolved = if inside_selection {
                    crate::view::explicit_selection_state(self)
                        .zip(crate::view::explicit_selection_target(self))
                } else {
                    mail_actions::ActionState::for_conversation(
                        &self.store,
                        &self.selection,
                        &self.privacy.block_list,
                        &id,
                    )
                    .zip(context_menu::conversation_target(
                        &self.store,
                        &self.selection,
                        &id,
                    ))
                };
                let Some((st, action_target)) = resolved else {
                    return Task::none();
                };
                let target = context_menu::ContextTarget::Conversation { id };
                let invoker = context_menu_invoker(&target, &self.store, &self.selection);
                self.replace_context_menu(context_menu::open(
                    target,
                    context_menu::conversation_items(st, &action_target),
                    at,
                    invoker,
                ));
            }
            Message::OpenSidebarMenu {
                selection,
                row_key,
                at,
            } => {
                let Some(model) =
                    context_menu::sidebar_items(&self.store, &self.sidebar_layout, &selection)
                else {
                    return Task::none();
                };
                let target = context_menu::ContextTarget::Sidebar { selection, row_key };
                let invoker = context_menu_invoker(&target, &self.store, &self.selection);
                self.replace_context_menu(context_menu::open(target, model, at, invoker));
            }
            Message::ContextMenuEvent(generation, event) => {
                if generation != self.context_menu_generation {
                    return Task::none();
                }
                let (chosen, restore) = self.context_menu.update::<Message>(&event);
                if let Some((_target, command)) = chosen {
                    return Task::batch([self.update(command), restore]);
                }
                return restore;
            }
            Message::MailAction(action, target) => {
                return self.apply_mail_action(action, target);
            }
            Message::ContextMarkAllRead(selection) => {
                self.perform_mark_all_read(&selection);
            }

            Message::AddFavorite(selection) => {
                return self.edit_sidebar_layout(|l| l.add_favorite(&selection));
            }
            Message::RemoveFavorite(selection) => {
                return self.edit_sidebar_layout(|l| l.remove_favorite(&selection));
            }
            Message::RemoveSmartMailbox(selection) => {
                let SidebarSelection::SmartMailbox(kind) = selection else {
                    return Task::none();
                };
                return self.edit_sidebar_layout(|l| l.remove_smart(kind));
            }
            Message::AddSmartMailbox(kind) => {
                return self.edit_sidebar_layout(|l| l.add_smart(kind));
            }
            Message::SetFavoritesVisible(visible) => {
                return self.edit_sidebar_layout(|l| {
                    l.set_favorites_visible(visible);
                    true
                });
            }
            Message::SetSmartMailboxesVisible(visible) => {
                return self.edit_sidebar_layout(|l| {
                    l.set_smart_visible(visible);
                    true
                });
            }
            Message::ReorderFavorite(request) => {
                return self.edit_sidebar_layout(|layout| layout.reorder_favorites(&request));
            }
            Message::ReorderSmartMailbox(request) => {
                return self.edit_sidebar_layout(|layout| layout.reorder_smart(&request));
            }

            Message::OpenRules => return self.open_rules_focus(),
            Message::CloseRules => {
                return self.dismiss_rules_focus(enzui_iced_core::modal::DismissCause::CloseButton);
            }
            Message::RulesHostEvent(event) => return self.reduce_rules_host(event),
            Message::NewRule => {
                if self.rules_ui.begin_new().is_err() {
                    self.rule_identity_notice = Some(RULE_ID_EXHAUSTED_NOTICE);
                    return Task::none();
                }
                self.rule_identity_notice = None;
                return self.open_rule_editor_focus(crate::rules_view::new_rule_invoker_id());
            }
            Message::EditRule(id) => {
                let rules: &model::rules::RuleSet = &self.rules;
                if let Some(rule) = rules.get(&id).cloned() {
                    let invoker = crate::rules_view::edit_rule_invoker_id(&id);
                    self.rule_identity_notice = None;
                    self.rules_ui.begin_edit(rule);
                    return self.open_rule_editor_focus(invoker);
                }
            }
            Message::CancelRuleEdit => {
                return self
                    .dismiss_rule_editor_focus(enzui_iced_core::modal::DismissCause::CloseButton);
            }
            Message::SaveRule => return self.save_rule_from_editor(),
            Message::RequestDeleteRule(id) => {
                self.rules_ui.pending_delete = Some(id);
                self.confirm_delete_rule = enzui_iced_core::confirm::ConfirmState::new(None);
            }
            Message::ConfirmDeleteRuleEvent(event) => {
                use enzui_iced_widgets::confirm_dialog::ConfirmDialogEvent;
                if matches!(
                    event,
                    ConfirmDialogEvent::FocusNext | ConfirmDialogEvent::FocusPrevious
                ) {
                    if let Some(op) = event.focus_operation::<Message>(true) {
                        return iced::advanced::widget::operate(op);
                    }
                    return Task::none();
                }
                let ConfirmDialogEvent::Resolved(cause) = event else {
                    return Task::none();
                };
                // Only an affirmative decision may publish a rule removal.
                let pending = self.rules_ui.pending_delete.take();
                if enzui_iced_core::confirm::ConfirmDecision::from_cause(cause).is_confirmed()
                    && let Some(id) = pending
                    // Deleting a missing rule must not publish, which with unreadable preferences
                    // would ask the store to replace the file.
                    && self.rules.get_mut().remove(&id)
                {
                    self.refresh_rules_count();
                    return self.save_preferences(preferences::SaveOrigin::Rules);
                }
            }
            Message::MoveRuleUp(id) => {
                self.rules.get_mut().move_up(&id);
                return self.save_preferences(preferences::SaveOrigin::Rules);
            }
            Message::MoveRuleDown(id) => {
                self.rules.get_mut().move_down(&id);
                return self.save_preferences(preferences::SaveOrigin::Rules);
            }
            Message::RuleDropped(req) => {
                if let Some(item) = req.items.first() {
                    self.rules.get_mut().move_to_slot(item.as_str(), req.slot.0);
                    return self.save_preferences(preferences::SaveOrigin::Rules);
                }
            }
            _ => unreachable!("reduce_batch5 handles only its own variants"),
        }
        Task::none()
    }
}
