//! OpenAI Codex / ChatGPT OAuth 凭据加载和令牌刷新。
//!
//! 从 `~/.codex/auth.json`（或 `$CODEX_HOME/auth.json`）读取现有的 Codex CLI 凭据，
//! 并使用 OpenAI 认证端点透明地刷新过期的访问令牌。
//!
//! # 安全性
//!
//! 令牌值永远不会被记录或打印。所有调试表示都会编辑敏感字段。

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

/// 存储在 `auth.json` 中的 OAuth 令牌负载。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
struct AuthTokens {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    account_id: Option<String>,
}

/// Codex CLI 的 `auth.json` 顶层结构。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
struct CodexAuthFile {
    tokens: Option<AuthTokens>,
    last_refresh: Option<String>,
}

/// 已解析的、可供 API 使用的 OAuth 凭据。
#[derive(Debug, Clone)]
pub struct CodexCredentials {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub account_id: Option<String>,
}

/// 用于提取过期的 JWT 声明子集。
#[derive(Debug, Deserialize)]
struct JwtClaims {
    exp: Option<u64>,
}

/// 解析 Codex 认证文件的路径。
///
/// 优先级：
/// 1. `OPENAI_CODEX_AUTH_FILE` 环境变量
/// 2. `$CODEX_HOME/auth.json`
/// 3. `~/.codex/auth.json`
pub fn auth_file_path() -> PathBuf {
    if let Ok(path) = std::env::var("OPENAI_CODEX_AUTH_FILE") {
        let p = PathBuf::from(&path);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let codex_home = std::env::var("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".codex")
        });
    codex_home.join("auth.json")
}

/// 尝试从 JWT 中提取 `exp`（纪元秒）而不验证签名。
/// 在任何解析失败时返回 `None`。
fn jwt_expiry_seconds(token: &str) -> Option<u64> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let payload = parts[1];
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: JwtClaims = serde_json::from_slice(&decoded).ok()?;
    claims.exp
}

/// 检查访问令牌是否已过期，带有 60 秒的安全余量。
fn token_is_expired(access_token: &str) -> bool {
    match jwt_expiry_seconds(access_token) {
        Some(exp) => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or(Duration::ZERO)
                .as_secs();
            // 60 秒安全余量
            now + 60 >= exp
        }
        // 如果无法解析过期时间，假设它可能已过期 — 尝试刷新。
        None => true,
    }
}

/// 从认证文件加载 Codex 凭据。
///
/// 如果文件不存在或没有可用令牌，返回 `Ok(None)`。
/// 仅在非"文件未找到"的解析/IO 错误时返回 `Err`。
pub fn load_credentials() -> Result<Option<CodexCredentials>> {
    let path = auth_file_path();
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("reading Codex auth file: {}", path.display()))?;
    let auth: CodexAuthFile = serde_json::from_str(&contents)
        .with_context(|| format!("parsing Codex auth file: {}", path.display()))?;
    let tokens = match auth.tokens {
        Some(t) => t,
        None => return Ok(None),
    };
    let access_token = match tokens.access_token {
        Some(t) if !t.trim().is_empty() => t,
        _ => return Ok(None),
    };
    Ok(Some(CodexCredentials {
        access_token,
        refresh_token: tokens.refresh_token,
        account_id: tokens.account_id,
    }))
}

/// 使用刷新令牌刷新过期的访问令牌。
///
/// 调用 OpenAI 令牌端点并返回新凭据。
/// 成功后，更新磁盘上的认证文件。同步（阻塞）因此可以
/// 在无提示词、同步配置凭据解析路径中运行，
/// 与 Kimi OAuth 刷新流程一致。
fn refresh_access_token(refresh_token: &str) -> Result<CodexCredentials> {
    let client = crate::tls::reqwest_blocking_client_builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("building token refresh client")?;
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", CODEX_CLIENT_ID),
    ];
    let response = client
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .context("sending token refresh request")?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        bail!("Token refresh failed (HTTP {status}): {body}");
    }
    let body: serde_json::Value = response.json().context("parsing token refresh response")?;
    let new_access = body["access_token"]
        .as_str()
        .context("missing access_token in refresh response")?
        .to_string();
    let new_refresh = body["refresh_token"].as_str().map(ToOwned::to_owned);
    let new_id = body["id_token"].as_str().map(ToOwned::to_owned);

    // 从 id_token 中提取 account_id（如果可用）。
    let account_id = new_id.as_deref().and_then(extract_account_id_from_id_token);

    let creds = CodexCredentials {
        access_token: new_access,
        refresh_token: new_refresh.or_else(|| Some(refresh_token.to_string())),
        account_id,
    };

    // 持久化已刷新的凭据。
    if let Err(e) = save_credentials(&creds, new_id.as_deref()) {
        tracing::warn!("持久化已刷新的 Codex 凭据失败: {e}");
    }

    Ok(creds)
}

/// 从 `https://api.openai.com/auth` JWT 声明命名空间中提取 `chatgpt_account_id`。
fn extract_account_id_from_id_token(id_token: &str) -> Option<String> {
    let parts: Vec<&str> = id_token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(parts[1]).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    value
        .get("https://api.openai.com/auth")?
        .get("chatgpt_account_id")?
        .as_str()
        .map(ToOwned::to_owned)
}

/// 将凭据保存回认证文件，保留文件权限。
fn save_credentials(creds: &CodexCredentials, id_token: Option<&str>) -> Result<()> {
    let path = auth_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating Codex auth dir: {}", parent.display()))?;
    }
    let auth = CodexAuthFile {
        tokens: Some(AuthTokens {
            access_token: Some(creds.access_token.clone()),
            refresh_token: creds.refresh_token.clone(),
            id_token: id_token.map(ToOwned::to_owned),
            account_id: creds.account_id.clone(),
        }),
        last_refresh: Some(chrono_humanize_if_available()),
    };
    let json = serde_json::to_string_pretty(&auth).context("serializing credentials")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true).mode(0o600);
        let mut file = opts
            .open(&path)
            .with_context(|| format!("writing Codex auth file: {}", path.display()))?;
        std::io::Write::write_all(&mut file, json.as_bytes())?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&path, &json)
            .with_context(|| format!("writing Codex auth file: {}", path.display()))?;
    }
    Ok(())
}

fn chrono_humanize_if_available() -> String {
    // 不带 chrono 依赖的简单 ISO 风格时间戳。
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| format!("自纪元以来的 {} 秒", d.as_secs()))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// 加载或刷新 Codex 凭据。
///
/// 1. 首先尝试环境变量覆盖（`OPENAI_CODEX_ACCESS_TOKEN` / `CODEX_ACCESS_TOKEN`）。
/// 2. 从认证文件加载。
/// 3. 如果访问令牌已过期且有刷新令牌，则刷新。
///
/// 同步，因此可以从无提示词的配置凭据解析路径调用（与 Kimi OAuth 流程一致）。
pub fn get_credentials() -> Result<CodexCredentials> {
    // 环境变量覆盖优先。
    if let Ok(token) = std::env::var("OPENAI_CODEX_ACCESS_TOKEN")
        && !token.trim().is_empty()
    {
        return Ok(CodexCredentials {
            access_token: token,
            refresh_token: None,
            account_id: codex_account_id_env(),
        });
    }
    if let Ok(token) = std::env::var("CODEX_ACCESS_TOKEN")
        && !token.trim().is_empty()
    {
        return Ok(CodexCredentials {
            access_token: token,
            refresh_token: None,
            account_id: codex_account_id_env(),
        });
    }

    let creds = load_credentials()?.with_context(missing_auth_message)?;

    // 检查访问令牌是否仍然有效。
    if !token_is_expired(&creds.access_token) {
        return Ok(creds);
    }

    // 尝试刷新。
    match creds.refresh_token {
        Some(ref rt) if !rt.trim().is_empty() => {
            tracing::info!("Codex 访问令牌已过期，正在刷新...");
            refresh_access_token(rt)
        }
        _ => bail!(
            "Codex access token expired and no refresh token available.\n\
             Run `codex login` to re-authenticate."
        ),
    }
}

#[must_use]
pub fn missing_auth_message() -> String {
    format!(
        "OpenAI Codex OAuth credentials not found.\n\
         \n\
         CodeWhale checked OPENAI_CODEX_ACCESS_TOKEN, CODEX_ACCESS_TOKEN, and {}.\n\
         Run `codex login` to authenticate with ChatGPT/Codex OAuth, or set OPENAI_CODEX_ACCESS_TOKEN for this process.",
        auth_file_path().display()
    )
}

/// 尽力获取 `chatgpt-account-id` 请求头的 ChatGPT 账户 ID。
///
/// 首先从环境变量覆盖解析，然后从磁盘上的认证文件解析。
/// 从不刷新也从不报错 — 缺少账户 ID 只是意味着该头被省略。
pub fn codex_account_id() -> Option<String> {
    if let Some(id) = codex_account_id_env() {
        return Some(id);
    }
    load_credentials().ok().flatten().and_then(|c| c.account_id)
}

/// 仅从环境变量覆盖中读取 ChatGPT 账户 ID。
fn codex_account_id_env() -> Option<String> {
    for var in ["OPENAI_CODEX_ACCOUNT_ID", "CODEX_ACCOUNT_ID"] {
        if let Ok(value) = std::env::var(var) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// OpenAI OAuth 常量（来自 Codex CLI 参考实现）。
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwt_expiry_parses_valid_token() {
        // 一个最小的 JWT，负载为 {"exp": 9999999999}。
        let payload = URL_SAFE_NO_PAD.encode(b"{\"exp\":9999999999}");
        let token = format!("header.{payload}.signature");
        assert_eq!(jwt_expiry_seconds(&token), Some(9999999999));
    }

    #[test]
    fn jwt_expiry_returns_none_for_malformed() {
        assert_eq!(jwt_expiry_seconds("not.a.jwt"), None);
        assert_eq!(jwt_expiry_seconds(""), None);
        assert_eq!(jwt_expiry_seconds("x"), None);
    }

    #[test]
    fn token_is_expired_detects_future() {
        // 遥远的未来 — 不应过期。
        let payload = URL_SAFE_NO_PAD.encode(b"{\"exp\":9999999999}");
        let token = format!("header.{payload}.sig");
        assert!(!token_is_expired(&token));
    }

    #[test]
    fn token_is_expired_detects_past() {
        // 很久以前。
        let payload = URL_SAFE_NO_PAD.encode(b"{\"exp\":1000000000}");
        let token = format!("header.{payload}.sig");
        assert!(token_is_expired(&token));
    }

    #[test]
    fn auth_file_path_respects_env() {
        // 只需验证它返回路径而不崩溃。
        let path = auth_file_path();
        assert!(path.to_string_lossy().contains("auth.json"));
    }

    #[test]
    fn missing_auth_message_mentions_oauth_checked_locations() {
        let message = missing_auth_message();

        assert!(message.contains("OpenAI Codex OAuth credentials not found"));
        assert!(message.contains("OPENAI_CODEX_ACCESS_TOKEN"));
        assert!(message.contains("CODEX_ACCESS_TOKEN"));
        assert!(message.contains(&auth_file_path().display().to_string()));
        assert!(message.contains("codex login"));
    }
}
