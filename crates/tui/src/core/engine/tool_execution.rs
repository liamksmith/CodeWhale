//! 引擎回合循环的低级工具执行辅助函数。
//!
//! 此模块将 MCP 分发、执行锁和并行工具扇出的机制
//! 从 `engine.rs` 中分离出来；回合循环仍拥有规划、
//! 审批以及工具结果如何写回会话状态的职责。

use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use super::*;

/// RAII 守卫，在交互式工具执行期间暂停 TUI 的终端状态所有权，
/// 然后在 drop 时恢复。
///
/// 背景：交互式工具（任何需要原始 TTY 的工具——外部
/// 编辑器、带标准输入的 `exec_shell` 等）需要 TUI 退出备选屏幕、
/// 禁用原始模式并释放鼠标捕获，以便子进程看到普通
/// 终端。TUI 监听 `Event::PauseEvents` / `Event::ResumeEvents`
/// 并相应地运行 `pause_terminal` / `resume_terminal`。
///
/// 早期代码在工具执行前发送 `PauseEvents`，在工具执行后发送
/// `ResumeEvents`。这在正常路径下有效，但如果工具的未来被丢弃
/// ——Ctrl+C 取消、子代理中止、工具等待时父任务被取消——
/// 第二个 `await` 永远不会到达，`ResumeEvents` 也永远不会发出。
/// 它还允许交互式子进程在 UI 实际离开备选屏幕/原始模式之前启动。
/// 这两种失败都会导致 TUI 陷入常规 shell 回滚：父 shell 滚动条接管，
/// 鼠标滚轮滚动主机终端而非转录，并且 TUI 在熟模式输出的底部渲染。
///
/// `Drop` 是同步执行的，不能使用 await，因此我们首先在事件通道的
/// **克隆**上使用 `try_send` 以非阻塞方式推送 `ResumeEvents`。如果
/// 通道已满，我们将恢复事件排队到活跃的 Tokio 运行时上，而不是
/// 丢弃它；否则引擎事件突发可能导致 UI 停留在暂停的终端状态。
pub(super) struct InteractiveTerminalGuard {
    tx: Option<mpsc::Sender<Event>>,
}

impl InteractiveTerminalGuard {
    /// 发送 `PauseEvents` 并激活守卫。如果 `interactive` 为 false，
    /// 守卫为空操作——`Drop` 会跳过恢复。
    pub(super) async fn engage(tx: mpsc::Sender<Event>, interactive: bool) -> Self {
        if !interactive {
            return Self { tx: None };
        }
        // 尽力而为：如果接收端已消失，TUI 已关闭
        // 且无需恢复。如果事件已投递，则等待 UI 实际
        // 释放终端后再启动子进程。
        let ack = Arc::new(tokio::sync::Notify::new());
        match tx
            .send(Event::PauseEvents {
                ack: Some(ack.clone()),
            })
            .await
        {
            Ok(()) => {
                if tokio::time::timeout(Duration::from_millis(750), ack.notified())
                    .await
                    .is_err()
                {
                    tracing::warn!(
                        target: "engine.tool_execution",
                        "InteractiveTerminalGuard: 等待终端暂停确认超时；\
                         继续执行交互式工具"
                    );
                }
            }
            Err(err) => {
                tracing::debug!(
                    target: "engine.tool_execution",
                    ?err,
                    "InteractiveTerminalGuard: PauseEvents 前事件通道已关闭"
                );
            }
        }
        Self { tx: Some(tx) }
    }
}

impl Drop for InteractiveTerminalGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            match tx.try_send(Event::ResumeEvents) {
                Ok(()) => {}
                Err(tokio::sync::mpsc::error::TrySendError::Full(event)) => {
                    match tokio::runtime::Handle::try_current() {
                        Ok(handle) => {
                            handle.spawn(async move {
                                if let Err(err) = tx.send(event).await {
                                    tracing::warn!(
                                        target: "engine.tool_execution",
                                        ?err,
                                        "InteractiveTerminalGuard: 异步 send(ResumeEvents) 失败；\
                                         终端可能保持在暂停状态直到下一个暂停/恢复周期"
                                    );
                                }
                            });
                        }
                        Err(err) => {
                            tracing::warn!(
                                target: "engine.tool_execution",
                                ?err,
                                "InteractiveTerminalGuard: 事件通道已满且无可用 Tokio 运行时 \
                                 来排队 ResumeEvents；终端可能保持在暂停状态直到 \
                                 下一个暂停/恢复周期"
                            );
                        }
                    }
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    tracing::debug!(
                        target: "engine.tool_execution",
                        "InteractiveTerminalGuard: ResumeEvents 前事件通道已关闭"
                    );
                }
            }
        }
    }
}

pub(super) fn emit_tool_audit(event: serde_json::Value) {
    let Some(path) = std::env::var_os("DEEPSEEK_TOOL_AUDIT_LOG") else {
        return;
    };
    emit_tool_audit_to_path(&PathBuf::from(path), event);
}

fn emit_tool_audit_to_path(path: &Path, event: serde_json::Value) {
    let line = match serde_json::to_string(&event) {
        Ok(line) => line,
        Err(e) => {
            tracing::error!("序列化工具审计事件失败: {e}");
            return;
        }
    };
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::error!(
            "创建审计日志目录 {} 失败: {e}",
            parent.display()
        );
        return;
    }
    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(mut file) => {
            if let Err(e) = writeln!(file, "{line}") {
                tracing::error!("写入审计日志 {} 失败: {e}", path.display());
            }
        }
        Err(e) => {
            tracing::error!("打开审计日志 {} 失败: {e}", path.display());
        }
    }
}

impl Engine {
    pub(super) async fn execute_mcp_tool_with_pool(
        pool: Arc<AsyncMutex<McpPool>>,
        name: &str,
        input: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let mut pool = pool.lock().await;
        let result = pool
            .call_tool(name, input)
            .await
            .map_err(|e| ToolError::execution_failed(format!("MCP 工具失败: {e}")))?;
        let content = serde_json::to_string(&result).unwrap_or_else(|_| result.to_string());
        Ok(ToolResult::success(content))
    }

    pub(super) async fn execute_parallel_tool(
        &mut self,
        input: serde_json::Value,
        tool_registry: Option<&crate::tools::ToolRegistry>,
        tool_exec_lock: Arc<RwLock<()>>,
    ) -> Result<ToolResult, ToolError> {
        let calls = parse_parallel_tool_calls(&input)?;
        let mcp_pool = if calls.iter().any(|(tool, _)| McpPool::is_mcp_tool(tool)) {
            Some(self.ensure_mcp_pool().await?)
        } else {
            None
        };
        let Some(registry) = tool_registry else {
            return Err(ToolError::not_available(
                "multi_tool_use.parallel 的工具注册表不可用",
            ));
        };

        let result_count = calls.len();
        let mut tasks = FuturesUnordered::new();
        let shell_permits = Arc::new(tokio::sync::Semaphore::new(MAX_PARALLEL_SHELL_EXEC));
        for (index, (tool_name, tool_input)) in calls.into_iter().enumerate() {
            if tool_name == MULTI_TOOL_PARALLEL_NAME {
                return Err(ToolError::invalid_input(
                    "multi_tool_use.parallel 不能调用自身",
                ));
            }
            if McpPool::is_mcp_tool(&tool_name) {
                if !mcp_tool_is_parallel_safe(&tool_name) {
                    return Err(ToolError::invalid_input(format!(
                        "工具 '{tool_name}' 是 MCP 工具，不能并行运行。\
                         允许的 MCP 工具：list_mcp_resources, list_mcp_resource_templates, \
                         mcp_read_resource, read_mcp_resource, mcp_get_prompt。"
                    )));
                }
            } else {
                let Some(spec) = registry.get(&tool_name) else {
                    return Err(ToolError::not_available(format!(
                        "工具 '{tool_name}' 未注册"
                    )));
                };
                if !spec.is_read_only_for(&tool_input) {
                    return Err(ToolError::invalid_input(format!(
                        "工具 '{tool_name}' 不是只读的，不能并行运行"
                    )));
                }
                if spec.approval_requirement_for(&tool_input) != ApprovalRequirement::Auto {
                    return Err(ToolError::invalid_input(format!(
                        "工具 '{tool_name}' 需要审批，不能并行运行"
                    )));
                }
                if !spec.supports_parallel_for(&tool_input) {
                    return Err(ToolError::invalid_input(format!(
                        "工具 '{tool_name}' 不支持并行执行"
                    )));
                }
            }

            let registry_ref = registry;
            let lock = tool_exec_lock.clone();
            let tx_event = self.tx_event.clone();
            let mcp_pool = mcp_pool.clone();
            let shell_permits = shell_permits.clone();
            let workspace = self.session.workspace.clone();
            tasks.push(async move {
                let _shell_permit = if tool_name == "exec_shell" {
                    shell_permits.acquire_owned().await.ok()
                } else {
                    None
                };
                let result = Engine::execute_tool_with_lock(
                    lock,
                    true,
                    false,
                    tx_event,
                    tool_name.clone(),
                    tool_input.clone(),
                    workspace,
                    Some(registry_ref),
                    mcp_pool,
                    None,
                )
                .await;
                (index, tool_name, result)
            });
        }

        let mut results: Vec<Option<ParallelToolResultEntry>> = Vec::with_capacity(result_count);
        results.resize_with(result_count, || None);
        while let Some((index, tool_name, result)) = tasks.next().await {
            let entry = match result {
                Ok(output) => {
                    let mut error = None;
                    if !output.success {
                        error = Some(output.content.clone());
                    }
                    ParallelToolResultEntry {
                        tool_name,
                        success: output.success,
                        content: output.content,
                        error,
                    }
                }
                Err(err) => {
                    let message = format!("{err}");
                    ParallelToolResultEntry {
                        tool_name,
                        success: false,
                        content: format!("错误: {message}"),
                        error: Some(message),
                    }
                }
            };
            results[index] = Some(entry);
        }
        let results = results.into_iter().flatten().collect();

        ToolResult::json(&ParallelToolResult { results })
            .map_err(|e| ToolError::execution_failed(e.to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn execute_tool_with_lock(
        lock: Arc<RwLock<()>>,
        supports_parallel: bool,
        interactive: bool,
        tx_event: mpsc::Sender<Event>,
        tool_name: String,
        tool_input: serde_json::Value,
        workspace: PathBuf,
        registry: Option<&crate::tools::ToolRegistry>,
        mcp_pool: Option<Arc<AsyncMutex<McpPool>>>,
        context_override: Option<crate::tools::ToolContext>,
    ) -> Result<ToolResult, ToolError> {
        let started_at = std::time::Instant::now();
        let dispatch = if McpPool::is_mcp_tool(&tool_name) {
            "mcp"
        } else if matches!(
            tool_name.as_str(),
            CODE_EXECUTION_TOOL_NAME | JS_EXECUTION_TOOL_NAME
        ) {
            "interpreter"
        } else if registry.is_some() {
            "registry"
        } else {
            "missing"
        };
        let input_bytes = serde_json::to_string(&tool_input)
            .map(|s| s.len())
            .unwrap_or(0);
        tracing::debug!(
            target: "engine.tool_execution",
            tool = %tool_name,
            dispatch,
            interactive,
            supports_parallel,
            input_bytes,
            "tool.exec.start",
        );

        let _guard = if supports_parallel {
            ToolExecGuard::Read(lock.read().await)
        } else {
            ToolExecGuard::Write(lock.write().await)
        };

        // RAII 暂停/恢复：确保 `Event::ResumeEvents` 总是在
        // drop 时触发，即使工具未来在等待中被取消。参见
        // `InteractiveTerminalGuard` 的文档注释了解此修复关闭的回归缺陷
        //（取消交互式工具后父终端回滚劫持 TUI）。
        let _terminal = InteractiveTerminalGuard::engage(tx_event, interactive).await;

        let outcome = if McpPool::is_mcp_tool(&tool_name) {
            if let Some(pool) = mcp_pool {
                Engine::execute_mcp_tool_with_pool(pool, &tool_name, tool_input).await
            } else {
                Err(ToolError::not_available(format!(
                    "工具 '{tool_name}' 未注册"
                )))
            }
        } else if tool_name == CODE_EXECUTION_TOOL_NAME {
            execute_code_execution_tool(&tool_input, &workspace).await
        } else if tool_name == JS_EXECUTION_TOOL_NAME {
            execute_js_execution_tool(&tool_input, &workspace).await
        } else if let Some(registry) = registry {
            registry
                .execute_full_with_context(&tool_name, tool_input, context_override.as_ref())
                .await
        } else {
            Err(ToolError::not_available(format!(
                "工具 '{tool_name}' 未注册"
            )))
        };

        let duration_ms = started_at.elapsed().as_millis() as u64;
        match &outcome {
            Ok(result) => {
                tracing::debug!(
                    target: "engine.tool_execution",
                    tool = %tool_name,
                    dispatch,
                    duration_ms,
                    success = result.success,
                    output_bytes = result.content.len(),
                    "tool.exec.end",
                );
            }
            Err(err) => {
                let kind = match err {
                    ToolError::InvalidInput { .. } => "invalid_input",
                    ToolError::MissingField { .. } => "missing_field",
                    ToolError::PathEscape { .. } => "path_escape",
                    ToolError::ExecutionFailed { .. } => "execution_failed",
                    ToolError::Timeout { .. } => "timeout",
                    ToolError::NotAvailable { .. } => "not_available",
                    ToolError::PermissionDenied { .. } => "permission_denied",
                };
                tracing::warn!(
                    target: "engine.tool_execution",
                    tool = %tool_name,
                    dispatch,
                    duration_ms,
                    error_kind = kind,
                    error = %err,
                    "tool.exec.end",
                );
            }
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    #[tokio::test]
    async fn terminal_guard_queues_resume_when_event_channel_is_full() {
        let (tx, mut rx) = mpsc::channel(1);
        tx.try_send(Event::status("filler")).expect("填充通道");

        drop(InteractiveTerminalGuard { tx: Some(tx) });

        assert!(matches!(rx.recv().await, Some(Event::Status { .. })));
        let resumed = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("已排队的恢复事件")
            .expect("事件通道仍打开");
        assert!(matches!(resumed, Event::ResumeEvents));
    }

    #[tokio::test]
    async fn terminal_guard_waits_for_pause_ack_before_returning() {
        let (tx, mut rx) = mpsc::channel(4);
        let task = tokio::spawn(InteractiveTerminalGuard::engage(tx, true));

        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("暂停事件")
            .expect("事件通道仍打开");
        let ack = match event {
            Event::PauseEvents { ack: Some(ack) } => ack,
            other => panic!("预期带确认的 PauseEvents，实际得到 {other:?}"),
        };

        tokio::task::yield_now().await;
        assert!(!task.is_finished(), "守卫在暂停确认前返回");

        ack.notify_one();
        let guard = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("确认后守卫返回")
            .expect("守卫任务已加入");

        drop(guard);
        let resumed = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("恢复事件")
            .expect("事件通道仍打开");
        assert!(matches!(resumed, Event::ResumeEvents));
    }

    #[test]
    fn emit_tool_audit_to_path_writes_jsonl_lines() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("audit.log");
        let marker = path.display().to_string();

        emit_tool_audit_to_path(
            &path,
            json!({
                "event": "tool.spillover",
                "test_marker": marker,
                "tool_id": "call-abc",
                "tool_name": "exec_shell",
                "path": "/tmp/foo.txt",
            }),
        );
        emit_tool_audit_to_path(
            &path,
            json!({
                "event": "tool.result",
                "test_marker": marker,
                "tool_id": "call-xyz",
                "success": true,
            }),
        );

        let body = std::fs::read_to_string(&path).expect("审计日志已写入");
        let entries: Vec<serde_json::Value> = body
            .lines()
            .map(|line| serde_json::from_str(line).expect("审计行是 JSON"))
            .filter(|entry: &serde_json::Value| {
                entry.get("test_marker").and_then(|v| v.as_str()) == Some(marker.as_str())
            })
            .collect();
        assert_eq!(entries.len(), 2, "两次标记的发出 -> 两行");

        // 每行往返为 JSON，具有预期的事件键。
        let first = &entries[0];
        assert_eq!(
            first.get("event").and_then(|v| v.as_str()),
            Some("tool.spillover")
        );
        assert_eq!(
            first.get("tool_id").and_then(|v| v.as_str()),
            Some("call-abc")
        );

        let second = &entries[1];
        assert_eq!(
            second.get("event").and_then(|v| v.as_str()),
            Some("tool.result")
        );
    }

    #[test]
    fn emit_tool_audit_creates_parent_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // 父目录尚不存在的路径——写入器应创建它。
        let nested = tmp.path().join("nested").join("dir").join("audit.log");
        emit_tool_audit_to_path(&nested, json!({"event": "test"}));
        assert!(nested.exists(), "写入器应为父链创建 mkdir -p");
    }
}
