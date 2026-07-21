//! CodeWhale 代理系统的工具规范特征。
//!
//! 本模块定义了工具的核心抽象：
//! - `ToolSpec`: 所有工具必须实现的主要特征
//! - `ToolContext`: 传递给工具的执行上下文
//! - `ToolResult`: 工具执行的统一结果类型
//! - `ToolCapability`: 工具的能力和需求

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::features::Features;
use crate::lsp::LspManager;
use crate::network_policy::NetworkPolicyDecider;
use crate::rlm::session::SessionObjectSnapshot;
use crate::rlm::session::{SharedRlmSessionStore, new_shared_rlm_session_store};
use crate::sandbox::backend::SandboxBackend;
use crate::tools::handle::{SharedHandleStore, new_shared_handle_store};
use crate::tools::shell::{SharedShellManager, new_shared_shell_manager};
use crate::worker_profile::ShellPolicy;
#[allow(unused_imports)]
pub use codewhale_tools::{
    ApprovalRequirement, ToolCapability, ToolError, ToolResult, optional_bool, optional_str,
    optional_u64, required_str, required_u64,
};

#[async_trait]
pub trait DynamicToolExecutor: Send + Sync {
    async fn execute_dynamic_tool(
        &self,
        thread_id: Option<String>,
        namespace: Option<String>,
        name: String,
        input: Value,
    ) -> Result<ToolResult, ToolError>;
}

/// 可选持久化运行时服务，提供给模型可见的工具使用。
///
/// 这些服务有意设计为可选的，以便现有的单元测试和一次性工具
/// 上下文能够继续工作。需要持久化任务/自动化状态的工具在相关服务
/// 未附加时会以清晰的"不可用"错误关闭。
#[derive(Clone)]
pub struct RuntimeToolServices {
    pub shell_manager: Option<SharedShellManager>,
    pub task_manager: Option<crate::task_manager::SharedTaskManager>,
    pub automations: Option<crate::automation_manager::SharedAutomationManager>,
    pub task_data_dir: Option<PathBuf>,
    pub active_task_id: Option<String>,
    pub active_thread_id: Option<String>,
    pub dynamic_tool_executor: Option<Arc<dyn DynamicToolExecutor>>,
    /// `shell_env` 注入（#456）以及任何未来工具端钩子事件的
    /// 钩子执行器。在活动引擎之外为 `None`，
    /// 不关心钩子的测试上下文会得到一个空操作实例。
    pub hook_executor: Option<std::sync::Arc<crate::hooks::HookExecutor>>,
    /// `var_handle` 负载的每会话后端存储。克隆的工具
    /// 上下文共享此 Arc，因此句柄可以在多次交互中存活。
    pub handle_store: SharedHandleStore,
    /// 每会话持久化 RLM 内核，由调用者选择的上下文名称作为键。
    pub rlm_sessions: SharedRlmSessionStore,
}

impl Default for RuntimeToolServices {
    fn default() -> Self {
        Self {
            shell_manager: None,
            task_manager: None,
            automations: None,
            task_data_dir: None,
            active_task_id: None,
            active_thread_id: None,
            dynamic_tool_executor: None,
            hook_executor: None,
            handle_store: new_shared_handle_store(),
            rlm_sessions: new_shared_rlm_session_store(),
        }
    }
}

impl std::fmt::Debug for RuntimeToolServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeToolServices")
            .field("shell_manager", &self.shell_manager.is_some())
            .field("task_manager", &self.task_manager.is_some())
            .field("automations", &self.automations.is_some())
            .field("task_data_dir", &self.task_data_dir)
            .field("active_task_id", &self.active_task_id)
            .field("active_thread_id", &self.active_thread_id)
            .field(
                "dynamic_tool_executor",
                &self.dynamic_tool_executor.is_some(),
            )
            .field("hook_executor", &self.hook_executor.is_some())
            .field("handle_store", &true)
            .field("rlm_sessions", &true)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileReadSnapshot {
    len: u64,
    modified: Option<SystemTime>,
}

#[derive(Debug, Default)]
pub struct FileReadTracker {
    reads: HashMap<PathBuf, FileReadSnapshot>,
}

pub type SharedFileReadTracker = Arc<Mutex<FileReadTracker>>;

fn new_shared_file_read_tracker() -> SharedFileReadTracker {
    Arc::new(Mutex::new(FileReadTracker::default()))
}

fn file_read_snapshot(path: &Path) -> Result<FileReadSnapshot, ToolError> {
    let metadata = fs::metadata(path).map_err(|e| {
        ToolError::execution_failed(format!("Failed to inspect {}: {e}", path.display()))
    })?;
    Ok(FileReadSnapshot {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

/// 命令执行的沙箱策略。
#[derive(Debug, Clone, Default)]
pub enum SandboxPolicy {
    /// 无沙箱（危险，但有时需要）
    #[default]
    None,
}

/// 执行期间传递给工具的上下文。
#[derive(Clone)]
pub struct ToolContext {
    /// 工作区根目录
    pub workspace: PathBuf,
    /// 用于后台任务和流式 IO 的共享 shell 管理器。
    pub shell_manager: SharedShellManager,
    /// 由 `read_file` 成功观察到的文件的每会话快照。
    /// 修改工具使用此信息拒绝针对未读取或过期内容的窄编辑。
    pub file_read_tracker: SharedFileReadTracker,
    /// 拥有通过此上下文启动的工具工作的子代理。根用户
    /// 轮次保持未设置；子上下文会标记此信息，以便长时间运行的 shell
    /// 作业可以在 UI 界面中被归因。
    pub owner_agent_id: Option<String>,
    pub owner_agent_name: Option<String>,
    /// 是否允许工作区之外的路径
    pub trust_mode: bool,
    /// 当前的沙箱策略
    #[allow(dead_code)]
    pub sandbox_policy: SandboxPolicy,
    /// 笔记文件路径
    pub notes_path: PathBuf,
    /// MCP 配置路径
    #[allow(dead_code)]
    pub mcp_config_path: PathBuf,
    /// 显式技能目录，用于模型可见的技能发现。
    pub skills_dir: Option<PathBuf>,
    /// 将技能发现限制为 CodeWhale 拥有的根目录加上 `skills_dir`。
    pub skills_scan_codewhale_only: bool,
    /// 提升的沙箱策略覆盖（在沙箱拒绝后重试时使用）。
    /// 此设置覆盖 shell 命令的默认沙箱行为。
    pub elevated_sandbox_policy: Option<crate::sandbox::SandboxPolicy>,
    /// 当 shell 命令因活动沙箱策略有意拒绝出站网络访问而
    /// 失败时，可选的面向用户的提示。
    pub shell_network_denied_hint: Option<String>,
    /// 工具是否应跳过安全检查自动批准（YOLO 模式）。
    /// 启用时，shell 执行的命令安全分析会被跳过。
    pub auto_approve: bool,
    /// 此执行上下文的有效 shell 策略。
    pub shell_policy: ShellPolicy,
    /// 运行中会话的有效特性标志集合。
    pub features: Features,
    /// 应限定在当前会话/线程范围内的工具状态命名空间。
    pub state_namespace: String,
    /// 用户信任的外部路径，即使它们位于 `workspace` 之外，代理也可以读写。
    /// 从 `~/.deepseek/workspace-trust.json` 加载，
    /// 并在用户运行 `/trust add <path>` 时刷新。与
    /// `trust_mode` 不同，后者是全有或全无的旧式开关（#29）。
    pub trusted_external_paths: Vec<PathBuf>,
    /// 在文件发现和工具操作期间是否跟随符号链接。
    /// 启用时，会遍历符号链接目录，并且解析到工作区之外的
    /// 符号链接路径仍然允许（符号链接本身必须在工作区内）。
    /// 反映 `workspace_follow_symlinks` 设置。
    pub follow_symlinks: bool,
    /// 每域名网络策略（#135）。为 `None` 时，网络工具回退到
    /// 宽松默认值，反映 v0.7.0 之前的行为，以便测试和其他
    /// 未构造实际策略的上下文能够继续工作。
    pub network_policy: Option<NetworkPolicyDecider>,
    /// 任务、门控、PR 尝试、GitHub 证据和自动化工具的
    /// 持久化运行时服务。
    pub runtime: RuntimeToolServices,
    /// 活动提示词/会话/历史的快照，以符号化 RLM 对象形式暴露。
    /// 工具仅接收紧凑卡片，除非通过 `rlm_open` 显式打开有界对象。
    pub session_objects: Option<SessionObjectSnapshot>,
    /// 活动引擎轮次的取消令牌。可能等待外部工作的工具应
    /// 观察此令牌，以便 UI 取消可以中断它们。
    pub cancel_token: Option<CancellationToken>,
    /// shell 执行的可选外部沙箱后端。
    /// 设置时，exec_shell 通过此后端路由命令，而不是生成本地进程。
    pub sandbox_backend: Option<std::sync::Arc<dyn SandboxBackend>>,
    /// 用户记忆文件的路径。当用户记忆功能（#489）禁用时为 `None` —
    /// 读取或写入该文件的工具应在 `None` 时短路处理，
    /// 而不是回退到工作区本地默认值。
    pub memory_path: Option<PathBuf>,
    /// 用于编辑后诊断注入的 LSP 管理器（#428）。当 LSP 禁用或
    /// 上下文构建于不需要诊断的测试中时为 `None`。编辑工具在
    /// 此管理器存在且启用时，会在其结果后附加 `<diagnostics>` 块。
    pub lsp_manager: Option<Arc<LspManager>>,

    /// 大型输出路由器（#548）。为 `Some` 时，超过配置令牌阈值的
    /// 工具结果在返回给父上下文之前，会通过 V4-Flash 摘要
    /// 子代理进行路由。`None` 禁用路由（例如在子代理和测试
    /// 上下文中以避免递归）。
    pub large_output_router: Option<crate::tools::large_output_router::LargeOutputRouter>,

    /// `web_search` 应使用的搜索后端。默认：DuckDuckGo。通过
    /// `config.toml` 中的 `[search] provider` 设置。
    pub search_provider: crate::config::SearchProvider,
    /// Tavily、Bocha、Metaso 或 Baidu 的 API 密钥。Bing 或 DuckDuckGo 为 `None`。
    /// Metaso 也会回退到 `METASO_API_KEY` 环境变量，然后是内置密钥。
    /// Baidu 也会回退到 `BAIDU_SEARCH_API_KEY`。
    pub search_api_key: Option<String>,
    /// `web_search` 的可选 DuckDuckGo 兼容 HTML 端点覆盖。
    pub search_base_url: Option<String>,

    /// 每会话工作坊变量存储（#548）。保存最新的大工具路由事件的原始内容，
    /// 以便父上下文稍后可以调用 `promote_to_context`。
    /// 当路由器禁用时为 `None`。
    pub workshop_vars: Option<
        std::sync::Arc<tokio::sync::Mutex<crate::tools::large_output_router::WorkshopVariables>>,
    >,
}

impl ToolContext {
    /// 使用默认设置创建一个新的 `ToolContext`。
    #[must_use]
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let workspace = workspace.into();
        let shell_manager = new_shared_shell_manager(workspace.clone());
        // 优先使用 .codewhale，回退到 .deepseek 用于项目本地状态
        let notes_path = codewhale_config::resolve_project_state_dir(&workspace, "notes.md")
            .expect("hardcoded project notes state path is valid")
            .1;
        let mcp_config_path = codewhale_config::resolve_project_state_dir(&workspace, "mcp.json")
            .expect("hardcoded project MCP state path is valid")
            .1;
        Self {
            workspace,
            shell_manager,
            file_read_tracker: new_shared_file_read_tracker(),
            owner_agent_id: None,
            owner_agent_name: None,
            trust_mode: false,
            sandbox_policy: SandboxPolicy::None,
            notes_path,
            mcp_config_path,
            skills_dir: None,
            skills_scan_codewhale_only: false,
            elevated_sandbox_policy: None,
            shell_network_denied_hint: None,
            auto_approve: false,
            shell_policy: ShellPolicy::Full,
            features: Features::with_defaults(),
            state_namespace: "workspace".to_string(),
            trusted_external_paths: Vec::new(),
            follow_symlinks: false,
            network_policy: None,
            runtime: RuntimeToolServices::default(),
            session_objects: None,
            cancel_token: None,
            sandbox_backend: None,
            memory_path: None,
            lsp_manager: None,
            large_output_router: None,
            search_provider: crate::config::SearchProvider::default(),
            search_api_key: None,
            search_base_url: None,
            workshop_vars: None,
        }
    }

    /// 创建一个包含所有指定设置的 `ToolContext`。
    #[allow(dead_code)]
    pub fn with_options(
        workspace: impl Into<PathBuf>,
        trust_mode: bool,
        notes_path: impl Into<PathBuf>,
        mcp_config_path: impl Into<PathBuf>,
    ) -> Self {
        let workspace = workspace.into();
        let shell_manager = new_shared_shell_manager(workspace.clone());
        Self {
            workspace,
            shell_manager,
            file_read_tracker: new_shared_file_read_tracker(),
            owner_agent_id: None,
            owner_agent_name: None,
            trust_mode,
            sandbox_policy: SandboxPolicy::None,
            notes_path: notes_path.into(),
            mcp_config_path: mcp_config_path.into(),
            skills_dir: None,
            skills_scan_codewhale_only: false,
            elevated_sandbox_policy: None,
            shell_network_denied_hint: None,
            auto_approve: false,
            shell_policy: ShellPolicy::Full,
            features: Features::with_defaults(),
            state_namespace: "workspace".to_string(),
            trusted_external_paths: Vec::new(),
            follow_symlinks: false,
            network_policy: None,
            runtime: RuntimeToolServices::default(),
            session_objects: None,
            cancel_token: None,
            sandbox_backend: None,
            memory_path: None,
            lsp_manager: None,
            large_output_router: None,
            search_provider: crate::config::SearchProvider::default(),
            search_api_key: None,
            search_base_url: None,
            workshop_vars: None,
        }
    }

    /// 创建一个自动批准模式（YOLO）的 `ToolContext`。
    pub fn with_auto_approve(
        workspace: impl Into<PathBuf>,
        trust_mode: bool,
        notes_path: impl Into<PathBuf>,
        mcp_config_path: impl Into<PathBuf>,
        auto_approve: bool,
    ) -> Self {
        let workspace = workspace.into();
        let shell_manager = new_shared_shell_manager(workspace.clone());
        Self {
            workspace,
            shell_manager,
            file_read_tracker: new_shared_file_read_tracker(),
            owner_agent_id: None,
            owner_agent_name: None,
            trust_mode,
            sandbox_policy: SandboxPolicy::None,
            notes_path: notes_path.into(),
            mcp_config_path: mcp_config_path.into(),
            skills_dir: None,
            skills_scan_codewhale_only: false,
            elevated_sandbox_policy: None,
            shell_network_denied_hint: None,
            auto_approve,
            shell_policy: ShellPolicy::Full,
            features: Features::with_defaults(),
            state_namespace: "workspace".to_string(),
            trusted_external_paths: Vec::new(),
            follow_symlinks: false,
            network_policy: None,
            runtime: RuntimeToolServices::default(),
            session_objects: None,
            cancel_token: None,
            sandbox_backend: None,
            memory_path: None,
            lsp_manager: None,
            large_output_router: None,
            search_provider: crate::config::SearchProvider::default(),
            search_api_key: None,
            search_base_url: None,
            workshop_vars: None,
        }
    }

    /// 为此上下文附加每域名网络策略（#135）。
    #[must_use]
    pub fn with_network_policy(mut self, policy: NetworkPolicyDecider) -> Self {
        self.network_policy = Some(policy);
        self
    }

    /// 为工具附加持久化运行时服务。
    #[must_use]
    pub fn with_runtime_services(mut self, runtime: RuntimeToolServices) -> Self {
        self.runtime = runtime;
        self
    }

    /// 用拥有该工具工作的子代理标记工具工作。
    #[must_use]
    pub fn with_owner_agent(
        mut self,
        agent_id: impl Into<String>,
        agent_name: impl Into<String>,
    ) -> Self {
        let agent_id = agent_id.into();
        let agent_name = agent_name.into();
        self.owner_agent_id = (!agent_id.trim().is_empty()).then_some(agent_id);
        self.owner_agent_name = (!agent_name.trim().is_empty()).then_some(agent_name);
        self
    }

    /// 为需要按名称解析模型可见技能的工具附加技能发现设置。
    #[must_use]
    pub fn with_skills_config(
        mut self,
        skills_dir: impl Into<PathBuf>,
        scan_codewhale_only: bool,
    ) -> Self {
        self.skills_dir = Some(skills_dir.into());
        self.skills_scan_codewhale_only = scan_codewhale_only;
        self
    }

    /// 为 RLM 工具附加活动提示词/历史/会话符号化对象。
    #[must_use]
    pub fn with_session_objects(mut self, snapshot: SessionObjectSnapshot) -> Self {
        self.session_objects = Some(snapshot);
        self
    }

    /// 附加活动引擎取消令牌。
    #[must_use]
    pub fn with_cancel_token(mut self, cancel_token: CancellationToken) -> Self {
        self.cancel_token = Some(cancel_token);
        self
    }

    /// 附加本次轮次的有效 shell 策略。
    #[must_use]
    pub fn with_shell_policy(mut self, policy: ShellPolicy) -> Self {
        self.shell_policy = policy;
        self
    }

    /// 为远程 shell 执行附加外部沙箱后端。
    #[must_use]
    #[allow(dead_code)]
    pub fn with_sandbox_backend(mut self, backend: std::sync::Arc<dyn SandboxBackend>) -> Self {
        self.sandbox_backend = Some(backend);
        self
    }

    /// 设置用户信任的外部路径（从 `~/.deepseek/workspace-trust.json` 加载）。
    /// 关于如何查询此列表，请参见 [`Self::resolve_path`]。
    #[must_use]
    pub fn with_trusted_external_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.trusted_external_paths = paths;
        self
    }

    /// 设置工具是否应跟随符号链接。启用时，
    /// `resolve_path` 允许解析到工作区之外的符号链接路径，
    /// 基于遍历的工具会遍历符号链接目录。
    /// 反映 `workspace_follow_symlinks` 设置。
    #[must_use]
    pub fn with_follow_symlinks(mut self, follow: bool) -> Self {
        self.follow_symlinks = follow;
        self
    }

    /// 附加 LSP 管理器，以便编辑工具在成功修改文件后
    /// 自动将诊断注入其结果（#428）。
    #[must_use]
    #[allow(dead_code)]
    pub fn with_lsp_manager(mut self, manager: Arc<LspManager>) -> Self {
        self.lsp_manager = Some(manager);
        self
    }

    /// 记录调用者已观察到文件的当前磁盘状态。
    /// 这有意为尽力而为，以便在成功的读写操作完成后，
    /// 不会因为操作后元数据查找与文件系统变更发生竞争而失败。
    pub fn note_file_read(&self, path: &Path) {
        let Ok(snapshot) = file_read_snapshot(path) else {
            return;
        };
        let Ok(mut tracker) = self.file_read_tracker.lock() else {
            return;
        };
        tracker.reads.insert(path.to_path_buf(), snapshot);
    }

    /// 在窄范围原地编辑之前要求成功且仍然新鲜的 `read_file` 快照。
    /// 这捕获了模型针对猜测或过期内容进行的编辑，
    /// 同时保持事务性补丁预检查的独立性。
    pub fn require_fresh_file_read(
        &self,
        path: &Path,
        requested_path: &str,
    ) -> Result<(), ToolError> {
        let prior = {
            let tracker = self.file_read_tracker.lock().map_err(|_| {
                ToolError::execution_failed(
                    "Failed to check read-before-edit state: tracker lock poisoned".to_string(),
                )
            })?;
            tracker.reads.get(path).cloned()
        };

        let Some(prior) = prior else {
            return Err(ToolError::execution_failed(format!(
                "Refusing edit_file for {} because it has not been read in this session. \
                 Recovery: call read_file with path=\"{requested_path}\" to inspect the current contents, \
                 then retry edit_file with a unique search string.",
                path.display()
            )));
        };

        let current = file_read_snapshot(path).map_err(|e| {
            ToolError::execution_failed(format!(
                "Refusing edit_file for {} because the file could not be checked for staleness ({e}). \
                 Recovery: call read_file with path=\"{requested_path}\" again, then retry edit_file.",
                path.display()
            ))
        })?;

        if current != prior {
            return Err(ToolError::execution_failed(format!(
                "Refusing edit_file for {} because it changed since the last read_file call. \
                 Recovery: call read_file with path=\"{requested_path}\" again and retry with the current contents.",
                path.display()
            )));
        }

        Ok(())
    }

    /// 解析相对于工作区的路径，验证它不会逃逸。
    ///
    /// 这处理现有文件（使用 canonicalize）和不存在文件
    ///（用于写入操作）——通过规范化父目录并附加文件名。
    /// 解析相对于工作区的路径，验证它不会逃逸。
    ///
    /// # 示例
    ///
    /// ```ignore
    /// # use crate::tools::spec::ToolContext;
    /// let ctx = ToolContext::new(".");
    /// let path = ctx.resolve_path("README.md")?;
    /// # Ok::<(), crate::tools::spec::ToolError>(())
    /// ```
    pub fn resolve_path(&self, raw: &str) -> Result<PathBuf, ToolError> {
        let candidate = if std::path::Path::new(raw).is_absolute() {
            PathBuf::from(raw)
        } else {
            self.workspace.join(raw)
        };

        // 在信任模式下，允许任何路径无需验证
        if self.trust_mode {
            // 仍然尝试规范化以保持一致性，但不要求必须成功
            return Ok(candidate.canonicalize().unwrap_or(candidate));
        }

        // 尝试规范化工作区路径
        let workspace_canonical = self
            .workspace
            .canonicalize()
            .unwrap_or_else(|_| self.workspace.clone());

        // 当 follow_symlinks 启用时，首先检查非规范化的（符号链接）
        // 路径是否在工作区内。工作区内解析到外部的符号链接
        // 是允许的——符号链接本身是门控。
        if self.follow_symlinks {
            let candidate_normalized = normalize_path(&candidate);
            let workspace_normalized = normalize_path(&self.workspace);
            let workspace_canonical_normalized = normalize_path(&workspace_canonical);

            if candidate_normalized.starts_with(&workspace_normalized)
                || candidate_normalized.starts_with(&workspace_canonical_normalized)
            {
                // 符号链接（或普通路径）在工作区内。
                // 返回规范化后的目标路径，以便文件 I/O 正常工作。
                if candidate.exists() {
                    return Ok(candidate.canonicalize().unwrap_or(candidate));
                }
                // 不存在的路径：规范化最深层的现有祖先
                return self.resolve_nonexistent_path(candidate, &workspace_canonical);
            }

            // 即使未解析符号链接，路径也在工作区之外。
            // 回退到标准逃逸检查。
        }

        // 对于初始检查，也尝试规范化候选路径
        // 这处理像 macOS 上 /var -> /private/var 这样的符号链接
        let candidate_canonical = candidate
            .canonicalize()
            .unwrap_or_else(|_| normalize_path(&candidate));
        let workspace_normalized = normalize_path(&workspace_canonical);

        // 检查候选路径是否在工作区内（比较规范化路径）
        if !candidate_canonical.starts_with(&workspace_normalized) {
            // 也尝试非规范化的工作区路径，适用于工作区本身尚未被规范化的情况
            let workspace_plain = normalize_path(&self.workspace);
            let candidate_normalized = normalize_path(&candidate);
            if !candidate_normalized.starts_with(&workspace_plain)
                && !self.is_trusted_external_path(&candidate_canonical)
                && !self.is_trusted_external_path(&candidate_normalized)
            {
                return Err(ToolError::PathEscape {
                    path: candidate_canonical,
                });
            }
        }

        // 对于现有路径，直接使用 canonicalize
        if candidate.exists() {
            let canonical = candidate.canonicalize().map_err(|e| {
                ToolError::execution_failed(format!(
                    "Failed to canonicalize {}: {}",
                    candidate.display(),
                    e
                ))
            })?;

            if !canonical.starts_with(&workspace_canonical)
                && !self.is_trusted_external_path(&canonical)
            {
                return Err(ToolError::PathEscape { path: canonical });
            }

            return Ok(canonical);
        }

        self.resolve_nonexistent_path(candidate, &workspace_canonical)
    }

    /// 通过规范化其最深层的现有祖先并验证结果在
    /// 工作区或信任的外部路径下来解析不存在的路径。
    fn resolve_nonexistent_path(
        &self,
        candidate: PathBuf,
        workspace_canonical: &Path,
    ) -> Result<PathBuf, ToolError> {
        let workspace_normalized = normalize_path(workspace_canonical);
        let workspace_plain = normalize_path(&self.workspace);
        let mut existing_ancestor = candidate.clone();
        let mut suffix_parts: Vec<std::ffi::OsString> = Vec::new();

        while !existing_ancestor.exists() {
            if let Some(file_name) = existing_ancestor.file_name() {
                suffix_parts.push(file_name.to_owned());
            }
            match existing_ancestor.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => {
                    existing_ancestor = parent.to_path_buf();
                }
                _ => {
                    // 未找到现有父目录；回退到简单检查
                    break;
                }
            }
        }
        let ancestor_normalized = normalize_path(&existing_ancestor);

        let canonical_ancestor = if existing_ancestor.exists() {
            existing_ancestor
                .canonicalize()
                .unwrap_or(existing_ancestor)
        } else {
            existing_ancestor
        };

        // 从规范化后的祖先重建完整路径
        let mut canonical = canonical_ancestor;
        for part in suffix_parts.into_iter().rev() {
            canonical.push(part);
        }
        let canonical = normalize_path(&canonical);

        if self.follow_symlinks
            && (ancestor_normalized.starts_with(&workspace_plain)
                || ancestor_normalized.starts_with(&workspace_normalized))
        {
            return Ok(canonical);
        }

        // 验证路径在工作区内，或者在用户信任的外部路径下
        //（来自斜杠命令的 `/trust add <path>`，持久化在
        // `~/.deepseek/workspace-trust.json` 中）。
        if !canonical.starts_with(workspace_canonical)
            && !canonical.starts_with(&workspace_normalized)
            && !self.is_trusted_external_path(&canonical)
        {
            return Err(ToolError::PathEscape { path: canonical });
        }

        Ok(canonical)
    }

    /// 检查 `path` 是否在用户信任的外部根目录下。
    /// 调用者应传入已规范化（或标准化）的路径。
    fn is_trusted_external_path(&self, path: &Path) -> bool {
        self.trusted_external_paths
            .iter()
            .any(|trusted| path.starts_with(trusted))
    }

    /// 设置信任模式。
    #[allow(dead_code)]
    pub fn with_trust_mode(mut self, trust: bool) -> Self {
        self.trust_mode = trust;
        self
    }

    /// 设置沙箱策略。
    #[allow(dead_code)]
    pub fn with_sandbox_policy(mut self, policy: SandboxPolicy) -> Self {
        self.sandbox_policy = policy;
        self
    }

    /// 设置工具执行的特性标志。
    pub fn with_features(mut self, features: Features) -> Self {
        self.features = features;
        self
    }

    /// 覆盖共享的 shell 管理器。
    pub fn with_shell_manager(mut self, shell_manager: SharedShellManager) -> Self {
        self.shell_manager = shell_manager;
        self
    }

    /// 设置提升的沙箱策略覆盖。
    ///
    /// 在沙箱拒绝后重试工具时使用，以提升的权限运行。
    pub fn with_elevated_sandbox_policy(mut self, policy: crate::sandbox::SandboxPolicy) -> Self {
        self.elevated_sandbox_policy = Some(policy);
        self
    }

    /// 设置由网络受限模式使用的 shell 网络拒绝提示。
    pub fn with_shell_network_denied_hint(mut self, hint: impl Into<String>) -> Self {
        self.shell_network_denied_hint = Some(hint.into());
        self
    }

    /// 设置用于会话范围工具状态的命名空间。
    pub fn with_state_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.state_namespace = namespace.into();
        self
    }

    /// 附加大型输出路由器（#548）。设置后，超过配置令牌阈值的
    /// 工具结果在返回给父上下文之前，会由 V4-Flash 子代理进行摘要合成。
    #[must_use]
    pub fn with_large_output_router(
        mut self,
        router: crate::tools::large_output_router::LargeOutputRouter,
        vars: std::sync::Arc<
            tokio::sync::Mutex<crate::tools::large_output_router::WorkshopVariables>,
        >,
    ) -> Self {
        self.large_output_router = Some(router);
        self.workshop_vars = Some(vars);
        self
    }
}

/// 使用 `context` 中存储的管理器收集 `paths` 的 LSP 诊断，
/// 并返回由换行符连接的渲染后的 `<diagnostics …>` 块。
///
/// 在以下情况下返回空字符串：
/// - `context.lsp_manager` 为 `None`
/// - 管理器的 `enabled` 标志为 `false`
/// - 没有文件产生诊断（例如全部干净，或语言未知）
///
/// 此函数设计为非阻塞的：所有失败模式（缺少 LSP 二进制文件、超时、未知语言）
/// 都会降级为空字符串，而不是向调用者传播错误。
pub async fn lsp_diagnostics_for_paths(context: &ToolContext, paths: &[PathBuf]) -> String {
    use crate::lsp::render_blocks;

    let manager = match context.lsp_manager.as_ref() {
        Some(m) if m.config().enabled => m,
        _ => return String::new(),
    };

    let mut blocks = Vec::new();
    for (idx, path) in paths.iter().enumerate() {
        if let Some(block) = manager.diagnostics_for(path, idx as u64).await {
            blocks.push(block);
        }
    }

    render_blocks(&blocks)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut prefix: Option<std::ffi::OsString> = None;
    let mut is_root = false;
    let mut stack: Vec<std::ffi::OsString> = Vec::new();

    for component in path.components() {
        match component {
            Component::Prefix(prefix_component) => {
                prefix = Some(prefix_component.as_os_str().to_owned());
            }
            Component::RootDir => {
                is_root = true;
            }
            Component::CurDir => {}
            Component::ParentDir => {
                let parent = Component::ParentDir.as_os_str();
                if let Some(last) = stack.pop() {
                    if last == parent {
                        stack.push(last);
                        stack.push(parent.to_owned());
                    }
                } else if !is_root {
                    stack.push(parent.to_owned());
                }
            }
            Component::Normal(part) => {
                stack.push(part.to_owned());
            }
        }
    }

    let mut normalized = PathBuf::new();
    if let Some(prefix) = prefix {
        normalized.push(prefix);
    }
    if is_root {
        normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR));
    }
    for part in stack {
        normalized.push(part);
    }
    normalized
}

/// 所有工具必须实现的核心特征。
#[async_trait]
pub trait ToolSpec: Send + Sync {
    /// 返回此工具的唯一名称（用于 API 调用）。
    fn name(&self) -> &str;

    /// 返回此工具功能的人类可读描述。
    fn description(&self) -> &str;

    /// 返回工具输入参数的 JSON Schema。
    fn input_schema(&self) -> Value;

    /// 返回此工具拥有的能力。
    fn capabilities(&self) -> Vec<ToolCapability>;

    /// 返回此工具的审批要求。
    fn approval_requirement(&self) -> ApprovalRequirement {
        let caps = self.capabilities();
        if caps.contains(&ToolCapability::ExecutesCode) {
            ApprovalRequirement::Required
        } else if caps.contains(&ToolCapability::WritesFiles) {
            ApprovalRequirement::Suggest
        } else {
            ApprovalRequirement::Auto
        }
    }

    /// 返回此具体工具输入的审批要求。
    fn approval_requirement_for(&self, _input: &Value) -> ApprovalRequirement {
        self.approval_requirement()
    }

    /// 返回此工具是否可沙箱化。
    #[allow(dead_code)]
    fn is_sandboxable(&self) -> bool {
        self.capabilities().contains(&ToolCapability::Sandboxable)
    }

    /// 返回此工具是否只读。
    fn is_read_only(&self) -> bool {
        let caps = self.capabilities();
        caps.contains(&ToolCapability::ReadOnly)
            && !caps.contains(&ToolCapability::WritesFiles)
            && !caps.contains(&ToolCapability::ExecutesCode)
    }

    /// 返回此具体工具输入是否只读。
    fn is_read_only_for(&self, _input: &Value) -> bool {
        self.is_read_only()
    }

    /// 返回此工具是否可以与其他工具并行执行。
    fn supports_parallel(&self) -> bool {
        false
    }

    /// 返回此具体工具输入是否可以并行运行。
    fn supports_parallel_for(&self, _input: &Value) -> bool {
        self.supports_parallel()
    }

    /// 返回此输入是否启动持久/分离工作并立即返回。
    /// 分离启动不是只读的，但在自动批准的轮次中，
    /// 它们不需要阻塞邻近的只读检查。
    fn starts_detached_for(&self, _input: &Value) -> bool {
        false
    }

    /// 返回此工具是否应从模型可见的工具目录中排除（延迟加载）。
    /// 标记为 `true` 的工具会被注册，但在通过工具搜索显式激活之前
    /// 不会发送给模型。
    fn defer_loading(&self) -> bool {
        false
    }

    /// 返回此工具是否应在面向模型的目录中展示。
    /// 隐藏的兼容性工具保持注册状态且可按名称执行，
    /// 以便保存的记录可以重放，而无需教新会话使用已弃用的拼写。
    fn model_visible(&self) -> bool {
        true
    }

    /// 使用给定的输入和上下文执行工具。
    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError>;
}

// === 单元测试 ===

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use tempfile::tempdir;

    #[test]
    fn test_tool_result_success() {
        let result = ToolResult::success("hello");
        assert!(result.success);
        assert_eq!(result.content, "hello");
        assert!(result.metadata.is_none());
    }

    #[test]
    fn test_tool_result_error() {
        let result = ToolResult::error("something failed");
        assert!(!result.success);
        assert_eq!(result.content, "something failed");
    }

    #[test]
    fn test_tool_result_json() {
        let data = json!({"key": "value"});
        let result = ToolResult::json(&data).unwrap();
        assert!(result.success);
        assert!(result.content.contains("key"));
    }

    #[test]
    fn test_tool_result_with_metadata() {
        let result = ToolResult::success("content").with_metadata(json!({"extra": true}));
        assert!(result.metadata.is_some());
    }

    #[test]
    fn test_tool_context_resolve_path_relative() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // 创建一个测试文件
        let test_file = tmp.path().join("test.txt");
        std::fs::write(&test_file, "test").expect("write");

        let resolved = ctx.resolve_path("test.txt").expect("resolve");
        assert!(resolved.ends_with("test.txt"));
    }

    #[test]
    fn test_tool_context_resolve_path_escape() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // 尝试逃逸工作区
        let result = ctx.resolve_path("/etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn test_tool_context_resolve_path_parent_traversal() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let result = ctx.resolve_path("../escape.txt");
        assert!(result.is_err());
    }

    #[test]
    fn test_tool_context_resolve_path_normalizes_parent() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let result = ctx.resolve_path("new/../safe.txt");
        assert!(result.is_ok());
    }

    #[test]
    fn test_tool_context_trust_mode() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf()).with_trust_mode(true);

        // 在信任模式下，绝对路径应该正常工作
        let result = ctx.resolve_path("/tmp");
        assert!(result.is_ok());
    }

    /// Issue #29: 即使路径位于工作区之外，用户信任的外部目录下的路径也
    /// 能成功解析，而不受信任的外部路径仍然返回 `PathEscape` 错误。
    #[test]
    fn test_tool_context_trusted_external_path_allows_escape() {
        let workspace = tempdir().expect("workspace tempdir");
        let trusted_root = tempdir().expect("trusted tempdir");
        let trusted_file = trusted_root.path().join("notes.md");
        std::fs::write(&trusted_file, "shared notes").unwrap();

        let ctx =
            ToolContext::new(workspace.path().to_path_buf()).with_trusted_external_paths(vec![
                trusted_root
                    .path()
                    .canonicalize()
                    .unwrap_or_else(|_| trusted_root.path().to_path_buf()),
            ]);

        let resolved = ctx
            .resolve_path(trusted_file.to_str().unwrap())
            .expect("trusted path should resolve");
        assert!(resolved.ends_with("notes.md"));

        // 工作区之外的路径且不在信任列表中的应该仍然失败。
        let other = tempdir().expect("untrusted tempdir");
        let other_file = other.path().join("secret.md");
        std::fs::write(&other_file, "x").unwrap();
        let err = ctx
            .resolve_path(other_file.to_str().unwrap())
            .expect_err("untrusted path must error");
        assert!(matches!(err, ToolError::PathEscape { .. }));
    }

    #[test]
    #[cfg(unix)]
    fn test_tool_context_follow_symlinks_allows_nonexistent_path_under_workspace_symlink() {
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&workspace).expect("mkdir workspace");
        std::fs::create_dir_all(outside.join("target")).expect("mkdir outside target");
        symlink(outside.join("target"), workspace.join("linked")).expect("symlink");

        let ctx = ToolContext::new(workspace).with_follow_symlinks(true);
        let resolved = ctx
            .resolve_path("linked/new.txt")
            .expect("path under workspace symlink should resolve");

        let expected = outside
            .join("target")
            .canonicalize()
            .expect("canonical target")
            .join("new.txt");
        assert_eq!(resolved, normalize_path(&expected));
    }

    #[test]
    #[cfg(unix)]
    fn test_tool_context_default_mode_rejects_nonexistent_path_under_workspace_symlink() {
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&workspace).expect("mkdir workspace");
        std::fs::create_dir_all(outside.join("target")).expect("mkdir outside target");
        symlink(outside.join("target"), workspace.join("linked")).expect("symlink");

        let ctx = ToolContext::new(workspace);
        let err = ctx
            .resolve_path("linked/new.txt")
            .expect_err("default mode should still reject workspace symlink escapes");

        assert!(matches!(err, ToolError::PathEscape { .. }));
    }

    #[test]
    fn test_required_str() {
        let input = json!({"name": "test", "count": 42});
        assert_eq!(required_str(&input, "name").unwrap(), "test");
        assert!(required_str(&input, "missing").is_err());
        assert!(required_str(&input, "count").is_err()); // 不是字符串
    }

    #[test]
    fn test_optional_str() {
        let input = json!({"name": "test"});
        assert_eq!(optional_str(&input, "name"), Some("test"));
        assert_eq!(optional_str(&input, "missing"), None);
    }

    #[test]
    fn test_required_u64() {
        let input = json!({"count": 42});
        assert_eq!(required_u64(&input, "count").unwrap(), 42);
        assert!(required_u64(&input, "missing").is_err());
    }

    #[test]
    fn test_optional_u64() {
        let input = json!({"count": 42});
        assert_eq!(optional_u64(&input, "count", 0), 42);
        assert_eq!(optional_u64(&input, "missing", 100), 100);
    }

    #[test]
    fn test_optional_bool() {
        let input = json!({"flag": true});
        assert!(optional_bool(&input, "flag", false));
        assert!(!optional_bool(&input, "missing", false));
    }

    #[test]
    fn test_tool_error_display() {
        let err = ToolError::missing_field("path");
        assert_eq!(
            format!("{err}"),
            "Failed to validate input: missing required field 'path'"
        );

        let err = ToolError::execution_failed("boom");
        assert_eq!(format!("{err}"), "Failed to execute tool: boom");
    }

    #[test]
    fn test_approval_requirement_default() {
        let level = ApprovalRequirement::default();
        assert_eq!(level, ApprovalRequirement::Auto);
    }
}
