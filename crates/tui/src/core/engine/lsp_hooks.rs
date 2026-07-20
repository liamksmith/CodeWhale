//! 引擎工具执行后的 LSP 诊断钩子。
//!
//! 回合循环只需询问"成功的编辑是否产生了诊断？"
//! 本模块负责工具输入路径提取和合成诊断消息的注入，
//! 使顶层引擎模块专注于会话编排。

use std::path::PathBuf;

use crate::tools::apply_patch::preflight_apply_patch;

use super::*;

/// #136: 从工具调用中推导被编辑的文件路径。对不修改文件的工具返回空向量。
/// 我们有意只处理三种已知的编辑工具——添加更多工具（例如专门的重构工具）
/// 只需在此处修改一行。
pub(super) fn edited_paths_for_tool(tool_name: &str, input: &serde_json::Value) -> Vec<PathBuf> {
    match tool_name {
        "edit_file" | "write_file" => {
            if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
                vec![PathBuf::from(path)]
            } else {
                Vec::new()
            }
        }
        "apply_patch" => preflight_apply_patch(input)
            .map(|preflight| {
                preflight
                    .touched_files
                    .into_iter()
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

impl Engine {
    /// #136: 编辑后钩子。检查工具名称 + 输入，推导出编辑的文件路径，
    /// 并向 LSP 管理器请求诊断信息。渲染后的块被放入 `pending_lsp_blocks` 队列，
    /// 在下一个 API 请求前刷新到会话消息流中。失败时静默处理——
    /// LSP 服务器缺失或崩溃绝不能阻塞代理。
    pub(super) async fn run_post_edit_lsp_hook(
        &mut self,
        tool_name: &str,
        tool_input: &serde_json::Value,
    ) {
        if !self.lsp_manager.config().enabled {
            return;
        }
        let paths = edited_paths_for_tool(tool_name, tool_input);
        let mut found = 0usize;
        let mut files = 0usize;
        for path in paths {
            let absolute = if path.is_absolute() {
                path.clone()
            } else {
                self.session.workspace.join(&path)
            };
            // 基于现有回合计数器使用简短的编辑序列号，
            // 以便日志输出保持关联，即使我们当前不按序列批量处理。
            let seq = self.turn_counter;
            if let Some(block) = self.lsp_manager.diagnostics_for(&absolute, seq).await {
                found = found.saturating_add(block.items.len());
                files = files.saturating_add(1);
                self.pending_lsp_blocks.push(block);
            }
        }
        if found > 0 {
            let _ = self
                .tx_event
                .send(Event::LspRepairUpdate {
                    diagnostics_found: found,
                    files,
                    injected: false,
                })
                .await;
        }
    }

    /// 将 `pending_lsp_blocks` 排空为一条合成用户消息，
    /// 以便模型在其下一个请求时看到诊断信息。当没有待处理块时跳过。
    /// 消息使用标准的 `text` 内容块形状（与工具后引导消息相同的形状），
    /// 因此我们无需发明新的信封格式。
    pub(super) async fn flush_pending_lsp_diagnostics(&mut self) {
        if self.pending_lsp_blocks.is_empty() {
            return;
        }
        let blocks = std::mem::take(&mut self.pending_lsp_blocks);
        let found: usize = blocks.iter().map(|b| b.items.len()).sum();
        let files = blocks.len();
        let rendered = crate::lsp::render_blocks(&blocks);
        if rendered.is_empty() {
            return;
        }
        self.add_session_message(self.runtime_text_message_with_turn_metadata(
            rendered,
            crate::core::ops::UserInputProvenance::Runtime,
        ))
        .await;
        let _ = self
            .tx_event
            .send(Event::LspRepairUpdate {
                diagnostics_found: found,
                files,
                injected: true,
            })
            .await;
    }
}
