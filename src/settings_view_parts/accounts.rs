use super::*;
use crate::model::settings::OAuthSignInStatus;
use iced::widget::column;

pub(crate) fn accounts_pane(s: &State) -> Element<'_, Msg> {
    if s.settings.add_account.open {
        return add_account_form(s);
    }
    if let Some(id) = s.settings.expanded_account.as_deref()
        && let Some(acct) = s.store.account(id)
    {
        return account_detail(s, acct);
    }

    let enz = s.theme_ref();
    let sp = &enz.spacing;

    let header = row![
        enz_text(account_count_label(s.store.accounts.len()))
            .size(TextSize::Footnote)
            .color(t::text_secondary(enz)),
        Space::new().width(Length::Fill),
        // The only entry point to `backend/config.rs`. The `…` marks that it opens a
        // further surface.
        enz_button(crate::l10n::tr("Add Account…"), Msg::BeginAddAccount)
            .kind(ButtonKind::Primary)
            .size(ButtonSize::Sm)
            .icon("plus"),
    ]
    .mirrored(crate::l10n::rtl())
    .align_y(Alignment::Center)
    .spacing(sp.sm());

    let mut items: Vec<Element<Msg>> = vec![header.into()];
    for acct in &s.store.accounts {
        items.push(account_row(s, acct));
    }
    if s.store.accounts.is_empty() {
        items.push(
            enz_text(crate::l10n::tr(
                "No accounts yet. Add one above to start receiving mail.",
            ))
            .size(TextSize::Footnote)
            .color(t::text_secondary(enz))
            .into(),
        );
    }

    column(items).spacing(sp.md()).into()
}

fn account_row<'a>(s: &'a State, acct: &'a crate::model::types::Account) -> Element<'a, Msg> {
    let _ = s;
    let id = acct.id.clone();
    enz_preferences_row(&acct.display_name)
        .subtitle(&acct.email)
        .icon("user")
        .action(
            match &acct.config {
                Some(cfg) => cfg.protocol.label(),
                // A fixture account has no server config; say so rather than an em dash.
                None => "Fixture",
            },
            Msg::ToggleAccountExpanded(id),
        )
        .into_element()
}

pub(crate) fn account_detail<'a>(
    s: &'a State,
    acct: &'a crate::model::types::Account,
) -> Element<'a, Msg> {
    if s.settings.edit_account.is_open_for(&acct.id) {
        return edit_account_form(s, acct);
    }
    let enz = s.theme_ref();
    let sp = &enz.spacing;
    let id = acct.id.clone();
    let removal_status = s.account_removal_status(&acct.id);
    let account_fields_enabled = removal_status.is_none();

    let mut rows: Vec<Element<Msg>> = Vec::new();
    match &acct.config {
        Some(cfg) => {
            rows.push(
                enz_labeled_content(crate::l10n::tr("Protocol"))
                    .value(cfg.protocol.label())
                    .into_element(),
            );
            rows.push(
                enz_labeled_content(crate::l10n::tr("Incoming"))
                    .value(format!(
                        "{}:{} · {}",
                        cfg.incoming.server,
                        cfg.incoming.port,
                        cfg.incoming.encryption.label()
                    ))
                    .into_element(),
            );
            rows.push(
                enz_labeled_content(crate::l10n::tr("Outgoing"))
                    .value(format!(
                        "{}:{} · {}",
                        cfg.smtp.server,
                        cfg.smtp.port,
                        cfg.smtp.encryption.label()
                    ))
                    .into_element(),
            );
            rows.push(
                enz_labeled_content(crate::l10n::tr("Username"))
                    .value(cfg.incoming.username.clone())
                    .into_element(),
            );
            rows.push(
                enz_labeled_content(crate::l10n::tr("Authentication"))
                    .value(cfg.auth.label())
                    .into_element(),
            );
        }
        None => rows.push(
            enz_text(crate::l10n::tr(
                "No server details attached. Use Add Account to configure a new one.",
            ))
            .size(TextSize::Footnote)
            .color(t::text_secondary(enz))
            .into(),
        ),
    }

    let append_id = id.clone();
    let sig_id = id.clone();

    // The dialog header renders this subpage's Back, so the pane has none.
    // Fixture accounts have no server configuration; configured accounts use the
    // credential-backed transport and must not inherit that warning.
    let (transport_title, transport_body, transport_tone) =
        if acct.config.is_some() && !s.is_fixture_mode() {
            (
                ACCOUNT_CONNECTED_TITLE,
                ACCOUNT_CONNECTED_BODY,
                BannerTone::Info,
            )
        } else {
            (
                ACCOUNT_LOCAL_ONLY_TITLE,
                ACCOUNT_LOCAL_ONLY_BODY,
                BannerTone::Warning,
            )
        };
    let transport_banner: Element<Msg> = enz_banner(transport_title)
        .body(transport_body)
        .tone(transport_tone)
        .dismissible(false)
        .id_prefix("mail-account-detail-transport")
        .into_element();

    let mut items: Vec<Element<Msg>> = vec![
        heading(&acct.display_name, enz),
        enz_text(&acct.email)
            .size(TextSize::Footnote)
            .color(t::text_secondary(enz))
            .into(),
        transport_banner,
        enz_preferences_group(column(rows).spacing(sp.xs()))
            .spacing_scale(*sp)
            .into_element(),
        enz_divider().into_element(),
        enz_checkbox(
            acct.append_signature,
            crate::l10n::tr("Append signature to new messages"),
            move |on| Msg::SetAccountAppendSignature(append_id.clone(), on),
        )
        .description(crate::l10n::tr(
            "When off, the signature is preserved but not auto-inserted.",
        ))
        .enabled(account_fields_enabled)
        .into_element(),
        enz_text_area(&s.signature_editor, move |a| {
            Msg::AccountSignatureAction(sig_id.clone(), a)
        })
        .label(crate::l10n::tr("Signature"))
        .placeholder(crate::l10n::tr("e.g.\n\nDanny\nHost of the Sunday lunches"))
        .size(FieldSize::Sm)
        .enabled(account_fields_enabled)
        .area_height(96.0)
        .into_element(),
    ];
    if !acct.append_signature {
        items.push(
            enz_text(crate::l10n::tr(
                "Signature will be stored but not auto-inserted.",
            ))
            .size(TextSize::Caption)
            .color(t::text_tertiary_on(enz, enz.palette.surface_elevated))
            .into(),
        );
    }
    if acct.config.is_some() && !s.is_fixture_mode() {
        items.push(enz_divider().into_element());
        if let Some((title, body, retryable)) = removal_status {
            items.push(
                enz_banner(title)
                    .body(body)
                    .tone(if retryable {
                        BannerTone::Danger
                    } else {
                        BannerTone::Info
                    })
                    .dismissible(false)
                    .id_prefix("mail-account-removal-status")
                    .into_element(),
            );
            let mut actions = row![]
                .mirrored(crate::l10n::rtl())
                .spacing(sp.sm())
                .align_y(Alignment::Center);
            if retryable {
                actions = actions.push(
                    enz_button(crate::l10n::tr("Retry"), Msg::RetryRemoveAccount)
                        .kind(ButtonKind::Primary),
                );
            }
            actions = actions.push(
                enz_button(crate::l10n::tr("Cancel"), Msg::CancelRemoveAccount)
                    .kind(ButtonKind::Secondary),
            );
            items.push(actions.into());
        } else {
            items.push(
                row![
                    enz_button(
                        crate::l10n::tr("Edit Account"),
                        Msg::BeginEditAccount(id.clone())
                    )
                    .kind(ButtonKind::Secondary),
                    enz_button(
                        crate::l10n::tr(REMOVE_ACCOUNT_LABEL),
                        Msg::RequestRemoveAccount(id)
                    )
                    .kind(ButtonKind::Destructive),
                ]
                .mirrored(crate::l10n::rtl())
                .spacing(sp.sm())
                .align_y(Alignment::Center)
                .into(),
            );
        }
    }
    column(items).spacing(sp.sm()).into()
}

fn edit_account_form<'a>(s: &'a State, acct: &'a crate::model::types::Account) -> Element<'a, Msg> {
    let enz = s.theme_ref();
    let sp = &enz.spacing;
    let edit = &s.settings.edit_account;
    let draft = &edit.draft;
    let status = s.account_edit_status(&acct.id);
    let fields_enabled = status.is_none();

    let incoming = column![
        enz_text(crate::l10n::tr("IMAP (Incoming)"))
            .size(TextSize::Footnote)
            .color(t::text_secondary(enz)),
        enz_text_field(
            crate::l10n::tr("Server"),
            "imap.example.com",
            &draft.imap_server,
            |value| { Msg::EditAccountFieldEdited(DraftField::IncomingServer, value) }
        )
        .enabled(fields_enabled)
        .id_prefix("mail-edit-incoming-server")
        .into_element(),
        enz_text_field(
            crate::l10n::tr("Username"),
            "you@example.com",
            &draft.imap_username,
            |value| { Msg::EditAccountFieldEdited(DraftField::IncomingUsername, value) }
        )
        .enabled(fields_enabled)
        .id_prefix("mail-edit-incoming-username")
        .into_element(),
        row![
            enz_text_field(
                crate::l10n::tr("Incoming port"),
                "993",
                &draft.imap_port,
                |value| { Msg::EditAccountFieldEdited(DraftField::IncomingPort, value) }
            )
            .enabled(fields_enabled)
            .id_prefix("mail-edit-incoming-port")
            .input_purpose(iced::advanced::input_method::Purpose::Digits)
            .into_element(),
            picker(
                "Incoming encryption",
                [Encryption::Tls].into_iter().map(Encryption::label),
                draft.imap_encryption.label(),
                |label| Msg::SetEditAccountIncomingEncryption(
                    Encryption::from_label(&label).unwrap_or_default()
                ),
            )
            .enabled(fields_enabled),
        ]
        .mirrored(crate::l10n::rtl())
        .spacing(sp.sm()),
    ]
    .spacing(sp.sm());

    let outgoing = column![
        enz_text(crate::l10n::tr("SMTP (Outgoing)"))
            .size(TextSize::Footnote)
            .color(t::text_secondary(enz)),
        enz_text_field(
            crate::l10n::tr("Server"),
            "smtp.example.com",
            &draft.smtp_server,
            |value| { Msg::EditAccountFieldEdited(DraftField::SmtpServer, value) }
        )
        .enabled(fields_enabled)
        .id_prefix("mail-edit-smtp-server")
        .into_element(),
        enz_text_field(
            crate::l10n::tr("Username"),
            "you@example.com",
            &draft.smtp_username,
            |value| { Msg::EditAccountFieldEdited(DraftField::SmtpUsername, value) }
        )
        .enabled(fields_enabled)
        .id_prefix("mail-edit-smtp-username")
        .into_element(),
        row![
            enz_text_field(
                crate::l10n::tr("Outgoing port"),
                "465",
                &draft.smtp_port,
                |value| { Msg::EditAccountFieldEdited(DraftField::SmtpPort, value) }
            )
            .enabled(fields_enabled)
            .id_prefix("mail-edit-smtp-port")
            .input_purpose(iced::advanced::input_method::Purpose::Digits)
            .into_element(),
            picker(
                "Outgoing encryption",
                [Encryption::Tls, Encryption::StartTls]
                    .into_iter()
                    .map(Encryption::label),
                draft.smtp_encryption.label(),
                |label| Msg::SetEditAccountSmtpEncryption(
                    Encryption::from_label(&label).unwrap_or_default()
                ),
            )
            .enabled(fields_enabled),
        ]
        .mirrored(crate::l10n::rtl())
        .spacing(sp.sm()),
    ]
    .spacing(sp.sm());

    let password = enz_secure_field(
        &edit.password,
        Msg::EditAccountPasswordEdited,
        edit.password_revealed,
        Msg::ToggleEditAccountPasswordRevealed,
    )
    .label(crate::l10n::tr("New password"))
    .placeholder(crate::l10n::tr("Leave blank to keep the current password"))
    .enabled(fields_enabled)
    .id_prefix("mail-edit-account-password")
    .into_element();

    let status_view: Element<Msg> = match status {
        Some((title, body, retryable)) => {
            let mut content = column![
                enz_banner(title)
                    .body(body)
                    .tone(if retryable {
                        BannerTone::Danger
                    } else {
                        BannerTone::Info
                    })
                    .dismissible(false)
                    .id_prefix("mail-account-edit-status")
                    .into_element(),
            ]
            .spacing(sp.sm());
            if retryable {
                content = content.push(
                    row![
                        enz_button(crate::l10n::tr("Retry"), Msg::RetryEditAccount)
                            .kind(ButtonKind::Primary),
                        enz_button(crate::l10n::tr("Cancel"), Msg::CancelEditAccount)
                            .kind(ButtonKind::Secondary),
                    ]
                    .mirrored(crate::l10n::rtl())
                    .spacing(sp.sm()),
                );
            }
            content.into()
        }
        None => match &edit.error {
            Some(error) => enz_badge(error)
                .tone(BadgeTone::Error)
                .size(BadgeSize::Sm)
                .icon("alert-triangle")
                .into_element(),
            None => Space::new().height(Length::Shrink).into(),
        },
    };

    let actions: Element<Msg> = if status.is_none() {
        row![
            Space::new().width(Length::Fill),
            enz_button(crate::l10n::tr("Cancel"), Msg::CancelEditAccount).kind(ButtonKind::Quiet),
            enz_button(crate::l10n::tr("Save Changes"), Msg::CommitEditAccount)
                .kind(ButtonKind::Primary),
        ]
        .mirrored(crate::l10n::rtl())
        .spacing(sp.sm())
        .align_y(Alignment::Center)
        .into()
    } else {
        Space::new().height(Length::Shrink).into()
    };

    column![
        heading("Edit Account", enz),
        add_account_transport_banner(s, "mail-edit-account-connection"),
        enz_text(crate::l10n::message(
            "mail-settings-for",
            &[("value", acct.email.clone())]
        ))
        .size(TextSize::Footnote)
        .color(t::text_secondary(enz)),
        enz_preferences_group(column![incoming, outgoing, password].spacing(sp.md()))
            .spacing_scale(*sp)
            .into_element(),
        status_view,
        actions,
    ]
    .spacing(sp.md())
    .into()
}

fn add_account_form(s: &State) -> Element<'_, Msg> {
    match s.settings.add_account.step {
        AddAccountStep::Email => add_account_email_step(s),
        AddAccountStep::Configure => add_account_configure_step(s),
    }
}

pub(crate) fn add_account_email_step(s: &State) -> Element<'_, Msg> {
    let enz = s.theme_ref();
    let sp = &enz.spacing;
    let add = &s.settings.add_account;

    // Rendered only when a preset matched, so an unknown domain shows nothing.
    let detected: Element<Msg> = match add.detected_label() {
        Some(label) => enz_badge(&label)
            .tone(BadgeTone::Success)
            .size(BadgeSize::Sm)
            .icon("check")
            .into_element(),
        None => Space::new().height(Length::Shrink).into(),
    };

    let actions = row![
        Space::new().width(Length::Fill),
        enz_button(crate::l10n::tr("Cancel"), Msg::CancelAddAccount).kind(ButtonKind::Quiet),
        {
            let b = enz_button(crate::l10n::tr("Continue"), Msg::AddAccountContinue)
                .kind(ButtonKind::Primary);
            if add.can_continue() {
                b
            } else {
                b.enabled(false)
            }
        },
    ]
    .mirrored(crate::l10n::rtl())
    .spacing(sp.sm())
    .align_y(Alignment::Center);

    let connection_banner = add_account_transport_banner(s, "mail-add-account-connection");

    column![
        // Publish the wizard step as a heading; group titles are decorative.
        heading("Add Account", enz),
        connection_banner,
        enz_preferences_group(
            column![
                enz_text_field(
                    crate::l10n::tr("Your name"),
                    "Full name",
                    &add.name,
                    Msg::AddAccountNameEdited
                )
                .id_prefix("mail-add-account-name")
                .into_element(),
                enz_text_field(
                    crate::l10n::tr("Enter your email address"),
                    "you@example.com",
                    &add.email,
                    Msg::AddAccountEmailEdited,
                )
                .id_prefix("mail-add-account-email")
                .input_purpose(iced::advanced::input_method::Purpose::Email)
                .into_element(),
                detected,
                // AccountConfig excludes this value; the reducer moves it to
                // Secret Service before publishing the account row.
                enz_secure_field(
                    &add.password,
                    Msg::AddAccountPasswordEdited,
                    add.password_revealed,
                    Msg::ToggleAddAccountPasswordRevealed,
                )
                .label(crate::l10n::tr("Password"))
                .placeholder(
                    if add
                        .detected_preset()
                        .is_some_and(|p| p.auth == crate::backend::AuthMethod::OAuth2)
                    {
                        "Leave empty to sign in through your browser"
                    } else {
                        "Account password or app password"
                    },
                )
                .id_prefix("mail-add-account-password")
                .into_element(),
                // The sub-caption sits against the field so it is announced with the field's
                // name.
                Element::from(
                    enz_text(if s.is_fixture_mode() {
                        PASSWORD_FIXTURE_NOTICE
                    } else {
                        PASSWORD_STORAGE_NOTICE
                    })
                    .size(TextSize::Caption)
                    .color(t::text_tertiary_on(enz, enz.palette.surface_elevated)),
                ),
            ]
            .spacing(sp.sm())
        )
        .description(&crate::l10n::tr(
            "Mail looks up your provider's server settings from your address."
        ))
        .spacing_scale(*sp)
        .into_element(),
        actions,
    ]
    .spacing(sp.md())
    .into()
}

pub(crate) fn heading<'a>(text: &'a str, enz: &enzui_iced_theme::EnzTheme) -> Element<'a, Msg> {
    enz_text(text)
        .size_px(enz.typography.section_size)
        .color(t::text_primary(enz))
        .into()
}

fn server_lookup_status(s: &State) -> Element<'_, Msg> {
    use crate::model::settings::ServerLookup;
    match &s.settings.add_account.lookup {
        ServerLookup::Idle => Space::new().height(Length::Shrink).into(),
        ServerLookup::Looking { domain } => row![
            enzui_iced_widgets::spinner::enz_spinner()
                .size(14.0)
                .into_element(),
            enz_text(crate::l10n::message(
                "mail-looking-up",
                &[("domain", domain.clone())]
            ))
            .size(TextSize::Footnote)
            .color(t::text_secondary(s.theme_ref())),
        ]
        .mirrored(crate::l10n::rtl())
        .spacing(s.theme_ref().spacing.xs())
        .align_y(Alignment::Center)
        .into(),
        ServerLookup::Found(line) => enz_badge(line)
            .tone(BadgeTone::Success)
            .size(BadgeSize::Sm)
            .icon("check")
            .into_element(),
        ServerLookup::NotFound { domain } => enz_text(crate::l10n::message(
            "mail-settings-not-found",
            &[("value", domain.to_owned())],
        ))
        .size(TextSize::Footnote)
        .color(t::text_secondary(s.theme_ref()))
        .into(),
    }
}

pub(crate) fn add_account_configure_step(s: &State) -> Element<'_, Msg> {
    let enz = s.theme_ref();
    let sp = &enz.spacing;
    let add = &s.settings.add_account;
    let d = &add.draft;

    let incoming_title = crate::l10n::message(
        "mail-incoming-protocol",
        &[("value", crate::l10n::tr(d.protocol.label()))],
    );

    let incoming = column![
        enz_text(incoming_title)
            .size(TextSize::Footnote)
            .color(t::text_secondary(enz)),
        enz_text_field(
            crate::l10n::tr("Server"),
            "imap.example.com",
            &d.imap_server,
            |v| { Msg::AddAccountFieldEdited(DraftField::IncomingServer, v) }
        )
        .id_prefix("mail-add-incoming-server")
        .into_element(),
        enz_text_field(
            crate::l10n::tr("Username"),
            "you@example.com",
            &d.imap_username,
            |v| { Msg::AddAccountFieldEdited(DraftField::IncomingUsername, v) }
        )
        .id_prefix("mail-add-incoming-username")
        .into_element(),
    ]
    .spacing(sp.sm());

    // SMTP fields show only for protocols that use SMTP; JMAP and Exchange have
    // their own outgoing transport.
    let outgoing: Element<Msg> = if d.protocol.uses_smtp() {
        column![
            enz_text(crate::l10n::tr("SMTP (Outgoing)"))
                .size(TextSize::Footnote)
                .color(t::text_secondary(enz)),
            enz_text_field(
                crate::l10n::tr("Server"),
                "smtp.example.com",
                &d.smtp_server,
                |v| { Msg::AddAccountFieldEdited(DraftField::SmtpServer, v) }
            )
            .id_prefix("mail-add-smtp-server")
            .into_element(),
            enz_text_field(
                crate::l10n::tr("Username"),
                "you@example.com",
                &d.smtp_username,
                |v| { Msg::AddAccountFieldEdited(DraftField::SmtpUsername, v) }
            )
            .id_prefix("mail-add-smtp-username")
            .into_element(),
        ]
        .spacing(sp.sm())
        .into()
    } else {
        enz_text(crate::l10n::tr(
            "SMTP (Outgoing) \u{2014} optional for this protocol",
        ))
        .size(TextSize::Footnote)
        .color(t::text_tertiary_on(enz, enz.palette.surface_elevated))
        .into()
    };

    let advanced = enz_expander(
        crate::l10n::tr("Advanced"),
        add.advanced_expanded,
        Msg::ToggleAddAccountAdvanced,
    )
    .subtitle(crate::l10n::tr("Ports, encryption and authentication"))
    .icon("settings")
    .content(
        column![
            enz_labeled_content(crate::l10n::tr("Protocol"))
                .value("IMAP")
                .into_element(),
            row![
                enz_text_field(crate::l10n::tr("Incoming port"), "993", &d.imap_port, |v| {
                    Msg::AddAccountFieldEdited(DraftField::IncomingPort, v)
                })
                .id_prefix("mail-add-incoming-port")
                .input_purpose(iced::advanced::input_method::Purpose::Digits)
                .into_element(),
                picker(
                    "Incoming encryption",
                    [Encryption::Tls].into_iter().map(Encryption::label),
                    d.imap_encryption.label(),
                    |label| Msg::SetAddAccountIncomingEncryption(
                        Encryption::from_label(&label).unwrap_or_default()
                    ),
                ),
            ]
            .mirrored(crate::l10n::rtl())
            .spacing(sp.sm()),
            row![
                enz_text_field(crate::l10n::tr("Outgoing port"), "465", &d.smtp_port, |v| {
                    Msg::AddAccountFieldEdited(DraftField::SmtpPort, v)
                })
                .id_prefix("mail-add-smtp-port")
                .input_purpose(iced::advanced::input_method::Purpose::Digits)
                .into_element(),
                picker(
                    "Outgoing encryption",
                    [Encryption::Tls, Encryption::StartTls]
                        .into_iter()
                        .map(Encryption::label),
                    d.smtp_encryption.label(),
                    |label| Msg::SetAddAccountSmtpEncryption(
                        Encryption::from_label(&label).unwrap_or_default()
                    ),
                ),
            ]
            .mirrored(crate::l10n::rtl())
            .spacing(sp.sm()),
            picker(
                "Authentication",
                AuthMethod::ALL.iter().map(|a| a.label()),
                d.auth.label(),
                |label| Msg::SetAddAccountAuth(AuthMethod::from_label(&label).unwrap_or_default()),
            ),
        ]
        .spacing(sp.sm())
        .into(),
    );

    let error: Element<Msg> = match &add.error {
        Some(msg) => enz_badge(msg)
            .tone(BadgeTone::Error)
            .size(BadgeSize::Sm)
            .icon("alert-triangle")
            .into_element(),
        None => Space::new().height(Length::Shrink).into(),
    };

    let actions = row![
        Space::new().width(Length::Fill),
        enz_button(crate::l10n::tr("Cancel"), Msg::CancelAddAccount).kind(ButtonKind::Quiet),
        {
            let b = enz_button(crate::l10n::tr("Add Account"), Msg::CommitAddAccount)
                .kind(ButtonKind::Primary);
            if add.can_commit() {
                b
            } else {
                b.enabled(false)
            }
        },
    ]
    .mirrored(crate::l10n::rtl())
    .spacing(sp.sm())
    .align_y(Alignment::Center);

    let connection_banner =
        add_account_transport_banner(s, "mail-add-account-connection-configure");
    let sign_in = oauth_sign_in_row(s);

    column![
        heading("Configure Server", enz),
        connection_banner,
        enz_text(crate::l10n::message(
            "mail-settings-for",
            &[("value", d.email.clone())]
        ))
        .size(TextSize::Footnote)
        .color(t::text_secondary(enz)),
        server_lookup_status(s),
        enz_preferences_group(
            column![
                enz_text_field(
                    crate::l10n::tr("Your name"),
                    "Full name",
                    &d.display_name,
                    |v| { Msg::AddAccountFieldEdited(DraftField::DisplayName, v) }
                )
                .id_prefix("mail-add-display-name")
                .into_element(),
                Element::from(incoming),
                outgoing,
            ]
            .spacing(sp.md())
        )
        .spacing_scale(*sp)
        .into_element(),
        sign_in,
        advanced.into_element(),
        error,
        actions,
    ]
    .spacing(sp.md())
    .into()
}

// The browser sign-in for an OAuth account, in place of a password.
// Nothing renders for a password account.
fn oauth_sign_in_row(s: &State) -> Element<'_, Msg> {
    let enz = s.theme_ref();
    let sp = &enz.spacing;
    let add = &s.settings.add_account;
    if add.draft.auth != crate::backend::AuthMethod::OAuth2 {
        return Space::new().height(Length::Shrink).into();
    }
    let Some(provider) = add.oauth_provider() else {
        return enz_badge(&crate::l10n::tr(
            "Mail has no browser sign-in for this server. Choose Password under Advanced.",
        ))
        .tone(BadgeTone::Warning)
        .size(BadgeSize::Sm)
        .icon("alert-triangle")
        .into_element();
    };
    let label = provider.label();
    let (status, control): (Element<Msg>, Element<Msg>) = match &add.oauth {
        OAuthSignInStatus::NotStarted => (
            enz_text(crate::l10n::message(
                "mail-provider-sign-in-info",
                &[("value", label.to_owned())],
            ))
            .size(TextSize::Footnote)
            .color(t::text_secondary(enz))
            .into(),
            enz_button(
                crate::l10n::message("mail-sign-in-with", &[("value", label.to_owned())]),
                Msg::AddAccountOAuthSignIn,
            )
            .kind(ButtonKind::Primary)
            .into(),
        ),
        OAuthSignInStatus::Waiting { provider } => (
            enz_text(crate::l10n::message(
                "mail-waiting-provider",
                &[("value", (*provider).to_owned())],
            ))
            .size(TextSize::Footnote)
            .color(t::text_secondary(enz))
            .into(),
            enz_button(
                crate::l10n::tr("Cancel sign-in"),
                Msg::AddAccountOAuthCancel,
            )
            .kind(ButtonKind::Ghost)
            .into(),
        ),
        OAuthSignInStatus::SignedIn { provider } => (
            enz_badge(&crate::l10n::message(
                "mail-signed-in-with",
                &[("value", (*provider).to_owned())],
            ))
            .tone(BadgeTone::Success)
            .size(BadgeSize::Sm)
            .icon("check")
            .into_element(),
            enz_button(crate::l10n::tr("Sign in again"), Msg::AddAccountOAuthSignIn)
                .kind(ButtonKind::Ghost)
                .into(),
        ),
        OAuthSignInStatus::Failed(message) => (
            enz_badge(message)
                .tone(BadgeTone::Error)
                .size(BadgeSize::Sm)
                .icon("alert-triangle")
                .into_element(),
            enz_button(crate::l10n::tr("Try again"), Msg::AddAccountOAuthSignIn)
                .kind(ButtonKind::Secondary)
                .into(),
        ),
    };
    let announced = match &add.oauth {
        OAuthSignInStatus::NotStarted => None,
        OAuthSignInStatus::Waiting { provider } => Some(crate::l10n::message(
            "mail-waiting-provider-announcement",
            &[("value", (*provider).to_owned())],
        )),
        OAuthSignInStatus::SignedIn { provider } => Some(crate::l10n::message(
            "mail-signed-in-with",
            &[("value", (*provider).to_owned())],
        )),
        OAuthSignInStatus::Failed(message) => Some(message.clone()),
    };
    let status: Element<Msg> = match announced {
        Some(text) => enzui_iced_widgets::semantics_container::SemanticsContainer::new(
            status,
            enzui_iced_core::live_region::Announcement::Status.region(text),
        )
        .into(),
        None => status,
    };
    enz_preferences_group(
        column![
            status,
            row![Space::new().width(Length::Fill), control]
                .mirrored(crate::l10n::rtl())
                .align_y(Alignment::Center),
        ]
        .spacing(sp.sm()),
    )
    .title(&crate::l10n::tr("Sign-in"))
    .spacing_scale(*sp)
    .into_element()
}

fn picker<'a, I>(
    label: &str,
    options: I,
    selected: &str,
    on_select: impl Fn(String) -> Msg + Clone + 'a,
) -> enzui_iced_widgets::combo_box::EnzComboBox<'a, Msg>
where
    I: IntoIterator<Item = &'static str>,
{
    let options: Vec<_> = options.into_iter().collect();
    let selected_index = options.iter().position(|option| *option == selected);
    let unchanged = on_select.clone();
    let canonical_selected = selected.to_owned();
    let invalid_index = canonical_selected.clone();
    enz_combo_box(
        crate::l10n::tr(label),
        options
            .iter()
            .map(|option| crate::l10n::tr(option))
            .collect(),
        None,
        move |_| unchanged(canonical_selected.clone()),
    )
    .id(enzui_iced_core::identity::derive_id(
        "mail-account-picker",
        label,
    ))
    .selected_index(selected_index)
    .on_select_index(move |index| {
        on_select(
            options
                .get(index)
                .map_or_else(|| invalid_index.clone(), |option| (*option).to_owned()),
        )
    })
}
