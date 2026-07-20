//! 每回合的工具注册表设置。
//!
//! 这样可以将模式/功能特定的注册表构建逻辑从发送路径中分离出来。

use super::*;
use crate::core::authority::shell_policy_for_mode;
use crate::tools::AgentToolSurfaceOptions;
use crate::worker_profile::ShellPolicy;

fn should_register_remember_tool(memory_enabled: bool, moraine_fallback: bool) -> bool {
    memory_enabled && !moraine_fallback
}

impl Engine {
    pub(super) fn agent_tool_surface_options(
        &self,
        shell_policy: ShellPolicy,
    ) -> AgentToolSurfaceOptions {
        let mut options = AgentToolSurfaceOptions::new(shell_policy);
        options.apply_patch_enabled = self.config.features.enabled(Feature::ApplyPatch);
        options.web_search_enabled = self.config.features.enabled(Feature::WebSearch);
        options.memory_tool_enabled =
            should_register_remember_tool(self.config.memory_enabled, self.config.moraine_fallback);
        options.vision_config = if self.config.features.enabled(Feature::VisionModel) {
            self.config.vision_config.clone()
        } else {
            None
        };
        options.speech_output_dir = self.config.speech_output_dir.clone();
        options.goal_state = Some(self.config.goal_state.clone());
        options
    }

    pub(super) fn build_turn_tool_registry_builder(
        &self,
        mode: AppMode,
        todo_list: SharedTodoList,
        plan_state: SharedPlanState,
    ) -> ToolRegistryBuilder {
        let shell_policy = shell_policy_for_mode(mode, self.session.allow_shell);
        if mode != AppMode::Plan {
            return ToolRegistryBuilder::new().with_agent_runtime_surface(
                self.deepseek_client.clone(),
                self.session.model.clone(),
                self.agent_tool_surface_options(shell_policy),
                todo_list,
                plan_state,
            );
        }

        let mut builder = {
            let builder = ToolRegistryBuilder::new()
                .with_read_only_file_tools()
                .with_search_tools()
                .with_git_tools()
                .with_git_history_tools()
                .with_diagnostics_tool()
                .with_skill_tools()
                .with_validation_tools()
                .with_handle_tools()
                .with_runtime_read_only_task_tools()
                .with_todo_tool(todo_list)
                .with_plan_tool(plan_state)
                .with_goal_tools(self.config.goal_state.clone());
            if shell_policy.allows_shell() {
                builder.with_shell_tools().with_runtime_task_shell_tools()
            } else {
                builder
            }
        };

        builder = builder
            .with_review_tool(self.deepseek_client.clone(), self.session.model.clone())
            .with_user_input_tool()
            .with_parallel_tool();

        // SlopLedger: 计划模式仅获得只读查询和导出工具。
        builder = builder.with_slop_ledger_read_only_tools();
        if self.config.features.enabled(Feature::WebSearch) {
            builder = builder.with_web_tools();
        }

        // 仅在用户已选择启用用户记忆时才注册 `remember` 工具 (#489)。
        // 没有该选项，该工具总是会失败；将其暴露出来只会浪费目录槽位。
        // TODO(v0.8.71): 当 Moraine 召回稳定时移除；参见 #3490, #3495
        if should_register_remember_tool(self.config.memory_enabled, self.config.moraine_fallback) {
            builder = builder.with_remember_tool();
        }

        // 当配置了 vision_model 且功能启用时注册 image_analyze 工具。
        if self.config.features.enabled(Feature::VisionModel)
            && let Some(ref vision_config) = self.config.vision_config
        {
            builder = builder.with_vision_tools(vision_config.clone());
        }

        // 无条件注册 `notify` 工具 (#1322)。它除了写入一次终端转义序列外没有其他副作用，
        // 并且会遵循用户的 `[notifications].method` 配置（包括 `off`），
        // 因此没有值得加门控的失败模式。
        builder = builder.with_notify_tool();

        // 注册 start_mcp_server 工具，以便 LLM 可以从对话上下文中动态启动
        // MCP 服务器。仅在池已初始化时（通过 ensure_mcp_pool 惰性初始化）。
        if let Some(ref pool) = self.mcp_pool {
            builder = builder.with_runtime_mcp_tool(Arc::clone(pool));
        }

        builder
    }
}

#[cfg(test)]
mod tests {
    use super::should_register_remember_tool;

    #[test]
    fn remember_tool_registration_respects_moraine_fallback() {
        assert!(should_register_remember_tool(true, false));
        assert!(!should_register_remember_tool(false, false));
        assert!(!should_register_remember_tool(true, true));
    }
}
