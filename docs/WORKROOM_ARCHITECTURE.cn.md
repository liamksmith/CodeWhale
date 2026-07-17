# Workroom 架构

## 目的

Workroom 是 CodeWhale 中用于持久化、可寻址的智能体工作线程的聊天原生抽象。它位于 Runtime API 的瞬态线程模型与面向用户的界面（TUI、移动端、聊天桥接）之间。

本文为 v0.9 架构草案说明。在 v0.8.62 中，仅协议数据类型和链接解析器已实现。Runtime 端点、持久化状态、移动端渲染以及模型可见的链接解析均为计划后续工作。

## 组件图

```
┌─────────────────────────────────────────────────────┐
│ 用户界面                                              │
│  ┌──────┐  ┌─────────┐  ┌──────────┐               │
│  │ TUI  │  │ 移动端   │  │ 桥接     │               │
│  └──┬───┘  └────┬────┘  └────┬─────┘               │
│     │           │            │                       │
│     └───────────┼────────────┘                       │
│                 │  未来的 HTTP + workroom 链接       │
├─────────────────┼───────────────────────────────────┤
│ Runtime API     │                                    │
│  ┌──────────────┴──────────────┐                    │
│  │ 计划中的 workroom 端点      │                    │
│  │  GET /workrooms             │                    │
│  │  GET /workroom/:id/threads  │                    │
│  │  GET /workroom/resolve      │                    │
│  └──────────────┬─────────────┘                    │
│                 │                                    │
│  ┌──────────────┴─────────────┐                    │
│  │ 现有端点                   │                    │
│  │  /thread /app /prompt ...  │                    │
│  └────────────────────────────┘                    │
└─────────────────────────────────────────────────────┘
```

## 数据流

1. **创建。** 未来的 workroom 在启动一个带有 workroom 上下文（标题、工作空间、外部引用）的线程时创建。workroom id 是稳定的，可以作为 `codewhale://workroom/...` 链接分享。

2. **事件发布。** 每个智能体操作（工具调用、审批、失败）作为 `WorkroomEvent` 记录在 workroom 的事件日志中。事件携带 `AgentAttribution` 元数据，追踪产生该事件的 provider、模型和智能体。

3. **链接解析。** 当 `codewhale://workroom/...` 链接出现在聊天界面中时，未来的 `resolve_workroom_link` 工具（或 API 端点）解析它并返回限定范围的上下文：线程元数据、外部引用和最近的事件摘要。调用模型随后可以决定是否读取完整的线程转录。

4. **列表。** 未来的 `/workrooms` 端点返回所有可见 workroom 的摘要（id、标题、updated_at、活跃线程数）。界面消费此信息用于收件箱/最近活动视图。

## 状态存储

持久化的 workroom 状态应与现有 CodeWhale 状态并存：

```
~/.codewhale/
├── workrooms/
│   ├── wr_abc123.json     # Workroom 元数据 + 事件日志
│   └── wr_def456.json
├── threads/               # 现有线程状态（不变）
├── checkpoints/
├── config.toml
└── ...
```

每个 `.json` 文件包含 workroom 元数据（`Workroom` 结构体）、`WorkroomThread` 描述符列表以及一组有界数量的最近 `WorkroomEvent` 记录。此状态存储尚未实现。

## Crate 职责

| Crate | 职责 |
|---|---|
| `codewhale-protocol` | 类型：`Workroom`、`WorkroomId`、`WorkroomThread`、`WorkroomEvent`、`WorkroomLink`、`ExternalThreadRef`、`AgentAttribution` |
| `codewhale-app-server` | 未来端点：`GET /workrooms`、`GET /workroom/:id/threads`、`GET /workroom/resolve` |
| `codewhale-tui` | 未来面向模型的链接解析和可选侧栏收件箱 |
| `codewhale-state` | 未来：持久化 workroom 存储（阶段 2） |

## 阶段状态

| 阶段 | 功能 | 状态 |
|---|---|---|
| 1 | RFC 设计文档 | ✅ 完成 |
| 1 | 协议数据类型 | ✅ 完成（含测试） |
| 1 | App-server workroom 端点 | ⏳ 未开始 |
| 1 | `resolve_workroom_link` 工具 | ⏳ 未开始 |
| 1 | 安全模型文档 | ✅ 完成 |
| 1 | 架构文档 | ✅ 完成 |
| 2 | 持久化 workroom 状态存储 | ⏳ 未开始 |
| 2 | 移动端页面 workroom 收件箱 | ⏳ 未开始 |
| 2 | 聊天桥接事件集成 | ⏳ 未开始 |
| 2 | TUI 侧栏收件箱 | ⏳ 未开始 |
