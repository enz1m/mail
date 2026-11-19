use enzui_iced_widgets::scroll_view::enz_scroll_view;
use iced::widget::{column, container, row};
use iced::{Alignment, Element, Length, Padding};

use enzui_iced_core::action_overflow::{Descriptor, VisibilityPriority};
use enzui_iced_core::live_region::Announcement;
use enzui_iced_core::menu_model::{MenuEntryModel, MenuModel};
use enzui_iced_widgets::action_bar::{ActionItem, enz_action_bar};
use enzui_iced_widgets::autocomplete_field::{AutocompleteOption, enz_autocomplete_field};
use enzui_iced_widgets::button::{ButtonKind, ButtonSize, enz_button};
use enzui_iced_widgets::combo_box::{ComboBoxSize, enz_combo_box};
use enzui_iced_widgets::enz_text::{TextSize, enz_text};
use enzui_iced_widgets::flow::enz_flow;
use enzui_iced_widgets::form_layout::enz_form_layout;
use enzui_iced_widgets::menu_button::{MenuButtonKind, enz_menu_button};
use enzui_iced_widgets::rich_edit::{
    InlineStyle, RichEditSwatch, RichEditToolbarDensity, enz_rich_edit_builder,
};
use enzui_iced_widgets::semantics_container::SemanticsContainer;
use enzui_iced_widgets::surface_stack::SpaceToken;
use enzui_iced_widgets::text_area::{AreaHeight, FieldSize, enz_text_area};
use enzui_iced_widgets::text_field::enz_text_field;

use crate::model::compose::ComposeFormat;
use crate::model::recipients::{self, Contact};
use crate::theme as t;
use crate::{Message as Msg, RecipientField, State};
use enzui_iced_widgets::toolbar_icon::enz_toolbar_icon;

pub(crate) const PENDING_SEND_HEADLINE: &str = "Pending send — Undo available";
pub(crate) const PENDING_SEND_DESCRIPTION: &str = "Delivery waits until the undo period ends unless you choose Send now. If submission fails, the message remains in the durable Outbox for Retry or Cancel.";
pub(crate) const DISCARD_LOCAL_SAVE_LABEL: &str = "Discard";

fn recipient_autocomplete_field<'a>(
    label: &'a str,
    placeholder: &'a str,
    value: &'a str,
    contacts: &[Contact],
    field: RecipientField,
    on_input: fn(String) -> Msg,
    trailing: Option<Element<'a, Msg>>,
) -> Element<'a, Msg> {
    let options: Vec<AutocompleteOption<String>> = recipients::suggest(contacts, value)
        .into_iter()
        .map(|c| {
            let line = c.address_line();
            // Keyed by bare email, which survives a refilter even when display names
            // collide. Not `rich_result`: a contact has one datum, the committed line, and
            // the rich shape elides that line.
            AutocompleteOption::new(c.email, line.clone(), line)
        })
        .collect();

    let field = enz_autocomplete_field(
        crate::l10n::tr(label),
        value,
        options,
        on_input,
        move |addr: &String| Msg::ComposeRecipientSelected(field, addr.clone()),
    )
    .id(recipient_field_id(field))
    .placeholder(crate::l10n::tr(placeholder))
    .on_custom_value(move |typed: String| Msg::ComposeRecipientCustom(field, typed));
    match trailing {
        Some(accessory) => field.trailing(accessory).into(),
        None => field.into(),
    }
}

pub(crate) fn recipient_field_id(field: RecipientField) -> iced::advanced::widget::Id {
    let label = match field {
        RecipientField::To => "To",
        RecipientField::Cc => "CC",
        RecipientField::Bcc => "BCC",
    };
    enzui_iced_core::identity::derive_id(
        enzui_iced_widgets::autocomplete_field::DEFAULT_ID_PREFIX,
        label,
    )
}

pub(crate) const ADD_CC: &str = "add-cc";
pub(crate) const ADD_BCC: &str = "add-bcc";

// The recipient lines the To field's menu can still add: CC and BCC, each
// until it is shown.
pub(crate) fn recipient_menu_model(compose: &crate::model::compose::ComposeState) -> MenuModel {
    let mut entries = Vec::with_capacity(2);
    if !cc_visible(compose) {
        entries.push(
            MenuEntryModel::command(ADD_CC, crate::l10n::tr("CC"))
                .description(crate::l10n::tr("Copy, seen by everyone")),
        );
    }
    if !compose.bcc_shown {
        entries.push(
            MenuEntryModel::command(ADD_BCC, crate::l10n::tr("BCC"))
                .description(crate::l10n::tr("Copy hidden from the others")),
        );
    }
    MenuModel::new(entries)
}

fn cc_visible(compose: &crate::model::compose::ComposeState) -> bool {
    compose.cc_shown || !compose.cc.is_empty()
}

// The widget id of the message body, whichever editor surface is mounted.
//
// Both format arms publish this id; only one is ever mounted.
pub(crate) fn compose_body_id() -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::from("mail-compose-body")
}

pub(crate) const COMPOSE_PAGE: iced::Color = iced::Color {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 1.0,
};

pub(crate) const COMPOSE_PAGE_DIM: iced::Color = iced::Color {
    r: 0xDA as f32 / 255.0,
    g: 0xDA as f32 / 255.0,
    b: 0xDE as f32 / 255.0,
    a: 1.0,
};

pub(crate) fn compose_page(enz: &enzui_iced_theme::EnzTheme) -> iced::Color {
    if enz.palette.is_dark() {
        COMPOSE_PAGE_DIM
    } else {
        COMPOSE_PAGE
    }
}

// The inks the composer offers for authored text.
//
// Each clears the body-text contrast floor on both [`COMPOSE_PAGE`] and
// [`COMPOSE_PAGE_DIM`] as authored, so no pick is silently adjusted.
pub(crate) fn compose_swatches() -> Vec<RichEditSwatch> {
    vec![
        RichEditSwatch::clear(crate::l10n::tr("Automatic colour")),
        RichEditSwatch::ink(
            iced::Color::from_rgb8(0xB3, 0x26, 0x1A),
            crate::l10n::tr("Red text"),
        ),
        RichEditSwatch::ink(
            iced::Color::from_rgb8(0x1A, 0x5F, 0x2E),
            crate::l10n::tr("Green text"),
        ),
        RichEditSwatch::ink(
            iced::Color::from_rgb8(0x1B, 0x3F, 0xA8),
            crate::l10n::tr("Blue text"),
        ),
        RichEditSwatch::ink(
            iced::Color::from_rgb8(0x6B, 0x21, 0x9A),
            crate::l10n::tr("Purple text"),
        ),
    ]
}

// The style toggles the composer offers.
//
// Underline and strikethrough are omitted because the editor paints no
// decoration for them; both still round-trip through the document and HTML.
pub(crate) fn composer_style_toggles() -> Vec<InlineStyle> {
    InlineStyle::ALL.to_vec()
}

fn format_entry_id(format: ComposeFormat) -> &'static str {
    match format {
        ComposeFormat::Plain => "compose-format-plain",
        ComposeFormat::Rich => "compose-format-rich",
        ComposeFormat::Html => "compose-format-html",
    }
}

pub(crate) fn format_for_menu_entry(id: &str) -> Option<ComposeFormat> {
    ComposeFormat::ALL
        .into_iter()
        .find(|format| format_entry_id(*format) == id)
}

pub(crate) fn compose_format_menu_model() -> MenuModel {
    MenuModel::new(
        ComposeFormat::ALL
            .into_iter()
            .map(|format| {
                MenuEntryModel::command(
                    format_entry_id(format),
                    crate::l10n::tr(format.menu_label()),
                )
                .icon(format.icon())
            })
            .collect(),
    )
}

pub(crate) fn back_button(s: &State) -> Element<'_, Msg> {
    enz_toolbar_icon(
        "navigation.back",
        if s.compose.can_save_draft()
            && (s.compose.is_user_edited() || s.compose.editing_draft_id.is_some())
        {
            Msg::SaveDraft
        } else {
            Msg::CloseCompose
        },
    )
    .label(crate::l10n::tr("Close composer"))
    .id(iced::advanced::widget::Id::new("mail-compose-close"))
    .into_element()
}

pub(crate) fn compose_pane(s: &State) -> Option<Element<'_, Msg>> {
    if !s.compose.open {
        return None;
    }
    let enz = s.theme_ref();
    let sp = &enz.spacing;

    let from_display =
        |a: &crate::model::types::Account| format!("{} <{}>", a.display_name, a.email);
    let from_element: Element<Msg> = if s.store.accounts.len() > 1 {
        let options: Vec<String> = s.store.accounts.iter().map(&from_display).collect();
        let account_ids = s
            .store
            .accounts
            .iter()
            .map(|account| account.id.clone())
            .collect::<Vec<_>>();
        let selected = account_ids
            .iter()
            .position(|id| *id == s.compose.from_account_id);
        enz_combo_box(crate::l10n::tr("From"), options, None, |_| {
            Msg::SetComposeFrom(String::new())
        })
        .selected_index(selected)
        .on_select_index(move |index| {
            Msg::SetComposeFrom(account_ids.get(index).cloned().unwrap_or_default())
        })
        .size(ComboBoxSize::Sm)
        .id_prefix("compose-from")
        .into()
    } else {
        let from_line = s
            .store
            .account(&s.compose.from_account_id)
            .map(|a| crate::l10n::message("mail-from-address", &[("address", from_display(a))]))
            .unwrap_or_default();
        enz_text(from_line)
            .size(TextSize::Footnote)
            .color(t::text_secondary(enz))
            .into()
    };
    let contacts = recipients::recent_contacts(&s.store);

    let add_line: Option<Element<Msg>> =
        (!cc_visible(&s.compose) || !s.compose.bcc_shown).then(|| {
            enz_menu_button(
                "",
                &s.compose_recipient_model,
                &s.compose_recipient_menu,
                Msg::ComposeRecipientMenuEvent,
            )
            .menu_id(iced::advanced::widget::Id::new(
                "mail-compose-recipient-menu",
            ))
            .kind(MenuButtonKind::Toolbar)
            .trigger_icon("plus")
            .alignment(enzui_iced_core::placement::Alignment::End)
            .aria_label(crate::l10n::tr("Add CC or BCC"))
            .into()
        });
    let to_row = recipient_autocomplete_field(
        "To",
        "email@example.com, other@example.com",
        &s.compose.to,
        &contacts,
        RecipientField::To,
        Msg::ComposeToEdited,
        add_line,
    );

    let cc_row: Option<Element<Msg>> = cc_visible(&s.compose).then(|| {
        recipient_autocomplete_field(
            "CC",
            "Copy, seen by everyone",
            &s.compose.cc,
            &contacts,
            RecipientField::Cc,
            Msg::ComposeCcEdited,
            None,
        )
    });
    let bcc_row: Option<Element<Msg>> = s.compose.bcc_shown.then(|| {
        recipient_autocomplete_field(
            "BCC",
            "Hidden copy, not seen by other recipients",
            &s.compose.bcc,
            &contacts,
            RecipientField::Bcc,
            Msg::ComposeBccEdited,
            None,
        )
    });

    let subject_field = enz_text_field(
        crate::l10n::tr("Subject"),
        crate::l10n::tr("Subject"),
        &s.compose.subject,
        Msg::ComposeSubjectEdited,
    )
    .id_prefix("compose-subject");

    // Format, formatting, colour and Attach share one row above the editor; the
    // framework row wraps at the narrow rung instead of clipping a control.
    let format_menu: Element<Msg> = enz_menu_button(
        crate::l10n::tr(s.compose.format.label()),
        &s.compose_format_model,
        &s.compose_format_menu,
        Msg::ComposeFormatMenuEvent,
    )
    .kind(MenuButtonKind::Quiet(ButtonSize::Sm))
    .menu_id(iced::advanced::widget::Id::new("mail-compose-format"))
    .trigger_icon(s.compose.format.icon())
    .aria_label(crate::l10n::message(
        "mail-message-format",
        &[("format", crate::l10n::tr(s.compose.format.menu_label()))],
    ))
    .into();

    let attach_button: Element<Msg> = enz_button(crate::l10n::tr("Attach file"), Msg::AttachFile)
        .kind(ButtonKind::Quiet)
        .size(ButtonSize::Sm)
        .icon("paperclip")
        .id(iced::advanced::widget::Id::from("mail-compose-attach"))
        .into_element();

    let body_field: Element<Msg> = if s.compose.format.is_rich() {
        container(
            enz_rich_edit_builder(
                &s.rich_controller,
                &s.rich_editor,
                Msg::ComposeRichEditAction,
            )
            .editor_height(AreaHeight::Fill)
            .editor_id(compose_body_id())
            .styles(composer_style_toggles())
            // Explicit so a host that stops painting alignment must change this line.
            .paragraph_alignment(true)
            .density(RichEditToolbarDensity::Compact)
            .paper(compose_page(enz))
            .colors(compose_swatches(), Msg::ComposeColorPicked)
            .toolbar_leading(format_menu)
            .toolbar_trailing(attach_button),
        )
        .height(Length::Fill)
        .into()
    } else {
        let row: Element<Msg> = enz_flow()
            .wrap(true)
            .gap(SpaceToken::Xs)
            .cross_gap(SpaceToken::Xs)
            .align_items(Alignment::Center)
            .spacing_scale(enz.spacing)
            .push(format_menu)
            .push(attach_button)
            .into_element();
        container(
            column![
                row,
                container(
                    enz_text_area(&s.body_editor, Msg::ComposeBodyAction)
                        .label(crate::l10n::tr("Message"))
                        .placeholder(crate::l10n::tr("Write your message..."))
                        .size(FieldSize::Md)
                        .id(compose_body_id())
                        .fill()
                        .fill_height(),
                )
                .height(Length::Fill),
            ]
            .spacing(4),
        )
        .height(Length::Fill)
        .into()
    };

    // `enz_action_bar` folds whole commands into an overflow menu at narrow widths;
    // higher `VisibilityPriority` folds last, so Send stays inline.
    let editing_scheduled = s
        .compose
        .editing_draft_id
        .as_ref()
        .and_then(|id| s.store.message(id))
        .is_some_and(|m| m.send_at.is_some());
    let can_send = s.compose.can_send();
    let can_save = s.compose.can_save_draft();
    let actions: Element<Msg> = {
        let items: Vec<ActionItem<'_, Msg>> = vec![
            ActionItem::new(
                Descriptor::new("compose.cancel").with_priority(VisibilityPriority(40)),
                || {
                    enz_button(crate::l10n::tr("Close"), Msg::CloseCompose)
                        .id(iced::advanced::widget::Id::new("mail-compose-cancel"))
                        .kind(ButtonKind::Quiet)
                        .into()
                },
            ),
            ActionItem::new(
                Descriptor::new("compose.save").with_priority(VisibilityPriority(60)),
                move || {
                    let b = enz_button(
                        crate::l10n::tr(if editing_scheduled {
                            "Save changes"
                        } else {
                            "Save Draft"
                        }),
                        Msg::SaveDraft,
                    )
                    .id(iced::advanced::widget::Id::new("mail-compose-save"))
                    .kind(ButtonKind::Secondary);
                    let b = if can_save { b } else { b.enabled(false) };
                    b.into()
                },
            ),
            ActionItem::new(
                Descriptor::new("compose.send-later").with_priority(VisibilityPriority(70)),
                move || {
                    enz_button(
                        crate::l10n::tr(if editing_scheduled {
                            "Change send time"
                        } else {
                            "Send later"
                        }),
                        Msg::OpenSchedule,
                    )
                    .id(iced::advanced::widget::Id::new("mail-compose-schedule"))
                    .kind(ButtonKind::Quiet)
                    .icon("clock")
                    .enabled(can_send)
                    .into()
                },
            ),
            ActionItem::new(
                Descriptor::new("compose.send").with_priority(VisibilityPriority(100)),
                move || {
                    let b = enz_button(
                        crate::l10n::tr(if editing_scheduled {
                            "Send now"
                        } else {
                            "Send"
                        }),
                        Msg::SendCompose,
                    )
                    .kind(ButtonKind::Primary)
                    .icon("send")
                    .id(iced::advanced::widget::Id::new("mail-compose-send"));
                    let b = if can_send { b } else { b.enabled(false) };
                    b.into()
                },
            ),
        ];
        enz_action_bar(items, Msg::ToggleComposeOverflow)
            .overflow_open(s.compose_overflow_open)
            .on_dismiss(Msg::CloseComposeOverflow)
            .breakpoints(enz.breakpoints)
            .overflow_trigger_id(iced::advanced::widget::Id::new("mail-compose-overflow"))
            .spacing(SpaceToken::Sm)
            // The bar aligns itself right; a `Fill` spacer beside it would halve the width
            // its fold is measured against.
            .align_x(iced::alignment::Horizontal::Right)
            .into()
    };

    let actions: Element<Msg> = if let Some(_draft_id) = &s.compose.editing_draft_id {
        let status = if s.local_state_error.is_some() {
            "Draft not saved"
        } else if s.draft_save_pending {
            "Saving draft locally…"
        } else {
            "Draft saved locally"
        };
        let text_el: Element<Msg> = enz_text(crate::l10n::tr(status))
            .size(TextSize::Footnote)
            .color(t::text_secondary(enz))
            .into();
        let saved_indicator: Element<Msg> = SemanticsContainer::new(
            text_el,
            Announcement::Status.region(crate::l10n::tr(status)),
        )
        .into();
        // No `Fill` spacer between indicator and bar: the bar measures its fold against
        // the width it is given (`compose_actions` pins this at the wide rung).
        row![saved_indicator, actions]
            .mirrored(crate::l10n::rtl())
            .align_y(Alignment::Center)
            .spacing(sp.sm())
            .into()
    } else {
        actions
    };

    // Only the addressing rows scroll; preview and its toggle stay outside the
    // scroll view so the Fill body cannot push them out of reach.
    //
    // Attachments list what is attached plus any portal notice; an empty list
    // takes no vertical space.
    let attachments_row: Element<Msg> = {
        let mut col = column![].spacing(sp.xs());
        for (i, att) in s.compose.attachments.iter().enumerate() {
            let chip = row![
                enz_text(format!("{} ({})", att.name, att.size_label()))
                    .size(TextSize::Footnote)
                    .color(t::text_secondary(enz)),
                enz_button(crate::l10n::tr("Remove"), Msg::RemoveComposeAttachment(i))
                    .id(iced::advanced::widget::Id::from(format!(
                        "mail-compose-remove-attachment-{i}"
                    )))
                    .kind(ButtonKind::Ghost)
                    .size(ButtonSize::Sm)
                    .icon("x"),
            ]
            .mirrored(crate::l10n::rtl())
            .spacing(sp.xs())
            .align_y(Alignment::Center);
            col = col.push(crate::native_drop::attachment_source(
                chip.into(),
                &s.attachment_store,
                att,
            ));
        }
        if let Some(notice) = &s.compose_attach_notice {
            col = col.push(
                enz_text(notice.clone())
                    .size(TextSize::Footnote)
                    .color(t::text_secondary(enz)),
            );
        }
        col.into()
    };

    let rows: Vec<Element<Msg>> = [Some(from_element), Some(to_row), cc_row, bcc_row]
        .into_iter()
        .flatten()
        .chain(std::iter::once(subject_field.into()))
        .collect();

    // `Sm` gaps for the short addressing rows. Only this block scrolls, so the
    // body never competes with metadata for one scroll offset.
    let fields = enz_scroll_view(
        enz_form_layout(rows)
            .row_gap(SpaceToken::Sm)
            .inset(SpaceToken::None)
            .spacing_scale(s.theme_ref().spacing),
    )
    .height(Length::Shrink);

    let body_field = container(body_field).height(Length::Fill);

    // A refused format save and a draft timestamp failure are reported in the
    // bottom-right corner (`view::notices`), not inside the form.
    let mut stacked: Vec<Element<Msg>> = vec![fields.into()];
    if s.attachment_reminder_open {
        stacked.push(
            column![
                enz_text(crate::l10n::tr(
                    "You mentioned an attachment, but no file is attached."
                )),
                row![
                    enz_button(crate::l10n::tr("Attach file"), Msg::AttachFile),
                    enz_button(
                        crate::l10n::tr("Send without attachment"),
                        Msg::SendWithoutAttachment
                    ),
                    enz_toolbar_icon("x", Msg::DismissAttachmentReminder)
                        .aria_label(crate::l10n::tr("Dismiss attachment reminder"))
                        .into_element()
                ]
                .mirrored(crate::l10n::rtl())
                .spacing(sp.sm())
            ]
            .spacing(sp.xs())
            .into(),
        );
    }
    stacked.push(body_field.into());
    stacked.push(attachments_row);
    stacked.push(actions);
    let panel_inner: Element<Msg> = iced::widget::Column::with_children(stacked)
        .spacing(sp.sm())
        .padding(Padding::from(sp.lg()))
        .width(Length::Fill)
        .height(Length::Fill)
        .into();

    let panel: Element<Msg> = SemanticsContainer::new(
        panel_inner,
        enzui_iced_core::semantics::Semantics::new(enzui_iced_core::semantics::Role::Group)
            .label(crate::l10n::tr("Compose message")),
    )
    .into();
    let generation = s.attachment_picker_generation;
    Some(
        enzui_iced_widgets::file_drop::enz_file_drop_target_with_receipt(
            enzui_iced_widgets::track_bounds(panel, &s.draft_bounds),
            move |receipt| {
                Msg::NativeDrop(crate::native_drop::Event::Received(generation, receipt))
            },
        )
        .copy_only(true)
        .enabled(s.accepts_attachment_drop())
        .key(generation)
        .into(),
    )
}
