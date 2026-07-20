//! Workroom 类型——用于线程化代理工作的持久聊天原生容器。
//!
//! 一个 [`Workroom`] 将线程、事件和外部引用分组为一个
//! 稳定、可寻址的表面，可以从 TUI、移动页面、聊天桥接
//! 和编程式 Runtime API 消费者访问。
//!
//! 完整设计见 `docs/rfcs/3209-workrooms.md`。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt::Write;

/// Workroom 的唯一标识符。
///
/// 跨重启保持稳定。对调用者不透明；通过 UUID v4 生成，
/// 带有 `wr_` 前缀以便链接识别。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct WorkroomId(pub String);

impl WorkroomId {
    /// 从 UUID v4 字符串创建新的 workroom id。
    pub fn new() -> Self {
        Self(format!("wr_{}", uuid::Uuid::new_v4().simple()))
    }
}

impl Default for WorkroomId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for WorkroomId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 线程化代理对话的持久容器。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workroom {
    pub id: WorkroomId,
    pub title: String,
    pub workspace: Option<String>,
    pub repo_identity: Option<RepoRef>,
    pub owner: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub visibility: WorkroomVisibility,
}

/// 附加到 workroom 的 GitHub 仓库标识。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoRef {
    pub owner: String,
    pub name: String,
}

/// Workroom 的可见性控制。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkroomVisibility {
    /// 只有本地用户可以访问。
    Private,
    /// 持有所列承载令牌之一的调用者可访问。
    Shared { allowed_tokens: Vec<String> },
}

/// Workroom 内的一个线程。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkroomThread {
    pub id: String,
    pub workroom_id: WorkroomId,
    pub title: String,
    pub kind: WorkroomThreadKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_ref: Option<ExternalThreadRef>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkroomThreadKind {
    Channel,
    DirectMessage,
    AgentTask,
    ApprovalQueue,
    ReceiptLog,
}

/// 可附加到 workroom 线程的外部引用。
///
/// 仅存储元数据——无 API 密钥、令牌或秘密。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExternalThreadRef {
    GitHubIssue {
        owner: String,
        repo: String,
        number: u64,
    },
    GitHubPullRequest {
        owner: String,
        repo: String,
        number: u64,
    },
    GitHubCommit {
        owner: String,
        repo: String,
        sha: String,
    },
    GitHubCheck {
        owner: String,
        repo: String,
        check_run_id: u64,
    },
}

/// Workroom 线程内的事件，归属于特定代理/模型。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkroomEvent {
    pub id: String,
    pub thread_id: String,
    pub workroom_id: WorkroomId,
    pub timestamp: DateTime<Utc>,
    pub kind: WorkroomEventKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentAttribution>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum WorkroomEventKind {
    Message { content: String },
    Mention { mentioned_user: String },
    ToolCall { tool_name: String, summary: String },
    ToolResult { tool_name: String, success: bool },
    ApprovalRequest { tool_name: String },
    ArtifactLinked { path: String, kind: String },
    Receipt { summary: String },
    Failure { error: String },
    NeedsHuman { reason: String },
    Resumed,
}

/// 记录哪个代理和模型产生了事件的归属元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAttribution {
    pub provider: String,
    pub model: String,
    pub agent_id: String,
}

/// 可解析为 workroom、线程或事件的可分享链接。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkroomLink {
    pub workroom_id: WorkroomId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
}

impl WorkroomLink {
    /// 解析 `codewhale://workroom/...` URL。
    ///
    /// 接受的形式：
    /// - `codewhale://workroom/wr_<id>`
    /// - `codewhale://workroom/wr_<id>/thread/<thread_id>`
    /// - `codewhale://workroom/wr_<id>/event/<event_id>`
    pub fn parse(url: &str) -> Option<Self> {
        let rest = url.strip_prefix("codewhale://workroom/")?;
        let mut segments = rest.split('/');
        let workroom_id = parse_segment_with_prefix(segments.next()?, "wr_")?;
        let next = segments.next();
        let (thread_id, event_id) = match next {
            None => (None, None),
            Some("thread") => {
                let thread_id = non_empty_segment(segments.next()?)?;
                match segments.next() {
                    None => (Some(thread_id), None),
                    Some("event") => {
                        let event_id = non_empty_segment(segments.next()?)?;
                        if segments.next().is_some() {
                            return None;
                        }
                        (Some(thread_id), Some(event_id))
                    }
                    _ => return None,
                }
            }
            Some("event") => {
                let event_id = non_empty_segment(segments.next()?)?;
                if segments.next().is_some() {
                    return None;
                }
                (None, Some(event_id))
            }
            _ => return None,
        };

        Some(Self {
            workroom_id: WorkroomId(workroom_id),
            thread_id,
            event_id,
        })
    }

    /// 序列化回 `codewhale://workroom/...` URL 形式。
    pub fn to_url(&self) -> String {
        let mut url = format!("codewhale://workroom/{}", self.workroom_id);
        if let Some(ref thread_id) = self.thread_id {
            write!(url, "/thread/{thread_id}").unwrap();
            if let Some(ref event_id) = self.event_id {
                write!(url, "/event/{event_id}").unwrap();
            }
        } else if let Some(ref event_id) = self.event_id {
            write!(url, "/event/{event_id}").unwrap();
        }
        url
    }
}

fn parse_segment_with_prefix(segment: &str, prefix: &str) -> Option<String> {
    let segment = non_empty_segment(segment)?;
    if segment.len() == prefix.len() || !segment.starts_with(prefix) {
        return None;
    }
    Some(segment)
}

fn non_empty_segment(segment: &str) -> Option<String> {
    if segment.is_empty() {
        None
    } else {
        Some(segment.to_string())
    }
}

/// Workroom 用于列表/收件箱视图的摘要投影。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkroomSummary {
    pub id: WorkroomId,
    pub title: String,
    pub updated_at: DateTime<Utc>,
    pub active_threads: usize,
}

/// Workroom 的分页列表。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkroomListResponse {
    pub workrooms: Vec<WorkroomSummary>,
}

/// `/workroom/resolve` 端点的响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkroomResolveResponse {
    pub link: WorkroomLink,
    pub thread_title: Option<String>,
    pub external_ref: Option<ExternalThreadRef>,
    pub recent_events: Vec<WorkroomEvent>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workroom_id_new_is_stable() {
        let id = WorkroomId::new();
        assert!(id.0.starts_with("wr_"));
        assert_eq!(id.0.len(), 35); // "wr_" + 32 hex chars
    }

    #[test]
    fn workroom_link_parse_workroom_only() {
        let link = WorkroomLink::parse("codewhale://workroom/wr_abc123def456").unwrap();
        assert_eq!(link.workroom_id.0, "wr_abc123def456");
        assert!(link.thread_id.is_none());
        assert!(link.event_id.is_none());
    }

    #[test]
    fn workroom_link_parse_with_thread() {
        let link = WorkroomLink::parse("codewhale://workroom/wr_abc/thread/thr_xyz").unwrap();
        assert_eq!(link.workroom_id.0, "wr_abc");
        assert_eq!(link.thread_id.as_deref(), Some("thr_xyz"));
        assert!(link.event_id.is_none());
    }

    #[test]
    fn workroom_link_parse_with_event() {
        let link = WorkroomLink::parse("codewhale://workroom/wr_abc/event/evt_789").unwrap();
        assert_eq!(link.workroom_id.0, "wr_abc");
        assert_eq!(link.event_id.as_deref(), Some("evt_789"));
        assert!(link.thread_id.is_none());
    }

    #[test]
    fn workroom_link_roundtrip() {
        let original = "codewhale://workroom/wr_abc/thread/thr_x/event/evt_y";
        let parsed = WorkroomLink::parse(original).unwrap();
        assert_eq!(parsed.to_url(), original);
    }

    #[test]
    fn workroom_link_reject_bad_prefix() {
        assert!(WorkroomLink::parse("http://workroom/wr_abc").is_none());
        assert!(WorkroomLink::parse("codewhale://not-workroom/wr_abc").is_none());
    }

    #[test]
    fn workroom_link_rejects_malformed_paths() {
        assert!(WorkroomLink::parse("codewhale://workroom/").is_none());
        assert!(WorkroomLink::parse("codewhale://workroom/abc").is_none());
        assert!(WorkroomLink::parse("codewhale://workroom/wr_").is_none());
        assert!(WorkroomLink::parse("codewhale://workroom/wr_abc/thread").is_none());
        assert!(WorkroomLink::parse("codewhale://workroom/wr_abc/thread/").is_none());
        assert!(WorkroomLink::parse("codewhale://workroom/wr_abc/unknown/x").is_none());
        assert!(WorkroomLink::parse("codewhale://workroom/wr_abc/event/evt/x").is_none());
    }

    #[test]
    fn external_thread_ref_serde_roundtrip() {
        let issue = ExternalThreadRef::GitHubIssue {
            owner: "Hmbown".into(),
            repo: "CodeWhale".into(),
            number: 3209,
        };
        let json = serde_json::to_string(&issue).unwrap();
        let back: ExternalThreadRef = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, ExternalThreadRef::GitHubIssue { .. }));
    }

    #[test]
    fn agent_attribution_serde_roundtrip() {
        let attr = AgentAttribution {
            provider: "deepseek".into(),
            model: "deepseek-v4-pro".into(),
            agent_id: "sub_agent_1".into(),
        };
        let json = serde_json::to_string(&attr).unwrap();
        let back: AgentAttribution = serde_json::from_str(&json).unwrap();
        assert_eq!(back.provider, "deepseek");
        assert_eq!(back.model, "deepseek-v4-pro");
    }
}
