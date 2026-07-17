# Workroom 安全模型

## 范围

本文档涵盖 CodeWhale Workroom 的安全边界——在 [RFC 3209](rfcs/3209-workrooms.md) 中描述的用于线程化智能体对话的持久化、可寻址容器。

Workroom **不**引入任何新的网络服务、云依赖或默认开启的公开分享。安全责任由控制 Runtime API 的操作者承担。

本文档描述 v0.9 workroom 界面预期的安全契约。在 v0.8.62 中，仅协议数据类型和链接解析已落地。持久化状态、Runtime API 端点、token 作用域、事件存储以及模型可见的链接解析仍为后续工作。

## 原则

1. **本地优先。** 未来持久化的 workroom 状态应存放在 CodeWhale 主目录下，受用户独占的文件系统权限保护。无云同步、无遥测、无第三方托管。

2. **链接不含密钥。** `codewhale://workroom/wr_...` URL 仅包含不透明 UUID。它们不携带 API 密钥、bearer token、密码或文件路径。仅有 workroom 链接的攻击者在没有 Runtime API 访问权限的情况下无法做任何事。

3. **无公开读取路径。** 未来的 workroom 端点必须要求在 `Authorization` 头中提供有效的 bearer token。不应存在未经认证的 `/workroom/...` 路由。

4. **事件不含密钥。** `WorkroomEvent` 载荷绝不能包含 API 密钥、认证 token 或明文凭据。`ArtifactLinked` 事件类型引用的是文件路径，而非内容。事件用于索引/引用，而非回放智能体工具输出。

5. **分享是显式的。** workroom 默认为 `Private`。操作者可将其标记为 `Shared` 并列出允许的 bearer token。操作者控制哪些 token 被签发、轮换和撤销。

## 威胁模型

| 威胁 | 缓解措施 |
|---|---|
| 攻击者获得 workroom 链接 | 链接仅包含不透明 UUID；解析需要 Runtime API 认证 |
| 攻击者暴力枚举 workroom ID | UUID v4（`2^122` 空间）；未来 API 应在暴露查询面之前添加限速 |
| 攻击者注入恶意事件 | 未来事件写入应仅流经受信任的 Runtime 客户端 |
| 攻击者窃取 workroom 状态 | 未来文件系统状态应由操作系统用户权限和运行时认证把关 |
| Bearer token 泄露 | 操作者轮换 token；未来分享规则应可在不触及 workroom 状态的情况下撤销 |

## API 认证

未来的 workroom 端点应继承与其他受保护路由（`/thread`、`/app`、`/tool` 等）相同的认证中间件：

- 需要 `Authorization: Bearer <token>` 头
- Token 对照运行时配置的 bearer token 进行验证
- 缺失或无效时返回 401 Unauthorized

## 未来工作

| 项目 | 风险 | 状态 |
|---|---|---|
| 静态事件加密 | 若 workroom 转为多用户模型，属于阶段 2 范围 | 未实现 |
| 共享 workroom 的审计日志 | 若共享 token 跨操作者使用，则有用 | 未实现 |
| Token 作用域（读/写/管理） | 目前所有 token 拥有完全访问权限 | 未计划 |
