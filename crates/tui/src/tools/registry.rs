//! 用于管理和执行工具的工具注册表。
//!
//! 注册表提供：
//! - 动态工具注册
//! - 按名称查找工具
//! - 转换为 API 工具格式
//! - 按能力过滤

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use std::path::{Path, PathBuf};

use codewhale_protocol::runtime::DynamicToolSpec;
use serde_json::Value;

use crate::client::DeepSeekClient;
use crate::models::Tool;
use crate::tools::goal::SharedGoalState;

use super::schema_canonicalize;
use super::schema_sanitize;
use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};

// === 类型 ===

/// 持有所有可用工具的注册表。
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn ToolSpec>>,
    context: ToolContext,
    /// 缓存的序列化工具目录。在发生变更后首次调用 `to_api_tools` 时惰性重建；
    /// 在多次读取之间固定，使得描述和 schema 的字节保持稳定，以利用 DeepSeek 的 KV
    /// 前缀缓存。在 `register` / `remove` / `clear` 时失效。
    api_cache: OnceLock<Vec<Tool>>,
}

impl ToolRegistry {
    /// 创建一个具有给定上下文的新空注册表。
    #[must_use]
    pub fn new(context: ToolContext) -> Self {
        Self {
            tools: HashMap::new(),
            context,
            api_cache: OnceLock::new(),
        }
    }

    /// 在注册表中注册一个工具。
    pub fn register(&mut self, tool: Arc<dyn ToolSpec>) {
        let name = tool.name().to_string();
        if self.tools.insert(name.clone(), tool).is_some() {
            tracing::warn!("Overwriting existing tool: {}", name);
        }
        self.invalidate_api_cache();
    }

    /// 一次注册多个工具。
    pub fn register_all(&mut self, tools: Vec<Arc<dyn ToolSpec>>) {
        for tool in tools {
            self.register(tool);
        }
    }

    /// 按名称获取工具。
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn ToolSpec>> {
        self.tools.get(name).cloned()
    }

    /// 检查工具是否存在。
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// 获取所有已注册的工具名称。
    #[must_use]
    #[allow(dead_code)]
    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(std::string::String::as_str).collect()
    }

    /// 获取已注册工具的数量。
    #[must_use]
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// 检查注册表是否为空。
    #[must_use]
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// 获取所有已注册的工具。
    #[must_use]
    pub fn all(&self) -> Vec<Arc<dyn ToolSpec>> {
        self.tools.values().cloned().collect()
    }

    /// 按名称使用给定输入执行工具。
    #[allow(dead_code)]
    pub async fn execute(&self, name: &str, input: Value) -> Result<String, ToolError> {
        let tool = self
            .get(name)
            .ok_or_else(|| ToolError::not_available(format!("tool '{name}' is not registered")))?;

        let result = tool.execute(input, &self.context).await?;
        Ok(result.content)
    }

    /// 按名称执行工具，返回完整的 `ToolResult`。
    pub async fn execute_full(&self, name: &str, input: Value) -> Result<ToolResult, ToolError> {
        let tool = self
            .get(name)
            .ok_or_else(|| ToolError::not_available(format!("tool '{name}' is not registered")))?;

        tool.execute(input, &self.context).await
    }

    /// 使用可选上下文覆盖执行工具。
    ///
    /// 用于在提升的沙箱策略下重试工具。
    /// 执行后，大型结果将通过 workshop 路由 (#548)。
    pub async fn execute_full_with_context(
        &self,
        name: &str,
        input: Value,
        context_override: Option<&ToolContext>,
    ) -> Result<ToolResult, ToolError> {
        let tool = self
            .get(name)
            .ok_or_else(|| ToolError::not_available(format!("tool '{name}' is not registered")))?;

        let ctx = context_override.unwrap_or(&self.context);
        let result = tool.execute(input.clone(), ctx).await?;

        // 大输出路由 (#548): 如果结果超过阈值且
        // 调用者未请求 `raw=true`，则通过 workshop 进行综合。
        let raw_bypass = input.get("raw").and_then(|v| v.as_bool()).unwrap_or(false);

        if let Some(router) = ctx.large_output_router.as_ref() {
            use crate::tools::large_output_router::{LargeOutputRouter, RouteDecision};
            match router.route(name, &result, raw_bypass) {
                RouteDecision::PassThrough => {}
                RouteDecision::Synthesise {
                    estimated_tokens,
                    threshold,
                } => {
                    // 将原始输出存储在 workshop 变量存储中。
                    if let Some(vars_arc) = ctx.workshop_vars.as_ref() {
                        let mut vars = vars_arc.lock().await;
                        vars.store_raw(name, &result.content);
                    }

                    // 使用注册表构建时所使用的同一模型（workshop Flash 模型）构建简洁的综合结果。
                    // 目前我们生成结构化头部 + 截断预览，无需实时 API 调用，
                    // 以便引擎在注册表层保持无依赖。后续可以在异步 LLM 调用安全时接入 Flash 客户端。
                    let preview_chars = 1_200usize;
                    let preview: String = result.content.chars().take(preview_chars).collect();
                    let ellipsis = if result.content.chars().count() > preview_chars {
                        "\n… [output truncated — full text in workshop variable `last_tool_result`]"
                    } else {
                        ""
                    };
                    let synthesis = format!("{preview}{ellipsis}");
                    let wrapped = LargeOutputRouter::wrap_synthesis(
                        name,
                        &synthesis,
                        estimated_tokens,
                        threshold,
                    );
                    tracing::debug!(
                        tool = name,
                        estimated_tokens,
                        threshold,
                        "large-output routed through workshop"
                    );
                    return Ok(ToolResult::success(wrapped));
                }
            }
        }

        Ok(result)
    }

    /// 获取当前工具上下文。
    #[must_use]
    pub fn context(&self) -> &ToolContext {
        &self.context
    }

    /// 将所有工具转换为 API 工具格式以发送给模型。
    ///
    /// 输出按工具名称排序以保证**前缀缓存稳定性** (#263)。
    /// Rust 的 `HashMap` 每个进程使用随机种子哈希器，因此在每次 `deepseek` 启动时，
    /// 原始的 `self.tools.values()` 迭代会以不同顺序输出工具，
    /// 使 DeepSeek 的 KV 前缀缓存在每次跨会话恢复时失效。
    /// 此处排序与 Claude Code 稳定其工具数组的方式一致（参见其参考实现中的 `assembleToolPool`）。
    ///
    /// 序列化的目录在首次调用时缓存，并在多次读取之间固定，
    /// 使得每个工具的 `description()` 和 `input_schema()` 在每个注册周期内只采样一次。
    /// 否则，MCP 适配器的上游描述在重连时发生变化，会在会话期间重写目录并破坏前缀缓存。
    /// 缓存在 `register`、`remove` 和 `clear` 时失效。
    #[must_use]
    pub fn to_api_tools(&self) -> Vec<Tool> {
        self.api_cache
            .get_or_init(|| self.build_api_tools())
            .clone()
    }

    fn build_api_tools(&self) -> Vec<Tool> {
        let mut tools: Vec<&Arc<dyn ToolSpec>> = self.tools.values().collect();
        tools.sort_by(|a, b| a.name().cmp(b.name()));
        tools
            .into_iter()
            .filter(|tool| tool.model_visible())
            .map(|tool| {
                let mut schema = tool.input_schema();
                schema_sanitize::sanitize(&mut schema);
                schema_canonicalize::canonicalize_schema(&mut schema);
                Tool {
                    tool_type: None,
                    name: tool.name().to_string(),
                    description: tool.description().to_string(),
                    input_schema: schema,
                    allowed_callers: Some(vec!["direct".to_string()]),
                    defer_loading: Some(tool.defer_loading()),
                    input_examples: None,
                    strict: None,
                    cache_control: None,
                }
            })
            .collect()
    }

    fn invalidate_api_cache(&mut self) {
        self.api_cache = OnceLock::new();
    }

    /// 将工具转换为 API 工具格式，最后一个工具可选启用缓存控制。
    #[must_use]
    pub fn to_api_tools_with_cache(&self, enable_cache: bool) -> Vec<Tool> {
        let mut tools = self.to_api_tools();
        if enable_cache && let Some(last) = tools.last_mut() {
            last.cache_control = Some(crate::models::CacheControl {
                cache_type: "ephemeral".to_string(),
            });
        }
        tools
    }

    /// 按能力过滤工具。
    #[must_use]
    #[allow(dead_code)]
    pub fn filter_by_capability(&self, capability: ToolCapability) -> Vec<Arc<dyn ToolSpec>> {
        self.tools
            .values()
            .filter(|t| t.capabilities().contains(&capability))
            .cloned()
            .collect()
    }

    /// 获取只读工具。
    #[must_use]
    #[allow(dead_code)]
    pub fn read_only_tools(&self) -> Vec<Arc<dyn ToolSpec>> {
        self.tools
            .values()
            .filter(|t| t.is_read_only())
            .cloned()
            .collect()
    }

    /// 获取需要审批的工具。
    #[must_use]
    #[allow(dead_code)]
    pub fn approval_required_tools(&self) -> Vec<Arc<dyn ToolSpec>> {
        self.tools
            .values()
            .filter(|t| t.approval_requirement() == ApprovalRequirement::Required)
            .cloned()
            .collect()
    }

    /// 获取建议审批的工具。
    #[must_use]
    #[allow(dead_code)]
    pub fn approval_suggested_tools(&self) -> Vec<Arc<dyn ToolSpec>> {
        self.tools
            .values()
            .filter(|t| {
                matches!(
                    t.approval_requirement(),
                    ApprovalRequirement::Suggest | ApprovalRequirement::Required
                )
            })
            .cloned()
            .collect()
    }

    /// 更新上下文（例如工作区变更时）。
    #[allow(dead_code)]
    pub fn set_context(&mut self, context: ToolContext) {
        self.context = context;
    }

    /// 获取当前上下文的可变引用。
    #[must_use]
    #[allow(dead_code)]
    pub fn context_mut(&mut self) -> &mut ToolContext {
        &mut self.context
    }

    /// 按名称移除工具。
    #[must_use]
    #[allow(dead_code)]
    pub fn remove(&mut self, name: &str) -> Option<Arc<dyn ToolSpec>> {
        let removed = self.tools.remove(name);
        if removed.is_some() {
            self.invalidate_api_cache();
        }
        removed
    }

    /// 将非规范工具名称解析为已注册的规范名称。
    ///
    /// 对已注册的工具名称运行确定性阶梯匹配：
    /// 1. 小写精确匹配。
    /// 2. 连字符/空格 → 下划线 (read-file → read_file)。
    /// 3. 驼峰式 → 蛇形式 (ReadFile → read_file)。
    /// 4. 去除尾部 `_tool` / `-tool` 后缀（两次）。
    /// 5. 通过简单前缀/后缀相似度进行模糊匹配。
    ///
    /// 当未找到解析结果时返回 `None`（由调用者提示"未知工具"）。
    #[must_use]
    pub fn resolve(&self, requested: &str) -> Option<&str> {
        let names: Vec<&str> = self.tools.keys().map(String::as_str).collect();
        let lower = requested.to_lowercase();

        // 1. ASCII 大小写不敏感精确匹配
        if let Some(n) = names.iter().find(|n| n.eq_ignore_ascii_case(requested)) {
            return Some(n);
        }
        // 2. 连字符/空格 → 下划线
        let snaked = lower.replace(['-', ' '], "_");
        if let Some(n) = names.iter().find(|n| **n == snaked) {
            return Some(n);
        }
        // 3. 驼峰式 → 蛇形式
        let cc = to_snake_case(requested);
        if let Some(n) = names.iter().find(|n| **n == cc) {
            return Some(n);
        }
        // 4. 去除 _tool/-tool/tool 后缀，执行两次
        let mut stripped = cc.clone();
        for _ in 0..2 {
            for suf in ["_tool", "-tool", "tool"] {
                if let Some(s) = stripped.strip_suffix(suf) {
                    stripped = s.to_string();
                    break;
                }
            }
        }
        if !stripped.is_empty()
            && let Some(n) = names.iter().find(|n| **n == stripped)
        {
            return Some(n);
        }
        // 5. 模糊匹配：简单前缀匹配（至少 3 个字符）
        if lower.len() >= 3 {
            for n in &names {
                if n.len() >= 3 && (n.starts_with(&lower) || lower.starts_with(n)) {
                    return Some(n);
                }
            }
        }
        None
    }

    /// 清除注册表中的所有工具。
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.tools.clear();
        self.invalidate_api_cache();
    }

    /// 按名称从注册表中移除一个工具。如果工具存在并已移除则返回 `true`，
    /// 如果不存在该名称的工具则返回 `false`。
    pub fn remove_tool(&mut self, name: &str) -> bool {
        let existed = self.tools.remove(name).is_some();
        if existed {
            self.invalidate_api_cache();
        }
        existed
    }

    /// 将 config.toml 中的工具覆盖应用于此注册表。
    ///
    /// 对于 `overrides` 中的每个条目：
    /// - `Disabled` 移除该工具。
    /// - `Script` / `Command` 用用户的实现替换该工具。
    ///
    /// `plugin_dir` 用作相对脚本路径的基础目录。
    pub fn apply_overrides(
        &mut self,
        overrides: &std::collections::HashMap<String, crate::config::ToolOverride>,
        plugin_dir: &Path,
    ) {
        for (tool_name, override_cfg) in overrides {
            match override_cfg {
                crate::config::ToolOverride::Disabled => {
                    if self.remove_tool(tool_name) {
                        tracing::info!("Tool '{}' disabled via config override", tool_name);
                    } else {
                        tracing::warn!("Cannot disable tool '{}': not registered", tool_name);
                    }
                }
                _ => {
                    // Script 和 Command 覆盖会创建替换工具。
                    use crate::tools::plugin::tool_from_override;
                    match tool_from_override(tool_name, override_cfg, plugin_dir) {
                        Some(replacement) => {
                            self.register(replacement);
                            tracing::info!("Tool '{}' replaced via config override", tool_name);
                        }
                        None => {
                            if self.remove_tool(tool_name) {
                                tracing::warn!(
                                    "Tool '{}' override did not create a replacement; removed the original tool to avoid override fallthrough",
                                    tool_name
                                );
                            } else {
                                tracing::warn!(
                                    "Tool '{}' override did not create a replacement and no registered tool existed",
                                    tool_name
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// 从目录中加载并注册插件工具。
    ///
    /// 每个具有有效前置元数据（`# name:`、`# description:` 等）的脚本
    /// 都会注册为一个 `ScriptPluginTool`。名称与已注册工具匹配的工具将覆盖之。
    pub fn load_plugins(&mut self, plugin_dir: &Path) {
        if !plugin_dir.exists() {
            tracing::debug!(
                "Plugin directory {} does not exist, skipping",
                plugin_dir.display()
            );
            return;
        }
        let plugins = crate::tools::plugin::load_plugin_tools(plugin_dir);
        let count = plugins.len();
        for tool in plugins {
            self.register(tool);
        }
        if count > 0 {
            tracing::info!(
                "Loaded {count} plugin tool(s) from {}",
                plugin_dir.display()
            );
        }
    }
}

/// 用于构建包含常用工具的 `ToolRegistry` 的构建器。
pub struct ToolRegistryBuilder {
    tools: Vec<Arc<dyn ToolSpec>>,
}

/// 依赖于特性/配置的原生 Agent 模式工具表面。
///
/// 父 Agent/Yolo 轮次和默认子代理都通过此选项对象构建，
/// 以便目录不会因新的一方可选工具被特性标志或配置状态所限而出现差异。
#[derive(Clone)]
pub struct AgentToolSurfaceOptions {
    pub shell_policy: crate::worker_profile::ShellPolicy,
    pub apply_patch_enabled: bool,
    pub web_search_enabled: bool,
    pub memory_tool_enabled: bool,
    pub vision_config: Option<crate::config::VisionModelConfig>,
    pub speech_output_dir: Option<PathBuf>,
    pub goal_state: Option<SharedGoalState>,
}

impl AgentToolSurfaceOptions {
    #[must_use]
    pub fn new(shell_policy: crate::worker_profile::ShellPolicy) -> Self {
        Self {
            shell_policy,
            apply_patch_enabled: false,
            web_search_enabled: false,
            memory_tool_enabled: false,
            vision_config: None,
            speech_output_dir: None,
            goal_state: None,
        }
    }
}

impl ToolRegistryBuilder {
    /// 创建一个新的构建器。
    #[must_use]
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// 添加一个自定义工具。
    #[must_use]
    pub fn with_tool(mut self, tool: Arc<dyn ToolSpec>) -> Self {
        self.tools.push(tool);
        self
    }

    #[must_use]
    pub fn with_dynamic_tools(mut self, dynamic_tools: &[DynamicToolSpec]) -> Self {
        for tool in dynamic_tools {
            self = self.with_tool(Arc::new(super::dynamic::RuntimeDynamicTool::new(
                tool.clone(),
            )));
        }
        self
    }

    /// 包含文件工具（read、write、edit、list）。
    #[must_use]
    pub fn with_file_tools(self) -> Self {
        use super::file::{EditFileTool, ListDirTool, ReadFileTool, WriteFileTool};
        self.with_tool(Arc::new(ReadFileTool))
            .with_tool(Arc::new(WriteFileTool))
            .with_tool(Arc::new(EditFileTool))
            .with_tool(Arc::new(ListDirTool))
    }

    /// 仅包含只读文件工具（read、list）。
    #[must_use]
    pub fn with_read_only_file_tools(self) -> Self {
        use super::file::{ListDirTool, ReadFileTool};
        self.with_tool(Arc::new(ReadFileTool))
            .with_tool(Arc::new(ListDirTool))
            .with_tool(Arc::new(
                super::tool_result_retrieval::RetrieveToolResultTool,
            ))
    }

    /// 包含 shell 执行工具。
    #[must_use]
    pub fn with_shell_tools(self) -> Self {
        use super::shell::{ExecShellTool, ShellCancelTool, ShellInteractTool, ShellWaitTool};
        self.with_tool(Arc::new(ExecShellTool))
            .with_tool(Arc::new(ShellWaitTool::new("exec_shell_wait")))
            .with_tool(Arc::new(ShellInteractTool::new("exec_shell_interact")))
            .with_tool(Arc::new(ShellCancelTool))
            .with_tool(Arc::new(ShellWaitTool::new("exec_wait")))
            .with_tool(Arc::new(ShellInteractTool::new("exec_interact")))
    }

    /// 包含搜索工具（`grep_files`）。
    #[must_use]
    pub fn with_search_tools(self) -> Self {
        use super::file_search::FileSearchTool;
        use super::search::GrepFilesTool;
        self.with_tool(Arc::new(GrepFilesTool))
            .with_tool(Arc::new(FileSearchTool))
    }

    /// 包含 git 检查工具（`git_status`、`git_diff`）。
    #[must_use]
    pub fn with_git_tools(self) -> Self {
        use super::git::{GitDiffTool, GitStatusTool};
        self.with_tool(Arc::new(GitStatusTool))
            .with_tool(Arc::new(GitDiffTool))
    }

    /// 包含 git 历史工具（`git_log`、`git_show`、`git_blame`）。
    #[must_use]
    pub fn with_git_history_tools(self) -> Self {
        use super::git_history::{GitBlameTool, GitLogTool, GitShowTool};
        self.with_tool(Arc::new(GitLogTool))
            .with_tool(Arc::new(GitShowTool))
            .with_tool(Arc::new(GitBlameTool))
    }

    /// 包含工作区诊断工具。
    #[must_use]
    pub fn with_diagnostics_tool(self) -> Self {
        use super::diagnostics::DiagnosticsTool;
        self.with_tool(Arc::new(DiagnosticsTool))
    }

    /// 仅当本主机上存在 `pandoc` 二进制文件时，才包含 `pandoc_convert` 工具。
    /// 与 v0.8.31 为 Python 引入的探测后决策模式相同——
    /// 当 pandoc 缺失时工具不会注册，因此模型永远不会看到其无法实际使用的二进制文件。
    #[must_use]
    pub fn with_pandoc_tools(self) -> Self {
        if crate::dependencies::resolve_pandoc().is_some() {
            use super::pandoc::PandocConvertTool;
            self.with_tool(Arc::new(PandocConvertTool))
        } else {
            self
        }
    }

    /// 仅当存在本地 OCR 后端时，才包含 `image_ocr` 工具。
    /// macOS 使用内置的 Vision 框架，而其他平台在安装了 Tesseract 时使用之。
    #[must_use]
    pub fn with_image_ocr_tools(self) -> Self {
        if super::image_ocr::ocr_available() {
            use super::image_ocr::ImageOcrTool;
            self.with_tool(Arc::new(ImageOcrTool))
        } else {
            self
        }
    }

    /// 包含 `load_skill` 工具 (#434)，使得模型可以通过一次调用将
    /// SKILL.md 正文 + 配套文件列表拉入上下文，
    /// 而不是针对系统提示中 `## Skills` 节显示的路径执行 `read_file` + `list_dir`。
    #[must_use]
    pub fn with_skill_tools(self) -> Self {
        use super::skill::LoadSkillTool;
        self.with_tool(Arc::new(LoadSkillTool))
    }

    /// 包含项目映射工具。
    #[must_use]
    pub fn with_project_tools(self) -> Self {
        use super::project::ProjectMapTool;
        self.with_tool(Arc::new(ProjectMapTool))
    }

    /// 包含 cargo 测试运行工具。
    #[must_use]
    pub fn with_test_runner_tool(self) -> Self {
        use super::test_runner::RunTestsTool;
        use super::verifier::RunVerifiersTool;
        self.with_tool(Arc::new(RunTestsTool))
            .with_tool(Arc::new(RunVerifiersTool))
    }

    /// 包含结构化数据验证工具（`validate_data`）。
    #[must_use]
    pub fn with_validation_tools(self) -> Self {
        use super::validate_data::ValidateDataTool;
        self.with_tool(Arc::new(ValidateDataTool))
    }

    /// 包含对溢出的历史工具结果的检索。
    #[must_use]
    pub fn with_tool_result_retrieval_tool(self) -> Self {
        use super::tool_result_retrieval::RetrieveToolResultTool;
        self.with_tool(Arc::new(RetrieveToolResultTool))
    }

    /// 包含持久化任务、门控、PR 尝试、GitHub 和自动化工具。
    ///
    /// 与 shell 相关的任务工具（`task_shell_start`、`task_shell_wait`）
    /// *不*包含在此处——当 `allow_shell` 为 true 时，使用 [`with_runtime_task_shell_tools`] 注册它们。
    #[must_use]
    pub fn with_runtime_task_tools(self) -> Self {
        use super::automation::{
            AutomationCreateTool, AutomationDeleteTool, AutomationListTool, AutomationPauseTool,
            AutomationReadTool, AutomationResumeTool, AutomationRunTool, AutomationUpdateTool,
        };
        use super::github::{
            GithubCloseIssueTool, GithubClosePrTool, GithubCommentTool, GithubIssueContextTool,
            GithubPrContextTool,
        };
        use super::tasks::{
            PrAttemptListTool, PrAttemptPreflightTool, PrAttemptReadTool, PrAttemptRecordTool,
            TaskCancelTool, TaskCreateTool, TaskGateRunTool, TaskListTool, TaskReadTool,
        };

        self.with_tool(Arc::new(TaskCreateTool))
            .with_tool(Arc::new(TaskListTool))
            .with_tool(Arc::new(TaskReadTool))
            .with_tool(Arc::new(TaskCancelTool))
            .with_tool(Arc::new(TaskGateRunTool))
            .with_tool(Arc::new(GithubIssueContextTool))
            .with_tool(Arc::new(GithubPrContextTool))
            .with_tool(Arc::new(PrAttemptRecordTool))
            .with_tool(Arc::new(PrAttemptListTool))
            .with_tool(Arc::new(PrAttemptReadTool))
            .with_tool(Arc::new(PrAttemptPreflightTool))
            .with_tool(Arc::new(AutomationCreateTool))
            .with_tool(Arc::new(AutomationListTool))
            .with_tool(Arc::new(AutomationReadTool))
            .with_tool(Arc::new(AutomationUpdateTool))
            .with_tool(Arc::new(AutomationPauseTool))
            .with_tool(Arc::new(AutomationResumeTool))
            .with_tool(Arc::new(AutomationDeleteTool))
            .with_tool(Arc::new(AutomationRunTool))
            .with_tool(Arc::new(GithubCommentTool))
            .with_tool(Arc::new(GithubCloseIssueTool))
            .with_tool(Arc::new(GithubClosePrTool))
    }

    /// 包含与 shell 相关的任务工具（`task_shell_start`、`task_shell_wait`）。
    ///
    /// 这些工具受 `allow_shell` 控制，因为 `task_shell_start`
    /// 直接委托给 `ExecShellTool`，提供与 `exec_shell` 相同的 shell 执行能力。
    #[must_use]
    pub fn with_runtime_task_shell_tools(self) -> Self {
        use super::tasks::{TaskShellStartTool, TaskShellWaitTool};
        self.with_tool(Arc::new(TaskShellStartTool))
            .with_tool(Arc::new(TaskShellWaitTool))
    }

    /// 仅包含只读的持久化任务、PR 尝试、GitHub 和自动化检查工具。
    /// 计划模式使用此表面，以便可以在不启动工作、更改远程仓库或修改自动化配置的情况下观察状态。
    #[must_use]
    pub fn with_runtime_read_only_task_tools(self) -> Self {
        use super::automation::{AutomationListTool, AutomationReadTool};
        use super::github::{GithubIssueContextTool, GithubPrContextTool};
        use super::tasks::{PrAttemptListTool, PrAttemptReadTool, TaskListTool, TaskReadTool};

        self.with_tool(Arc::new(TaskListTool))
            .with_tool(Arc::new(TaskReadTool))
            .with_tool(Arc::new(GithubIssueContextTool))
            .with_tool(Arc::new(GithubPrContextTool))
            .with_tool(Arc::new(PrAttemptListTool))
            .with_tool(Arc::new(PrAttemptReadTool))
            .with_tool(Arc::new(AutomationListTool))
            .with_tool(Arc::new(AutomationReadTool))
    }

    /// 包含网络搜索和获取工具。
    ///
    /// 这些工具在 `tool_setup.rs` 中受 `Feature::WebSearch` 特性门控。
    /// `finance` 通过 `with_finance_tool()` 单独注册，不受网络搜索特性门控。
    #[must_use]
    pub fn with_web_tools(self) -> Self {
        use super::dev_server_readiness::WaitForDevServerTool;
        use super::fetch_url::FetchUrlTool;
        use super::web_run::WebRunTool;
        use super::web_search::WebSearchTool;
        self.with_tool(Arc::new(WebSearchTool))
            .with_tool(Arc::new(FetchUrlTool))
            .with_tool(Arc::new(WaitForDevServerTool))
            .with_tool(Arc::new(WebRunTool))
    }

    /// 包含 `finance` 市场数据工具。
    ///
    /// 该工具在 agent 模式下无条件注册，不受 `Feature::WebSearch` 门控
    /// （它获取的是金融数据，而非网络搜索结果）。
    #[must_use]
    pub fn with_finance_tool(self) -> Self {
        use super::finance::FinanceTool;
        self.with_tool(Arc::new(FinanceTool::new()))
    }

    /// 注册 `image_analyze` 视觉工具。
    /// 仅在 config.toml 中配置了 `[vision_model]` 时注册。
    #[must_use]
    pub fn with_vision_tools(self, config: crate::config::VisionModelConfig) -> Self {
        use crate::vision::tools::ImageAnalyzeTool;
        self.with_tool(Arc::new(ImageAnalyzeTool::new(config)))
    }

    /// 之前注册了 OpenAI 风格的 `multi_tool_use.parallel` 元工具。
    /// DeepSeek-V4 拥有原生并行工具调用（一次助手轮次中多个 `tool_calls` 条目），
    /// 而该元工具名称会触发模型幻觉生成 OpenAI 内部 XML 包装器
    /// （`<multi_tool_use.parallel><tool_name>…</tool_name>…`），而非发出原生调用。
    /// 保留为空操作以保持现有调用者可编译；引擎的兼容性分发器仍处理遗留的发射。
    #[must_use]
    pub fn with_parallel_tool(self) -> Self {
        self
    }

    /// 包含 request_user_input 工具。
    #[must_use]
    pub fn with_user_input_tool(self) -> Self {
        use super::user_input::RequestUserInputTool;
        self.with_tool(Arc::new(RequestUserInputTool))
    }

    /// 包含补丁工具（`apply_patch`）。
    #[must_use]
    pub fn with_patch_tools(self) -> Self {
        use super::apply_patch::ApplyPatchTool;
        self.with_tool(Arc::new(ApplyPatchTool))
    }

    /// 包含 `revert_turn` 工具。由于会修改工作区，因此需要审批；
    /// 模型在用户要求"撤销上一次编辑"时使用它。
    /// 由每个工作区的快照辅助仓库（`crate::snapshot`）支持。
    #[must_use]
    pub fn with_revert_turn_tool(self) -> Self {
        use super::revert_turn::RevertTurnTool;
        self.with_tool(Arc::new(RevertTurnTool))
    }

    /// 包含 Xiaomi MiMo 语音/TTS 工具（`speech`、`tts`）。
    #[must_use]
    pub fn with_speech_tools(
        self,
        client: Option<DeepSeekClient>,
        output_dir: Option<PathBuf>,
    ) -> Self {
        use super::speech::SpeechTool;
        self.with_tool(Arc::new(SpeechTool::new(
            "speech",
            client.clone(),
            output_dir.clone(),
        )))
        .with_tool(Arc::new(SpeechTool::new("tts", client, output_dir)))
    }

    /// 包含持久化 RLM 会话工具。
    #[must_use]
    pub fn with_rlm_tool(self, client: Option<DeepSeekClient>, _root_model: String) -> Self {
        use super::rlm::{
            RlmCloseTool, RlmConfigureTool, RlmEvalTool, RlmOpenTool, RlmSessionObjectsTool,
        };
        self.with_tool(Arc::new(RlmSessionObjectsTool))
            .with_tool(Arc::new(RlmOpenTool))
            .with_tool(Arc::new(RlmEvalTool::new(client)))
            .with_tool(Arc::new(RlmConfigureTool))
            .with_tool(Arc::new(RlmCloseTool))
    }

    /// 包含 `handle_read`，用于符号化 `var_handle` 载荷的有界投影读取器。
    #[must_use]
    pub fn with_handle_tools(self) -> Self {
        use super::handle::HandleReadTool;
        self.with_tool(Arc::new(HandleReadTool))
    }

    /// 包含审查工具。
    #[must_use]
    pub fn with_review_tool(self, client: Option<DeepSeekClient>, model: String) -> Self {
        use super::review::ReviewTool;
        self.with_tool(Arc::new(ReviewTool::new(client, model)))
    }

    /// 包含笔记工具。
    #[must_use]
    pub fn with_note_tool(self) -> Self {
        use super::shell::NoteTool;
        self.with_tool(Arc::new(NoteTool))
    }

    /// 包含 FIM（Fill-in-the-Middle）编辑工具。
    #[must_use]
    pub fn with_fim_tool(self, client: Option<DeepSeekClient>, model: String) -> Self {
        use super::fim::FimEditTool;
        self.with_tool(Arc::new(FimEditTool::new(client, model)))
    }

    /// 包含 `remember` 工具——模型可调用的向用户记忆文件添加条目的功能 (#489)。
    /// 仅在用户已选择启用记忆特性时注册；否则该工具会出现在模型的目录中，
    /// 但始终会因"memory disabled"而失败。
    #[must_use]
    pub fn with_remember_tool(self) -> Self {
        use super::remember::RememberTool;
        self.with_tool(Arc::new(RememberTool))
    }

    /// 包含 slop 台账工具 (#2127)——对未解决架构遗留问题的持久化追踪：
    /// 追加、查询、更新、导出。
    /// 无条件注册；台账 JSON 文件在首次追加时自动创建。
    #[must_use]
    pub fn with_slop_ledger_tools(self) -> Self {
        use crate::slop_ledger::{
            SlopLedgerAppendTool, SlopLedgerExportTool, SlopLedgerQueryTool, SlopLedgerUpdateTool,
        };
        self.with_tool(Arc::new(SlopLedgerAppendTool))
            .with_tool(Arc::new(SlopLedgerQueryTool))
            .with_tool(Arc::new(SlopLedgerUpdateTool))
            .with_tool(Arc::new(SlopLedgerExportTool))
    }

    /// slop 台账工具 (#2127) 的只读子集，用于计划模式：
    /// 仅查询和导出——不包含追加或更新。
    #[must_use]
    pub fn with_slop_ledger_read_only_tools(self) -> Self {
        use crate::slop_ledger::{SlopLedgerExportTool, SlopLedgerQueryTool};
        self.with_tool(Arc::new(SlopLedgerQueryTool))
            .with_tool(Arc::new(SlopLedgerExportTool))
    }

    /// 包含 `notify` 工具——模型可调用的桌面通知 (#1322)。
    /// 通过现有的 `tui::notifications` OSC 9 / BEL 管道路由，
    /// 因此用户的 `[notifications].method` 配置会自动生效（包括 `off`）。
    /// 始终可以安全注册，因为该工具除了单次终端转义写入外没有任何副作用。
    #[must_use]
    pub fn with_notify_tool(self) -> Self {
        use super::notify::NotifyTool;
        self.with_tool(Arc::new(NotifyTool))
    }

    /// 将已连接池中的 MCP 工具作为一等注册表公民包含进来。
    /// 每个 MCP 工具被包装在一个实现 `ToolSpec` 的轻量级适配器中，
    /// 因此统一的 `ToolRegistryBuilder` 流程可以像处理原生工具一样处理它们。
    ///
    /// MCP 工具默认标记为 `defer_loading`（发现辅助工具除外），
    /// 以保持模型可见目录的紧凑性。
    #[must_use]
    pub fn with_mcp_tools(
        mut self,
        mcp_pool: std::sync::Arc<tokio::sync::Mutex<crate::mcp::McpPool>>,
    ) -> Self {
        // 从池中快照当前工具列表（非阻塞）。
        // 适配器在执行时通过池惰性解析。
        if let Ok(pool) = mcp_pool.try_lock() {
            for (name, tool) in pool.all_tools() {
                let adapter = Arc::new(McpToolAdapter {
                    name: name.clone(),
                    tool: tool.clone(),
                    pool: mcp_pool.clone(),
                });
                self.tools.push(adapter);
            }
        }
        self
    }

    /// 注册 `start_mcp_server` 工具，用于从对话上下文中动态添加 MCP 服务器。
    /// 不注册 MCP 工具适配器——这些适配器由 `engine.mcp_tools()` 中的 `pool.to_api_tools()` 返回。
    #[must_use]
    pub fn with_runtime_mcp_tool(
        mut self,
        mcp_pool: std::sync::Arc<tokio::sync::Mutex<crate::mcp::McpPool>>,
    ) -> Self {
        self.tools
            .push(Arc::new(super::runtime_mcp::StartRuntimeMcpServer::new(
                mcp_pool,
            )));
        self
    }

    /// 包含所有 agent 工具（文件工具 + shell + 笔记 + 搜索）。
    ///
    /// 网络和补丁工具不在此处注册——调用方必须在检查特性标志后通过
    /// `.with_web_tools()` 和 `.with_patch_tools()` 添加它们（参见 `tool_setup.rs`）。
    /// 这样可以防止当 `tool_setup.rs` 在 `with_agent_tools` 之上有条件地注册它们时出现重复注册。
    #[must_use]
    #[allow(dead_code)] // legacy allow_shell convenience wrapper; used by tests, prod uses with_agent_tools_policy
    pub fn with_agent_tools(self, allow_shell: bool) -> Self {
        self.with_agent_tools_policy(crate::worker_profile::ShellPolicy::from_legacy_allow_shell(
            allow_shell,
        ))
    }

    /// 在类型化 shell 策略下包含所有 agent 工具。
    #[must_use]
    pub fn with_agent_tools_policy(self, shell_policy: crate::worker_profile::ShellPolicy) -> Self {
        let builder = self
            .with_file_tools()
            .with_note_tool()
            .with_search_tools()
            .with_user_input_tool()
            .with_parallel_tool()
            .with_git_tools()
            .with_git_history_tools()
            .with_diagnostics_tool()
            .with_project_tools()
            .with_skill_tools()
            .with_test_runner_tool()
            .with_validation_tools()
            .with_tool_result_retrieval_tool()
            .with_handle_tools()
            .with_runtime_task_tools()
            .with_revert_turn_tool()
            .with_pandoc_tools()
            .with_image_ocr_tools()
            .with_finance_tool();

        if shell_policy.allows_shell() {
            builder.with_shell_tools().with_runtime_task_shell_tools()
        } else {
            builder
        }
    }

    /// 包含父运行时和默认子代理共享的原生 Agent 模式表面，但不包括 `agent` 启动器本身。
    #[must_use]
    pub fn with_agent_runtime_surface(
        self,
        client: Option<DeepSeekClient>,
        model: String,
        options: AgentToolSurfaceOptions,
        todo_list: super::todo::SharedTodoList,
        plan_state: super::plan::SharedPlanState,
    ) -> Self {
        let speech_client = client.clone();
        let mut builder = self
            .with_agent_tools_policy(options.shell_policy)
            .with_todo_tool(todo_list)
            .with_plan_tool(plan_state)
            .with_review_tool(client.clone(), model.clone())
            .with_slop_ledger_tools()
            .with_rlm_tool(client.clone(), model.clone())
            .with_fim_tool(client, model)
            .with_speech_tools(speech_client, options.speech_output_dir.clone());

        if let Some(goal_state) = options.goal_state {
            builder = builder.with_goal_tools(goal_state);
        }
        if options.apply_patch_enabled {
            builder = builder.with_patch_tools();
        }
        if options.web_search_enabled {
            builder = builder.with_web_tools();
        }
        if options.memory_tool_enabled {
            builder = builder.with_remember_tool();
        }
        if let Some(vision_config) = options.vision_config {
            builder = builder.with_vision_tools(vision_config);
        }

        builder.with_notify_tool()
    }

    /// 完整的子代理继承 Agent 表面的旧版便捷包装器。
    ///
    /// 新的生产调用方应优先使用 [`Self::with_full_agent_surface_options`]，
    /// 以便特性/配置门控组（网络、补丁、记忆、视觉等）与父 Agent 模式注册表保持对等。
    ///
    /// `allow_shell` 反映会话的 shell 权限。`manager` 和 `runtime`
    /// 是子代理运行时——子代理传递自己的运行时，
    /// 以便孙代理可以在相同的深度/取消信封内生成。
    #[must_use]
    #[allow(dead_code)]
    #[allow(clippy::too_many_arguments)]
    pub fn with_full_agent_surface(
        self,
        client: Option<DeepSeekClient>,
        model: String,
        manager: super::subagent::SharedSubAgentManager,
        runtime: super::subagent::SubAgentRuntime,
        allow_shell: bool,
        todo_list: super::todo::SharedTodoList,
        plan_state: super::plan::SharedPlanState,
    ) -> Self {
        self.with_full_agent_surface_policy(
            client,
            model,
            manager,
            runtime,
            crate::worker_profile::ShellPolicy::from_legacy_allow_shell(allow_shell),
            todo_list,
            plan_state,
        )
    }

    /// 在已解析的特性/配置选项下包含完整的子代理继承 Agent 表面。
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn with_full_agent_surface_options(
        self,
        client: Option<DeepSeekClient>,
        model: String,
        manager: super::subagent::SharedSubAgentManager,
        runtime: super::subagent::SubAgentRuntime,
        options: AgentToolSurfaceOptions,
        todo_list: super::todo::SharedTodoList,
        plan_state: super::plan::SharedPlanState,
    ) -> Self {
        self.with_agent_runtime_surface(client, model, options, todo_list, plan_state)
            .with_subagent_tools(manager, runtime)
    }

    /// 完整的子代理继承 Agent 表面的旧版类型化 shell 包装器。
    ///
    /// 新的生产调用方应将已解析的 [`AgentToolSurfaceOptions`]
    /// 传递给 [`Self::with_full_agent_surface_options`]。
    #[must_use]
    #[allow(dead_code)]
    #[allow(clippy::too_many_arguments)]
    pub fn with_full_agent_surface_policy(
        self,
        client: Option<DeepSeekClient>,
        model: String,
        manager: super::subagent::SharedSubAgentManager,
        runtime: super::subagent::SubAgentRuntime,
        shell_policy: crate::worker_profile::ShellPolicy,
        todo_list: super::todo::SharedTodoList,
        plan_state: super::plan::SharedPlanState,
    ) -> Self {
        let mut options = AgentToolSurfaceOptions::new(shell_policy);
        options.speech_output_dir = runtime.speech_output_dir.clone();
        self.with_full_agent_surface_options(
            client, model, manager, runtime, options, todo_list, plan_state,
        )
    }

    /// 包含带有共享 `TodoList` 的待办/工作进度工具。
    ///
    /// `work_update` 是唯一的模型可见进度表面 (#4132)。
    /// `checklist_*` 和 `todo_*` 仍注册为隐藏的兼容别名，
    /// 以便保存的对话记录和旧提示仍可重放。
    #[must_use]
    pub fn with_todo_tool(self, todo_list: super::todo::SharedTodoList) -> Self {
        use super::todo::{TodoAddTool, TodoListTool, TodoUpdateTool, TodoWriteTool};
        self.with_tool(Arc::new(TodoWriteTool::work_update(todo_list.clone())))
            .with_tool(Arc::new(TodoWriteTool::checklist(todo_list.clone())))
            .with_tool(Arc::new(TodoWriteTool::todo(todo_list.clone())))
            .with_tool(Arc::new(TodoAddTool::checklist(todo_list.clone())))
            .with_tool(Arc::new(TodoAddTool::todo(todo_list.clone())))
            .with_tool(Arc::new(TodoUpdateTool::checklist(todo_list.clone())))
            .with_tool(Arc::new(TodoUpdateTool::todo(todo_list.clone())))
            .with_tool(Arc::new(TodoListTool::checklist(todo_list.clone())))
            .with_tool(Arc::new(TodoListTool::todo(todo_list.clone())))
    }

    /// 包含带有共享 `PlanState` 的计划工具。
    #[must_use]
    pub fn with_plan_tool(self, plan_state: super::plan::SharedPlanState) -> Self {
        use super::plan::UpdatePlanTool;
        self.with_tool(Arc::new(UpdatePlanTool::new(plan_state)))
    }

    /// 包含运行时目标工具（`create_goal`、`get_goal`、`update_goal`）。
    #[must_use]
    pub fn with_goal_tools(self, goal_state: super::goal::SharedGoalState) -> Self {
        use super::goal::{CreateGoalTool, GetGoalTool, UpdateGoalTool};
        self.with_tool(Arc::new(CreateGoalTool::new(goal_state.clone())))
            .with_tool(Arc::new(GetGoalTool::new(goal_state.clone())))
            .with_tool(Arc::new(UpdateGoalTool::new(goal_state)))
    }

    /// 包含子代理管理工具。
    #[must_use]
    pub fn with_subagent_tools(
        self,
        manager: super::subagent::SharedSubAgentManager,
        runtime: super::subagent::SubAgentRuntime,
    ) -> Self {
        use super::subagent::AgentTool;
        use super::workflow::WorkflowTool;
        use super::workflow_trigger::soft_auto_policy_is_linked;

        // 在发布构建中保持软自动触发策略链接 (#4127)。
        debug_assert!(
            soft_auto_policy_is_linked(),
            "workflow soft-auto policy must stay linked"
        );

        self.with_tool(Arc::new(WorkflowTool::new(
            Arc::clone(&manager),
            runtime.clone(),
        )))
        .with_tool(Arc::new(AgentTool::new(manager, runtime)))
    }

    /// 使用给定的上下文构建注册表。
    #[must_use]
    pub fn build(self, context: ToolContext) -> ToolRegistry {
        let mut registry = ToolRegistry::new(context);
        registry.register_all(self.tools);
        registry
    }
}

impl Default for ToolRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// 将驼峰式转换为蛇形式。
fn to_snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// 包装 MCP 工具定义的适配器，使其可以与原生工具一起存在于统一的 `ToolRegistry` 中 (§5.B)。
#[allow(dead_code)]
struct McpToolAdapter {
    name: String,
    tool: crate::mcp::McpTool,
    pool: std::sync::Arc<tokio::sync::Mutex<crate::mcp::McpPool>>,
}

#[async_trait::async_trait]
impl ToolSpec for McpToolAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        // McpTool.description 是 Option<String>；当缺失时回退到前缀名称。
        self.tool.description.as_deref().unwrap_or(&self.name)
    }

    fn input_schema(&self) -> Value {
        self.tool.input_schema.clone()
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        // 保守地将 MCP 工具视为需要审批和网络访问，除非它们是已知的发现辅助工具。
        let name_lower = self.name.to_lowercase();
        if name_lower.contains("list_mcp")
            || name_lower.contains("read_mcp")
            || name_lower.contains("mcp_read")
            || name_lower.contains("mcp_get_prompt")
        {
            vec![ToolCapability::ReadOnly]
        } else {
            vec![ToolCapability::Network, ToolCapability::RequiresApproval]
        }
    }

    fn defer_loading(&self) -> bool {
        // 发现辅助工具保持加载状态；其他所有工具延迟加载。
        let keep_loaded = matches!(
            self.name.as_str(),
            "list_mcp_resources"
                | "list_mcp_resource_templates"
                | "mcp_read_resource"
                | "read_mcp_resource"
                | "mcp_get_prompt"
        );
        !keep_loaded
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let mut pool = self.pool.lock().await;
        let result = pool
            .call_tool(&self.name, input)
            .await
            .map_err(|e| ToolError::execution_failed(format!("MCP tool failed: {e}")))?;
        let content = serde_json::to_string(&result).unwrap_or_else(|_| result.to_string());
        Ok(ToolResult::success(content))
    }
}

// === 单元测试 ===

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use serde_json::{Value, json};
    use tempfile::tempdir;

    use crate::config::ToolOverride;
    use crate::tools::ToolRegistryBuilder;
    use crate::tools::spec::{
        ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, required_str,
    };

    use super::ToolRegistry;

    /// 用于单元测试的简单测试工具
    struct TestTool {
        name: String,
        description: String,
    }

    #[async_trait::async_trait]
    impl ToolSpec for TestTool {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            &self.description
        }

        fn input_schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"]
            })
        }

        fn capabilities(&self) -> Vec<ToolCapability> {
            vec![ToolCapability::ReadOnly]
        }

        async fn execute(
            &self,
            input: Value,
            _context: &ToolContext,
        ) -> Result<ToolResult, ToolError> {
            let message = required_str(&input, "message")?;
            Ok(ToolResult::success(format!("Echo: {message}")))
        }
    }

    fn make_test_tool(name: &str) -> Arc<TestTool> {
        Arc::new(TestTool {
            name: name.to_string(),
            description: "A test tool".to_string(),
        })
    }

    #[test]
    fn test_registry_register_and_get() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);

        let tool = make_test_tool("test_tool");
        registry.register(tool);

        assert!(registry.contains("test_tool"));
        assert!(!registry.contains("nonexistent"));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn resolve_exact_match_is_ascii_case_insensitive() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);

        registry.register(make_test_tool("read_file"));

        assert_eq!(registry.resolve("READ_FILE"), Some("read_file"));
    }

    #[test]
    fn todo_aliases_stay_callable_but_hidden_from_model_catalog() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let registry = ToolRegistryBuilder::new()
            .with_todo_tool(crate::tools::todo::new_shared_todo_list())
            .build(ctx);

        // 规范拼写 + 旧版拼写仍可按名称调用以支持重放。
        for name in [
            "work_update",
            "checklist_write",
            "checklist_add",
            "checklist_update",
            "checklist_list",
            "todo_write",
            "todo_add",
            "todo_update",
            "todo_list",
        ] {
            assert!(registry.contains(name), "{name} should remain callable");
        }

        let api_names = registry
            .to_api_tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();

        assert!(
            api_names.iter().any(|name| name == "work_update"),
            "work_update should be the sole model-visible progress surface"
        );
        for hidden in [
            "checklist_write",
            "checklist_add",
            "checklist_update",
            "checklist_list",
            "todo_write",
            "todo_add",
            "todo_update",
            "todo_list",
        ] {
            assert!(
                api_names.iter().all(|name| name != hidden),
                "{hidden} should be hidden from the model catalog"
            );
        }
    }

    #[test]
    fn apply_overrides_removes_original_when_replacement_is_missing() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistryBuilder::new()
            .with_read_only_file_tools()
            .build(ctx);

        assert!(registry.contains("read_file"));
        assert!(registry.contains("list_dir"));

        let mut overrides = HashMap::new();
        overrides.insert(
            "read_file".to_string(),
            ToolOverride::Script {
                path: "missing-wrapper.sh".to_string(),
                args: None,
            },
        );

        registry.apply_overrides(&overrides, tmp.path());

        assert!(!registry.contains("read_file"));
        assert!(registry.contains("list_dir"));
    }

    #[test]
    fn builder_registers_speech_alias_tools() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let registry = ToolRegistryBuilder::new()
            .with_speech_tools(None, None)
            .build(ctx);

        assert!(registry.contains("speech"));
        assert!(registry.contains("tts"));
    }

    #[test]
    fn test_registry_names() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);

        registry.register(make_test_tool("tool_a"));
        registry.register(make_test_tool("tool_b"));

        let names = registry.names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"tool_a"));
        assert!(names.contains(&"tool_b"));
    }

    #[test]
    fn test_registry_to_api_tools() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);

        registry.register(make_test_tool("my_tool"));

        let api_tools = registry.to_api_tools();
        assert_eq!(api_tools.len(), 1);
        assert_eq!(api_tools[0].name, "my_tool");
        assert_eq!(api_tools[0].description, "A test tool");
    }

    #[test]
    fn api_tools_with_cache_marks_last_tool_ephemeral() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);

        registry.register(make_test_tool("tool_a"));
        registry.register(make_test_tool("tool_b"));

        let api_tools = registry.to_api_tools_with_cache(true);
        assert_eq!(api_tools.len(), 2);
        assert!(api_tools[0].cache_control.is_none());
        assert_eq!(
            api_tools[1]
                .cache_control
                .as_ref()
                .map(|c| c.cache_type.as_str()),
            Some("ephemeral")
        );
    }

    /// `description()` 按预构建字符串脚本逐个推进的工具，每次调用前进一个。
    /// 用于演示 api-tools 缓存在首次读取时固定描述字节，而不是每轮重新采样
    /// （#263 后续；镜像了 reference-cc 的 `getToolSchemaCache`）。
    struct VaryingDescriptionTool {
        name: String,
        descriptions: Vec<String>,
        next: std::sync::atomic::AtomicUsize,
    }

    impl VaryingDescriptionTool {
        fn new(name: &str, descriptions: &[&str]) -> Self {
            Self {
                name: name.to_string(),
                descriptions: descriptions.iter().map(|s| (*s).to_string()).collect(),
                next: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl ToolSpec for VaryingDescriptionTool {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            let idx = self
                .next
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                .min(self.descriptions.len() - 1);
            &self.descriptions[idx]
        }

        fn input_schema(&self) -> Value {
            json!({"type": "object", "properties": {}, "required": []})
        }

        fn capabilities(&self) -> Vec<ToolCapability> {
            vec![ToolCapability::ReadOnly]
        }

        async fn execute(
            &self,
            _input: Value,
            _context: &ToolContext,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::success("ok".to_string()))
        }
    }

    #[test]
    fn to_api_tools_pins_description_bytes_across_calls() {
        // 缓存稳定性后续的回归测试：一个在重连时返回不同 `description()` 的 MCP 适配器
        // （或任何描述不是 `&'static str` 的工具）否则会在会话期间重写目录字节
        // 并错过前缀缓存。注册表会固定首次调用的值，直到发生变更为止。
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);
        registry.register(Arc::new(VaryingDescriptionTool::new(
            "varying",
            &["first description", "second description"],
        )));

        let first = registry.to_api_tools();
        let second = registry.to_api_tools();

        assert_eq!(first.len(), 1);
        assert_eq!(first[0].description, "first description");
        assert_eq!(
            first, second,
            "api-tools catalog must be byte-identical across reads with no mutation in between"
        );
    }

    #[test]
    fn register_invalidates_api_tools_cache() {
        // 反向测试：当发生真正的变更时（新工具注册、现有工具被移除或调用 `clear`），
        // 缓存必须被丢弃，以便下一次读取反映实时的注册表状态。
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);
        registry.register(Arc::new(VaryingDescriptionTool::new(
            "varying",
            &["first description", "second description"],
        )));

        let before = registry.to_api_tools();
        assert_eq!(before.len(), 1);

        registry.register(make_test_tool("late_arrival"));

        let after = registry.to_api_tools();
        assert_eq!(after.len(), 2, "cache must rebuild after register");
        assert!(after.iter().any(|t| t.name == "varying"));
        assert!(after.iter().any(|t| t.name == "late_arrival"));
        // 变更工具的描述在缓存重建时会前进——上面的第一次读取采样了 `first description`；
        // 这次重建采样了 `second description`。要点仅仅是字节在真正变更后*可以*改变，
        // 而非它们总是会改变。
        let varying_after = after
            .iter()
            .find(|t| t.name == "varying")
            .expect("varying tool present");
        assert_eq!(varying_after.description, "second description");
    }

    #[test]
    fn remove_and_clear_invalidate_api_tools_cache() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);
        registry.register(make_test_tool("alpha"));
        registry.register(make_test_tool("beta"));

        let before = registry.to_api_tools();
        assert_eq!(before.len(), 2);

        let _ = registry.remove("alpha");
        let after_remove = registry.to_api_tools();
        assert_eq!(after_remove.len(), 1);
        assert_eq!(after_remove[0].name, "beta");

        registry.clear();
        let after_clear = registry.to_api_tools();
        assert!(after_clear.is_empty(), "cache must clear with the registry");
    }

    #[test]
    fn to_api_tools_emits_alphabetical_order_regardless_of_registration_order() {
        // #263 的回归测试：HashMap 迭代在进程启动间是非确定性的，
        // 这会使 DeepSeek 的 KV 前缀缓存在每次跨会话恢复时失效。
        // `to_api_tools` 必须按名称排序输出，而不受注册顺序影响，
        // 以便两次连续调用（以及两次不同的启动）产生字节相同的输出。
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let order_a = {
            let mut registry = ToolRegistry::new(ctx.clone());
            registry.register(make_test_tool("zebra"));
            registry.register(make_test_tool("alpha"));
            registry.register(make_test_tool("mango"));
            registry
                .to_api_tools()
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>()
        };

        let order_b = {
            let mut registry = ToolRegistry::new(ctx.clone());
            registry.register(make_test_tool("alpha"));
            registry.register(make_test_tool("mango"));
            registry.register(make_test_tool("zebra"));
            registry
                .to_api_tools()
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>()
        };

        assert_eq!(order_a, vec!["alpha", "mango", "zebra"]);
        assert_eq!(order_a, order_b);
    }

    #[test]
    fn test_registry_remove() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);

        registry.register(make_test_tool("removable"));
        assert!(registry.contains("removable"));

        let _ = registry.remove("removable");
        assert!(!registry.contains("removable"));
    }

    #[test]
    fn test_registry_clear() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);

        registry.register(make_test_tool("tool1"));
        registry.register(make_test_tool("tool2"));
        assert_eq!(registry.len(), 2);

        registry.clear();
        assert!(registry.is_empty());
    }

    #[tokio::test]
    async fn test_registry_execute() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);

        registry.register(make_test_tool("echo"));

        let result = registry
            .execute("echo", json!({"message": "hello"}))
            .await
            .expect("execute");

        assert_eq!(result, "Echo: hello");
    }

    #[tokio::test]
    async fn test_registry_execute_unknown_tool() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let registry = ToolRegistry::new(ctx);

        let result = registry.execute("nonexistent", json!({})).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_builder_basic() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let registry = ToolRegistryBuilder::new()
            .with_tool(make_test_tool("custom"))
            .build(ctx);

        assert!(registry.contains("custom"));
    }

    #[test]
    fn test_filter_by_capability() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);

        registry.register(make_test_tool("readonly_tool"));

        let readonly = registry.filter_by_capability(ToolCapability::ReadOnly);
        assert_eq!(readonly.len(), 1);

        let writes = registry.filter_by_capability(ToolCapability::WritesFiles);
        assert_eq!(writes.len(), 0);
    }

    #[test]
    fn test_read_only_tools() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let mut registry = ToolRegistry::new(ctx);

        registry.register(make_test_tool("reader"));

        let readonly = registry.read_only_tools();
        assert_eq!(readonly.len(), 1);
        assert_eq!(readonly[0].name(), "reader");
    }

    #[test]
    fn test_builder_with_web_tools_no_longer_includes_finance() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let registry = ToolRegistryBuilder::new().with_web_tools().build(ctx);

        // finance 已在 v0.8.49 中移至 with_finance_tool()；
        // with_web_tools() 注册网络搜索/获取以及本地开发服务器就绪检查。
        assert!(registry.contains("web_search"));
        assert!(registry.contains("fetch_url"));
        assert!(registry.contains("wait_for_dev_server"));
        assert!(registry.contains("web.run"));
        assert!(!registry.contains("finance"));
    }

    #[test]
    fn test_builder_with_finance_tool() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let registry = ToolRegistryBuilder::new().with_finance_tool().build(ctx);

        assert!(registry.contains("finance"));
    }

    #[test]
    fn test_builder_with_agent_tools_includes_finance() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let registry = ToolRegistryBuilder::new()
            .with_agent_tools(false)
            .build(ctx);

        assert!(registry.contains("finance"));
    }

    #[test]
    fn agent_tools_with_allow_shell_false_excludes_shell_tools() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let registry = ToolRegistryBuilder::new()
            .with_agent_tools(false)
            .build(ctx);

        assert!(
            !registry.contains("exec_shell"),
            "exec_shell should be excluded when allow_shell is false"
        );
        assert!(
            !registry.contains("task_shell_start"),
            "task_shell_start should be excluded when allow_shell is false"
        );
        assert!(
            !registry.contains("task_shell_wait"),
            "task_shell_wait should be excluded when allow_shell is false"
        );
    }

    #[test]
    fn agent_tools_with_shell_policy_readonly_includes_shell_tools() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let registry = ToolRegistryBuilder::new()
            .with_agent_tools_policy(crate::worker_profile::ShellPolicy::ReadOnly)
            .build(ctx);

        assert!(
            registry.contains("exec_shell"),
            "read-only shell policy should expose shell tools; execution enforces mutating-command denial"
        );
        assert!(registry.contains("task_shell_start"));
        assert!(registry.contains("task_shell_wait"));
    }

    #[test]
    fn agent_tools_with_allow_shell_true_includes_shell_tools() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let registry = ToolRegistryBuilder::new().with_agent_tools(true).build(ctx);

        assert!(
            registry.contains("exec_shell"),
            "exec_shell should be included when allow_shell is true"
        );
        assert!(
            registry.contains("task_shell_start"),
            "task_shell_start should be included when allow_shell is true"
        );
        assert!(
            registry.contains("task_shell_wait"),
            "task_shell_wait should be included when allow_shell is true"
        );
    }

    /// #2683 — `exec_wait` 和 `exec_interact` 是 `exec_shell_wait` 和 `exec_shell_interact`
    /// 的旧版别名。它们必须保持可调用（用于保存的对话记录重放），但对模型可见目录隐藏。
    #[test]
    fn shell_alias_tools_hidden_from_model_catalog() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let registry = ToolRegistryBuilder::new().with_shell_tools().build(ctx);

        // 旧版别名保持可调用。
        for alias in ["exec_wait", "exec_interact"] {
            assert!(registry.contains(alias), "{alias} should remain callable");
        }

        let api_names: Vec<String> = registry
            .to_api_tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect();

        // 规范名称对模型可见。
        for canonical in ["exec_shell_wait", "exec_shell_interact"] {
            assert!(
                api_names.iter().any(|n| n == canonical),
                "{canonical} should be model-visible"
            );
        }

        // 旧版别名被隐藏。
        for alias in ["exec_wait", "exec_interact"] {
            assert!(
                api_names.iter().all(|n| n != alias),
                "{alias} should be hidden from the model catalog"
            );
        }
    }
}
