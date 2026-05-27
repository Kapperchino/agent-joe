use crate::openai_config::OpenAICodexConfig;
use anyhow::{Context, anyhow};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use getrandom::fill as getrandom_fill;
use reqwest::{Client, Url, header};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const AUTH_BASE_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const SCOPE: &str = "openid profile email offline_access";
const CODEX_ORIGINATOR: &str = "codex_cli_rs";
const REFRESH_SKEW_MS: u64 = 60_000;

#[derive(Debug, Clone)]
pub struct CodexAuthorizationFlow {
    pub auth_url: String,
    pub state: String,
    pub verifier: String,
}

#[derive(Debug, Clone)]
pub struct ParsedRedirect {
    pub code: String,
    pub state: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    id_token: String,
    #[serde(default)]
    expires_in: u64,
}

#[derive(Debug, Deserialize)]
struct AccessTokenClaims {
    #[serde(rename = "https://api.openai.com/auth")]
    auth: Option<ChatGptAuthClaims>,
}

#[derive(Debug, Deserialize)]
struct ChatGptAuthClaims {
    chatgpt_account_id: Option<String>,
}

pub fn create_authorization_flow() -> anyhow::Result<CodexAuthorizationFlow> {
    let verifier = random_urlsafe(32)?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_urlsafe(24)?;

    let mut url = Url::parse(AUTH_BASE_URL)?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("scope", SCOPE)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("state", &state)
        .append_pair("originator", CODEX_ORIGINATOR);

    Ok(CodexAuthorizationFlow {
        auth_url: url.to_string(),
        state,
        verifier,
    })
}

pub fn parse_redirect_input(input: &str) -> anyhow::Result<ParsedRedirect> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("Redirect URL is empty"));
    }

    if let Ok(url) = Url::parse(trimmed) {
        let mut code = None;
        let mut state = None;

        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "code" => code = Some(value.to_string()),
                "state" => state = Some(value.to_string()),
                _ => {}
            }
        }

        let code = code.context("Redirect URL did not contain an authorization code")?;
        return Ok(ParsedRedirect { code, state });
    }

    Ok(ParsedRedirect {
        code: trimmed.to_string(),
        state: None,
    })
}

pub async fn exchange_authorization_code(
    code: &str,
    verifier: &str,
) -> anyhow::Result<OpenAICodexConfig> {
    let client = Client::new();
    let body = serde_urlencoded::to_string([
        ("grant_type", "authorization_code"),
        ("client_id", CLIENT_ID),
        ("code", code),
        ("code_verifier", verifier),
        ("redirect_uri", REDIRECT_URI),
    ])?;
    let response = client
        .post(TOKEN_URL)
        .header(
            header::CONTENT_TYPE,
            "application/x-www-form-urlencoded; charset=utf-8",
        )
        .body(body)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("Token exchange failed: {status}: {body}"));
    }

    let token_response: TokenResponse = response.json().await?;
    config_from_token_response(token_response)
}

pub async fn refresh_codex_tokens(config: &mut OpenAICodexConfig) -> anyhow::Result<bool> {
    if !codex_needs_refresh(config) {
        return Ok(false);
    }

    let client = Client::new();
    let body = serde_urlencoded::to_string([
        ("grant_type", "refresh_token"),
        ("client_id", CLIENT_ID),
        ("refresh_token", config.refresh_token.as_str()),
    ])?;
    let response = client
        .post(TOKEN_URL)
        .header(
            header::CONTENT_TYPE,
            "application/x-www-form-urlencoded; charset=utf-8",
        )
        .body(body)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("Token refresh failed: {status}: {body}"));
    }

    let token_response: TokenResponse = response.json().await?;
    let refreshed = config_from_token_response(token_response)?;
    *config = refreshed;
    Ok(true)
}

pub fn codex_needs_refresh(config: &OpenAICodexConfig) -> bool {
    let now_ms = now_unix_ms();
    config.expires_at_ms != 0 && config.expires_at_ms <= now_ms.saturating_add(REFRESH_SKEW_MS)
}

fn config_from_token_response(token_response: TokenResponse) -> anyhow::Result<OpenAICodexConfig> {
    let account_id = extract_account_id(&token_response.access_token)?;
    let now_ms = now_unix_ms();
    let expires_at_ms = now_ms.saturating_add(token_response.expires_in.saturating_mul(1000));

    Ok(OpenAICodexConfig {
        id_token: token_response.id_token,
        access_token: token_response.access_token,
        refresh_token: token_response.refresh_token,
        account_id,
        last_refresh: Duration::from_millis(now_ms),
        expires_at_ms,
    })
}

fn extract_account_id(access_token: &str) -> anyhow::Result<String> {
    let payload = access_token
        .split('.')
        .nth(1)
        .context("Access token did not contain a JWT payload")?;
    let claims = URL_SAFE_NO_PAD
        .decode(payload)
        .context("Failed to decode the access token payload")?;
    let decoded: AccessTokenClaims = serde_json::from_slice(&claims)?;

    decoded
        .auth
        .and_then(|auth| auth.chatgpt_account_id)
        .context("Access token did not include a ChatGPT account id")
}

fn random_urlsafe(byte_len: usize) -> anyhow::Result<String> {
    let mut bytes = vec![0_u8; byte_len];
    getrandom_fill(&mut bytes).map_err(|err| anyhow!("Failed to generate random bytes: {err}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
