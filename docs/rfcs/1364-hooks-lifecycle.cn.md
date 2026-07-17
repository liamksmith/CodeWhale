# RFC：Hook 生命周期数据流

**Issue：** #1364
**状态：** 草案
**日期：** 2026-05-28

## 1. 问题

CodeWhale 已经拥有生命周期 hook 和 MCP 支持，但当前的 hook 表面主要是仅观察者模式。这阻碍了需要参与代理数据流的可移植扩展：

- 在用户消息到达模型之前进行记忆/上下文注入
- 为下一轮准备上下文的事后轮次背景分析
- 用于编排和审计扩展的子代理生命周期可见性

当前的 `message_submit` 事件在分发之前触发，但其输出被忽略。`TurnComplete`、`AgentSpawned` 和 `AgentComplete` 内部存在，但它们不作为可配置的 hook 事件暴露。

## 2. PR 拆分

此 issue 应作为三个 PR 实现。每个 PR 应可独立审查，并且每个 PR 都应使 hook 系统处于有用状态。

### PR 1：可变的 `message_submit`

为 `message_submit` 添加结构化 hook 执行路径，可以在用户提交的文本发送到引擎之前转换或阻止它。

范围：

- 保持现有的 `[[hooks.hooks]]` 配置形态
- 通过 stdin 向 hook 传递 JSON 负载
- 将包含 `text` 的 stdout JSON 解释为替换用户文本
- 将退出码 `2` 视为有意阻止
- 按配置顺序串行运行多个提交 hook
- 保持现有环境变量以实现兼容性
- 保持 `shell_env` 标准输出解析不变

非目标：

- 不进行工具参数变更
- 不对所有 hook 事件使用全局 stdout JSON 语义
- 不进行转录或模型响应变更

### PR 2：`turn_end`

将现有的轮次完成生命周期暴露为 hook 事件。

范围：

- 添加事件名称为 `turn_end` 的 `HookEvent::TurnEnd`
- 在核心应用状态、用量、成本、通知和回执状态更新后，从 UI 的 `EngineEvent::TurnComplete` 分支触发
- 通过 stdin 以 JSON 形式传递轮次元数据
- 使失败非阻塞且仅警告
- 在负载中包含 `stop_hook_active` 字段，初始为 `false`，以便合约后续支持重入保护

非目标：

- 不更改轮次状态
- 不阻塞用户输入
- 不从 `turn_end` 进行转录变更

v0.9 分支的实现说明：窄范围的 #2578 收集使用为子代理生命周期 hook 引入的共享结构化观察者路径。它在排队后续分发之前触发，在队列恢复状态已知后触发，因此负载可以报告排队消息计数，而不会让 hook 更改接下来发送的内容。`turn_end` 忽略 stdout；只有 `message_submit` 具有 stdout 变更合约。

### PR 3：子代理生命周期观察者 hook

将子代理启动和完成暴露为仅观察者 hook 事件。

范围：

- 添加事件名称为 `subagent_spawn` 的 `HookEvent::SubagentSpawn`
- 添加事件名称为 `subagent_complete` 的 `HookEvent::SubagentComplete`
- 从现有的 `AgentSpawned` 和 `AgentComplete` UI 分支触发
- 通过 stdin 以 JSON 形式传递子代理元数据
- 使失败非阻塞且仅警告

非目标：

- 第一个版本不进行子代理生成门控
- 不进行子代理 prompt/结果变更
- 不更改子代理调度

## 3. PR 1 详细计划

### 3.1 合约

配置：

```toml
[[hooks.hooks]]
event = "message_submit"
command = "~/.deepseek/hooks/inject-memory.sh"
timeout_secs = 2
continue_on_error = true
```

stdin 上的输入负载：

```json
{
  "event": "message_submit",
  "text": "原始用户文本",
  "session_id": "sess_xxxx",
  "workspace": "/path/to/workspace",
  "mode": "agent",
  "model": "deepseek-chat",
  "total_tokens": 1234
}
```

stdout 上的输出负载：

```json
{ "text": "替换用户文本" }
```

规则：

- 退出 `0` 且 stdout JSON 包含 `text: string` 则替换当前文本
- 退出 `0` 且 stdout 为空则保持当前文本不变
- 退出 `0` 且 JSON 不包含 `text` 则保持当前文本不变
- 退出 `2` 则在消息追加到历史或发送到引擎之前阻止提交
- 其他非零退出遵循 `continue_on_error`
  - `true`：警告，保持当前文本，继续后续 hook
  - `false`：停止后续 hook 并用错误消息阻止提交
- `message_submit` 上的 `background = true` 保持仅观察者模式，不能转换或阻止提交

多个 hook：

- hook 按配置顺序运行
- 每个 hook 接收最新转换后的文本
- 最终转换后的文本是文件提及扩展、技能包装、自动路由、历史和 `api_messages` 使用的唯一文本

### 3.2 实现步骤

1. 在 `crates/tui/src/hooks.rs` 中添加结构化提交结果类型：

```rust
pub enum MessageSubmitOutcome {
    Unchanged,
    Replaced(String),
    Blocked { reason: String },
}
```

2. 添加一个支持 stdin 的同步执行器：

```rust
fn execute_sync_with_stdin(
    &self,
    hook: &Hook,
    env_vars: &HashMap<String, String>,
    stdin_json: &serde_json::Value,
) -> HookResult
```

这应该复用 `execute_sync` 现有的超时、工作目录、stdout、stderr 和错误处理行为。

3. 添加一个 `message_submit` 转换入口点：

```rust
pub fn execute_message_submit_transform(
    &self,
    context: &HookContext,
    original_text: &str,
) -> MessageSubmitOutcome
```

此方法应该：

- 通过现有条件匹配过滤配置的 `MessageSubmit` hook
- 使用当前文本为每个 hook 构建 JSON 负载
- 通过 `execute_sync_with_stdin` 运行非后台 hook
- 使用现有的仅观察者路径运行后台 hook
- 仅为非后台 hook 解析 stdout JSON
- 返回最终文本或阻止结果

4. 在 `dispatch_user_message` 中应用转换后的消息：

- 在 `last_submitted_prompt`、文件提及、历史和 `api_messages` 之前运行转换
- 创建本地可变的 `QueuedMessage` 或替换显示文本
- 如果被阻止，显示状态消息或 toast 并不进行分发就返回

5. 更新 `/hooks events`：

- 保持 `message_submit` 列出
- 更新描述说明它可以转换或阻止用户文本

6. 更新面向用户的文档：

- 文档化 stdin/stdout 合约
- 文档化退出码 `2`
- 文档化 `shell_env` 仍使用 `KEY=VALUE` stdout

### 3.3 测试计划

`crates/tui/src/hooks.rs` 中的单元测试：

- 将 stdout `{"text":"changed"}` 解析为替换
- 空 stdout 表示不变
- 不含 `text` 的 JSON 表示不变
- 格式错误的 stdout 表示不变，具有警告语义
- 退出 `2` 映射为阻止
- 多个 hook 按顺序应用转换
- 后台 `message_submit` hook 不能转换
- `continue_on_error = false` 在非零失败时阻止

TUI 集成或聚焦分发测试：

- 转换后的文本写入 `api_messages`
- 转换后的文本写入可见历史
- 转换后的文本被文件提及扩展使用
- 被阻止的提交不追加用户历史
- 被阻止的提交不推送 API 消息
- 被阻止的提交保持加载状态为 false

手动冒烟测试：

1. 添加一个配置 hook，在每个提交的消息前添加 `[hooked] `。
2. 提交 `hello`。
3. 验证转录和模型输入使用 `[hooked] hello`。
4. 将 hook 替换为退出 `2` 的 hook。
5. 提交 `hello`。
6. 验证没有轮次开始，且 TUI 显示阻止原因。

## 4. 共享负载约定

所有新的结构化 hook 负载应包括：

- `event`
- `session_id`
- `workspace`
- `mode`
- `model`

事件特定的负载应仅添加对扩展作者稳定且有用的字段。避免在第一个版本中泄露密钥、完整工具输出或无界转录内容。

## 5. 兼容性

- 现有 hook 配置保持有效。
- 现有仅观察者 hook 继续工作。
- 现有环境变量保持可用。
- `shell_env` 保持其现有的 stdout `KEY=VALUE` 合约。
- 结构化 stdout 仅在 PR 1 中由 `message_submit` 解释。结构化观察者 hook（如 `turn_end`、`subagent_spawn` 和 `subagent_complete`）在 stdin 上接收 JSON，但其 stdout 被调用方忽略。

## 6. 审查检查点

PR 1 仅在以下条件下接受：

- 提交变更被测试覆盖
- 提交阻止被测试覆盖
- 不变路径保持当前行为
- `shell_env` 测试仍证明旧的 stdout 合约
- 文档清楚地将 `message_submit` 标记为唯一可变 hook

PR 2 仅在以下条件下接受：

- `turn_end` 在 `TurnComplete` 应用状态更新后触发
- 失败仅为警告
- 负载包含状态和用量

PR 3 仅在以下条件下接受：

- 子代理 hook 仅观察者
- 失败不影响子代理生命周期
- 负载不包含无界或秘密数据
