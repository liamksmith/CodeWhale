//! 阿里云 OpenSandbox 后端适配器。
//!
//! 将 shell 命令发送到兼容 OpenSandbox 的 HTTP API 以进行远程执行。
//! API 端点为 `POST {base_url}/v1/sandbox/run`，
//! JSON 请求体为 `{"cmd": "...", "env": {...}}`，
//! 期望的 JSON 响应为 `{"stdout": "...", "stderr": "...", "exit_code": 0}`。

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use super::backend::{SandboxBackend, SandboxOutput};

/// 发送到 OpenSandbox `/v1/sandbox/run` 端点的请求体。
#[derive(Debug, Serialize)]
struct SandboxRunRequest {
    /// 要执行的完整 shell 命令。
    cmd: String,
    /// 在沙箱中设置的环境变量。
    env: HashMap<String, String>,
}

/// 来自 OpenSandbox `/v1/sandbox/run` 端点的响应体。
#[derive(Debug, Deserialize)]
struct SandboxRunResponse {
    /// 命令的标准输出。
    stdout: String,
    /// 命令的标准错误。
    stderr: String,
    /// 退出码（0 表示成功）。
    exit_code: i32,
}

/// 兼容 OpenSandbox 的远程执行后端。
///
/// 使用基础 URL（例如 `"http://localhost:8080"`）、可选的 API 密钥（作为 `Bearer` 令牌发送）
/// 和超时秒数构造。
pub struct OpenSandboxBackend {
    base_url: String,
    api_key: Option<String>,
    timeout_secs: u64,
    client: reqwest::Client,
}

impl OpenSandboxBackend {
    /// 创建一个新的 OpenSandbox 后端。
    ///
    /// `base_url` 应为 OpenSandbox API 的根路径
    ///（例如 `"http://localhost:8080"`）。`api_key` 是可选的，设置后会作为
    /// `Authorization: Bearer <key>` 发送。`timeout_secs` 控制 HTTP 请求超时。
    pub fn new(base_url: String, api_key: Option<String>, timeout_secs: u64) -> Result<Self> {
        let client = crate::tls::reqwest_client_builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .context("failed to construct HTTP client for OpenSandbox backend")?;

        Ok(Self {
            base_url,
            api_key,
            timeout_secs,
            client,
        })
    }

    /// 构建沙箱运行端点的完整 URL。
    fn run_url(&self) -> String {
        format!("{}/v1/sandbox/run", self.base_url.trim_end_matches('/'))
    }
}

#[async_trait]
impl SandboxBackend for OpenSandboxBackend {
    async fn exec(&self, cmd: &str, env: &HashMap<String, String>) -> Result<SandboxOutput> {
        let request_body = SandboxRunRequest {
            cmd: cmd.to_string(),
            env: env.clone(),
        };

        let mut req = self.client.post(self.run_url()).json(&request_body);

        if let Some(ref api_key) = self.api_key {
            req = req.bearer_auth(api_key);
        }

        let response = req
            .send()
            .await
            .context("Failed to reach OpenSandbox endpoint")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("OpenSandbox returned HTTP {}: {}", status.as_u16(), body);
        }

        let parsed: SandboxRunResponse = response
            .json()
            .await
            .context("Failed to parse OpenSandbox response")?;

        Ok(SandboxOutput {
            stdout: parsed.stdout,
            stderr: parsed.stderr,
            exit_code: parsed.exit_code,
        })
    }
}
