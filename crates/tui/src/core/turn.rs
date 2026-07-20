//! 对话轮次（Turn）上下文与跟踪。
//!
//! 一个“轮次”指一条用户消息及其产生的 AI 响应，
//! 包括期间发生的任何工具调用。
//!
//! ## 快照生命周期钩子
//!
//! [`pre_turn_snapshot`] 和 [`post_turn_snapshot`] 分别在一个轮次开始和结束时，
//! 将工作区级别的快照保存到侧边 git 仓库中（参见 `crate::snapshot`）。
//! 它们被设计为非阻塞且非致命：任何 IO 错误都会以 WARN 级别记录并吞掉，
//! 这样即便文件系统损坏或 `git` 二进制缺失也不会中断代理循环。
//! `/restore N` 命令和 `revert_turn` 工具都会消费这些快照。

use crate::models::Usage;
use crate::snapshot::SnapshotRepo;
use std::path::Path;
use std::time::{Duration, Instant};

/// 单个轮次（用户消息 + AI 响应）的上下文。
#[derive(Debug)]
pub struct TurnContext {
    /// 轮次 ID
    pub id: String,

    /// 轮次开始的时间
    #[allow(dead_code)]
    pub started_at: Instant,

    /// 轮次中的当前步骤（工具调用迭代次数）
    pub step: u32,

    /// 允许的最大步骤数
    pub max_steps: u32,

    /// 本轮次中已执行的工具调用次数。
    tool_call_count: usize,

    /// 轮次是否已被取消
    #[allow(dead_code)]
    pub cancelled: bool,

    /// 本轮次的用量统计
    pub usage: Usage,
}

impl TurnContext {
    /// 创建一个新的轮次上下文
    pub fn new(max_steps: u32) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            started_at: Instant::now(),
            step: 0,
            max_steps,
            tool_call_count: 0,
            cancelled: false,
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
                ..Usage::default()
            },
        }
    }

    /// 步数计数器加一
    pub fn next_step(&mut self) -> bool {
        self.step += 1;
        self.step <= self.max_steps
    }

    /// 检查轮次是否已达到最大步数
    pub fn at_max_steps(&self) -> bool {
        self.step >= self.max_steps
    }

    /// 记录一次工具调用。
    pub fn record_tool_call(&mut self) {
        self.tool_call_count += 1;
    }

    /// 本轮次是否执行了至少一次工具调用。
    pub fn has_tool_calls(&self) -> bool {
        self.tool_call_count > 0
    }

    /// 取消轮次
    #[allow(dead_code)]
    pub fn cancel(&mut self) {
        self.cancelled = true;
    }

    /// 获取已过去的时间
    #[allow(dead_code)]
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// 累加来自 API 响应的用量
    pub fn add_usage(&mut self, usage: &Usage) {
        self.usage.input_tokens += usage.input_tokens;
        self.usage.output_tokens += usage.output_tokens;
        self.usage.prompt_cache_hit_tokens = add_optional_usage(
            self.usage.prompt_cache_hit_tokens,
            usage.prompt_cache_hit_tokens,
        );
        self.usage.prompt_cache_miss_tokens = add_optional_usage(
            self.usage.prompt_cache_miss_tokens,
            usage.prompt_cache_miss_tokens,
        );
        self.usage.reasoning_tokens =
            add_optional_usage(self.usage.reasoning_tokens, usage.reasoning_tokens);
    }
}

fn add_optional_usage(total: Option<u32>, delta: Option<u32>) -> Option<u32> {
    match (total, delta) {
        (Some(total), Some(delta)) => Some(total.saturating_add(delta)),
        (None, Some(delta)) => Some(delta),
        (Some(total), None) => Some(total),
        (None, None) => None,
    }
}

/// 要嵌入快照标签中的用户提示摘要的最大字符数。
/// 较长的提示会被截断并添加省略号。
const USER_PROMPT_LABEL_MAX: usize = 100;

/// 生成包含用户提示的快照标签，以便在 `/restore` 列表中提高可读性。
///
/// 提取提示的第一行（最多 `USER_PROMPT_LABEL_MAX` 个字符），
/// 附加到传统的 `类型:序号` 标签后，让用户能识别每个快照属于哪个轮次。
fn format_snapshot_label(prefix: &str, turn_seq: u64, user_prompt: Option<&str>) -> String {
    let base = format!("{prefix}:{turn_seq}");
    match user_prompt {
        None | Some("") => base,
        Some(prompt) => {
            let first_line = prompt.lines().next().unwrap_or("");
            let truncated: String = first_line.chars().take(USER_PROMPT_LABEL_MAX).collect();
            if truncated.chars().count() < first_line.chars().count() {
                format!("{base}: {truncated}…")
            } else {
                format!("{base}: {truncated}")
            }
        }
    }
}

/// 在轮次开始前拍摄 `pre-turn:<序号>` 工作区快照。
///
/// `cap_bytes` 是工作区大小上限，用于控制首次初始化（透传给 [`SnapshotRepo::open_or_init_with_cap`]）；
/// 传入 `0` 表示禁用该上限。
/// `user_prompt` 是本次轮次用户消息的可选摘要片段，嵌入快照标签中使 `/restore` 列表更可读。
///
/// 成功时返回快照 SHA，任何错误时返回 `None`。错误会以 WARN 级别记录；
/// 轮次循环不得因本函数而阻塞。
pub fn pre_turn_snapshot(
    workspace: &Path,
    turn_seq: u64,
    cap_bytes: u64,
    user_prompt: Option<&str>,
) -> Option<String> {
    snapshot_with_label(
        workspace,
        &format_snapshot_label("pre-turn", turn_seq, user_prompt),
        cap_bytes,
    )
}

/// 在执行修改文件的工具调用（write_file、edit_file、apply_patch）之前，
/// 拍摄 `tool:<调用ID>` 工作区快照。
///
/// 这支持精细撤销：`/undo` 可以恢复到最近的 `tool:<调用ID>` 快照，
/// 从而仅撤销最后一次文件写入。
///
/// 成功时返回快照 SHA，任何错误时返回 `None`。错误会以 WARN 级别记录且是非致命的。
pub fn pre_tool_snapshot(workspace: &Path, call_id: &str, cap_bytes: u64) -> Option<String> {
    snapshot_with_label(workspace, &format!("tool:{call_id}"), cap_bytes)
}

/// 在轮次结束后拍摄 `post-turn:<序号>` 工作区快照。失败模型同 [`pre_turn_snapshot`]。
pub fn post_turn_snapshot(
    workspace: &Path,
    turn_seq: u64,
    cap_bytes: u64,
    user_prompt: Option<&str>,
) -> Option<String> {
    snapshot_with_label(
        workspace,
        &format_snapshot_label("post-turn", turn_seq, user_prompt),
        cap_bytes,
    )
}

fn snapshot_with_label(workspace: &Path, label: &str, cap_bytes: u64) -> Option<String> {
    // 尝试打开已有的快照仓库，如果不存在就创建一个
    match SnapshotRepo::open_or_init_with_cap(workspace, cap_bytes) {
        Ok(repo) => {
            // 调用 repo.snapshot(label) 执行快照。成功后 id 是一个 newtype（元组结构体）
            // id.0 访问第一个（通常是唯一一个）元素，就是 SHA 字符串。包装成 Some。
            let id = match repo.snapshot(label) {
                Ok(id) => Some(id.0),
                Err(e) => {
                    tracing::warn!(target: "snapshot", "snapshot '{label}' failed: {e}");
                    return None;
                }
            };
            // 裁剪最旧的快照以限制磁盘使用（#1112）
            if let Err(e) = repo.prune_keep_last_n(crate::snapshot::DEFAULT_MAX_SNAPSHOTS) {
                tracing::warn!(target: "snapshot", "snapshot prune failed: {e}");
            }
            id
        }
        Err(e) => {
            tracing::warn!(target: "snapshot", "snapshot repo init failed: {e}");
            None
        }
    }
}
