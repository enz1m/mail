use super::sidebar_list::LIST_PANE_DIVIDER_WIDTH;
use super::*;

pub(crate) fn shell_tokens() -> ShellTokens {
    let mut tokens = ShellTokens {
        sidebar_header: SidebarHeader::SplitBand,
        ..ShellTokens::flat_page()
    }
    .sidebar_width(220.0)
    .pane_gutter(0.0)
    .edge_inset(0.0);
    tokens.pane_radius = 0.0;
    tokens
}

pub(crate) const SIDEBAR_HEADER_TITLE: &str = "Mailboxes";

fn sidebar_titlebar(_s: &State) -> Element<'_, Msg> {
    // The framework's pane header: the title on the sidebar's content edge,
    // the settings icon one icon gap from the sidebar toggle.
    enz_title_bar()
        .id("mail-app-sidebar-header")
        .title(crate::l10n::tr(SIDEBAR_HEADER_TITLE))
        .push(
            enz_toolbar_icon("settings", Msg::OpenSettings)
                .label(crate::l10n::tr("Settings…"))
                .id(settings_button_id()),
        )
        .into_element()
}

pub(crate) fn welcome_shell(s: &State) -> Element<'_, Msg> {
    let page = move || {
        enz_status_page(crate::l10n::tr("Welcome to Mail"))
        .icon("mail")
        .description(
            crate::l10n::tr("Add your email account to read and send mail here. Most accounts need only your name, address and password."),
        )
        .primary_action(
            enzui_iced_core::Action::new("mail.welcome.add", Msg::WelcomeAddAccount)
                .with_label(crate::l10n::tr("Add Account…")),
        )
        .primary_action_emphasis(enzui_iced_widgets::action_emphasis::ActionEmphasis::Suggested)
        .id_prefix("mail-welcome")
        .into_element()
    };
    let content = move || -> Element<'_, Msg> {
        iced::widget::container(page())
            .center(Length::Fill)
            .padding(s.themes.theme().spacing.lg())
            .into()
    };
    enz_app_shell(content)
        .layout(ShellLayout::TitlebarContent)
        .tokens(shell_tokens().edge_inset(0.0))
        .titlebar(move || mail_titlebar(s, false))
        .on_window_gesture(Msg::Window)
        .into_element()
}

pub(crate) fn wide_shell(s: &State) -> Element<'_, Msg> {
    mail_shell(s)
}

pub(crate) fn medium_with_titlebar(s: &State) -> Element<'_, Msg> {
    mail_shell(s)
}

pub(crate) fn narrow_with_titlebar(s: &State) -> Element<'_, Msg> {
    mail_shell(s)
}

fn mail_shell(s: &State) -> Element<'_, Msg> {
    use enzui_iced_widgets::nav_split::{SidebarPreference, SidebarPresentation};
    let detail = s.mode() == LayoutMode::Narrow
        && (s.narrow_nav.pane() == NarrowPane::Detail || s.compose.open);
    let list_width = if s.mode() == LayoutMode::Wide {
        340.0
    } else {
        320.0
    };
    let mut tokens = shell_tokens()
        .secondary_width(list_width)
        .secondary_leading(true);
    tokens.titlebar.tone = enzui_iced_widgets::surface::SurfaceTone::SurfaceAlt;
    let mut shell = enz_app_shell(move || {
        if s.mode() == LayoutMode::Narrow && !detail {
            list_pane(s, Length::Fill)
        } else {
            reader_pane(s, Length::Fill)
        }
    })
    .layout(ShellLayout::TitlebarSidebarContent)
    .tokens(tokens)
    .breakpoints(s.theme_ref().breakpoints)
    .state(s.nav)
    .forced_by(move |nav| {
        if s.mode() != LayoutMode::Wide {
            SidebarPresentation::Overlay
        } else if nav.preference == SidebarPreference::Hidden {
            SidebarPresentation::Hidden
        } else {
            SidebarPresentation::Inline
        }
    })
    .sidebar_available(!detail)
    .sidebar_label(crate::l10n::tr(SIDEBAR_HEADER_TITLE))
    .on_sidebar_state(Msg::SidebarState)
    .sidebar_titlebar(move || sidebar_titlebar(s))
    .sidebar(move || sidebar_pane(s, Length::Fill))
    .titlebar_background_with_geometry(move |geometry| {
        if s.mode() == LayoutMode::Narrow {
            if detail {
                reader_titlebar_surface(s)
            } else {
                search_surface(s, Space::new().into(), Length::Fill)
            }
        } else if geometry.secondary_content_fits && geometry.split_axis.is_horizontal() {
            row![
                search_surface(s, Space::new().into(), Length::Fixed(list_width)),
                reader_titlebar_surface(s)
            ]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        } else {
            reader_titlebar_surface(s)
        }
    })
    .titlebar(move || mail_titlebar(s, true))
    .on_window_gesture(Msg::Window);
    if s.mode() != LayoutMode::Narrow {
        shell = shell.secondary_content(move || list_pane(s, Length::Fill));
    }
    shell.into_element()
}

fn mail_titlebar(s: &State, main: bool) -> Element<'_, Msg> {
    if !main {
        return enz_title_bar()
            .title(crate::l10n::tr("Mail"))
            .actions(
                vec![enzui_iced_widgets::action_bar::ActionItem::icon(
                    "mail.settings",
                    "settings",
                    crate::l10n::tr("Settings…"),
                    Msg::OpenSettings,
                )],
                Msg::ToggleTitlebarOverflow,
            )
            .action_menu_id_prefix("mail-titlebar-actions")
            .overflow_open(s.titlebar_overflow_open)
            .action_dismiss(Msg::CloseTitlebarOverflow)
            .action_overflow_trigger_id(titlebar_overflow_id())
            .into_element();
    }
    let detail = s.mode() == LayoutMode::Narrow
        && (s.narrow_nav.pane() == NarrowPane::Detail || s.compose.open);
    if s.mode() == LayoutMode::Narrow {
        if s.compose.open {
            return composer_titlebar(s);
        }
        let inset = s.theme_ref().spacing.sm();
        if !detail {
            return container(titlebar_search(s, TITLEBAR_SEARCH_WIDTH))
                .center(Length::Fill)
                .padding([0.0, inset])
                .into();
        }
        let rtl = crate::l10n::rtl();
        return row![
            compact_nav_control(),
            container(reader::titlebar_actions(s))
                .width(Length::Fill)
                .align_x(if rtl {
                    Alignment::Start
                } else {
                    Alignment::End
                }),
        ]
        .mirrored(rtl)
        .align_y(Alignment::Center)
        .spacing(s.theme_ref().spacing.xs())
        .padding([0.0, inset])
        .width(Length::Fill)
        .into();
    }
    let list_width = if s.mode() == LayoutMode::Wide {
        340.0
    } else {
        320.0
    };
    let recovery = if s.mode() != LayoutMode::Wide
        || s.nav.preference == enzui_iced_widgets::nav_split::SidebarPreference::Hidden
    {
        enzui_iced_widgets::toolbar_icon::TitlebarAffordanceTokens::DEFAULT.effective_control_size()
    } else {
        0.0
    };
    let lead = shell_tokens().effective_toggle_lead();
    let search_width = list_width - recovery - lead;
    let reserved_inset = if recovery == 0.0 { lead } else { 0.0 };
    let inset = s.theme_ref().spacing.sm().max(reserved_inset);
    let left_inset = inset - reserved_inset;
    let right_inset = inset + LIST_PANE_DIVIDER_WIDTH;
    let field_width = search_width - left_inset - right_inset;
    row![
        container(titlebar_search(s, field_width))
            .center_y(Length::Fill)
            .padding(Padding::ZERO.left(left_inset).right(right_inset))
            .width(search_width),
        if s.compose.open {
            composer_titlebar(s)
        } else {
            enz_title_bar()
                .tone(enzui_iced_widgets::surface::SurfaceTone::Transparent)
                .id("mail-titlebar")
                .padding_x(s.themes.theme().spacing.xs())
                .center(reader::titlebar_actions(s))
                .into_element()
        },
    ]
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .into()
}

fn reader_titlebar_surface(s: &State) -> Element<'_, Msg> {
    let background = t::reader_background(s.themes.theme());
    container(Space::new())
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(background.into()),
            ..Default::default()
        })
        .into()
}

fn search_surface<'a>(s: &State, content: Element<'a, Msg>, width: Length) -> Element<'a, Msg> {
    let background = t::list_background(s.themes.theme());
    let separator = t::list_separator(s.themes.theme());
    container(row![
        container(content).width(Length::Fill).height(Length::Fill),
        container(
            Space::new()
                .width(LIST_PANE_DIVIDER_WIDTH)
                .height(Length::Fill)
        )
        .style(move |_| container::Style {
            background: Some(separator.into()),
            ..Default::default()
        }),
    ])
    .width(width)
    .height(Length::Fill)
    .style(move |_| container::Style {
        background: Some(background.into()),
        ..Default::default()
    })
    .into()
}

fn composer_titlebar(s: &State) -> Element<'_, Msg> {
    enz_title_bar()
        .tone(enzui_iced_widgets::surface::SurfaceTone::Transparent)
        .id("mail-titlebar")
        .leading(crate::compose_view::back_button(s))
        .into_element()
}

pub(crate) fn titlebar_overflow_id() -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::new("mail-titlebar-overflow")
}

fn compact_nav_control() -> Element<'static, Msg> {
    enz_toolbar_icon("navigation.back", Msg::NarrowBack)
        .variant(ToolbarIconVariant::Standard)
        .label(crate::l10n::tr("Back to message list"))
        .id(iced::advanced::widget::Id::new("mail-titlebar-back"))
        .into_element()
}
