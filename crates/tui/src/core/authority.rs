//! Turn authority and mode/posture policy projections.
//!
//! 将模式、审批、Shell、沙箱、信任和输入来源的决策集中在一起，避免各处逻辑不一致。
//! 
//! authority.rs 是 CodeWhale TUI 中权限/授权体系的"单一真相来源"。它把"当前模式 →   
//! 能做什么"的映射集中在一处，这样各个子系统（提示词、工具目录、运行时检查）就不会各自
//! 为政。

use std::path::Path;

use crate::sandbox::SandboxPolicy;  // 沙箱策略（决定能访问哪些文件/网络）
use crate::tui::app::AppMode;       // 应用模式（Agent / Plan / Yolo / Operate / Auto）
use crate::tui::approval::ApprovalMode;  // 审批模式（Bypass 跳过 / Suggest 建议 / Auto 自动）
use crate::worker_profile::ShellPolicy;  // Shell 执行策略（Full 允许 / None 禁止）

use super::ops::UserInputProvenance;  // 用户输入的"来源"，比如是真人用户输入的、还是运行时自动生成的、还是子代理转交的。

/// 会话级模式偏好
/// 这是"持久化的 Agent 时代权限基线"，Plan 和 YOLO模式切换回来时都会恢复到这个基线。避免模式切换
/// 时权限混乱。
/// Durable Agent-era permission baseline that Plan/YOLO restore to (#3386).
///
/// 在 Agent 模式下，用户设定了一套权限配置（比如是否允许执行 shell 命令、是否信任某些操作等）
/// 当用户从 Plan/YOLO 模式切换回来时，权限会还原到这套配置，而不是回到某个默认值
/// 以前，模式切换和权限管理是耦合的，逻辑混乱。每个模式（Plan、YOLO、Agent）都会直接修改权限开关，
/// 导致状态互相干扰。旧方案靠退出时“拍快照”来恢复，但这是临时性、不稳健的做法（容易漏恢复或恢复错）
/// 新方案：只维护一个权威基线，就是用户在 Agent 模式下设定的权限配置，所有切换都以它为准
/// 一句话总结
// ModeSessionPrefs 是一个“锚点”——用户在 Agent 模式下设定的权限配置，作为唯一可信来源，Plan/YOLO 
// 模式切换进出时都以此为基准进行恢复，避免了之前各模式乱改权限导致的状态错乱问题。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ModeSessionPrefs {
    /// Agent 模式下是否允许执行 shell 命令
    pub(crate) agent_allow_shell: bool,
    /// Agent 模式下是否启用信任模式（信任模式下会自动批准更多操作）
    pub(crate) agent_trust_mode: bool,
    /// Agent 模式下默认的审批策略
    pub(crate) agent_approval_mode: ApprovalMode,
}

/// [`AppMode`]模式解析后的有效权限
/// The permission policy a given [`AppMode`] resolves to (#3386).
/// 这是解析后的最终权限策略——某个模式 + 用户偏好计算得出。
/// 四个字段就是"当前能做什么"的结论：是否允许 shell、是否信任、什么审批策略。
#[derive(Debug, Clone, Copy)]
pub(crate) struct EffectiveModePolicy {
    #[allow(dead_code)]
    pub(crate) mode: AppMode,
    pub(crate) allow_shell: bool,
    pub(crate) trust_mode: bool,
    pub(crate) approval_mode: ApprovalMode,
}

/// 核心权限映射函数
/// Resolve a mode's effective permission policy from the durable Agent baseline.
///
/// This is the single source of truth for the mode/permission table:
/// - `Plan`   -> 纯只读——允许执行 shell，不信任，审批策略是 Suggest（系统会建议但需要用户明确同意）。
/// - `Agent`  -> 使用用户在会话偏好中设置的权限基线。(`prefs`).
/// - `Auto`   -> compatibility alias for Agent; not a separate behavior.
/// - `Operate` -> Agent baseline plus orchestration posture in prompts(Agent + 编排姿态)。
/// - `Yolo`   -> legacy compat; 完全信任: shell + trust + `Bypass`(审批直接绕过).
#[must_use]
pub(crate) fn base_policy_for_mode(mode: AppMode, prefs: &ModeSessionPrefs) -> EffectiveModePolicy {
    match mode {
        AppMode::Plan => EffectiveModePolicy {
            mode,
            allow_shell: false,
            trust_mode: false,
            approval_mode: ApprovalMode::Suggest,
        },
        AppMode::Agent | AppMode::Auto | AppMode::Operate => EffectiveModePolicy {
            mode,
            allow_shell: prefs.agent_allow_shell,
            trust_mode: prefs.agent_trust_mode,
            approval_mode: prefs.agent_approval_mode,
        },
        AppMode::Yolo => EffectiveModePolicy {
            mode,
            allow_shell: true,
            trust_mode: true,
            approval_mode: ApprovalMode::Bypass,
        },
    }
}

/// Effective authority for one engine turn after provenance narrowing.
/// 单个引擎轮次的权限快照
#[derive(Debug, Clone)]
pub(crate) struct TurnAuthority {
    pub(crate) mode: AppMode,
    pub(crate) allow_shell: bool,
    pub(crate) trust_mode: bool,
    pub(crate) auto_approve: bool,
    pub(crate) approval_mode: ApprovalMode,
    pub(crate) dynamic_active_tools: Vec<&'static str>,   // 动态激活的工具名称列表
    pub(crate) status: Option<String>,  // 用于存放状态信息（比如"因为输入来源不是真人用户，所以降级了权限"）。
}

impl TurnAuthority {

    /// 从各个字段直接构造一个 TurnAuthority。dynamic_active_tools 和 status 用默认空值。
    #[must_use]
    pub(crate) fn from_effective_fields(
        mode: AppMode,
        allow_shell: bool,
        trust_mode: bool,
        auto_approve: bool,
        approval_mode: ApprovalMode,
    ) -> Self {
        Self {
            mode,
            allow_shell,
            trust_mode,
            auto_approve,
            approval_mode,
            dynamic_active_tools: Vec::new(),
            status: None,
        }
    }

    /// 委托给下方的 agent_approval_mode_for_turn 函数。如果 auto_approve 为 true，就返回
    /// Bypass；否则返回原始的 approval_mode。
    #[must_use]
    pub(crate) fn approval_mode_for_session(&self) -> ApprovalMode {
        agent_approval_mode_for_turn(self.auto_approve, self.approval_mode)
    }

    /// 委托给下方的 shell_policy_for_mode 函数。Plan 模式或 allow_shell=false 时返回 None，
    /// 否则 Full
    #[must_use]
    pub(crate) fn shell_policy(&self) -> ShellPolicy {
        shell_policy_for_mode(self.mode, self.allow_shell)
    }

    /// 委托给 sandbox_policy_for_mode。需要传入工作区路径来决定可写根目录。
    #[must_use]
    pub(crate) fn sandbox_policy(&self, workspace: &Path) -> SandboxPolicy {
        sandbox_policy_for_mode(self.mode, workspace)
    }
}

/// 根据输入来源收窄权限
#[must_use]
pub(crate) fn effective_input_policy(
    provenance: UserInputProvenance,
    requested_mode: AppMode,
    _content: &str,
    allow_shell: bool,
    trust_mode: bool,
    auto_approve: bool,
    approval_mode: ApprovalMode,
) -> TurnAuthority {
    let mut mode = requested_mode;
    let mut trust_mode = trust_mode;
    let mut auto_approve = auto_approve;
    let mut approval_mode = approval_mode;
    let mut status = None;

    // 如果这个来源不能继承自动授权，就要降级权限。
    if !provenance_can_inherit_standing_auto_authority(provenance) {
        // 原本拥有自动权限，现在被收回了。
        let had_auto_authority = matches!(mode, AppMode::Yolo)
            || trust_mode
            || auto_approve
            || matches!(approval_mode, ApprovalMode::Bypass);
        if matches!(mode, AppMode::Yolo) {
            mode = AppMode::Agent;
        }
        // 降级操作：
        // - Yolo → Agent
        // - 信任模式关闭 
        // - 自动批准关闭
        // - 审批从 Auto/Bypass → Suggest（需要用户明确同意）
        // 这是一种安全措施：不是真人用户直接发出的指令，不能享受最高权限。
        trust_mode = false;
        auto_approve = false;
        if matches!(approval_mode, ApprovalMode::Auto | ApprovalMode::Bypass) {
            approval_mode = ApprovalMode::Suggest;
        }

        // 如果原本有自动权限，就生成一条状态信息，告诉用户"权限已被降级"。
        if had_auto_authority {
            status = Some(format!(
                "Input provenance '{}' cannot inherit standing auto-approval authority; continuing with approvals required.",
                // "输入来源 '{}' 无法继承既有的自动批准权限；将继续要求人工批准。"
                provenance.as_str()
            ));
        }
    }

    // 最后构造并返回 TurnAuthority。
    TurnAuthority {
        mode,
        allow_shell,
        trust_mode,
        auto_approve,
        approval_mode,
        dynamic_active_tools: Vec::new(),
        status,
    }
}

/// 只有这三种来源可以继承自动授权：
/// - `ExternalUser`：真人用户（通过 TUI/CLI 输入） 
/// - `Runtime`：运行时本身（比如定时任务、钩子触发）
/// - `SubAgentHandoff`：子代理交还结果时 
/// 其他来源（比如 MCP 工具返回的、Webhook 事件等）不能自动批准，必须走审批流程。
#[must_use]
pub(crate) fn provenance_can_inherit_standing_auto_authority(
    provenance: UserInputProvenance,
) -> bool {
    matches!(
        provenance,
        UserInputProvenance::ExternalUser
            | UserInputProvenance::Runtime
            | UserInputProvenance::SubAgentHandoff
    )
}

/// 如果 auto_approve 开关打开 → 直接 Bypass（跳过所有审批）。
/// 否则保持原有的 approval_mode。
#[must_use]
pub(crate) fn agent_approval_mode_for_turn(
    auto_approve: bool,
    approval_mode: ApprovalMode,
) -> ApprovalMode {
    if auto_approve {
        ApprovalMode::Bypass
    } else {
        approval_mode
    }
}

/// Pick the sandbox policy that gates shell commands for a given UI mode.
/// - Plan 模式：沙箱只读，任何写操作都被阻止。
/// - Agent/Auto/Operate 模式：限制在 workspace 内可写——能写工作区目录。
/// - Yolo 模式：完全访问（DangerFullAccess里的 Danger 就是在警告你"危险"）。
#[must_use]
pub(crate) fn sandbox_policy_for_mode(mode: AppMode, workspace: &Path) -> SandboxPolicy {
    match mode {
        AppMode::Plan => SandboxPolicy::ReadOnly,
        AppMode::Agent | AppMode::Auto | AppMode::Operate => SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![workspace.to_path_buf()],
            network_access: true,    // 允许网络访问
            exclude_tmpdir: false,   // 不排除临时目录
            exclude_slash_tmp: false,
        },
        AppMode::Yolo => SandboxPolicy::DangerFullAccess,
    }
}

/// Resolve the effective shell policy for a turn from legacy shell opt-in plus mode.
/// - 如果压根不允许 allow_shell = false → 直接返回 None
/// - Plan 模式天然禁止 shell。
/// - 其他模式在 allow_shell = true 时返回 ShellPolicy::Full.
#[must_use]
pub(crate) fn shell_policy_for_mode(mode: AppMode, allow_shell: bool) -> ShellPolicy {
    if !allow_shell {
        return ShellPolicy::None;
    }
    match mode {
        AppMode::Plan => ShellPolicy::None,
        AppMode::Agent | AppMode::Auto | AppMode::Operate | AppMode::Yolo => ShellPolicy::Full,
    }
}
