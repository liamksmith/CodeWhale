//! 可插拔的沙箱后端抽象。
//!
//! 外部沙箱后端将 shell 命令执行路由到远程服务（例如阿里云 OpenSandbox），而不是在本地生成进程。
//! 这与操作系统级别的沙箱模块（Seatbelt / Landlock / Windows）互补——当配置后，外部后端*完全替换*本地执行。

use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;

/// 沙箱后端执行的输出。
#[derive(Debug, Clone)]
pub struct SandboxOutput {
    /// 命令的标准输出。
    pub stdout: String,
    /// 命令的标准错误。
    pub stderr: String,
    /// 退出码（0 表示成功）。
    pub exit_code: i32,
}

/// 外部沙箱后端的类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxKind {
    /// 无外部沙箱 — 在本地执行命令。
    None,
    /// 阿里云 OpenSandbox 远程执行。
    OpenSandbox,
}

impl SandboxKind {
    /// 从配置中解析沙箱后端名称（不区分大小写）。
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" | "" => Some(Self::None),
            "opensandbox" | "open-sandbox" | "open_sandbox" => Some(Self::OpenSandbox),
            _ => None,
        }
    }

    /// 人类可读的标签。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::OpenSandbox => "opensandbox",
        }
    }
}

/// 外部沙箱后端的抽象接口。
///
/// 实现将命令发送到远程执行环境并返回结构化输出。该 trait 是 `Send + Sync` 的，
/// 因此可以存储在 `Arc` 中并在异步任务间共享。
#[async_trait]
pub trait SandboxBackend: Send + Sync {
    /// 执行 shell 命令并返回其输出。
    ///
    /// `cmd` 是完整的 shell 命令字符串（例如 `"ls -la"`）。
    /// `env` 包含要设置的额外环境变量。
    async fn exec(&self, cmd: &str, env: &HashMap<String, String>) -> Result<SandboxOutput>;
}

use crate::config::Config;

/// 从配置中创建已配置的沙箱后端。
///
/// 当未配置外部沙箱后端时（即 `sandbox_backend` 键缺失、为空或为 `"none"`），返回 `None`。
/// 当设置为 `"opensandbox"` 时，使用 `sandbox_url` 和 `sandbox_api_key` 构造
/// [`OpenSandboxBackend`](super::opensandbox::OpenSandboxBackend)。
pub fn create_backend(config: &Config) -> Result<Option<Box<dyn SandboxBackend>>> {
    let kind = config
        .sandbox_backend
        .as_deref()
        .and_then(SandboxKind::parse)
        .unwrap_or(SandboxKind::None);

    match kind {
        SandboxKind::None => Ok(None),
        SandboxKind::OpenSandbox => {
            let base_url = config
                .sandbox_url
                .clone()
                .unwrap_or_else(|| "http://localhost:8080".to_string());
            let api_key = config.sandbox_api_key.clone();
            let backend = super::opensandbox::OpenSandboxBackend::new(base_url, api_key, 30)?;
            Ok(Some(Box::new(backend)))
        }
    }
}
