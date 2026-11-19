use iced::widget::{Space, column, container, row};
use iced::{Alignment, Element, Length, Padding};

use enzui_iced_widgets::button::{ButtonKind, ButtonSize, enz_button};
use enzui_iced_widgets::combo_box::enz_combo_box;
use enzui_iced_widgets::confirm_dialog::enz_confirm_dialog;
use enzui_iced_widgets::divider::enz_divider;
use enzui_iced_widgets::enz_text::{TextSize, enz_text};
use enzui_iced_widgets::modal_surface::enz_modal_surface;
use enzui_iced_widgets::reorder_list::enz_reorder_list;
use enzui_iced_widgets::scroll_view::enz_scroll_view;
use enzui_iced_widgets::semantics_container::SemanticsContainer;
use enzui_iced_widgets::surface::{SurfaceTone, enz_surface};
use enzui_iced_widgets::text_field::enz_text_field;

use enzui_iced_core::live_region::Announcement;
use enzui_iced_core::semantics::{Role, Semantics};
use enzui_iced_theme::SpacingScale;

use crate::model::layout::LayoutMode;
use crate::model::rules::{
    Action, ConditionCombinator, ConditionField, ConditionMatch, Rule, RuleDraft,
    action_kind_index, mailbox_index,
};
use crate::model::types::MailboxKind;
use crate::theme as t;
use crate::{Message as Msg, State};

pub(crate) const APPLY_STATUS_ID: &str = "mail-rules-apply-status";
pub(crate) const RULE_IDENTITY_NOTICE_ID: &str = "mail-rules-identity-notice";
pub(crate) const EDITOR_IDENTITY_NOTICE_ID: &str = "mail-rule-editor-identity-notice";
pub(crate) const APPLY_ACTION_ID: &str = "mail-rules-apply-action";

pub(crate) fn new_rule_invoker_id() -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::new("mail-new-rule")
}

pub(crate) fn edit_rule_invoker_id(id: &str) -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::from(format!("mail-edit-rule-{id}"))
}

pub(crate) fn with_rules<'a>(s: &'a State, base: Element<'a, Msg>) -> Element<'a, Msg> {
    use crate::modal_focus::RuleDialog;
    let with_editor = s.rule_dialogs.view_with(
        base,
        |key| match key {
            RuleDialog::List => rules_list_dialog(s),
            RuleDialog::Editor => s
                .rules_ui
                .draft
                .as_ref()
                .map_or_else(|| Space::new().into(), |draft| rule_editor_dialog(s, draft)),
        },
        Msg::RulesHostEvent,
        |key, host| {
            let label = match key {
                RuleDialog::List => crate::l10n::tr("Rules"),
                RuleDialog::Editor => s
                    .rules_ui
                    .draft
                    .as_ref()
                    .map_or_else(String::new, |draft| crate::l10n::tr(draft.title())),
            };
            host.label(label).placement(if s.window_size.width < 600.0 {
                enzui_iced_widgets::modal_host::SurfacePlacement::BottomEdge
            } else {
                enzui_iced_widgets::modal_host::SurfacePlacement::Centred
            })
        },
    );

    let dialog = s.rules_ui.pending_delete.as_ref().map(|id| {
        // `Tracked::get` shadows `RuleSet::get`, so name the inner type.
        let rules: &crate::model::rules::RuleSet = &s.rules;
        let name = rules
            .get(id)
            .map_or_else(|| "this rule".to_owned(), |r| r.name.clone());
        enz_confirm_dialog(
            crate::l10n::tr("Delete rule"),
            crate::l10n::message("mail-rule-delete", &[("name", name)]),
            &s.confirm_delete_rule,
            Msg::ConfirmDeleteRuleEvent,
        )
        .confirm_label(crate::l10n::tr("Delete rule"))
        .cancel_label(crate::l10n::tr("Cancel"))
        .destructive(true)
        .alert(true)
        .id_prefix("mail-delete-rule")
    });
    enzui_iced_widgets::confirm_dialog::confirm_dialog_host(
        with_editor,
        dialog,
        Msg::ConfirmDeleteRuleEvent,
    )
}

fn rule_identity_notice<'a>(s: &'a State, id: &'static str) -> Element<'a, Msg> {
    let Some(text) = &s.rule_identity_notice else {
        return Space::new().height(0).into();
    };
    let label: Element<Msg> = enz_text(crate::l10n::tr(text))
        .size(TextSize::Caption)
        .color(t::text_primary(s.theme_ref()))
        .into();
    SemanticsContainer::leaf(label, Announcement::Alert.region(crate::l10n::tr(text)))
        .id(iced::advanced::widget::Id::new(id))
        .into()
}

fn rule_header<'a>(
    title: &str,
    on_back: Msg,
    id: &'static str,
    trailing: Vec<Element<'a, Msg>>,
) -> Element<'a, Msg> {
    enzui_iced_widgets::enz_pane_header(crate::l10n::tr(title))
        .heading(enzui_iced_core::semantics::HeadingLevel::Page)
        .leading(
            enz_button(crate::l10n::tr("Back"), on_back)
                .kind(ButtonKind::Ghost)
                .size(ButtonSize::Sm)
                .icon(if crate::l10n::rtl() {
                    "arrow-right"
                } else {
                    "arrow-left"
                })
                .id(iced::advanced::widget::Id::new(id))
                .into(),
        )
        .trailing(row(trailing).mirrored(crate::l10n::rtl()).into())
        .into_element()
}

fn rules_list_dialog(s: &State) -> Element<'_, Msg> {
    let enz = s.theme_ref();
    let sp = &enz.spacing;

    let header = rule_header("Rules", Msg::CloseRules, "mail-rules-back", Vec::new());

    let body: Element<Msg> = if s.rules.is_empty() {
        column![
            enz_text(crate::l10n::tr("No rules yet"))
                .size(TextSize::Body)
                .color(t::text_primary(enz)),
            enz_text(crate::l10n::tr(
                "Rules run top-to-bottom over your mailbox."
            ))
            .size(TextSize::Footnote)
            .color(t::text_secondary(enz)),
        ]
        .spacing(sp.xs())
        .into()
    } else {
        let n = s.rules.len();
        let rows: Vec<(String, Element<Msg>)> = s
            .rules
            .rules
            .iter()
            .enumerate()
            .map(|(i, r)| (r.id.clone(), rule_row(s, r, i, n)))
            .collect();
        enz_reorder_list(rows, Msg::RuleDropped)
            .spacing(sp.xs())
            .collection_id("mail-rules")
            .into()
    };

    let apply_status: Element<Msg> = match &s.last_rules_result {
        Some(text) => {
            let label: Element<Msg> = enz_text(text.clone())
                .size(TextSize::Caption)
                .color(t::text_secondary(enz))
                .into();
            SemanticsContainer::leaf(label, Announcement::Status.region(text.clone()))
                .id(iced::advanced::widget::Id::new(APPLY_STATUS_ID))
                .into()
        }
        None => Space::new().height(0).into(),
    };

    let footer = row![
        enz_button(crate::l10n::tr("New rule"), Msg::NewRule)
            .kind(ButtonKind::Primary)
            .size(ButtonSize::Sm)
            .icon("plus")
            .id(new_rule_invoker_id()),
        Space::new().width(Length::Fill),
        apply_status,
        enz_button(crate::l10n::tr("Apply rules"), Msg::ApplyRules)
            .kind(ButtonKind::Secondary)
            .size(ButtonSize::Sm)
            .id(iced::advanced::widget::Id::new(APPLY_ACTION_ID)),
    ]
    .mirrored(crate::l10n::rtl())
    .align_y(Alignment::Center)
    .spacing(sp.sm());

    let (gutter, panel_w, panel_h) = crate::settings_view::dialog_geometry(s.mode(), s.window_size);

    // Rule mutations share the preferences write; a refused load or write is
    // reported in the bottom-right corner.
    let inner: Element<Msg> = column![
        header,
        enz_divider().into_element(),
        rule_identity_notice(s, RULE_IDENTITY_NOTICE_ID),
        container(body).height(Length::Fill).width(Length::Fill),
        enz_divider().into_element(),
        footer,
    ]
    .spacing(sp.md())
    .padding(Padding::from(sp.lg()))
    .width(Length::Fill)
    .height(Length::Fill)
    .into();

    let panel = enz_modal_surface(inner)
        .background(t::reader_background(enz))
        .border_color(t::divider_color(enz))
        .width(Length::Fixed(panel_w))
        .height(Length::Fixed(panel_h));

    container(panel).padding(gutter).into()
}

fn rule_row<'a>(s: &'a State, r: &'a Rule, index: usize, total: usize) -> Element<'a, Msg> {
    let enz = s.theme_ref();
    let sp = &enz.spacing;

    let id_up = r.id.clone();
    let id_down = r.id.clone();
    let id_edit = r.id.clone();
    let id_del = r.id.clone();

    let mut nav: Vec<Element<Msg>> = Vec::new();
    if index > 0 {
        nav.push(
            enz_button(crate::l10n::tr("Move up"), Msg::MoveRuleUp(id_up))
                .kind(ButtonKind::Ghost)
                .size(ButtonSize::Sm)
                .icon("chevron-up")
                .id(iced::advanced::widget::Id::from(format!(
                    "mail-rule-up-{}",
                    r.id
                )))
                .into(),
        );
    }
    if index + 1 < total {
        nav.push(
            enz_button(crate::l10n::tr("Move down"), Msg::MoveRuleDown(id_down))
                .kind(ButtonKind::Ghost)
                .size(ButtonSize::Sm)
                .icon("chevron-down")
                .id(iced::advanced::widget::Id::from(format!(
                    "mail-rule-down-{}",
                    r.id
                )))
                .into(),
        );
    }

    let text_col = column![
        enz_text(r.name.clone())
            .size(TextSize::Body)
            .color(t::text_primary(enz)),
        enz_text(r.summary())
            .size(TextSize::Footnote)
            .color(t::text_secondary(enz)),
    ]
    .spacing(sp.space1);
    let verbs = row![
        enz_button(crate::l10n::tr("Edit rule"), Msg::EditRule(id_edit.clone()))
            .kind(ButtonKind::Ghost)
            .size(ButtonSize::Sm)
            .id(edit_rule_invoker_id(&id_edit)),
        enz_button(
            crate::l10n::tr("Delete rule"),
            Msg::RequestDeleteRule(id_del.clone())
        )
        .kind(ButtonKind::Ghost)
        .size(ButtonSize::Sm)
        .id(iced::advanced::widget::Id::from(format!(
            "mail-rule-delete-{id_del}"
        ))),
    ]
    .mirrored(crate::l10n::rtl())
    .spacing(sp.xs())
    .align_y(Alignment::Center);

    let nav_row = row(nav).spacing(sp.xs()).align_y(Alignment::Center);

    // At the narrow rung the four inline controls clip at 375 px, so the verbs
    // wrap onto a second line.
    let inner: Element<Msg> = if s.mode() == crate::model::layout::LayoutMode::Narrow {
        column![
            text_col,
            row![nav_row, verbs]
                .mirrored(crate::l10n::rtl())
                .spacing(sp.xs())
                .align_y(Alignment::Center),
        ]
        .spacing(sp.xs())
        .into()
    } else {
        row![text_col, Space::new().width(Length::Fill), nav_row, verbs,]
            .mirrored(crate::l10n::rtl())
            .spacing(sp.sm())
            .align_y(Alignment::Center)
            .into()
    };

    let surface: Element<'a, Msg> = enz_surface(inner)
        .tone(SurfaceTone::SurfaceAlt)
        .radius(s.theme_ref().radius.step(s.theme_ref().shape.row))
        .padding(Padding::from([sp.xs(), sp.sm()]))
        .fill_width()
        .into();

    SemanticsContainer::new(
        surface,
        Semantics::new(Role::Group).label(crate::l10n::message(
            "mail-rule-accessible-name",
            &[("name", r.name.clone())],
        )),
    )
    .into()
}

fn rule_editor_dialog<'a>(s: &'a State, draft: &'a RuleDraft) -> Element<'a, Msg> {
    let enz = s.theme_ref();
    let sp = &enz.spacing;

    let header = rule_header(
        draft.title(),
        Msg::CancelRuleEdit,
        "mail-rule-editor-back",
        vec![
            enz_button(crate::l10n::tr("Save rule"), Msg::SaveRule)
                .kind(ButtonKind::Primary)
                .size(ButtonSize::Sm)
                .id(iced::advanced::widget::Id::new("mail-rule-save"))
                .into(),
        ],
    );

    let name = enz_text_field(
        crate::l10n::tr("Rule name"),
        "Name this rule",
        &draft.rule.name,
        Msg::RuleNameEdited,
    )
    .id_prefix("rule-name");

    let combinator = picker(
        "mail-rule-combinator",
        "Match",
        ConditionCombinator::ALL.iter().map(|c| c.label()),
        draft.rule.combinator.label(),
        Msg::RuleCombinatorSelected,
    );

    let mode = s.mode();
    let mut cond_rows: Vec<Element<Msg>> = Vec::new();
    for (i, c) in draft.rule.conditions.iter().enumerate() {
        cond_rows.push(condition_row(i, c, sp, mode));
    }
    let conditions = column![
        row![
            enz_text(crate::l10n::tr("Conditions"))
                .size(TextSize::Footnote)
                .color(t::text_secondary(enz)),
            Space::new().width(Length::Fill),
            combinator,
        ]
        .mirrored(crate::l10n::rtl())
        .align_y(Alignment::Center)
        .spacing(sp.sm()),
        column(cond_rows).spacing(sp.xs()),
        enz_button(crate::l10n::tr("Add condition"), Msg::RuleAddCondition)
            .id(iced::advanced::widget::Id::new("mail-rule-add-condition"))
            .kind(ButtonKind::Ghost)
            .size(ButtonSize::Sm)
            .icon("plus"),
    ]
    .spacing(sp.xs());

    let mut action_rows: Vec<Element<Msg>> = Vec::new();
    for (i, a) in draft.rule.actions.iter().enumerate() {
        action_rows.push(action_row(i, a, sp, mode));
    }
    let actions = column![
        enz_text(crate::l10n::tr("Actions"))
            .size(TextSize::Footnote)
            .color(t::text_secondary(enz)),
        column(action_rows).spacing(sp.xs()),
        enz_button(crate::l10n::tr("Add action"), Msg::RuleAddAction)
            .id(iced::advanced::widget::Id::new("mail-rule-add-action"))
            .kind(ButtonKind::Ghost)
            .size(ButtonSize::Sm)
            .icon("plus"),
    ]
    .spacing(sp.xs());

    let (gutter, panel_w, panel_h) = crate::settings_view::dialog_geometry(s.mode(), s.window_size);

    // The body scrolls so a long rule keeps `Add action` reachable; the header
    // stays outside so Back and `Save rule` are always painted.
    let body: Element<Msg> = column![
        name.into_element(),
        conditions,
        enz_divider().into_element(),
        actions,
    ]
    .spacing(sp.md())
    .width(Length::Fill)
    .into();
    let scrolling = enz_scroll_view(body).height(Length::Fill);

    let inner: Element<Msg> = column![
        header,
        enz_divider().into_element(),
        rule_identity_notice(s, EDITOR_IDENTITY_NOTICE_ID),
        scrolling,
    ]
    .spacing(sp.md())
    .padding(Padding::from(sp.lg()))
    .width(Length::Fill)
    .height(Length::Fill)
    .into();

    let panel = enz_modal_surface(inner)
        .background(t::reader_background(enz))
        .border_color(t::divider_color(enz))
        .width(Length::Fixed(panel_w))
        .height(Length::Fixed(panel_h));

    container(panel).padding(gutter).into()
}

fn condition_row<'a>(
    i: usize,
    c: &'a crate::model::rules::Condition,
    sp: &SpacingScale,
    mode: LayoutMode,
) -> Element<'a, Msg> {
    let field = picker(
        &format!("mail-rule-field-{i}"),
        "",
        ConditionField::ALL.iter().map(|f| f.label()),
        c.field.label(),
        move |idx| Msg::RuleConditionFieldSelected(i, idx),
    );
    let op = picker(
        &format!("mail-rule-match-{i}"),
        "",
        ConditionMatch::ALL.iter().map(|m| m.label()),
        c.op.label(),
        move |idx| Msg::RuleConditionMatchSelected(i, idx),
    );
    let value = enz_text_field("", crate::l10n::tr("value"), &c.value, move |v| {
        Msg::RuleConditionValueEdited(i, v)
    })
    .id_prefix(format!("mail-rule-condition-value-{i}"));
    let value_el: Element<Msg> = value.into_element();
    let remove = enz_button(
        crate::l10n::tr("Remove condition"),
        Msg::RuleRemoveCondition(i),
    )
    .kind(ButtonKind::Ghost)
    .size(ButtonSize::Sm)
    .icon("x")
    .id(iced::advanced::widget::Id::from(format!(
        "mail-rule-condition-remove-{i}"
    )));

    if mode == LayoutMode::Narrow {
        column![
            field,
            op,
            value_el,
            row![Space::new().width(Length::Fill), remove]
                .mirrored(crate::l10n::rtl())
                .align_y(Alignment::Center),
        ]
        .spacing(sp.xs())
        .into()
    } else {
        row![field, op, container(value_el).width(Length::Fill), remove,]
            .mirrored(crate::l10n::rtl())
            .spacing(sp.sm())
            .align_y(Alignment::Center)
            .into()
    }
}

fn action_row<'a>(
    i: usize,
    a: &'a Action,
    sp: &SpacingScale,
    mode: LayoutMode,
) -> Element<'a, Msg> {
    let kind = picker(
        &format!("mail-rule-action-{i}"),
        "",
        Action::ALL_KINDS.iter().copied(),
        Action::ALL_KINDS[action_kind_index(a)],
        move |idx| Msg::RuleActionKindSelected(i, idx),
    );

    // Only `Move to` actions expose a mailbox picker; the others are complete.
    let mailbox: Element<Msg> = match a {
        Action::MoveTo(kind_v) => picker(
            &format!("mail-rule-mailbox-{i}"),
            "",
            crate::model::rules::ACTION_MAILBOXES
                .iter()
                .map(|k| k.label()),
            MailboxKind::label(*kind_v),
            move |idx| Msg::RuleActionMailboxSelected(i, idx),
        ),
        _ => {
            let _ = mailbox_index;
            Space::new().width(0).into()
        }
    };

    let remove = enz_button(crate::l10n::tr("Remove action"), Msg::RuleRemoveAction(i))
        .id(iced::advanced::widget::Id::from(format!(
            "mail-rule-action-remove-{i}"
        )))
        .kind(ButtonKind::Ghost)
        .size(ButtonSize::Sm)
        .icon("x");

    if mode == LayoutMode::Narrow {
        column![
            kind,
            mailbox,
            row![Space::new().width(Length::Fill), remove]
                .mirrored(crate::l10n::rtl())
                .align_y(Alignment::Center),
        ]
        .spacing(sp.xs())
        .into()
    } else {
        row![kind, mailbox, Space::new().width(Length::Fill), remove,]
            .mirrored(crate::l10n::rtl())
            .spacing(sp.sm())
            .align_y(Alignment::Center)
            .into()
    }
}

fn picker<'a, I>(
    key: &str,
    label: &str,
    options: I,
    selected: &str,
    on_select: impl Fn(usize) -> Msg + Clone + 'a,
) -> Element<'a, Msg>
where
    I: IntoIterator<Item = &'static str>,
{
    let options: Vec<_> = options.into_iter().collect();
    let selected_index = options.iter().position(|option| *option == selected);
    let unchanged = on_select.clone();
    enz_combo_box(
        crate::l10n::tr(label),
        options
            .iter()
            .map(|option| crate::l10n::tr(option))
            .collect(),
        None,
        move |_| unchanged(selected_index.unwrap_or(0)),
    )
    .id(iced::advanced::widget::Id::from(key.to_owned()))
    .selected_index(selected_index)
    .on_select_index(on_select)
    .into_element()
}
