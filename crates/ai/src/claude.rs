//! Claude Pro/Max subscription, as pi does it (`/login anthropic`, the
//! copy-code flow): OAuth with PKCE, tokens kept in the data dir and
//! refreshed when they expire. Subscription tokens only work with Claude
//! Code's headers and its identity as the first system block.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use genai::resolver::{AuthData, Endpoint};
use genai::{Headers, ModelIden, ServiceTarget};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
const SCOPES: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
/// pi sends this version; Anthropic checks that the client looks like Claude Code.
const USER_AGENT: &str = "claude-cli/2.1.322";
pub(crate) const IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

static STORE: RwLock<Option<PathBuf>> = RwLock::new(None);
/// Refresh tokens are single use: one refresh at a time.
/// ponytail: per process; two shroom windows refreshing at once log one out.
static REFRESH: LazyLock<futures_util::lock::Mutex<()>> =
    LazyLock::new(|| futures_util::lock::Mutex::new(()));

#[derive(Serialize, Deserialize)]
struct Tokens {
    access: String,
    refresh: String,
    /// Unix ms, 5 minutes early.
    expires: u64,
}

/// Where the login is kept (`<data>/claude.json`); set at start.
pub fn set_store(path: PathBuf) {
    if let Ok(mut store) = STORE.write() {
        *store = Some(path);
    }
}

fn store() -> Option<PathBuf> {
    STORE.read().ok()?.clone()
}

fn var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// A subscription token from the environment: `ANTHROPIC_OAUTH_TOKEN`
/// (`claude setup-token`), or an `sk-ant-oat…` token set as the API key.
fn env_token() -> Option<String> {
    var("ANTHROPIC_OAUTH_TOKEN")
        .or_else(|| var("ANTHROPIC_API_KEY").filter(|key| key.contains("sk-ant-oat")))
}

/// Anthropic calls go through the subscription: a token is in the
/// environment, or the user logged in and set no API key (a key wins).
pub fn active() -> bool {
    env_token().is_some()
        || (var("ANTHROPIC_API_KEY").is_none() && store().is_some_and(|path| path.exists()))
}

/// A login waiting for the code the browser shows.
pub struct Login {
    /// Open this in a browser.
    pub url: String,
    verifier: String,
}

/// Start a login: the URL to sign in at, then [`Login::finish`] with the code.
pub fn login() -> Result<Login, String> {
    let mut bytes = [0u8; 32];
    // ponytail: unix only; the getrandom crate if shroom ever runs elsewhere.
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|err| format!("no randomness for the login: {err}"))?;
    let b64 = |data: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data);
    let verifier = b64(&bytes);
    let challenge = b64(&Sha256::digest(verifier.as_bytes()));
    let url = reqwest::Url::parse_with_params(
        AUTHORIZE_URL,
        [
            ("code", "true"),
            ("client_id", CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", REDIRECT_URI),
            ("scope", SCOPES),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("state", &verifier),
        ],
    )
    .map_err(|err| err.to_string())?;
    Ok(Login {
        url: url.into(),
        verifier,
    })
}

impl Login {
    /// Trade what the user pasted (`code#state`, or the redirect URL) for
    /// tokens, and keep them.
    pub async fn finish(&self, pasted: &str) -> Result<(), String> {
        let (code, state) = parse_code(pasted);
        if code.is_empty() {
            return Err("no code in what was pasted".into());
        }
        if state.as_deref().is_some_and(|state| state != self.verifier) {
            return Err("that code belongs to another login: start again".into());
        }
        let tokens = post(serde_json::json!({
            "grant_type": "authorization_code",
            "client_id": CLIENT_ID,
            "code": code,
            "state": state.unwrap_or_else(|| self.verifier.clone()),
            "redirect_uri": REDIRECT_URI,
            "code_verifier": self.verifier,
        }))
        .await?;
        save(&store().ok_or("no data dir for the login")?, &tokens)
    }
}

/// `code#state`, a bare code, or the callback URL with both as parameters.
fn parse_code(pasted: &str) -> (String, Option<String>) {
    let text = pasted.trim();
    if let Ok(url) = reqwest::Url::parse(text)
        && url.query().is_some()
    {
        let get = |key: &str| {
            url.query_pairs()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.into_owned())
        };
        return (get("code").unwrap_or_default(), get("state"));
    }
    match text.split_once('#') {
        Some((code, state)) => (code.to_string(), Some(state.to_string())),
        None => (text.to_string(), None),
    }
}

async fn post(body: serde_json::Value) -> Result<Tokens, String> {
    crate::tls();
    let response = reqwest::Client::new()
        .post(TOKEN_URL)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .timeout(Duration::from_secs(30))
        .body(body.to_string())
        .send()
        .await
        .map_err(|err| err.to_string())?;
    let status = response.status();
    let text = response.text().await.map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("{status}: {}", crate::first_line(&text)));
    }
    #[derive(Deserialize)]
    struct Raw {
        access_token: String,
        refresh_token: String,
        expires_in: u64,
    }
    let raw: Raw = serde_json::from_str(&text).map_err(|err| format!("odd token reply: {err}"))?;
    Ok(Tokens {
        access: raw.access_token,
        refresh: raw.refresh_token,
        expires: now_ms() + raw.expires_in.saturating_sub(300) * 1000,
    })
}

/// Write-then-rename, readable by the user only.
fn save(path: &Path, tokens: &Tokens) -> Result<(), String> {
    let text = serde_json::to_string(tokens).map_err(|err| err.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    }
    let partial = path.with_extension("part");
    std::fs::write(&partial, text).map_err(|err| err.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&partial, path).map_err(|err| err.to_string())
}

/// The bearer token for the next call, refreshed first when it expired.
async fn token() -> Result<String, String> {
    if let Some(token) = env_token() {
        return Ok(token);
    }
    let path = store().ok_or("not logged in to Claude")?;
    let _one = REFRESH.lock().await;
    let text = std::fs::read_to_string(&path).map_err(|_| "not logged in to Claude".to_string())?;
    let tokens: Tokens = serde_json::from_str(&text).map_err(|err| err.to_string())?;
    if tokens.expires > now_ms() {
        return Ok(tokens.access);
    }
    let fresh = post(serde_json::json!({
        "grant_type": "refresh_token",
        "client_id": CLIENT_ID,
        "refresh_token": tokens.refresh,
    }))
    .await
    .map_err(|err| format!("the Claude login expired, log in again (ctrl+p): {err}"))?;
    save(&path, &fresh)?;
    Ok(fresh.access)
}

/// Where and how a subscription call goes: Anthropic's messages endpoint,
/// the bearer token and Claude Code's headers.
pub(crate) async fn target(model: ModelIden) -> Result<ServiceTarget, String> {
    let token = token().await?;
    let headers = Headers::from(vec![
        ("authorization", format!("Bearer {token}")),
        ("anthropic-version", "2023-06-01".into()),
        (
            "anthropic-beta",
            "claude-code-20250219,oauth-2025-04-20".into(),
        ),
        ("anthropic-dangerous-direct-browser-access", "true".into()),
        ("user-agent", USER_AGENT.into()),
        ("x-app", "cli".into()),
        ("accept", "application/json".into()),
    ]);
    Ok(ServiceTarget {
        endpoint: Endpoint::from_static("https://api.anthropic.com/v1/"),
        auth: AuthData::RequestOverride {
            url: MESSAGES_URL.into(),
            headers,
        },
        model,
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_login_url_carries_pkce_and_pasted_codes_parse() {
        let login = login().unwrap();
        let url = reqwest::Url::parse(&login.url).unwrap();
        let get = |key: &str| {
            url.query_pairs()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.into_owned())
                .unwrap()
        };
        assert_eq!(get("state"), login.verifier);
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(login.verifier.as_bytes()));
        assert_eq!(get("code_challenge"), challenge);
        assert_eq!(get("redirect_uri"), REDIRECT_URI);
        assert_eq!(get("scope"), SCOPES);

        assert_eq!(
            parse_code(" abc#xyz \n"),
            ("abc".into(), Some("xyz".into()))
        );
        assert_eq!(parse_code("abc"), ("abc".into(), None));
        assert_eq!(
            parse_code("https://platform.claude.com/oauth/code/callback?code=abc&state=xyz"),
            ("abc".into(), Some("xyz".into()))
        );
    }
}
