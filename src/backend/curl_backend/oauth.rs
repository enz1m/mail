//! Browser OAuth uses RFC 8252 and Secret Service refresh grants.
//! Access tokens and rotated refresh tokens remain process-local.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use curl::easy::{Easy, List};
use enzui_services::credential_store::Secret;
use enzui_services::oauth::loopback::{LoopbackCancel, LoopbackError, LoopbackRedirect};
use enzui_services::oauth::{
    CallbackError, ClientConfig, MAX_TOKEN_RESPONSE_BYTES, PendingAuthorization, SecretText,
    TokenError, TokenRequest, TokenResponse,
};

use super::types::{MailFailure, MailFailureKind, MailResult};
use super::{AccountConfig, CONNECT_TIMEOUT, TOTAL_TIMEOUT};

// The first line of an encoded grant.
const GRANT_HEADER: &str = "enzui-mail-oauth/1";

pub(crate) const SIGN_IN_WAIT: Duration = Duration::from_secs(300);

const EXPIRY_MARGIN: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum OAuthProvider {
    Google,
    Microsoft,
}

impl OAuthProvider {
    pub(crate) fn for_server(host: &str) -> Option<Self> {
        match host.trim().to_ascii_lowercase().as_str() {
            "imap.gmail.com" | "smtp.gmail.com" => Some(Self::Google),
            "outlook.office365.com" | "smtp.office365.com" | "smtp-mail.outlook.com" => {
                Some(Self::Microsoft)
            }
            _ => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Google => "Google",
            Self::Microsoft => "Microsoft",
        }
    }

    fn token(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::Microsoft => "microsoft",
        }
    }

    fn from_token(token: &str) -> Option<Self> {
        match token {
            "google" => Some(Self::Google),
            "microsoft" => Some(Self::Microsoft),
            _ => None,
        }
    }

    fn authorization_endpoint(self) -> &'static str {
        match self {
            Self::Google => "https://accounts.google.com/o/oauth2/v2/auth",
            Self::Microsoft => "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
        }
    }

    fn token_endpoint(self) -> &'static str {
        match self {
            Self::Google => "https://oauth2.googleapis.com/token",
            Self::Microsoft => "https://login.microsoftonline.com/common/oauth2/v2.0/token",
        }
    }

    fn scopes(self) -> &'static [&'static str] {
        match self {
            Self::Google => &["https://mail.google.com/"],
            Self::Microsoft => &[
                "https://outlook.office.com/IMAP.AccessAsUser.All",
                "https://outlook.office.com/SMTP.Send",
                "offline_access",
            ],
        }
    }

    fn extra(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Google => &[("access_type", "offline"), ("prompt", "consent")],
            Self::Microsoft => &[],
        }
    }

    fn configured_client(self) -> (Option<String>, Option<String>) {
        let (build_id, build_secret, id_var, secret_var) = match self {
            Self::Google => (
                option_env!("ENZUI_MAIL_GOOGLE_CLIENT_ID"),
                option_env!("ENZUI_MAIL_GOOGLE_CLIENT_SECRET"),
                "ENZUI_MAIL_GOOGLE_CLIENT_ID",
                "ENZUI_MAIL_GOOGLE_CLIENT_SECRET",
            ),
            Self::Microsoft => (
                option_env!("ENZUI_MAIL_MICROSOFT_CLIENT_ID"),
                None,
                "ENZUI_MAIL_MICROSOFT_CLIENT_ID",
                "ENZUI_MAIL_MICROSOFT_CLIENT_SECRET",
            ),
        };
        let read = |build: Option<&str>, var: &str| {
            build
                .map(str::to_owned)
                .or_else(|| std::env::var(var).ok())
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        (read(build_id, id_var), read(build_secret, secret_var))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct OAuthClient {
    provider: OAuthProvider,
    config: ClientConfig,
    token_endpoint: String,
    client_id: String,
    client_secret: Option<SecretText>,
}

impl OAuthClient {
    pub(crate) fn configured(provider: OAuthProvider) -> Result<Self, String> {
        #[cfg(test)]
        if let Some(client) = TEST_CLIENT.with(|slot| slot.borrow().clone()) {
            return Ok(client);
        }
        let (id, secret) = provider.configured_client();
        let Some(id) = id else {
            let var = match provider {
                OAuthProvider::Google => "ENZUI_MAIL_GOOGLE_CLIENT_ID",
                OAuthProvider::Microsoft => "ENZUI_MAIL_MICROSOFT_CLIENT_ID",
            };
            return Err(format!(
                "This build has no {} client ID, so it cannot sign in with {}. Set {var}, or use an app password.",
                provider.label(),
                provider.label()
            ));
        };
        Self::with_endpoints(
            provider,
            provider.authorization_endpoint(),
            provider.token_endpoint(),
            &id,
            secret.map(SecretText::new),
        )
        .map_err(|error| format!("The {} client is misconfigured: {error}", provider.label()))
    }

    pub(crate) fn with_endpoints(
        provider: OAuthProvider,
        authorization_endpoint: &str,
        token_endpoint: &str,
        client_id: &str,
        client_secret: Option<SecretText>,
    ) -> Result<Self, String> {
        let mut config = ClientConfig::new(authorization_endpoint, token_endpoint, client_id)
            .map_err(|e| e.to_string())?
            .with_scopes(provider.scopes().iter().copied())
            .map_err(|e| e.to_string())?;
        if let Some(secret) = &client_secret {
            config = config.with_client_secret(secret.clone());
        }
        Ok(Self {
            provider,
            config,
            token_endpoint: token_endpoint.to_owned(),
            client_id: client_id.to_owned(),
            client_secret,
        })
    }
}

struct Grant {
    provider: OAuthProvider,
    client_id: String,
    client_secret: Option<SecretText>,
    token_endpoint: String,
    refresh_token: SecretText,
}

impl Grant {
    fn encode(&self) -> Secret {
        let mut text = format!(
            "{GRANT_HEADER}\nprovider={}\nclient_id={}\ntoken_endpoint={}\n",
            self.provider.token(),
            self.client_id,
            self.token_endpoint
        );
        if let Some(secret) = &self.client_secret {
            text.push_str("client_secret=");
            text.push_str(secret.expose_secret());
            text.push('\n');
        }
        text.push_str("refresh_token=");
        text.push_str(self.refresh_token.expose_secret());
        text.push('\n');
        Secret::new(text.into_bytes())
    }

    fn decode(secret: &Secret) -> MailResult<Self> {
        let invalid = || {
            MailFailure::new(
                MailFailureKind::Authentication,
                "the stored OAuth grant is unreadable; sign in again",
            )
        };
        let text = std::str::from_utf8(secret.expose_secret()).map_err(|_| invalid())?;
        let mut lines = text.lines();
        if lines.next() != Some(GRANT_HEADER) {
            return Err(invalid());
        }
        let (mut provider, mut client_id, mut client_secret, mut endpoint, mut refresh) =
            (None, None, None, None, None);
        for line in lines {
            let (name, value) = line.split_once('=').ok_or_else(invalid)?;
            let slot = match name {
                "provider" => &mut provider,
                "client_id" => &mut client_id,
                "client_secret" => &mut client_secret,
                "token_endpoint" => &mut endpoint,
                "refresh_token" => &mut refresh,
                _ => return Err(invalid()),
            };
            if slot.replace(value.to_owned()).is_some() {
                return Err(invalid());
            }
        }
        let refresh = refresh
            .filter(|token| !token.is_empty())
            .ok_or_else(invalid)?;
        Ok(Self {
            provider: provider
                .as_deref()
                .and_then(OAuthProvider::from_token)
                .ok_or_else(invalid)?,
            client_id: client_id.filter(|id| !id.is_empty()).ok_or_else(invalid)?,
            client_secret: client_secret.map(SecretText::new),
            token_endpoint: endpoint.ok_or_else(invalid)?,
            refresh_token: SecretText::new(refresh),
        })
    }

    fn client(&self) -> MailResult<ClientConfig> {
        // The authorization endpoint is not used to refresh; the token
        // endpoint stands in so the framework's endpoint check still runs.
        let mut config =
            ClientConfig::new(&self.token_endpoint, &self.token_endpoint, &self.client_id)
                .map_err(|error| {
                    MailFailure::new(MailFailureKind::Configuration, error.to_string())
                })?;
        if let Some(secret) = &self.client_secret {
            config = config.with_client_secret(secret.clone());
        }
        Ok(config)
    }
}

pub(crate) fn is_grant(secret: &Secret) -> bool {
    secret
        .expose_secret()
        .starts_with(format!("{GRANT_HEADER}\n").as_bytes())
}

pub(crate) struct OAuthSignIn {
    redirect: LoopbackRedirect,
    pending: PendingAuthorization,
    client: OAuthClient,
}

impl std::fmt::Debug for OAuthSignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthSignIn")
            .field("provider", &self.client.provider)
            .field("pending", &self.pending)
            .finish_non_exhaustive()
    }
}

pub(crate) struct OAuthSignInStart {
    pub(crate) sign_in: OAuthSignIn,
    pub(crate) authorization_url: String,
    pub(crate) cancel: LoopbackCancel,
}

pub(crate) fn begin_sign_in(
    client: OAuthClient,
    login_hint: &str,
) -> Result<OAuthSignInStart, String> {
    let redirect = LoopbackRedirect::bind("/oauth2/callback")
        .map_err(|error| format!("Could not start the sign-in listener: {error}"))?
        .with_page(SIGNED_IN_PAGE);
    let mut extra: Vec<(&str, &str)> = client.provider.extra().to_vec();
    if !login_hint.is_empty() {
        extra.push(("login_hint", login_hint));
    }
    let pending = client
        .config
        .begin(redirect.redirect_uri(), &extra)
        .map_err(|error| format!("Could not start the sign-in: {error}"))?;
    Ok(OAuthSignInStart {
        authorization_url: pending.authorization_url().to_owned(),
        cancel: redirect.canceller(),
        sign_in: OAuthSignIn {
            redirect,
            pending,
            client,
        },
    })
}

const SIGNED_IN_PAGE: &str = "<!doctype html><html><head><meta charset=\"utf-8\">\
<title>Signed in to Mail</title></head><body style=\"font-family:sans-serif;margin:3em\">\
<p>You are signed in. Close this tab and return to Mail.</p></body></html>";

// Why a sign-in did not produce a grant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SignInError {
    // The person cancelled, or the dialog closed.
    Cancelled,
    // The browser never came back.
    TimedOut,
    Failed(String),
}

impl SignInError {
    pub(crate) fn user_message(&self) -> String {
        match self {
            Self::Cancelled => "Sign-in was cancelled.".into(),
            Self::TimedOut => "The browser did not return from the sign-in in time.".into(),
            Self::Failed(message) => message.clone(),
        }
    }
}

impl OAuthSignIn {
    // Waits for the browser, checks the callback and trades the code for
    // tokens. Blocks: run it off the UI thread. Returns the grant to store.
    pub(crate) fn finish(self, wait: Duration) -> Result<Secret, SignInError> {
        // Only this sign-in's callback ends the wait: a forged or stale one
        // is refused and the listener keeps the port.
        let pending = &self.pending;
        let callback = self
            .redirect
            .wait_for(Instant::now() + wait, |callback| {
                pending.is_callback(callback)
            })
            .map_err(|error| match error {
                LoopbackError::Cancelled => SignInError::Cancelled,
                LoopbackError::TimedOut => SignInError::TimedOut,
                LoopbackError::Io(error) => {
                    SignInError::Failed(format!("The sign-in listener failed: {error}"))
                }
                other => SignInError::Failed(other.to_string()),
            })?;
        let code = self
            .pending
            .complete(&callback)
            .map_err(|error| match error {
                CallbackError::Provider(provider) if provider.error == "access_denied" => {
                    SignInError::Failed(format!(
                        "{} sign-in was declined.",
                        self.client.provider.label()
                    ))
                }
                other => SignInError::Failed(format!("The sign-in response was refused: {other}")),
            })?;
        let tokens = post_token_request(&self.pending.token_request(&code)).map_err(|failure| {
            SignInError::Failed(format!(
                "The token exchange failed: {}",
                failure.diagnostic()
            ))
        })?;
        let refresh_token = tokens.refresh_token.clone().ok_or_else(|| {
            SignInError::Failed(format!(
                "{} issued no refresh token, so Mail could not stay signed in.",
                self.client.provider.label()
            ))
        })?;
        let grant = Grant {
            provider: self.client.provider,
            client_id: self.client.client_id.clone(),
            client_secret: self.client.client_secret.clone(),
            token_endpoint: self.client.token_endpoint.clone(),
            refresh_token,
        };
        if [&grant.client_id, &grant.token_endpoint]
            .iter()
            .any(|value| value.contains(['\n', '\r']))
            || grant.refresh_token.expose_secret().contains(['\n', '\r'])
            || grant
                .client_secret
                .as_ref()
                .is_some_and(|secret| secret.expose_secret().contains(['\n', '\r']))
        {
            return Err(SignInError::Failed(
                "the provider returned an unusable token".into(),
            ));
        }
        cache_token(&grant.refresh_token, &tokens, None);
        Ok(grant.encode())
    }
}

struct CachedToken {
    access: SecretText,
    expires: Instant,
    refresh: Option<SecretText>,
}

// Access tokens by the stored refresh token they came from. A handful of
// accounts at most, so a list; the key stays a `SecretText`.
fn tokens() -> &'static Mutex<Vec<(SecretText, CachedToken)>> {
    static TOKENS: std::sync::OnceLock<Mutex<Vec<(SecretText, CachedToken)>>> =
        std::sync::OnceLock::new();
    TOKENS.get_or_init(|| Mutex::new(Vec::new()))
}

fn cache_token(stored_refresh: &SecretText, reply: &TokenResponse, rotated: Option<SecretText>) {
    let lifetime = Duration::from_secs(reply.expires_in.unwrap_or(3600));
    let entry = CachedToken {
        access: reply.access_token.clone(),
        expires: Instant::now() + lifetime.saturating_sub(EXPIRY_MARGIN),
        refresh: rotated,
    };
    if let Ok(mut list) = tokens().lock() {
        list.retain(|(key, _)| key != stored_refresh);
        list.push((stored_refresh.clone(), entry));
    }
}

// The access token for an OAuth account, refreshed when it is near expiry.
// Blocks on the token endpoint when a refresh is due.
pub(crate) fn access_token(_config: &AccountConfig, secret: &Secret) -> MailResult<SecretText> {
    let grant = Grant::decode(secret)?;
    let refresh = {
        let list = tokens()
            .lock()
            .map_err(|_| MailFailure::new(MailFailureKind::Worker, "OAuth token cache poisoned"))?;
        match list
            .iter()
            .find(|(key, _)| *key == grant.refresh_token)
            .map(|(_, cached)| cached)
        {
            Some(cached) if Instant::now() < cached.expires => return Ok(cached.access.clone()),
            Some(cached) => cached
                .refresh
                .clone()
                .unwrap_or_else(|| grant.refresh_token.clone()),
            None => grant.refresh_token.clone(),
        }
    };
    let request = grant.client()?.refresh_request(&refresh, &[]);
    let reply = post_token_request(&request)?;
    let rotated = reply
        .refresh_token
        .clone()
        .filter(|token| *token != grant.refresh_token);
    cache_token(&grant.refresh_token, &reply, rotated);
    Ok(reply.access_token)
}

pub(crate) fn forget_tokens() {
    if let Ok(mut list) = tokens().lock() {
        list.clear();
    }
}

fn post_token_request(request: &TokenRequest) -> MailResult<TokenResponse> {
    let mut easy = Easy::new();
    let curl = |error: curl::Error| super::transport::curl_failure(error);
    easy.url(request.endpoint()).map_err(curl)?;
    easy.proxy("").map_err(curl)?;
    easy.post(true).map_err(curl)?;
    easy.post_fields_copy(request.body().expose_secret().as_bytes())
        .map_err(curl)?;
    let mut headers = List::new();
    headers
        .append(&format!("Content-Type: {}", TokenRequest::CONTENT_TYPE))
        .map_err(curl)?;
    headers.append("Accept: application/json").map_err(curl)?;
    easy.http_headers(headers).map_err(curl)?;
    easy.connect_timeout(CONNECT_TIMEOUT).map_err(curl)?;
    easy.timeout(TOTAL_TIMEOUT).map_err(curl)?;
    easy.ssl_verify_host(true).map_err(curl)?;
    easy.ssl_verify_peer(true).map_err(curl)?;
    easy.follow_location(false).map_err(curl)?;
    easy.verbose(false).map_err(curl)?;
    let mut body = Vec::new();
    let mut too_large = false;
    let performed = {
        let mut transfer = easy.transfer();
        transfer
            .write_function(|data| {
                if body.len() + data.len() > MAX_TOKEN_RESPONSE_BYTES {
                    too_large = true;
                    return Ok(0);
                }
                body.extend_from_slice(data);
                Ok(data.len())
            })
            .map_err(curl)?;
        transfer.perform()
    };
    if too_large {
        return Err(MailFailure::new(
            MailFailureKind::ResponseTooLarge,
            "the token reply exceeded its limit",
        ));
    }
    performed.map_err(curl)?;
    let text = String::from_utf8(body)
        .map_err(|_| MailFailure::new(MailFailureKind::Protocol, "the token reply is not UTF-8"))?;
    TokenResponse::parse(&text).map_err(|error| match error {
        // A revoked or expired refresh token: the account must sign in again.
        TokenError::Provider(provider) if provider.error == "invalid_grant" => MailFailure::new(
            MailFailureKind::Authentication,
            "the provider revoked the sign-in; sign in again",
        ),
        other => MailFailure::new(
            MailFailureKind::Protocol,
            format!("token endpoint: {other}"),
        ),
    })
}

#[cfg(test)]
#[path = "oauth_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "oauth_test_support.rs"]
pub(crate) mod test_support;

#[cfg(test)]
std::thread_local! {
    // A client for the reducer tests, which cannot set the environment.
    pub(crate) static TEST_CLIENT: std::cell::RefCell<Option<OAuthClient>> =
        const { std::cell::RefCell::new(None) };
}
