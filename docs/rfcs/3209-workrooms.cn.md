# RFC：CodeWhale Workrooms — 聊天原生的线程化代理工作

**Issue：** #3209
**状态：** 草案
**日期：** 2026-06-17
**目标：** v0.9.0

本文档是设计脚手架。v0.8.62 分支当前仅携带共享协议类型和链接解析器；运行时端点、移动 UI 集成、持久化状态和模型可见工具仍是后续工作。

## 1. 问题

CodeWhale 代理工作当前存在于瞬态 TUI 会话、本地 Runtime API 线程、Fleet 运行和聊天桥接消息循环中 — 每个都有其自己的生命周期、状态表示和上下文边界。没有一个一等抽象能够：

- 让用户在一个界面（TUI）上开始工作并在另一个界面（移动端）上恢复它
- 为代理工作线程提供稳定、可共享的链接
- 将 GitHub issue/PR/提交作为上下文附加，而无需复制转录
- 记录哪个代理/模型为多代理工作流生成了每个事件
- 提供统一的提及、审批、失败和完成收件箱

## 2. 提案的抽象：`Workroom`

`Workroom` 是一个持久的、可寻址的容器，用于涉及一个或多个代理、模型和人类参与者的线程化对话。它映射到现有的 Runtime API 线程基础设施，并扩展了以下内容：

### 2.1 核心类型

```rust
/// 工作室的唯一标识符，在重启之间保持稳定。
pub struct WorkroomId(pub String);  // 例如："wr_abc123def456"

/// 一个工作室聚合了线程、成员和元数据。
pub struct Workroom {
    pub id: WorkroomId,
    pub title: String,
    pub workspace: Option<String>,      // 仓库根目录或项目路径
    pub repo_identity: Option<RepoRef>, // GitHub 仓库标识（owner/name）
    pub owner: String,                  // 本地用户或身份句柄
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub visibility: WorkroomVisibility,
}

pub enum WorkroomVisibility {
    Private,
    Shared { allowed_tokens: Vec<String> },
}

/// 工作室内的一个线程 — 可以是频道、私信或链接的外部引用。
pub struct WorkroomThread {
    pub id: String,
    pub workroom_id: WorkroomId,
    pub title: String,
    pub kind: WorkroomThreadKind,
    pub external_ref: Option<ExternalThreadRef>,
    pub created_at: DateTime<Utc>,
}

pub enum WorkroomThreadKind {
    Channel,
    DirectMessage,
    AgentTask,       // 由代理为子工作生成
    ApprovalQueue,   // 待处理的人类审批
    ReceiptLog,      // 已完成的代理回执
}

/// 可以附加到工作室线程的外部引用。
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

/// 工作室线程内的事件，归属于代理/模型。
pub struct WorkroomEvent {
    pub id: String,
    pub thread_id: String,
    pub workroom_id: WorkroomId,
    pub timestamp: DateTime<Utc>,
    pub kind: WorkroomEventKind,
    pub agent: Option<AgentAttribution>,
}

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

pub struct AgentAttribution {
    pub provider: String,   // 例如："deepseek"
    pub model: String,      // 例如："deepseek-v4-pro"
    pub agent_id: String,   // 子代理或 fleet 工作者 id
}

/// 可以粘贴到任何界面并解析回工作室的链接。
pub struct WorkroomLink {
    pub workroom_id: WorkroomId,
    pub thread_id: Option<String>,
    pub event_id: Option<String>,
}
```

### 2.2 链接格式

```
codewhale://workroom/wr_abc123def456
codewhale://workroom/wr_abc123def456/thread/thr_xyz
codewhale://workroom/wr_abc123def456/event/evt_789
```

### 2.3 与现有基础设施的映射

| Workroom 概念 | 现有映射 |
|---|---|
| `Workroom` | 新抽象；未来在 Runtime API 线程旁边的持久化状态 |
| `WorkroomThread` | 映射到 Runtime API 中的 `ThreadId` |
| `WorkroomEvent` | 包装现有线程/fleet 事件，附带代理归属 |
| `WorkroomLink` | 可由 Runtime API 解析的新 URL 方案 |
| `ExternalThreadRef` | 新的；仅元数据，无密钥/token 存储 |
| `AgentAttribution` | 从子代理元数据和 fleet 工作者身份中提取 |

## 3. 计划的 Runtime API 端点

### 3.1 `GET /workrooms`

列出对经过身份验证的调用方可见的所有工作室。

响应：
```json
{
  "workrooms": [
    {
      "id": "wr_abc123",
      "title": "PR #3231 — DeepInfra 支持",
      "updated_at": "2026-06-15T12:00:00Z",
      "active_threads": 3
    }
  ]
}
```

### 3.2 `GET /workroom/:id/threads`

列出工作室内的活动线程。

### 3.3 `GET /workroom/resolve?link=codewhale://workroom/wr_abc/thread/thr_x`

将工作室链接解析为限定上下文（线程元数据、最近事件），而无需重放完整转录。

### 3.4 计划的工具：`resolve_workroom_link`

一个模型可见的工具，接受 `codewhale://workroom/...` URL 并返回限定上下文（线程标题、最近事件摘要、外部引用）。在支持的运行时解析行为存在之前，不应注册此工具。

## 4. 安全模型

- **默认本地优先。** 持久化的工作室状态应位于 CodeWhale 主目录下，与现有状态并列。不假设任何云服务。
- **需要 Runtime API 认证。** 计划的工作室端点必须使用与其他运行时界面相同的 `Authorization: Bearer <token>` 保护。
- **链接中不含密钥。** 工作室链接仅包含不透明 ID，绝不包含 API 密钥或 token。解析需要本地 Runtime API 访问。
- **事件中不含密钥。** 事件负载不得包含 API 密钥、认证 token 或明文凭证。`ArtifactLinked` 事件类型引用路径，而非内容。
- **共享语义。** `WorkroomVisibility::Shared` 列出允许的 bearer token，而非用户名。操作者控制哪些 token 可以访问工作室。
- **无公共链接。** 工作室没有未经身份验证的读取路径。

## 5. 集成点

### 5.1 移动控制页面

`/mobile` 的移动页面已经列出活动线程。用 `/workrooms` 投影替换其临时的线程列表，使其渲染与 TUI 和聊天桥接器看到的相同的收件箱。

### 5.2 聊天桥接器（Telegram、Feishu）

聊天桥接器当前维护其自己的消息循环。每个桥接器应将桥接来源的消息作为 `WorkroomEvent::Message` 发布到指定的工作室线程，并将 `WorkroomEvent::Mention` 事件作为桥接通知消费。

### 5.3 TUI

TUI 应在侧边栏中展示工作室收件箱事件（提及、审批），并允许将 `codewhale://` 链接粘贴到输入框中进行上下文解析。

## 6. 实现计划

### 阶段 1：基础（本 PR）
- [x] RFC 设计文档
- [x] `WorkroomId`、`Workroom`、`WorkroomThread`、`WorkroomEvent`、`WorkroomLink` 类型
- [x] `ExternalThreadRef`（作为工作室上下文的 GitHub 引用）
- [x] `AgentAttribution`（多代理/模型事件归属）
- [x] 安全模型文档
- [x] 架构文档

### 阶段 2：集成（后续）
- [ ] 持久化工作室状态存储
- [ ] Runtime API 端点：`GET /workrooms`、`GET /workroom/:id/threads`
- [ ] `resolve_workroom_link` 工具用于链接解析
- [ ] 移动页面消费工作室投影
- [ ] 聊天桥接器发布/消费工作室事件
- [ ] TUI 收件箱侧边栏
- [ ] 输入框中的工作室链接粘贴解析

## 7. 阶段 1 的非目标

- 不提供托管的公共 CodeWhale 云服务
- 不默认开启 Slack/Discord/Feishu/Telegram/GitHub App 集成
- 在没有明确认证方案的情况下不提供任意公共分享链接
- 不提供模型特定的工作室格式
- 不迁移现有线程（仅新工作室）
