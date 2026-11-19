//! Validated account configuration excludes credentials; Secret Service owns passwords and grants.
//! Persisted protocol variants remain readable even when the production transport refuses them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(crate) enum MailProtocol {
    #[default]
    Imap,
    Pop3,
    // JMAP (RFC 8620/8621), handling both directions; `AccountConfig` still
    // carries an SMTP fallback.
    Jmap,
    Exchange,
}

impl MailProtocol {
    pub(crate) fn label(self) -> &'static str {
        match self {
            MailProtocol::Imap => "IMAP",
            MailProtocol::Pop3 => "POP3",
            MailProtocol::Jmap => "JMAP",
            MailProtocol::Exchange => "Exchange",
        }
    }

    pub(crate) fn uses_smtp(self) -> bool {
        matches!(self, MailProtocol::Imap | MailProtocol::Pop3)
    }

    pub(crate) fn default_incoming_port(self, encryption: Encryption) -> u16 {
        match self {
            MailProtocol::Imap => encryption.default_imap_port(),
            MailProtocol::Pop3 => match encryption {
                Encryption::Tls => 995,
                _ => 110,
            },
            MailProtocol::Jmap | MailProtocol::Exchange => 443,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AccountConfig {
    // Opaque id assigned by the model when the account is added. Kept here so
    // backends and the sidebar can round-trip it as a plain string.
    pub(crate) id: String,
    // Non-secret Secret Service owner component. New accounts begin with
    // their account id; connection-setting edits rotate it before publication.
    pub(crate) credential_owner: String,
    pub(crate) display_name: String,
    pub(crate) email: String,
    pub(crate) protocol: MailProtocol,
    // Incoming server config. For IMAP this is the traditional server;
    // for POP3 this holds the POP3 server; for JMAP / Exchange this
    // carries the HTTPS endpoint (port 443, TLS on).
    pub(crate) incoming: IncomingConfig,
    pub(crate) imap: ImapConfig,
    pub(crate) smtp: SmtpConfig,
    // Auth method both sides use. `OAuth2` signs in with a token refreshed
    // from the grant the browser sign-in stored.
    pub(crate) auth: AuthMethod,
}

// Incoming server settings that generalise across every supported
// protocol. IMAP + POP3 populate `server` / `port` / `username` /
// `encryption`; JMAP + Exchange populate `server` (the HTTPS endpoint)
// with `port = 443` and `encryption = Tls`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IncomingConfig {
    pub(crate) server: String,
    pub(crate) port: u16,
    pub(crate) username: String,
    pub(crate) encryption: Encryption,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ImapConfig {
    pub(crate) server: String,
    pub(crate) port: u16,
    // The account's IMAP login name (often the full email; some servers use
    // only the local part).
    pub(crate) username: String,
    pub(crate) encryption: Encryption,
}

impl From<IncomingConfig> for ImapConfig {
    fn from(v: IncomingConfig) -> Self {
        ImapConfig {
            server: v.server,
            port: v.port,
            username: v.username,
            encryption: v.encryption,
        }
    }
}

impl From<&IncomingConfig> for ImapConfig {
    fn from(v: &IncomingConfig) -> Self {
        ImapConfig {
            server: v.server.clone(),
            port: v.port,
            username: v.username.clone(),
            encryption: v.encryption,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SmtpConfig {
    pub(crate) server: String,
    pub(crate) port: u16,
    pub(crate) username: String,
    pub(crate) encryption: Encryption,
}

// Connection encryption. Matches Apple Mail's "Connection Security" picker.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(crate) enum Encryption {
    // Plaintext; only for closed-network testing.
    None,
    StartTls,
    // Implicit TLS on the secure port (IMAP 993 / SMTP 465). Recommended.
    #[default]
    Tls,
}

impl Encryption {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Encryption::None => "None",
            Encryption::StartTls => "STARTTLS",
            Encryption::Tls => "SSL/TLS",
        }
    }

    pub(crate) fn from_label(label: &str) -> Option<Self> {
        let normalized = label.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "none" | "plain" | "plaintext" => Some(Encryption::None),
            "starttls" | "start-tls" | "start tls" => Some(Encryption::StartTls),
            "tls" | "ssl" | "ssl/tls" => Some(Encryption::Tls),
            _ => None,
        }
    }

    pub(crate) fn default_imap_port(self) -> u16 {
        match self {
            Encryption::None | Encryption::StartTls => 143,
            Encryption::Tls => 993,
        }
    }

    pub(crate) fn default_smtp_port(self) -> u16 {
        match self {
            Encryption::None => 25,
            Encryption::StartTls => 587,
            Encryption::Tls => 465,
        }
    }
}

// Authentication method. `OAuth2` is the browser sign-in for Gmail and
// Outlook (`curl_backend::oauth`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(crate) enum AuthMethod {
    #[default]
    Plain,
    Login,
    OAuth2,
}

impl AuthMethod {
    pub(crate) const ALL: [AuthMethod; 3] =
        [AuthMethod::Plain, AuthMethod::Login, AuthMethod::OAuth2];

    pub(crate) fn label(self) -> &'static str {
        match self {
            AuthMethod::Plain => "Password",
            AuthMethod::Login => "Login",
            AuthMethod::OAuth2 => "OAuth 2.0",
        }
    }

    pub(crate) fn from_label(label: &str) -> Option<Self> {
        let l = label.trim().to_ascii_lowercase();
        match l.as_str() {
            "password" | "plain" => Some(AuthMethod::Plain),
            "login" => Some(AuthMethod::Login),
            "oauth 2.0" | "oauth" | "oauth2" => Some(AuthMethod::OAuth2),
            _ => None,
        }
    }
}

// This conservative address parser gates configuration; it does not implement full RFC 5322.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ParsedEmail {
    pub(crate) local: String,
    pub(crate) domain: String,
}

impl ParsedEmail {
    pub(crate) fn as_string(&self) -> String {
        format!("{}@{}", self.local, self.domain)
    }
}

pub(crate) fn parse_email(input: &str) -> Option<ParsedEmail> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (local, domain) = trimmed.split_once('@')?;
    if local.is_empty() || domain.is_empty() {
        return None;
    }
    // The local part cannot start or end with a dot, and cannot contain two
    // consecutive dots. Everything else is a single character-class check.
    if local.starts_with('.') || local.ends_with('.') || local.contains("..") {
        return None;
    }
    if !local
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
    {
        return None;
    }
    // The domain must have at least one dot (so `x@localhost` fails, matching
    // Apple Mail's Continue gate) and only letters/digits/hyphens/dots.
    if !domain.contains('.') {
        return None;
    }
    if !domain
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    {
        return None;
    }
    if domain.starts_with('.') || domain.ends_with('.') || domain.contains("..") {
        return None;
    }
    for label in domain.split('.') {
        if label.starts_with('-') || label.ends_with('-') || label.is_empty() {
            return None;
        }
    }
    Some(ParsedEmail {
        local: local.to_owned(),
        domain: domain.to_ascii_lowercase(),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProviderPreset {
    pub(crate) display_name: &'static str,
    pub(crate) domain: &'static str,
    pub(crate) protocol: MailProtocol,
    pub(crate) imap_server: &'static str,
    pub(crate) imap_encryption: Encryption,
    pub(crate) smtp_server: &'static str,
    pub(crate) smtp_encryption: Encryption,
    // Recommended auth method: OAuth 2.0 for Gmail, Outlook and Fastmail, plain
    // password otherwise.
    pub(crate) auth: AuthMethod,
}

impl ProviderPreset {
    pub(crate) fn imap_port(&self) -> u16 {
        self.imap_encryption.default_imap_port()
    }
    pub(crate) fn smtp_port(&self) -> u16 {
        self.smtp_encryption.default_smtp_port()
    }
}

pub(crate) const PROVIDER_PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        display_name: "iCloud",
        domain: "icloud.com",
        protocol: MailProtocol::Imap,
        imap_server: "imap.mail.me.com",
        imap_encryption: Encryption::Tls,
        smtp_server: "smtp.mail.me.com",
        smtp_encryption: Encryption::StartTls,
        auth: AuthMethod::Plain,
    },
    ProviderPreset {
        display_name: "iCloud",
        domain: "me.com",
        protocol: MailProtocol::Imap,
        imap_server: "imap.mail.me.com",
        imap_encryption: Encryption::Tls,
        smtp_server: "smtp.mail.me.com",
        smtp_encryption: Encryption::StartTls,
        auth: AuthMethod::Plain,
    },
    ProviderPreset {
        display_name: "Gmail",
        domain: "gmail.com",
        protocol: MailProtocol::Imap,
        imap_server: "imap.gmail.com",
        imap_encryption: Encryption::Tls,
        smtp_server: "smtp.gmail.com",
        smtp_encryption: Encryption::Tls,
        auth: AuthMethod::OAuth2,
    },
    ProviderPreset {
        display_name: "Outlook",
        domain: "outlook.com",
        protocol: MailProtocol::Imap,
        imap_server: "outlook.office365.com",
        imap_encryption: Encryption::Tls,
        smtp_server: "smtp.office365.com",
        smtp_encryption: Encryption::StartTls,
        auth: AuthMethod::OAuth2,
    },
    ProviderPreset {
        display_name: "Outlook",
        domain: "hotmail.com",
        protocol: MailProtocol::Imap,
        imap_server: "outlook.office365.com",
        imap_encryption: Encryption::Tls,
        smtp_server: "smtp.office365.com",
        smtp_encryption: Encryption::StartTls,
        auth: AuthMethod::OAuth2,
    },
    ProviderPreset {
        display_name: "Yahoo",
        domain: "yahoo.com",
        protocol: MailProtocol::Imap,
        imap_server: "imap.mail.yahoo.com",
        imap_encryption: Encryption::Tls,
        smtp_server: "smtp.mail.yahoo.com",
        smtp_encryption: Encryption::Tls,
        auth: AuthMethod::Plain,
    },
    ProviderPreset {
        display_name: "Fastmail",
        domain: "fastmail.com",
        protocol: MailProtocol::Imap,
        imap_server: "imap.fastmail.com",
        imap_encryption: Encryption::Tls,
        smtp_server: "smtp.fastmail.com",
        smtp_encryption: Encryption::Tls,
        auth: AuthMethod::OAuth2,
    },
];

pub(crate) fn preset_for_domain(domain: &str) -> Option<&'static ProviderPreset> {
    let target = domain.trim().to_ascii_lowercase();
    PROVIDER_PRESETS.iter().find(|p| p.domain == target)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct AccountConfigDraft {
    pub(crate) display_name: String,
    pub(crate) email: String,
    pub(crate) protocol: MailProtocol,
    pub(crate) imap_server: String,
    pub(crate) imap_port: String,
    pub(crate) imap_username: String,
    pub(crate) imap_encryption: Encryption,
    pub(crate) smtp_server: String,
    pub(crate) smtp_port: String,
    pub(crate) smtp_username: String,
    pub(crate) smtp_encryption: Encryption,
    pub(crate) auth: AuthMethod,
}

impl AccountConfigDraft {
    // Fresh draft: encryption defaults to TLS with the matching ports.
    pub(crate) fn new() -> Self {
        Self {
            protocol: MailProtocol::Imap,
            imap_encryption: Encryption::Tls,
            smtp_encryption: Encryption::StartTls,
            imap_port: Encryption::Tls.default_imap_port().to_string(),
            smtp_port: Encryption::StartTls.default_smtp_port().to_string(),
            auth: AuthMethod::Plain,
            ..Self::default()
        }
    }

    pub(crate) fn from_config(config: &AccountConfig) -> Self {
        Self {
            display_name: config.display_name.clone(),
            email: config.email.clone(),
            protocol: config.protocol,
            imap_server: config.incoming.server.clone(),
            imap_port: config.incoming.port.to_string(),
            imap_username: config.incoming.username.clone(),
            imap_encryption: config.incoming.encryption,
            smtp_server: config.smtp.server.clone(),
            smtp_port: config.smtp.port.to_string(),
            smtp_username: config.smtp.username.clone(),
            smtp_encryption: config.smtp.encryption,
            auth: config.auth,
        }
    }

    pub(crate) fn from_preset(email: &ParsedEmail, preset: &ProviderPreset) -> Self {
        let mut d = Self::new();
        preset.display_name.clone_into(&mut d.display_name);
        d.email = email.as_string();
        d.protocol = preset.protocol;
        preset.imap_server.clone_into(&mut d.imap_server);
        d.imap_port = preset.imap_port().to_string();
        d.imap_username = email.as_string();
        d.imap_encryption = preset.imap_encryption;
        preset.smtp_server.clone_into(&mut d.smtp_server);
        d.smtp_port = preset.smtp_port().to_string();
        d.smtp_username = email.as_string();
        d.smtp_encryption = preset.smtp_encryption;
        d.auth = preset.auth;
        d
    }

    pub(crate) fn apply_discovered(&mut self, found: &super::discovery::Discovered) {
        self.protocol = MailProtocol::Imap;
        self.imap_server.clone_from(&found.imap_server);
        self.imap_port = found.imap_port.to_string();
        self.imap_username.clone_from(&found.imap_username);
        self.imap_encryption = found.imap_encryption;
        self.smtp_server.clone_from(&found.smtp_server);
        self.smtp_port = found.smtp_port.to_string();
        self.smtp_username.clone_from(&found.smtp_username);
        self.smtp_encryption = found.smtp_encryption;
        self.auth = found.auth;
    }

    pub(crate) fn from_email(email: &ParsedEmail) -> Self {
        let mut d = Self::new();
        d.email = email.as_string();
        d.display_name.clone_from(&email.local);
        d.imap_username = email.as_string();
        d.smtp_username = email.as_string();
        d
    }

    pub(crate) fn set_imap_encryption(&mut self, enc: Encryption) {
        let previous_default = self
            .protocol
            .default_incoming_port(self.imap_encryption)
            .to_string();
        let was_default = self.imap_port == previous_default;
        self.imap_encryption = enc;
        if was_default {
            self.imap_port = self.protocol.default_incoming_port(enc).to_string();
        }
    }

    pub(crate) fn set_smtp_encryption(&mut self, enc: Encryption) {
        let was_default = self.smtp_port == self.smtp_encryption.default_smtp_port().to_string();
        self.smtp_encryption = enc;
        if was_default {
            self.smtp_port = enc.default_smtp_port().to_string();
        }
    }

    pub(crate) fn validate(&self, id: impl Into<String>) -> Result<AccountConfig, DraftError> {
        let email = parse_email(&self.email)
            .ok_or_else(|| DraftError::field("email", "Enter a valid email address"))?;
        if self.display_name.trim().is_empty() {
            return Err(DraftError::field("display_name", "Enter a display name"));
        }
        if self.imap_server.trim().is_empty() {
            return Err(DraftError::field("imap_server", "IMAP server is required"));
        }
        if self.smtp_server.trim().is_empty() {
            return Err(DraftError::field("smtp_server", "SMTP server is required"));
        }
        let imap_port = parse_port(&self.imap_port)
            .ok_or_else(|| DraftError::field("imap_port", "IMAP port must be 1–65535"))?;
        let smtp_port = parse_port(&self.smtp_port)
            .ok_or_else(|| DraftError::field("smtp_port", "SMTP port must be 1–65535"))?;
        if self.imap_username.trim().is_empty() {
            return Err(DraftError::field(
                "imap_username",
                "IMAP username is required",
            ));
        }
        if self.smtp_username.trim().is_empty() {
            return Err(DraftError::field(
                "smtp_username",
                "SMTP username is required",
            ));
        }
        let incoming = IncomingConfig {
            server: self.imap_server.trim().to_owned(),
            port: imap_port,
            username: self.imap_username.trim().to_owned(),
            encryption: self.imap_encryption,
        };
        let id = id.into();
        Ok(AccountConfig {
            credential_owner: id.clone(),
            id,
            display_name: self.display_name.trim().to_owned(),
            email: email.as_string(),
            protocol: self.protocol,
            imap: ImapConfig::from(&incoming),
            incoming,
            smtp: SmtpConfig {
                server: self.smtp_server.trim().to_owned(),
                port: smtp_port,
                username: self.smtp_username.trim().to_owned(),
                encryption: self.smtp_encryption,
            },
            auth: self.auth,
        })
    }
}

fn parse_port(s: &str) -> Option<u16> {
    let n: u32 = s.trim().parse().ok()?;
    if (1..=65_535).contains(&n) {
        Some(n as u16)
    } else {
        None
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DraftError {
    pub(crate) field: &'static str,
    pub(crate) message: &'static str,
}

impl DraftError {
    pub(crate) fn field(field: &'static str, message: &'static str) -> Self {
        Self { field, message }
    }
}
