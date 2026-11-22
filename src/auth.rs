//! Microsoft identity sign-in for a desktop app.
//!
//! The Teams web client signs in with an authorization-code + PKCE flow bound to its own
//! redirect page, which a desktop app can't receive. We use the OAuth device-code flow
//! instead: the app shows a short code, the person enters it on Microsoft's own page in
//! any browser, and we get a refresh token. From that one refresh token we mint an access
//! token per Teams service (each has its own audience), exactly as the web client does
//! with its silent token requests.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Microsoft Teams' public (native) client registration. It is pre-authorised for every
/// Teams service audience and allows the device-code flow, which the web client's SPA
/// registration does not.
pub const CLIENT_ID: &str = "1fec8e78-bce4-4aaf-ab1b-5451cc387264";
const AUTHORITY: &str = "https://login.microsoftonline.com";

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCode {
    pub user_code: String,
    pub device_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_interval() -> u64 {
    5
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    expires_in: u64,
}

#[derive(Debug, Deserialize)]
struct TokenError {
    error: String,
    #[serde(default)]
    error_description: String,
}

/// What survives a restart. Only the refresh token is secret; it lives in a 0600 file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoredSession {
    pub tenant: String,
    pub refresh_token: String,
    pub display_name: String,
    pub user_principal: String,
    pub user_object_id: String,
}

#[derive(Debug, Clone)]
struct CachedToken {
    token: String,
    valid_until: Instant,
}

#[derive(Clone)]
pub struct Auth {
    http: reqwest::blocking::Client,
    pub session: StoredSession,
    tokens: HashMap<String, CachedToken>,
}

pub enum PollOutcome {
    Pending,
    Done(Box<Auth>),
}

fn session_path() -> PathBuf {
    crate::brand::config_dir().join("session.json")
}

pub fn http_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(crate::api::USER_AGENT)
        .build()
        .expect("HTTP client")
}

pub fn start_device_code(http: &reqwest::blocking::Client) -> Result<DeviceCode, String> {
    let scope = format!("{} openid profile offline_access", crate::api::SCOPE_SKYPE);
    let resp = http
        .post(format!("{AUTHORITY}/organizations/oauth2/v2.0/devicecode"))
        .form(&[("client_id", CLIENT_ID), ("scope", scope.as_str())])
        .send()
        .map_err(crate::api::unreachable("Microsoft sign-in"))?;
    let status = resp.status();
    let body = resp.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(describe_error(&body));
    }
    serde_json::from_str(&body).map_err(|e| format!("Unexpected sign-in answer: {e}"))
}

/// One poll of the token endpoint while the person types the code in their browser.
pub fn poll_device_code(
    http: &reqwest::blocking::Client,
    code: &DeviceCode,
) -> Result<PollOutcome, String> {
    let resp = http
        .post(format!("{AUTHORITY}/organizations/oauth2/v2.0/token"))
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("client_id", CLIENT_ID),
            ("device_code", code.device_code.as_str()),
        ])
        .send()
        .map_err(crate::api::unreachable("Microsoft sign-in"))?;
    let status = resp.status();
    let body = resp.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        if let Ok(err) = serde_json::from_str::<TokenError>(&body) {
            if err.error == "authorization_pending" || err.error == "slow_down" {
                return Ok(PollOutcome::Pending);
            }
        }
        return Err(describe_error(&body));
    }
    let tok: TokenResponse =
        serde_json::from_str(&body).map_err(|e| format!("Unexpected sign-in answer: {e}"))?;
    let claims = tok
        .id_token
        .as_deref()
        .and_then(jwt_claims)
        .or_else(|| jwt_claims(&tok.access_token))
        .unwrap_or_default();
    let claim = |k: &str| {
        claims
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let session = StoredSession {
        tenant: claim("tid"),
        refresh_token: tok.refresh_token.clone().unwrap_or_default(),
        display_name: claim("name"),
        user_principal: {
            let upn = claim("preferred_username");
            if upn.is_empty() { claim("upn") } else { upn }
        },
        user_object_id: claim("oid"),
    };
    let mut auth = Auth {
        http: http.clone(),
        session,
        tokens: HashMap::new(),
    };
    auth.remember(crate::api::SCOPE_SKYPE, &tok.access_token, tok.expires_in);
    auth.save();
    Ok(PollOutcome::Done(Box::new(auth)))
}

impl Auth {
    pub fn load(http: reqwest::blocking::Client) -> Option<Auth> {
        let text = std::fs::read_to_string(session_path()).ok()?;
        let session: StoredSession = serde_json::from_str(&text).ok()?;
        if session.refresh_token.is_empty() {
            return None;
        }
        Some(Auth {
            http,
            session,
            tokens: HashMap::new(),
        })
    }

    pub fn forget() {
        let _ = std::fs::remove_file(session_path());
    }

    fn save(&self) {
        let path = session_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let Ok(text) = serde_json::to_string_pretty(&self.session) else {
            return;
        };
        if std::fs::write(&path, text).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }

    fn remember(&mut self, scope: &str, token: &str, expires_in: u64) {
        // Renew a few minutes early so a request never leaves with a token about to lapse.
        let life = Duration::from_secs(expires_in.saturating_sub(300).max(60));
        self.tokens.insert(
            scope.to_string(),
            CachedToken {
                token: token.to_string(),
                valid_until: Instant::now() + life,
            },
        );
    }

    /// An access token for one Teams service, e.g. `https://api.spaces.skype.com/.default`.
    pub fn token(&mut self, scope: &str) -> Result<String, String> {
        if let Some(t) = self.tokens.get(scope) {
            if t.valid_until > Instant::now() {
                return Ok(t.token.clone());
            }
        }
        let tenant = if self.session.tenant.is_empty() {
            "organizations".to_string()
        } else {
            self.session.tenant.clone()
        };
        let full_scope = format!("{scope} openid profile offline_access");
        let resp = self
            .http
            .post(format!("{AUTHORITY}/{tenant}/oauth2/v2.0/token"))
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", CLIENT_ID),
                ("refresh_token", self.session.refresh_token.as_str()),
                ("scope", full_scope.as_str()),
            ])
            .send()
            .map_err(crate::api::unreachable("Microsoft sign-in"))?;
        let status = resp.status();
        let body = resp.text().map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(describe_error(&body));
        }
        let tok: TokenResponse =
            serde_json::from_str(&body).map_err(|e| format!("Unexpected sign-in answer: {e}"))?;
        if let Some(rt) = tok.refresh_token.filter(|r| !r.is_empty()) {
            if rt != self.session.refresh_token {
                self.session.refresh_token = rt;
                self.save();
            }
        }
        self.remember(scope, &tok.access_token, tok.expires_in);
        Ok(tok.access_token)
    }
}

/// Turns an AAD error body into one readable sentence; the AADSTS code stays in it so a
/// tenant policy block (e.g. AADSTS53003, device-code flow blocked) can be looked up.
fn describe_error(body: &str) -> String {
    match serde_json::from_str::<TokenError>(body) {
        Ok(err) => {
            let first_line = err.error_description.lines().next().unwrap_or("").trim();
            match err.error.as_str() {
                "expired_token" => "The code expired. Start the sign-in again.".into(),
                "authorization_declined" => "Sign-in was declined in the browser.".into(),
                "invalid_grant" => format!("Your sign-in has expired. Sign in again. ({first_line})"),
                _ => format!("Sign-in failed: {first_line}"),
            }
        }
        Err(_) => format!("Sign-in failed: {}", body.chars().take(200).collect::<String>()),
    }
}

/// Reads a JWT's payload without checking its signature: we only use it to label the
/// signed-in person and to find their tenant and object id.
pub fn jwt_claims(jwt: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64url_decode(payload)?;
    serde_json::from_slice(&bytes).ok()
}

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0;
    for c in input.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => continue,
            _ => return None,
        };
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
