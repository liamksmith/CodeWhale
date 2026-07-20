//! 工作线程运行时配置文件 —— CodeWhale 工作线程的每个角色能力契约
//!（#3217, #3211, #3213，以及子权限交集问题
//! #414 / #426 / #1186）。
//!
//! 这是 **工作流基础**：每个分离的工作线程——无论是作为
//! `agent` 子代理还是 Fleet 工作线程启动——都应在配置文件下运行，
//! 该配置文件限定其可以执行的操作（权限、shell 访问、工具范围、模型
//! 路由、递归预算、前台/后台）。子配置文件始终
//! 从父配置文件 **派生**，并且永远不能超越它。
//!
//! 范围：此模块定义契约和父→子派生，并附带
//! 测试。`agent` 和 Fleet 工作线程记录现在构建并持久化这些
//! 配置文件，以便父可见的工作线程投影具有单一能力
//! 契约。每个声明字段的运行时强制执行仍是增量
//! 后续工作（#3217）。

#![allow(dead_code)] // 基础：消费者在后续工作中接入（#3217）。

use crate::tools::subagent::SubAgentType;
use serde::{Deserialize, Serialize};

/// 工作线程可以执行的粗略能力类别，超出读取访问（读取
/// 始终允许）。子线程只能持有父线程能力的 *子集*。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionSet {
    /// 可以修改工作区（`write_file` / `edit_file` / `apply_patch`）。
    pub write: bool,
    /// 可以使用网络工具（web search/fetch、联网的 MCP 服务器）。
    pub network: bool,
}

impl PermissionSet {
    /// 完整能力（写入 + 网络）。
    pub const fn full() -> Self {
        Self {
            write: true,
            network: true,
        }
    }

    /// 只读：无写入，无网络。
    pub const fn read_only() -> Self {
        Self {
            write: false,
            network: false,
        }
    }

    /// 交集：只有当 **两个** 集合都授予时才授予能力。
    /// 这是核心的非升级原语 —— `parent.intersect(child)`
    /// 永远不能产生父线程缺乏的能力。
    #[must_use]
    pub fn intersect(self, other: Self) -> Self {
        Self {
            write: self.write && other.write,
            network: self.network && other.network,
        }
    }
}

/// Shell 访问策略 —— 替代旧的每工作线程 shell 布尔值
///（#3217）。从最严格到最宽松排序，以便 `min` 产生两者中更安全的策略。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ShellPolicy {
    /// 无 shell 访问。
    None,
    /// 仅只读/非变异命令（策略强制存在于
    /// exec/sandbox 层；这是声明的意图）。
    ReadOnly,
    /// 完全 shell 访问。
    Full,
}

impl ShellPolicy {
    /// 将旧式顶层 shell opt-in 转换为类型化的 shell 策略。
    #[must_use]
    pub const fn from_legacy_allow_shell(allow_shell: bool) -> Self {
        if allow_shell { Self::Full } else { Self::None }
    }

    /// 在此策略下是否应暴露任何 shell 工具。
    #[must_use]
    pub const fn allows_shell(self) -> bool {
        !matches!(self, Self::None)
    }

    /// 两个策略中更严格（更安全）的一个。子线程永远不能超过
    /// 其父线程的 shell 策略。
    #[must_use]
    pub fn min_with(self, other: Self) -> Self {
        if self <= other { self } else { other }
    }
}

/// 工作线程可以调用哪些工具。镜像现有的 `AgentWorkerToolProfile`
///（`Inherited` / `Explicit`），以便在接入时可以协调两者。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolScope {
    /// 继承父线程的工具表面。
    Inherit,
    /// 仅明确列出的工具名称。
    Explicit(Vec<String>),
}

/// 如何选择工作线程的模型。新的面向模型的生成默认为
/// 父线程/会话模型；仅在父线程明确要求该路由时，
/// 子线程才使用更小/更快的同族兄弟。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelRoute {
    /// 与父线程/会话相同的模型。
    Inherit,
    /// 当已知时，明确请求更小/更快的同族兄弟。
    Faster,
    /// 来自旧隐藏自动路由器的旧持久化路由。新生成不会
    /// 产生此值；运行时为了兼容性将其视为 `Faster`。
    Auto,
    /// 明确的模型 ID，在生成时针对活跃提供商进行验证。
    Fixed(String),
}

/// 单个工作线程运行所依据的能力契约。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerRuntimeProfile {
    pub role: SubAgentType,
    pub permissions: PermissionSet,
    pub shell: ShellPolicy,
    pub tools: ToolScope,
    pub model: ModelRoute,
    /// 明确的提供商覆盖；`None` 继承父线程/会话提供商。
    pub provider: Option<String>,
    /// 明确的推理/思考层级；`None` 继承父线程/会话层级。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// 从父会话的 `--disallowed-tools` 继承的工具拒绝列表
    ///（#4042）。拒绝始终优先于允许，即使在明确的允许列表
    /// 和角色姿态之上。条目支持通配符匹配：精确名称
    ///（`exec_shell`）或 `prefix*` 通配符（`mcp_*`），不区分大小写比较。
    ///
    /// 子线程只能 *添加* 条目 —— `derive_child()` 取
    /// 父线程和子线程拒绝列表的并集，因此后代永远不能删除
    /// 祖先施加的限制。唯一不以父线程列表开始的方式是在生成时
    /// 显式设置 `inherit_disallowed_tools: false`，
    /// 这会在注册表读取之前清除克隆运行时的列表。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denied_tools: Vec<String>,
    /// 剩余嵌套委托预算。工作线程可以在 `max_spawn_depth > 0` 时
    /// 生成子线程；每层递减一次。受工作空间上限限制。
    pub max_spawn_depth: u32,
    /// 工作线程是分离（后台）还是内联（前台）运行。
    pub background: bool,
}

impl WorkerRuntimeProfile {
    /// 角色的默认配置文件 —— 每个角色的姿态。镜像
    /// `docs/SUBAGENTS.md` 中记录的角色立场（explore/plan/review 是
    /// 只读的；verifier 运行测试；implementer/general 执行写入）。
    #[must_use]
    pub fn for_role(role: SubAgentType) -> Self {
        let (permissions, shell) = match role {
            // 只读调查者。
            SubAgentType::Explore | SubAgentType::Review => {
                (PermissionSet::read_only(), ShellPolicy::ReadOnly)
            }
            // 规划者：仅分析，无 shell。
            SubAgentType::Plan => (PermissionSet::read_only(), ShellPolicy::None),
            // 验证者：不修改代码，但运行测试套件。
            SubAgentType::Verifier => (PermissionSet::read_only(), ShellPolicy::Full),
            // 执行者。
            SubAgentType::Implementer | SubAgentType::General => {
                (PermissionSet::full(), ShellPolicy::Full)
            }
            // 自定义从锁定状态开始；调用者明确打开特定工具。
            SubAgentType::Custom => (PermissionSet::read_only(), ShellPolicy::None),
        };
        Self {
            role,
            permissions,
            shell,
            tools: ToolScope::Inherit,
            model: ModelRoute::Inherit,
            provider: None,
            reasoning_effort: None,
            denied_tools: Vec::new(),
            max_spawn_depth: codewhale_config::DEFAULT_SPAWN_DEPTH,
            background: true,
        }
    }

    /// 从父配置文件与 `requested` 子配置文件派生子配置文件。
    /// 结果是两者的 **交集** —— 永远不能授予子线程
    /// 父线程缺乏的能力（#414 / #426 / #1186）：
    ///
    /// - 权限是 AND 运算，
    /// - shell 取更严格的政策，
    /// - 显式父工具集限制子工具集，
    /// - 生成深度预算递减一层并限制到上限，
    /// - 工具拒绝列表是两者的 **并集** —— 子线程可以添加
    ///   限制但永远不能删除祖先施加的限制（#4042）。
    ///
    /// 子线程保持自己的请求角色、模型路由和
    /// 前台/后台偏好（这些不授予能力），但其
    /// 提供商在未设置时回退到父线程的。
    #[must_use]
    pub fn derive_child(&self, requested: &WorkerRuntimeProfile) -> WorkerRuntimeProfile {
        let permissions = self.permissions.intersect(requested.permissions);
        let shell = self.shell.min_with(requested.shell);
        // 拒绝列表并集：子线程永远不能删除祖先
        // 施加的限制。通配符条目原样合并（不展开）。
        let mut denied_tools = self.denied_tools.clone();
        for rule in &requested.denied_tools {
            if !denied_tools.contains(rule) {
                denied_tools.push(rule.clone());
            }
        }
        let tools = match (&self.tools, &requested.tools) {
            // 父线程限制为一组 → 子线程只能在其内部缩小。
            (ToolScope::Explicit(parent), ToolScope::Explicit(child)) => ToolScope::Explicit(
                child
                    .iter()
                    .filter(|name| parent.contains(name))
                    .cloned()
                    .collect(),
            ),
            (ToolScope::Explicit(parent), ToolScope::Inherit) => {
                ToolScope::Explicit(parent.clone())
            }
            // 父线程继承完整表面 → 子线程的请求成立。
            (ToolScope::Inherit, child) => child.clone(),
        };
        // 子线程获得的预算最多比父线程少一级，且永远不超过
        // 其请求，受硬上限限制。
        let max_spawn_depth = requested
            .max_spawn_depth
            .min(self.max_spawn_depth.saturating_sub(1))
            .min(codewhale_config::MAX_SPAWN_DEPTH_CEILING);
        WorkerRuntimeProfile {
            role: requested.role.clone(),
            permissions,
            shell,
            tools,
            model: requested.model.clone(),
            provider: requested.provider.clone().or_else(|| self.provider.clone()),
            reasoning_effort: requested
                .reasoning_effort
                .clone()
                .or_else(|| self.reasoning_effort.clone()),
            denied_tools,
            max_spawn_depth,
            background: requested.background,
        }
    }

    /// 此工作线程是否仍可以生成子线程（预算剩余）。
    #[must_use]
    pub fn can_spawn_child(&self) -> bool {
        self.max_spawn_depth > 0
    }
}

impl Default for WorkerRuntimeProfile {
    fn default() -> Self {
        Self::for_role(SubAgentType::General)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_intersection_never_escalates() {
        let parent = PermissionSet::read_only();
        let greedy_child = PermissionSet::full();
        // 即使子线程请求所有权限，只读父线程仍然胜出。
        let got = parent.intersect(greedy_child);
        assert_eq!(got, PermissionSet::read_only());
    }

    #[test]
    fn shell_policy_min_takes_the_safer() {
        assert_eq!(
            ShellPolicy::ReadOnly.min_with(ShellPolicy::Full),
            ShellPolicy::ReadOnly
        );
        assert_eq!(
            ShellPolicy::None.min_with(ShellPolicy::ReadOnly),
            ShellPolicy::None
        );
        assert_eq!(
            ShellPolicy::Full.min_with(ShellPolicy::Full),
            ShellPolicy::Full
        );
    }

    #[test]
    fn for_role_postures_match_role_stances() {
        let explore = WorkerRuntimeProfile::for_role(SubAgentType::Explore);
        assert!(!explore.permissions.write, "explore must not write");
        assert_eq!(explore.shell, ShellPolicy::ReadOnly);
        assert_eq!(
            explore.model,
            ModelRoute::Inherit,
            "explore should not silently downgrade the child model"
        );

        let implementer = WorkerRuntimeProfile::for_role(SubAgentType::Implementer);
        assert!(implementer.permissions.write, "implementer writes");
        assert_eq!(implementer.shell, ShellPolicy::Full);

        let verifier = WorkerRuntimeProfile::for_role(SubAgentType::Verifier);
        assert!(
            !verifier.permissions.write,
            "verifier reports, does not patch"
        );
        assert_eq!(
            verifier.shell,
            ShellPolicy::Full,
            "verifier runs the test suite"
        );
    }

    #[test]
    fn child_cannot_escalate_beyond_a_readonly_parent() {
        let parent = WorkerRuntimeProfile::for_role(SubAgentType::Explore); // 只读
        let greedy = WorkerRuntimeProfile::for_role(SubAgentType::Implementer); // 想要写入 + 完全 shell
        let child = parent.derive_child(&greedy);
        assert!(
            !child.permissions.write,
            "a read-only parent cannot bear a writing child"
        );
        assert!(!child.permissions.network);
        assert_eq!(
            child.shell,
            ShellPolicy::ReadOnly,
            "child shell clamped to parent's"
        );
    }

    #[test]
    fn child_explicit_tools_are_bounded_by_parent() {
        let mut parent = WorkerRuntimeProfile::for_role(SubAgentType::General);
        parent.tools = ToolScope::Explicit(vec!["read_file".into(), "grep_files".into()]);
        let mut requested = WorkerRuntimeProfile::for_role(SubAgentType::General);
        requested.tools = ToolScope::Explicit(vec!["read_file".into(), "write_file".into()]);
        let child = parent.derive_child(&requested);
        match child.tools {
            ToolScope::Explicit(names) => {
                assert_eq!(
                    names,
                    vec!["read_file".to_string()],
                    "write_file not in parent set is dropped"
                );
            }
            ToolScope::Inherit => panic!("expected explicit tool scope"),
        }
    }

    #[test]
    fn spawn_depth_decrements_and_clamps() {
        let mut parent = WorkerRuntimeProfile::for_role(SubAgentType::General);
        parent.max_spawn_depth = 2;
        let mut requested = WorkerRuntimeProfile::for_role(SubAgentType::General);
        requested.max_spawn_depth = 99; // 尝试获取比父线程更多的配额
        let child = parent.derive_child(&requested);
        assert_eq!(
            child.max_spawn_depth, 1,
            "child budget is at most parent-1, never the requested 99"
        );
        assert!(child.can_spawn_child());

        let mut leaf_parent = WorkerRuntimeProfile::for_role(SubAgentType::General);
        leaf_parent.max_spawn_depth = 1;
        let grandchild = leaf_parent.derive_child(&requested);
        assert_eq!(grandchild.max_spawn_depth, 0);
        assert!(
            !grandchild.can_spawn_child(),
            "budget exhausted at the leaf"
        );
    }

    #[test]
    fn child_provider_falls_back_to_parent() {
        let mut parent = WorkerRuntimeProfile::for_role(SubAgentType::General);
        parent.provider = Some("moonshot".to_string());
        let requested = WorkerRuntimeProfile::for_role(SubAgentType::Explore); // provider None
        let child = parent.derive_child(&requested);
        assert_eq!(child.provider.as_deref(), Some("moonshot"));
    }

    #[test]
    fn child_reasoning_effort_uses_requested_then_parent() {
        let mut parent = WorkerRuntimeProfile::for_role(SubAgentType::General);
        parent.reasoning_effort = Some("low".to_string());

        let requested = WorkerRuntimeProfile::for_role(SubAgentType::Explore);
        let inherited = parent.derive_child(&requested);
        assert_eq!(inherited.reasoning_effort.as_deref(), Some("low"));

        let mut requested = WorkerRuntimeProfile::for_role(SubAgentType::Explore);
        requested.reasoning_effort = Some("max".to_string());
        let overridden = parent.derive_child(&requested);
        assert_eq!(overridden.reasoning_effort.as_deref(), Some("max"));
    }

    #[test]
    fn child_denied_tools_union_never_drops_parent_restriction() {
        // 子线程只能 *添加* 拒绝条目；它永远不能删除
        // 祖先施加的限制（#4042 非升级不变量）。
        let mut parent = WorkerRuntimeProfile::for_role(SubAgentType::General);
        parent.denied_tools = vec!["exec_shell".into(), "mcp_*".into()];

        // 子线程请求自己的拒绝列表并（尝试性地）试图省略
        // 父线程的 exec_shell —— 并集保留两者。
        let mut requested = WorkerRuntimeProfile::for_role(SubAgentType::Implementer);
        requested.denied_tools = vec!["write_file".into()];

        let child = parent.derive_child(&requested);
        assert!(child.denied_tools.contains(&"exec_shell".to_string()));
        assert!(child.denied_tools.contains(&"mcp_*".to_string()));
        assert!(child.denied_tools.contains(&"write_file".to_string()));
    }
}
