//! LSP 集成：编辑后诊断注入（#136）。
//!
//! 代理成功编辑文件（`edit_file`、`apply_patch` 或 `write_file`）后，
//! 引擎向 [`LspManager`] 请求该文件的诊断信息。管理器在首次使用时
//! 惰性启动相应的 LSP 服务器，发送 `didOpen`/`didChange`，等待有界
//! 超时内的 `publishDiagnostics`，规范化结果，并将其返回给引擎。
//!
//! 失败模式设计为非阻塞：缺少 LSP 二进制文件、服务器崩溃或超时，
//! 都会降级为"本轮无诊断"而不会阻塞代理。当二进制文件缺失时，
//! 我们每种语言记录一次一次性警告。
//!
//! # 接线
//!
//! ```text
//! Engine  ── after successful edit ──▶  LspManager.diagnostics_for(path, seq)
//!                                              │
//!                                              ▼
//!                                       per-language LspClient
//!                                              │
//!                                              ▼
//!                                      LspTransport (stdio)
//! ```
//!
//! # 配置
//!
//! `~/.deepseek/config.toml` 中的 `[lsp]` 表控制行为：
//! `enabled`、`poll_after_edit_ms`、`max_diagnostics_per_file`、`include_warnings`、
//! 可选的 `servers` 覆盖，以及用于注册内置注册表未覆盖的文件扩展名
//! （例如 Ruby、PHP、C#）的 LSP 服务器的 `custom` 表。
//! 请参阅 [`LspConfig`] 了解默认值，以及 `config.example.toml` 了解文档。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::timeout;

pub mod client;
pub mod diagnostics;
pub mod registry;

pub use client::{LspTransport, StdioLspTransport};
pub use diagnostics::{Diagnostic, DiagnosticBlock, Severity, render_blocks};
pub use registry::Language;

/// 用户为某个文件扩展名定义的 LSP 服务器。
///
/// 通过配置文件中的 `[lsp.custom.<ext>]` 注册。扩展键是文件后缀
/// （不含前导点号），例如 `"php"`、`"rb"`、`"cs"`。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CustomLspDef {
    /// LSP `languageId` value used in `textDocument/didOpen`.
    pub language_id: String,
    /// Executable to spawn.
    pub command: String,
    /// Arguments passed to the executable.
    #[serde(default)]
    pub args: Vec<String>,
}

/// `[lsp]` 配置模式。镜像 `config.example.toml` 中记录的 TOML 键。未知键会被忽略。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct LspConfig {
    /// 总开关。当为 `false` 时，管理器跳过所有操作并返回空诊断列表。
    pub enabled: bool,
    /// 等待 LSP 服务器在 `didOpen`/`didChange` 后发布诊断的最大时间（毫秒）。默认 5000 毫秒。
    pub poll_after_edit_ms: u64,
    /// 每个文件保留的最大诊断数。按严重性排序后多余项将被丢弃。默认 20。
    pub max_diagnostics_per_file: usize,
    /// 当为 `true` 时，警告（严重性 2）会保留在输出中。当为 `false`（默认）时，
    /// 仅显示错误（严重性 1）。
    pub include_warnings: bool,
    /// `Language -> (cmd, args)` 表的可选覆盖。键使用 [`Language::as_key`]（例如 `"rust"`）。
    pub servers: HashMap<String, Vec<String>>,
    /// 用户为内置注册表未涵盖的文件扩展名定义的 LSP 服务器。按键是扩展名（例如 `"php"`、`"rb"`）。
    #[serde(default)]
    pub custom: HashMap<String, CustomLspDef>,
}

impl Default for LspConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_after_edit_ms: 5_000,
            max_diagnostics_per_file: 20,
            include_warnings: false,
            servers: HashMap::new(),
            custom: HashMap::new(),
        }
    }
}

impl LspConfig {
    /// 解析 `lang` 的 `(command, args)`。用户提供的覆盖优先于内置注册表。
    fn resolve_command(&self, lang: Language) -> Option<(String, Vec<String>)> {
        if let Some(parts) = self.servers.get(lang.as_key())
            && let Some((first, rest)) = parts.split_first()
        {
            return Some((first.clone(), rest.to_vec()));
        }
        let (cmd, args) = registry::server_for(lang)?;
        Some((
            cmd.to_string(),
            args.iter().map(|a| (*a).to_string()).collect(),
        ))
    }
}

/// LspManager 持有一个惰性填充的 `Language -> Transport` 映射。
/// 在会话生命周期内，同一语言的多个文件复用同一个传输层。
pub struct LspManager {
    config: LspConfig,
    workspace: PathBuf,
    /// 每种语言的传输层。包装在 `Arc` 中，以便在驱动单个传输层的 I/O 前释放外部锁。
    transports: AsyncMutex<HashMap<Language, Arc<dyn LspTransport>>>,
    /// 每种语言的"已警告用户二进制文件缺失"防护，避免每次编辑都刷审计日志。
    missing_warned: AsyncMutex<HashSet<Language>>,
    /// 测试接缝：设置后，`diagnostics_for` 使用这些替代启动真实的 LSP 进程。按键是语言。
    test_transports: AsyncMutex<HashMap<Language, Arc<dyn LspTransport>>>,
    /// 用户定义的自定义语言服务器每种扩展名的传输层。
    custom_transports: AsyncMutex<HashMap<String, Arc<dyn LspTransport>>>,
    /// 自定义服务器的每种扩展名的"已警告"防护。
    custom_missing_warned: AsyncMutex<HashSet<String>>,
}

impl LspManager {
    /// 构建一个新的管理器。不会启动任何 LSP 服务器——这是惰性的。
    #[must_use]
    pub fn new(config: LspConfig, workspace: PathBuf) -> Self {
        Self {
            config,
            workspace,
            transports: AsyncMutex::new(HashMap::new()),
            missing_warned: AsyncMutex::new(HashSet::new()),
            test_transports: AsyncMutex::new(HashMap::new()),
            custom_transports: AsyncMutex::new(HashMap::new()),
            custom_missing_warned: AsyncMutex::new(HashSet::new()),
        }
    }

    /// 对已解析配置的只读访问。引擎使用它在 `enabled = false` 时完全跳过编辑后钩子。
    #[must_use]
    pub fn config(&self) -> &LspConfig {
        &self.config
    }

    /// 为一种语言注入伪造的传输层。测试使用此方法以避免在 CI 中 fork 真实的 LSP 服务器。
    #[cfg(test)]
    pub async fn install_test_transport(&self, lang: Language, transport: Arc<dyn LspTransport>) {
        self.test_transports.lock().await.insert(lang, transport);
    }

    /// 轮询 LSP 服务器获取 `file` 的诊断信息。返回渲染后的 [`DiagnosticBlock`]
    /// （已截断至配置的每文件最大值），当管理器被禁用/没有服务器/轮询超时时返回 `None`。
    ///
    /// `_edit_seq` 参数目前为空操作；它存在于签名中以便引擎在 v0.7.x 添加请求批处理时
    /// 能够将诊断关联回特定的编辑。
    pub async fn diagnostics_for(&self, file: &Path, _edit_seq: u64) -> Option<DiagnosticBlock> {
        if !self.config.enabled {
            return None;
        }

        let lang = registry::detect_language(file);
        if lang == Language::Other {
        // 自定义扩展名回退：为内置注册表未覆盖的文件扩展名检查用户定义的 LSP 服务器。
            if let Some(custom) = self.config.custom_for_extension(file) {
                return self.diagnostics_for_custom(file, custom).await;
            }
            return None;
        }

        let text = match tokio::fs::read_to_string(file).await {
            Ok(text) => text,
            Err(err) => {
                tracing::debug!(?err, file = %file.display(), "lsp: read file failed");
                return None;
            }
        };

        let transport = match self.transport_for(lang).await {
            Some(t) => t,
            None => return None,
        };

        self.poll_diagnostics(file, &text, transport).await
    }

    /// 共享的诊断轮询：发送 didOpen/didChange、等待、过滤、排序和截断。
    async fn poll_diagnostics(
        &self,
        file: &Path,
        text: &str,
        transport: Arc<dyn LspTransport>,
    ) -> Option<DiagnosticBlock> {
        let wait = Duration::from_millis(self.config.poll_after_edit_ms);
        let inner_wait = wait;
        let raw = match timeout(wait, transport.diagnostics_for(file, text, inner_wait)).await {
            Ok(Ok(items)) => items,
            Ok(Err(err)) => {
                tracing::debug!(?err, file = %file.display(), "lsp: diagnostics call failed");
                return None;
            }
            Err(_) => {
                tracing::debug!(file = %file.display(), "lsp: diagnostics timed out");
                return None;
            }
        };

        // 过滤、排序和截断。
        let include_warnings = self.config.include_warnings;
        let mut items: Vec<Diagnostic> = raw
            .into_iter()
            .filter(|d| match d.severity {
                Severity::Error => true,
                Severity::Warning => include_warnings,
                _ => false,
            })
            .collect();
        items.sort_by_key(|d| match d.severity {
            Severity::Error => 0u8,
            Severity::Warning => 1u8,
            Severity::Information => 2u8,
            Severity::Hint => 3u8,
        });
        let mut block = DiagnosticBlock {
            file: relative_to_workspace(&self.workspace, file),
            items,
        };
        block.truncate(self.config.max_diagnostics_per_file);
        if block.items.is_empty() {
            None
        } else {
            Some(block)
        }
    }

    /// 用户定义的自定义语言服务器的诊断路径。
    async fn diagnostics_for_custom(
        &self,
        file: &Path,
        custom: &CustomLspDef,
    ) -> Option<DiagnosticBlock> {
        let ext = file.extension()?.to_str()?.to_ascii_lowercase();
        let text = match tokio::fs::read_to_string(file).await {
            Ok(t) => t,
            Err(err) => {
                tracing::debug!(?err, file = %file.display(), "lsp: read file failed");
                return None;
            }
        };
        let transport = match self.transport_for_custom(&ext, custom).await {
            Some(t) => t,
            None => return None,
        };
        self.poll_diagnostics(file, &text, transport).await
    }

    /// 为扩展名惰性启动自定义 LSP 服务器。
    async fn transport_for_custom(
        &self,
        ext: &str,
        def: &CustomLspDef,
    ) -> Option<Arc<dyn LspTransport>> {
        if let Some(t) = self.custom_transports.lock().await.get(ext) {
            return Some(t.clone());
        }
        match StdioLspTransport::spawn(
            &def.command,
            &def.args,
            &def.language_id,
            self.workspace.clone(),
        )
        .await
        {
            Ok(t) => {
                let arc: Arc<dyn LspTransport> = Arc::new(t);
                self.custom_transports
                    .lock()
                    .await
                    .insert(ext.to_string(), arc.clone());
                Some(arc)
            }
            Err(err) => {
                let key = ext.to_string();
                let mut warned = self.custom_missing_warned.lock().await;
                if warned.insert(key) {
                    tracing::warn!(
                        extension = %ext,
                        command = %def.command,
                        error = %err,
                        "lsp: custom server unavailable; diagnostics disabled for this extension"
                    );
                }
                None
            }
        }
    }

    /// 解析（并惰性启动）`lang` 的传输层。测试可以通过 `install_test_transport` 绕过此操作（仅在 cfg-test 中）。
    async fn transport_for(&self, lang: Language) -> Option<Arc<dyn LspTransport>> {
        if let Some(t) = self.test_transports.lock().await.get(&lang) {
            return Some(t.clone());
        }

        if let Some(t) = self.transports.lock().await.get(&lang) {
            return Some(t.clone());
        }

        let (cmd, args) = self.config.resolve_command(lang)?;
        match StdioLspTransport::spawn(&cmd, &args, lang.language_id(), self.workspace.clone())
            .await
        {
            Ok(transport) => {
                let arc: Arc<dyn LspTransport> = Arc::new(transport);
                self.transports.lock().await.insert(lang, arc.clone());
                Some(arc)
            }
            Err(err) => {
                self.warn_missing_once(lang, &cmd, &err).await;
                None
            }
        }
    }

    async fn warn_missing_once(&self, lang: Language, cmd: &str, err: &anyhow::Error) {
        let mut warned = self.missing_warned.lock().await;
        if warned.insert(lang) {
            tracing::warn!(
                language = %lang.as_key(),
                command = %cmd,
                error = %err,
                "lsp: server unavailable; diagnostics disabled for this language"
            );
        }
    }

    /// 尽力关闭每个已启动的传输层。在会话结束时调用。
    #[allow(dead_code)]
    pub async fn shutdown_all(&self) {
        let transports: Vec<Arc<dyn LspTransport>> =
            self.transports.lock().await.values().cloned().collect();
        let custom: Vec<Arc<dyn LspTransport>> = self
            .custom_transports
            .lock()
            .await
            .values()
            .cloned()
            .collect();
        for transport in transports {
            transport.shutdown().await;
        }
        for transport in custom {
            transport.shutdown().await;
        }
    }
}

impl LspConfig {
    /// Look up a [`CustomLspDef`] for `file` when the built-in registry
    /// would return `Language::Other`. Returns `None` when the extension is
    /// unknown or no custom server is registered for it.
    fn custom_for_extension(&self, file: &Path) -> Option<&CustomLspDef> {
        let ext = file.extension()?.to_str()?;
        self.custom.get(&ext.to_ascii_lowercase())
    }
}

/// Render `path` relative to the workspace when possible. Falls back to
/// `path.file_name()` (per the issue's hard rule about not using
/// `display().to_string()` on the bare path) when relativization fails.
fn relative_to_workspace(workspace: &Path, path: &Path) -> PathBuf {
    if let Ok(rel) = path.strip_prefix(workspace) {
        return rel.to_path_buf();
    }
    PathBuf::from(
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| String::from("unknown")),
    )
}

/// 用于测试/空运行。构建一个始终返回 `None` 的空管理器。
/// 引擎即使当用户禁用了 LSP 时也会构造一个 `LspManager`，因此该字段始终存在。
impl LspManager {
    #[must_use]
    pub fn disabled() -> Self {
        Self::new(
            LspConfig {
                enabled: false,
                ..LspConfig::default()
            },
            PathBuf::new(),
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 伪造传输层：返回固定的诊断列表。集成测试使用此方法以避免在 CI 中启动真实的 LSP 服务器。
    pub(crate) struct FakeTransport {
        items: Vec<Diagnostic>,
        calls: AtomicUsize,
    }

    impl FakeTransport {
        pub(crate) fn new(items: Vec<Diagnostic>) -> Self {
            Self {
                items,
                calls: AtomicUsize::new(0),
            }
        }

        pub(crate) fn call_count(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl LspTransport for FakeTransport {
        async fn diagnostics_for(
            &self,
            _path: &Path,
            _text: &str,
            _wait: Duration,
        ) -> anyhow::Result<Vec<Diagnostic>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(self.items.clone())
        }

        async fn shutdown(&self) {}
    }

    #[tokio::test]
    async fn returns_none_when_disabled() {
        let mgr = LspManager::new(
            LspConfig {
                enabled: false,
                ..LspConfig::default()
            },
            PathBuf::from("/tmp"),
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("foo.rs");
        tokio::fs::write(&path, b"fn main() {}").await.unwrap();
        assert!(mgr.diagnostics_for(&path, 1).await.is_none());
    }

    #[tokio::test]
    async fn returns_none_for_unknown_language() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = LspManager::new(LspConfig::default(), dir.path().to_path_buf());
        let path = dir.path().join("notes.txt");
        tokio::fs::write(&path, b"hi").await.unwrap();
        assert!(mgr.diagnostics_for(&path, 1).await.is_none());
    }

    #[tokio::test]
    async fn forwards_errors_through_fake_transport() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = LspManager::new(LspConfig::default(), dir.path().to_path_buf());
        let path = dir.path().join("foo.rs");
        tokio::fs::write(&path, b"let x: i32 = \"oops\";")
            .await
            .unwrap();

        let fake = Arc::new(FakeTransport::new(vec![Diagnostic {
            line: 1,
            column: 14,
            severity: Severity::Error,
            message: "expected i32, found &str".to_string(),
        }]));
        mgr.install_test_transport(Language::Rust, fake.clone())
            .await;

        let block = mgr.diagnostics_for(&path, 1).await.expect("has block");
        let rendered = block.render();
        assert!(rendered.contains("ERROR [1:14] expected i32, found &str"));
        assert!(rendered.contains("foo.rs"));
        assert_eq!(fake.call_count(), 1);
    }

    #[tokio::test]
    async fn drops_warnings_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = LspManager::new(LspConfig::default(), dir.path().to_path_buf());
        let path = dir.path().join("foo.rs");
        tokio::fs::write(&path, b"fn main() {}").await.unwrap();

        let fake = Arc::new(FakeTransport::new(vec![
            Diagnostic {
                line: 1,
                column: 1,
                severity: Severity::Warning,
                message: "unused import".to_string(),
            },
            Diagnostic {
                line: 2,
                column: 1,
                severity: Severity::Error,
                message: "type error".to_string(),
            },
        ]));
        mgr.install_test_transport(Language::Rust, fake).await;

        let block = mgr.diagnostics_for(&path, 1).await.expect("has block");
        assert_eq!(block.items.len(), 1);
        assert_eq!(block.items[0].severity, Severity::Error);
    }

    #[tokio::test]
    async fn keeps_warnings_when_opted_in() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = LspManager::new(
            LspConfig {
                include_warnings: true,
                ..LspConfig::default()
            },
            dir.path().to_path_buf(),
        );
        let path = dir.path().join("foo.rs");
        tokio::fs::write(&path, b"fn main() {}").await.unwrap();

        let fake = Arc::new(FakeTransport::new(vec![
            Diagnostic {
                line: 1,
                column: 1,
                severity: Severity::Warning,
                message: "unused".to_string(),
            },
            Diagnostic {
                line: 2,
                column: 1,
                severity: Severity::Error,
                message: "broken".to_string(),
            },
        ]));
        mgr.install_test_transport(Language::Rust, fake).await;

        let block = mgr.diagnostics_for(&path, 1).await.expect("has block");
        assert_eq!(block.items.len(), 2);
        // 排序后错误排在前面。
        assert_eq!(block.items[0].severity, Severity::Error);
        assert_eq!(block.items[1].severity, Severity::Warning);
    }

    #[tokio::test]
    async fn truncates_to_max_per_file() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = LspManager::new(
            LspConfig {
                max_diagnostics_per_file: 3,
                ..LspConfig::default()
            },
            dir.path().to_path_buf(),
        );
        let path = dir.path().join("foo.rs");
        tokio::fs::write(&path, b"fn main() {}").await.unwrap();

        let fake = Arc::new(FakeTransport::new(
            (0..10)
                .map(|i| Diagnostic {
                    line: i + 1,
                    column: 1,
                    severity: Severity::Error,
                    message: format!("err {i}"),
                })
                .collect(),
        ));
        mgr.install_test_transport(Language::Rust, fake).await;

        let block = mgr.diagnostics_for(&path, 1).await.expect("has block");
        assert_eq!(block.items.len(), 3);
    }

    #[tokio::test]
    async fn render_blocks_concatenates() {
        let blocks = vec![
            DiagnosticBlock {
                file: PathBuf::from("a.rs"),
                items: vec![Diagnostic {
                    line: 1,
                    column: 1,
                    severity: Severity::Error,
                    message: "err in a".to_string(),
                }],
            },
            DiagnosticBlock {
                file: PathBuf::from("b.rs"),
                items: vec![Diagnostic {
                    line: 2,
                    column: 2,
                    severity: Severity::Error,
                    message: "err in b".to_string(),
                }],
            },
        ];
        let rendered = render_blocks(&blocks);
        assert!(rendered.contains("file=\"a.rs\""));
        assert!(rendered.contains("file=\"b.rs\""));
    }

    #[test]
    fn relative_path_falls_back_to_filename_when_outside_workspace() {
        let workspace = PathBuf::from("/foo/bar");
        let path = PathBuf::from("/baz/qux.rs");
        assert_eq!(
            relative_to_workspace(&workspace, &path),
            PathBuf::from("qux.rs")
        );
    }

    #[test]
    fn config_resolve_uses_overrides() {
        let mut cfg = LspConfig::default();
        cfg.servers.insert(
            "rust".to_string(),
            vec!["custom-rls".to_string(), "--lsp".to_string()],
        );
        let (cmd, args) = cfg.resolve_command(Language::Rust).unwrap();
        assert_eq!(cmd, "custom-rls");
        assert_eq!(args, vec!["--lsp".to_string()]);
    }

    #[test]
    fn config_resolve_falls_back_to_registry() {
        let cfg = LspConfig::default();
        let (cmd, _) = cfg.resolve_command(Language::Rust).unwrap();
        assert_eq!(cmd, "rust-analyzer");
    }

    // ── 自定义服务器扩展名测试 ─────────────────────────────────────

    #[test]
    fn custom_for_extension_none_for_empty_config() {
        let cfg = LspConfig::default();
        assert!(cfg.custom_for_extension(&PathBuf::from("foo.rb")).is_none());
    }

    #[test]
    fn custom_for_extension_finds_registered_extension() {
        let mut cfg = LspConfig::default();
        cfg.custom.insert(
            "rb".to_string(),
            CustomLspDef {
                language_id: "ruby".to_string(),
                command: "ruby-lsp".to_string(),
                args: vec!["--stdio".to_string()],
            },
        );
        let def = cfg
            .custom_for_extension(&PathBuf::from("lib/hello.rb"))
            .expect("should find rb");
        assert_eq!(def.language_id, "ruby");
        assert_eq!(def.command, "ruby-lsp");
    }

    #[test]
    fn custom_for_extension_case_insensitive() {
        let mut cfg = LspConfig::default();
        cfg.custom.insert(
            "cs".to_string(),
            CustomLspDef {
                language_id: "csharp".to_string(),
                command: "csharp-ls".to_string(),
                args: vec![],
            },
        );
        assert!(cfg.custom_for_extension(&PathBuf::from("App.CS")).is_some());
        assert!(cfg.custom_for_extension(&PathBuf::from("App.Cs")).is_some());
    }

    #[tokio::test]
    async fn custom_fallback_only_for_other_language() {
        // 即使配置了 [lsp.custom.go]，.go 文件仍必须使用内置的 gopls 路径——custom 是回退，而非覆盖。
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = LspConfig::default();
        cfg.custom.insert(
            "go".to_string(),
            CustomLspDef {
                language_id: "go".to_string(),
                command: "custom-gopls".to_string(),
                args: vec![],
            },
        );
        let mgr = LspManager::new(cfg, dir.path().to_path_buf());
        let path = dir.path().join("main.go");
        tokio::fs::write(&path, b"package main\n").await.unwrap();

        // 为内置 Go 路径注入伪造传输层；我们不为自定义路径注入——因此如果它错误地走了自定义路由，将返回 None。
        let fake = Arc::new(FakeTransport::new(vec![Diagnostic {
            line: 1,
            column: 1,
            severity: Severity::Error,
            message: "builtin-go-diag".to_string(),
        }]));
        mgr.install_test_transport(Language::Go, fake).await;

        // 未注入自定义传输层——如果命中 custom 则返回 None。如果命中内置则返回伪造的诊断。
        let block = mgr.diagnostics_for(&path, 1).await.expect("has block");
        let rendered = block.render();
        assert!(
            rendered.contains("builtin-go-diag"),
            "should use built-in Go transport, not custom override: {rendered}"
        );
    }

    #[tokio::test]
    async fn diagnostics_for_custom_returns_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = LspConfig::default();
        cfg.custom.insert(
            "rb".to_string(),
            CustomLspDef {
                language_id: "ruby".to_string(),
                command: "ruby-lsp".to_string(),
                args: vec![],
            },
        );
        let mgr = LspManager::new(cfg, dir.path().to_path_buf());
        let path = dir.path().join("app.rb");
        tokio::fs::write(&path, b"def foo; end\n").await.unwrap();

        // 将伪造传输层注入到自定义传输层映射中。
        let fake = Arc::new(FakeTransport::new(vec![Diagnostic {
            line: 1,
            column: 5,
            severity: Severity::Error,
            message: "ruby type error".to_string(),
        }]));
        mgr.custom_transports
            .lock()
            .await
            .insert("rb".to_string(), fake.clone());

        let block = mgr.diagnostics_for(&path, 1).await.expect("has block");
        let rendered = block.render();
        assert!(rendered.contains("ruby type error"));
        assert_eq!(fake.call_count(), 1);
    }

    #[tokio::test]
    async fn custom_unregistered_extension_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = LspConfig::default();
        let mgr = LspManager::new(cfg, dir.path().to_path_buf());
        let path = dir.path().join("script.lua");
        tokio::fs::write(&path, b"print('hi')\n").await.unwrap();

        // 没有为 .lua 配置自定义，且 Lua 不是内置的 → 应为 None。
        assert!(mgr.diagnostics_for(&path, 1).await.is_none());
    }
}
