# Chat Export

**Model:** deepseek-v4-pro
**Workspace:** D:\tmp\source\CodeWhale
**Date:** 2026-07-20 14:25:31

---

**You:**

我是一个rust新手，我需要你逐行解释crates\tui\src\core\engine.rs这个文件中handle_send_message函数（从2416行到2918行）

---

*Thinking:*

用户想要我逐行解释 `crates\tui\src\core\engine.rs` 文件中 `handle_send_message` 函数，从第2416行到第2918行。这是一个教学性质的请求。

让我先读取这个文件的相关部分。

---

**Assistant:**

好的，让我先读取这个函数的完整代码。

---

**Tool:**

• ▷ read done · Reading crates/tui/src/core/engine.rs
▏ done: Reading crates/tui/src/core/engine.rs

---

*Thinking:*

文件被截断了。让我继续读取剩余部分。

---

**Assistant:**

文件被截断了，让我继续读取剩余部分：

---

**Tool:**

• ▷ read done · Reading crates/tui/src/core/engine.rs
▏ done: Reading crates/tui/src/core/engine.rs

---

*Thinking:*

好的，现在我有了 handle_send_message 函数的完整代码（从 2416 到 2918 行）。但这个函数内部还有很多代码，我需要继续读取中间部分的代码。让我看看中间截断的部分。

我已经有了：
- 2416-2915：第一部分
- 2916-3415：从 2916 往后

但我注意到 2916 行附近是 `handle_send_message` 函数结束的地方（2918行是 `}`），之后是 `handle_manual_compaction` 函数。然而中间有一些代码被截断了（在第一个结果中标记为 "truncated 8092 chars from middle"）。

让我重新读取中间被截断的部分。

---

**Assistant:**

中间有截断，让我再精确读取缺失的部分：

---

**Tool:**

• ▷ read done · 2 items
  ●●  2 done, 0 running
▏ done: Reading crates/tui/src/core/engine.rs
▏ done: Reading crates/tui/src/core/engine.rs

---

*Thinking:*

继续读取后续部分。

---

*Thinking:*

Now I have the complete function from 2416 to 2918. Let me read the earlier part that was also truncated to make sure I have everything. Let me also read the part that was truncated from the first read (around lines 2492-2510).

---

**Assistant:**

让我再获取第一个截断段中缺失的部分：

---

**Tool:**

• ▷ read done · 2 items
  ●●  2 done, 0 running
▏ done: Reading crates/tui/src/core/engine.rs
▏ done: Reading crates/tui/src/core/engine.rs

---

*Thinking:*

Now let me also read lines 2590-2680 which were in the middle truncation of the second read.

---

*Thinking:*

Now I have the complete function from 2416 to 2918. Let me organize this and provide a line-by-line explanation.

Here's the complete structure:

**2416-2439**: 函数签名 - 接收大量参数
**2440-2452**: 计算输入策略并发送状态事件
**2453-2454**: 重置取消令牌
**2456-2459**: 应用运行时模式策略
**2461-2462**: 排空旧的转向消息
**2464-2466**: 创建 TurnContext，递增 turn_counter
**2468-2476**: 立即发送 TurnStarted 事件
**2478-2495**: （如果启用）拍摄工作区快照
**2497-2501**: 清除重试状态
**2503-2505**: 克隆用户提示用于快照标签
**2507-2527**: 检查 API 客户端，如果路由激活失败则返回错误
**2529-2550**: 如果 API 客户端为 None，返回错误
**2552-2555**: 观察用户消息，判断是否需强制更新计划
**2557-2566**: 构造并添加用户消息到会话
**2568-2588**: 同步 goal 状态
**2589-2596**: 更新各种配置字段
**2598-2600**: 刷新系统提示词，发送会话更新事件
**2602-2614**: 构建工具注册表构建器
**2616-2637**: 准备子代理 fork 上下文
**2639-2683**: 创建子代理邮箱（mailbox）和 drainer 任务
**2685-2689**: 获取 MCP 连接池
**2691-2757**: 构建完整的工具注册表（含子代理运行时）
**2759-2766**: 加载插件工具
**2768-2772**: 获取 MCP 工具
**2773-2800**: 构建模型工具目录
**2801-2805**: 保存工具目录和 base_url 用于事件
**2807-2836**: 主 turn 循环（带 panic 捕获）
**2837-2840**: 更新会话用量统计
**2842-2854**: 发送 TurnComplete 事件
**2856-2874**: 拍摄事后快照
**2876-2917**: 跨轮次 goal 持续执行

Now let me write the Chinese explanation.

---

**Assistant:**

现在我已经有了 `handle_send_message` 函数的完整代码（2416-2918行）。让我为你逐段进行详细解释。

---

## `handle_send_message` 函数逐行解析

这是整个 CodeWhale 引擎中**最核心**的函数——它负责处理每一次用户发送消息的完整生命周期。当你按下回车键发送消息那一刻起，直到 AI 回复完成，这中间所有的流程都由这个函数编排。

---

### 第 1 段：函数签名（2416-2439）

```rust
async fn handle_send_message(
    &mut self,
    content: String,                   // 用户输入的文本内容
    mode: AppMode,                     // 当前模式：Agent / Plan / Yolo
    provider: Option<ApiProvider>,     // API 供应商（如 DeepSeek、OpenAI）
    model: String,                     // 模型名称
    goal_objective: Option<String>,    // Goal 目标描述（如果有）
    goal_token_budget: Option<u32>,    // Goal 的 token 预算上限
    goal_status: GoalStatus,           // Goal 状态：Active / Paused / Completed
    reasoning_effort: Option<String>,  // 推理深度设置
    reasoning_effort_auto: bool,       // 是否自动选择推理深度
    auto_model: bool,                  // 是否自动选择模型
    allow_shell: bool,                 // 是否允许执行 shell 命令
    trust_mode: bool,                  // 是否开启信任模式
    auto_approve: bool,                // 是否自动批准所有操作
    approval_mode: ApprovalMode,       // 批准模式
    translation_enabled: bool,         // 是否启用翻译
    show_thinking: bool,               // 是否显示思考过程
    allowed_tools: Option<Vec<String>>,// 允许使用的工具白名单
    dynamic_tools: Vec<DynamicToolSpec>,// 动态注册的工具
    hook_executor: Option<Arc<HookExecutor>>, // Hook 执行器
    verbosity: Option<String>,         // 详细程度
    provenance: UserInputProvenance,   // 输入来源（用户手动 / 运行时自动）
)
```

**解读**：这是一个异步方法（`async fn`），接收 `&mut self`，意味着它会修改引擎的内部状态。参数非常多——涵盖了本次对话轮次的所有配置：从用户输入内容、使用的模型、安全策略（shell/trust/approval），到工具白名单、hook 系统等。`provenance` 很重要：它区分了"用户手动发送"和"Goal 自动继续"两种场景。

---

### 第 2 段：计算有效输入策略（2440-2452）

```rust
// 2440: 调用辅助函数，综合所有参数计算出一个策略对象
let input_policy = effective_input_policy(
    provenance,                                       // 输入来源
    mode,                                             // 当前模式
    &content,                                         // 用户内容
    allow_shell,                                      // 是否允许 shell
    trust_mode,                                       // 信任模式
    mode == AppMode::Yolo || auto_approve,            // 是否自动批准
    approval_mode,                                    // 批准模式
);
// 2450: 如果策略中包含状态消息，发送到事件通道
if let Some(status) = input_policy.status.clone() {
    let _ = self.tx_event.send(Event::status(status)).await;
}
```

**解读**：`effective_input_policy` 是一个纯函数，把所有零散的输入参数"熔炼"成一个统一的 `InputPolicy` 对象。这个策略决定了本轮对话中：
- 工具是否需要审批
- shell 命令是否受限制
- 当前处于什么模式

如果策略中包含了要给用户看的状态消息（比如"已切换到 Yolo 模式"），就立即通过事件通道发送出去。

---

### 第 3 段：重置取消令牌（2453-2454）

```rust
// 2453: 为新一轮对话重置取消令牌（防止上一轮被取消的状态残留）
self.reset_cancel_token();
```

**解读**：这是非常关键的一行。`cancel_token` 是 tokio 的取消机制——当用户按 Esc 键时，上一轮的 `cancel_token` 会被触发，导致正在进行的 API 调用被取消。新轮次开始时必须创建一个全新的 token，否则新轮次一启动就会立即被"取消"。

---

### 第 4 段：应用模式策略 + 排空旧消息（2456-2462）

```rust
// 2456-2459: 记录完整的有效模式策略，防止 UI 切换模式后元数据读取到过期策略
self.apply_runtime_mode_policy(&input_policy);

// 2461-2462: 排空上一轮残留的转向消息
while self.rx_steer.try_recv().is_empty() {}
```

**解读**：
- `apply_runtime_mode_policy` 在注释中特别提到了 #3568 号 bug——曾经 UI 切换模式后，引擎内部还在用旧的策略，导致审批门控读到了过期的状态。这个方法就是修复。
- `rx_steer` 是一个用于运行时转向（steer）的通道——比如用户在 AI 回复过程中注入新的指令。新轮次开始前先把旧消息清空，防止污染。

---

### 第 5 段：创建 Turn 上下文 + 立即发送开始事件（2464-2476）

```rust
// 2464-2465: 创建 TurnContext，包含一个唯一的 turn id
let mut turn = TurnContext::new(self.config.max_steps);
// 2466: 递增 turn 计数器
self.turn_counter = self.turn_counter.saturating_add(1);

// 2468-2476: 立即发送 TurnStarted 事件——让 UI 知道回合已激活
let _ = self
    .tx_event
    .send(Event::TurnStarted {
        turn_id: turn.id.clone(),
    })
    .await;
```

**解读**：这段代码有一个重要的设计决策——**先发事件，再做快照**。注释中解释得很清楚：在 WSL2 等慢文件系统上，快照可能需要 30+ 秒，如果等快照完成再发 `TurnStarted`，UI 会长时间卡在"等待中"的状态。所以引擎先告诉 UI"我开始了"，然后再慢慢做快照。

---

### 第 6 段：拍摄事前快照（2478-2495）

```rust
// 2478-2481: 在操作任何工具之前，先拍工作区快照
if self.config.snapshots_enabled {
    // 2482-2487: 克隆用户提示用于快照标签
    let snapshot_prompt = content.clone();
    let pre_workspace = self.session.workspace.clone();
    let pre_seq = self.turn_counter;
    let pre_cap = self.config.snapshots_max_workspace_bytes;
    // 2491-2494: 在阻塞线程池上执行 git 工作，保持异步运行时响应
    let _ = tokio::task::spawn_blocking(move || {
        pre_turn_snapshot(&pre_workspace, pre_seq, pre_cap, Some(&snapshot_prompt))
    })
    .await;
}
```

**解读**：
- **快照是什么**：用 git 记录工作区在 AI 操作前的状态，之后如果 AI 搞砸了，用户可以用 `/restore` 恢复。
- **为什么用 `spawn_blocking`**：`pre_turn_snapshot` 内部会执行 git 命令（`git add`、`git commit` 等），这些是同步阻塞操作。如果直接在异步运行时上跑，会阻塞整个事件循环。`spawn_blocking` 把它们放到专用的阻塞线程池上，保持异步运行时对其他事件（比如 UI 重绘）的响应。
- 注意这里用了 `.await`——等快照完成后才继续。

---

### 第 7 段：清除重试状态 + 克隆快照标签（2497-2505）

```rust
// 2497-2501: 新回合清除上一轮的失败重试横幅
crate::retry_status::clear();

// 2503-2505: 在 content 被移动之前先克隆一份，用于事后快照标签
let snapshot_prompt_post = content.clone();
```

**解读**：
- `retry_status::clear()` 确保 UI 底部的"上次操作失败，正在重试..."横幅不会跨轮次残留。
- `snapshot_prompt_post` 是事后快照的标签——因为 `content` 马上会被移动到消息构造中（`move` 语义），所以这里提前克隆一份。

---

### 第 8 段：检查 API 客户端是否可用（2507-2550）

```rust
// 2507-2527: 如果指定了 provider，尝试激活路由；失败则发出错误并返回
if let Some(provider) = provider
    && let Err(message) = self.activate_runtime_route(provider, &model)
{
    self.deepseek_client_error = Some(message.clone());
    let _ = self
        .tx_event
        .send(Event::error(ErrorEnvelope::fatal_auth(message.clone())))
        .await;
    let _ = self
        .tx_event
        .send(Event::TurnComplete {
            usage: turn.usage.clone(),
            status: TurnOutcomeStatus::Failed,
            error: Some(message),
            tool_catalog: None,
            base_url: None,
        })
        .await;
    return;  // ← 提前返回！
}

// 2529-2550: 如果 API 客户端为 None，同样返回错误
if self.deepseek_client.is_none() {
    let message = /* 从 deepseek_client_error 取错误信息，或默认文本 */;
    // 同样发送 Error 和 TurnComplete 事件，然后 return
}
```

**解读**：这是**第一道防线**。如果 API 密钥无效、网络不通、或者模型配置错误，这里就会拦截，不会继续执行后续逻辑。注意返回时一定要发送 `TurnComplete` 事件——UI 在等着这个事件来解除"加载中"状态。**注释中特意提到 `&&` 操作符是 Rust 的"if-let 链"语法（RFC 2497），用来在一个 if 中同时做模式匹配和条件检查。**

---

### 第 9 段：观察用户消息 + 判断是否需要强制更新计划（2552-2555）

```rust
// 2552-2554: 让 working_set 观察用户消息（用于后续的文件推荐、上下文管理）
self.session
    .working_set
    .observe_user_message(&content, &self.session.workspace);
// 2555: 判断是否需要先更新计划再执行
let force_update_plan_first = should_force_update_plan_first(input_policy.mode, &content);
```

**解读**：
- `working_set.observe_user_message` 是上下文智能的一部分——它会分析用户消息中提到哪些文件、哪些路径，之后优先把这些内容保持在上下文中。
- `should_force_update_plan_first` 检查用户消息是否包含 `/plan` 或类似的需要先创建/更新计划的指令。

---

### 第 10 段：构造并添加用户消息（2557-2566）

```rust
// 2558-2565: 构造带有 turn 元数据的用户消息
let user_msg = self.user_text_message_with_turn_metadata_for_route_and_provenance(
    content,                      // 用户输入内容（这里被 move 了）
    &model,                       // 模型名
    auto_model,                   // 是否自动选模型
    reasoning_effort.as_deref(),  // 推理深度
    reasoning_effort_auto,        // 是否自动推理深度
    provenance,                   // 输入来源
);
// 2566: 添加到会话消息列表
self.session.add_message(user_msg);
```

**解读**：这里 `content` 被**移动**（move）到了消息构造中，之后就不能再使用了——这也是为什么前面需要 `snapshot_prompt_post = content.clone()`。构造出的消息包含了 turn 元数据（turn_id、时间戳等），这些信息用于后续的会话持久化和上下文管理。

---

### 第 11 段：同步 Goal 状态（2568-2588）

```rust
// 2568-2570: 保存旧的 goal 配置，用于比较是否有变化
let previous_goal_objective = self.config.goal_objective.clone();
let previous_goal_token_budget = self.config.goal_token_budget;
let previous_goal_status = self.config.goal_status;

// 2572-2576: 更新当前 model 和 goal 配置
self.session.model = model;
self.config.model.clone_from(&self.session.model);
self.config.goal_objective = goal_objective.clone();
self.config.goal_token_budget = goal_token_budget;
self.config.goal_status = goal_status;

// 2577-2588: 如果 goal 发生了变化，同步到持久化的 goal_state
if normalized_goal_objective(previous_goal_objective.as_deref())
    != normalized_goal_objective(goal_objective.as_deref())
    || previous_goal_token_budget != goal_token_budget
    || previous_goal_status != goal_status
{
    sync_goal_state_from_host(/* ... */);
}
```

**解读**：Goal 是 CodeWhale 的"持久化任务"系统——用户可以设置一个目标，引擎会跨多个轮次持续工作直到完成。这段代码在每次新轮次开始时检查 goal 是否变化（比如用户修改了目标、调整了预算、暂停了任务），如果变化了就同步到持久化存储。`normalized_goal_objective` 做规范化处理，避免仅空格差异触发不必要的同步。

---

### 第 12 段：更新会话配置（2589-2600）

```rust
// 2589-2596: 把本轮传入的各种配置写入 session 和 config
self.config.allowed_tools = allowed_tools;
self.config.hook_executor = hook_executor;
self.session.reasoning_effort = reasoning_effort;
self.session.reasoning_effort_auto = reasoning_effort_auto;
self.session.auto_model = auto_model;
self.config.translation_enabled = translation_enabled;
self.config.show_thinking = show_thinking;
self.config.verbosity = verbosity;

// 2598-2600: 刷新系统提示词（因为配置变了），通知 UI 会话已更新
self.refresh_system_prompt();
self.emit_session_updated().await;
```

**解读**：这里把函数参数中"会话级"的配置同步到引擎的内部状态中。`refresh_system_prompt()` 会根据新的配置重新组装系统提示词（包括用户自定义的 constitution、memory 等），然后 `emit_session_updated()` 通知 UI 刷新显示（比如会话列表、token 使用量等）。

---

### 第 13 段：构建工具注册表构建器（2602-2614）

```rust
// 2602-2604: 克隆 todo_list 和 plan_state（都是 Arc，克隆很便宜）
let todo_list = self.config.todos.clone();
let plan_state = self.config.plan_state.clone();

// 2606: 构建工具上下文（包含工作区路径、shell 策略等）
let tool_context = self.build_tool_context(input_policy.mode, input_policy.auto_approve);

// 2607-2611: 确保 MCP 连接池已初始化
if self.config.features.enabled(Feature::Mcp) {
    let _ = self.ensure_mcp_pool().await;
}

// 2612-2614: 创建工具注册表构建器
let builder = self
    .build_turn_tool_registry_builder(input_policy.mode, todo_list, plan_state)
    .with_dynamic_tools(&dynamic_tools);
```

**解读**：这段代码为后续的"工具调用"做准备：
- `tool_context` 包含了工具执行所需的环境信息
- MCP（Model Context Protocol）连接池用于管理外部工具服务器
- `build_turn_tool_registry_builder` 创建构建器，根据当前模式（Agent/Plan/Yolo）决定哪些工具可用，并注入动态工具

---

### 第 14 段：准备子代理 fork 上下文（2616-2637）

```rust
// 2616-2617: 检查子代理功能是否启用
let subagents_available =
    self.config.subagents_enabled && self.config.features.enabled(Feature::Subagents);

// 2619-2637: 如果子代理可用，捕获当前会话状态用于 fork 新子代理
let fork_context_for_runtime = if subagents_available {
    let state = StructuredState::capture(
        input_policy.mode.label(),
        self.config.workspace.clone(),
        std::env::current_dir().ok(),
        &self.session.working_set,
        &self.config.todos,
        &self.config.plan_state,
        Some(&self.subagent_manager),
    )
    .await;
    Some(SubAgentForkContext {
        system: self.session.system_prompt.clone(),
        messages: self.messages_with_turn_metadata(),
        structured_state_block: state.to_system_block(),
    })
} else {
    None
};
```

**解读**：子代理（SubAgent）是 CodeWhale 的并行任务执行能力。当用户说"同时帮我改三个文件"时，引擎会 fork 出三个子代理，每个专注一个文件。`SubAgentForkContext` 是子代理的"初始上下文"——包含系统提示词、历史消息、工作区状态等。子代理拿到这些后就可以独立工作了。

---

### 第 15 段：创建子代理邮箱（2639-2683）

```rust
// 2639-2683: 为子代理创建邮箱系统，用于子代理向父代理发送结构化消息
let mailbox_for_runtime = if subagents_available {
    let cancel_token = self.cancel_token.child_token();  // 子令牌，父被取消时子也取消
    let (mailbox, mut receiver) = Mailbox::new(cancel_token.clone());
    let tx_event_clone = self.tx_event.clone();
    // 2648-2679: spawn 一个 drainer 任务，持续接收子代理消息并转为 UI 事件
    spawn_supervised(
        "subagent-mailbox-drainer",
        std::panic::Location::caller(),
        async move {
            let mut best_effort_sent_at: HashMap<String, Instant> = HashMap::new();
            while let Some(envelope) = receiver.recv().await {
                let event = Event::SubAgentMailbox {
                    seq: envelope.seq,
                    message: envelope.message,
                };
                // 2658-2673: 对"尽力而为"类型消息做频率限制
                if /* 是 best_effort 消息 */ {
                    if !subagent_mailbox_best_effort_send_permitted(/* ... */) {
                        continue;  // 频率过高，丢弃
                    }
                    // try_send：如果通道满了就丢弃（不阻塞）
                    match tx_event_clone.try_send(event) { /* ... */ }
                }
                // 2674-2676: 非 best_effort 消息，用 send（会等待）
                if tx_event_clone.send(event).await.is_err() {
                    break;
                }
            }
        },
    );
    Some((mailbox, cancel_token))
} else {
    None
};
```

**解读**：这是一个精巧的设计——子代理完成工作后需要向父代理报告，但不能直接写父代理的状态。于是通过**邮箱（Mailbox）**传递消息：
- 子代理写入 `mailbox`（sender 端）
- `drainer` 任务从 `receiver` 端读取，转为 `Event::SubAgentMailbox` 事件
- UI 收到事件后在对应位置渲染子代理结果卡片

消息还区分了"尽力而为"（best-effort）和"必须送达"两种——比如子代理的进度更新属于 best-effort，丢了无所谓；但最终结果必须送达。

---

### 第 16 段：获取 MCP 连接池（2685-2689）

```rust
let mcp_pool = if self.config.features.enabled(Feature::Mcp) {
    self.ensure_mcp_pool().await.ok()
} else {
    None
};
```

**解读**：MCP 连接池用于管理与外部 MCP 服务器的连接。这里获取一个可克隆的引用，后续传给子代理运行时，让子代理也能使用 MCP 工具。

---

### 第 17 段：构建完整工具注册表（2691-2757）

```rust
let mut tool_registry = if subagents_available {
    // 2692-2717: 创建子代理运行时配置
    let runtime = if let Some(client) = self.deepseek_client.clone() {
        let runtime_allow_shell =
            self.session.allow_shell && !matches!(input_policy.mode, AppMode::Plan);
        let runtime_shell_policy =
            shell_policy_for_mode(input_policy.mode, runtime_allow_shell);
        let mut rt = SubAgentRuntime::new(
            client,                                     // API 客户端
            self.session.model.clone(),                 // 模型
            tool_context.clone(),                       // 工具上下文
            runtime_allow_shell,                        // 是否允许 shell
            Some(self.tx_event.clone()),                // 事件发送通道
            Arc::clone(&self.subagent_manager),          // 子代理管理器
        )
        .with_role_models(self.subagent_role_models())  // 角色模型映射
        .with_api_config(self.api_config.clone())       // API 配置
        .with_fleet_roster(self.config.fleet_roster.clone()) // Fleet 名册
        .with_auto_model(self.session.auto_model)       // 自动模型选择
        .with_reasoning_effort(/* ... */)               // 推理深度
        .with_agent_tool_surface_options(/* ... */)     // 工具表面选项
        .with_max_spawn_depth(self.config.max_spawn_depth) // 最大嵌套深度
        .with_step_api_timeout(self.config.subagent_api_timeout) // API 超时
        .with_speech_output_dir(self.config.speech_output_dir.clone()) // 语音输出
        .with_mcp_pool(mcp_pool.clone())                // MCP 连接池
        .with_todos(self.config.todos.clone())          // Todo 列表
        .with_parent_completion_tx(/* ... */)           // 父完成通知
        /* 后续还有更多 with_ 链式调用 */
        // 2731-2733: 如果有 fork_context，注入
        if let Some(context) = fork_context_for_runtime.clone() {
            rt = rt.with_fork_context(context);
        }
        // 2734-2738: 注入邮箱和取消令牌
        if let Some((mailbox, cancel_token)) = mailbox_for_runtime.as_ref() {
            rt = rt.with_mailbox(mailbox.clone())
                   .with_cancel_token(cancel_token.clone());
        }
        Some(rt)
    } else {
        None
    };
    // 2743-2754: 用运行时构建带子代理工具的注册表
    if let Some(subagent_runtime) = runtime {
        Some(builder
            .with_subagent_tools(self.subagent_manager.clone(), subagent_runtime)
            .build(tool_context))
    } else {
        // 2749-2753: 没有 API 客户端则降级为基础工具集
        Some(builder.build(tool_context))
    }
} else {
    Some(builder.build(tool_context))  // 子代理禁用时只构建基础工具集
};
```

**解读**：这是函数中最长的一段。它构建了完整的工具注册表——也就是"AI 可以使用哪些工具"。关键点：
- **构建器模式**（Builder Pattern）：`SubAgentRuntime` 使用大量的 `with_xxx()` 方法链式配置，每个方法返回 `Self`，这是 Rust 中非常常见的模式。
- **`Arc` 共享所有权**：`tool_context`、`subagent_manager`、`mcp_pool` 等都用 `Arc` 包装，因为子代理和父代理需要共享同一份数据。
- **降级策略**：如果 API 客户端不可用（比如未配置），工具注册表仍然会构建，只是不包含子代理工具——引擎不会因为缺少子代理功能就崩溃。

---

### 第 18 段：加载插件和 MCP 工具（2759-2772）

```rust
// 2759-2766: 加载用户插件工具，配置覆盖优先
let mut plugin_tool_names: HashSet<String> = HashSet::new();
if let Some(ref mut tool_registry) = tool_registry {
    plugin_tool_names = configure_plugin_tools(tool_registry, self.config.tools.as_ref());
}

// 2768-2772: 获取 MCP 工具列表
let mcp_tools = if self.config.features.enabled(Feature::Mcp) {
    self.mcp_tools().await
} else {
    Vec::new()
};
```

**解读**：
- **插件工具**：用户可以把自己写的脚本放在 `~/.codewhale/tools/` 目录下，引擎会自动发现并注册。如果 `config.toml` 中有同名工具的手动配置，手动配置优先。
- **MCP 工具**：通过 MCP 协议连接的外部工具服务器提供的工具。

---

### 第 19 段：构建模型工具目录（2773-2805）

```rust
let tools = tool_registry.as_ref().map(|registry| {
    // 2774-2777: 解析模型的工具能力配置
    let capability = crate::model_profile::resolved_capability_profile(
        self.api_config.api_provider(),
        &self.config.model,
    );
    // 2778-2781: 如果启用 MCP，强制加载 start_mcp_server 工具
    let mut always_load = self.config.tools_always_load.clone();
    if self.config.features.enabled(Feature::Mcp) {
        always_load.insert("start_mcp_server".to_string());
    }
    // 2782-2788: 构建工具目录
    let mut catalog = build_model_tool_catalog_with_surface(
        registry.to_api_tools_with_cache(true),
        mcp_tools,
        input_policy.mode,
        &always_load,
        capability.tool_surface_budget,
    );
    // 2789-2793: 插件工具标记为不延迟加载
    for tool in &mut catalog {
        if plugin_tool_names.contains(&tool.name) {
            tool.defer_loading = Some(false);
        }
    }
    // 2794-2798: 根据白名单/黑名单过滤工具
    filter_tool_catalog_for_gates(
        &mut catalog,
        self.config.allowed_tools.as_deref(),
        self.config.disallowed_tools.as_deref(),
    );
    catalog
});

// 2801-2805: 保存工具目录副本，用于 TurnComplete 事件
let tool_catalog_for_event = tools.clone();
let base_url_for_event = self
    .deepseek_client
    .as_ref()
    .map(|client| client.base_url().to_string());
```

**解读**：
- **工具目录（Tool Catalog）**：这是最终发送给 AI 模型的工具列表。不同的模型有不同的工具承载能力（tool_surface_budget），有的模型只能承载 50 个工具定义，有的能承载 200 个。
- **延迟加载（defer_loading）**：对于大型工具（如某些 MCP 工具），可以先只发工具名和描述，等 AI 真正调用时再加载完整定义。但插件工具是用户明确配置的，不延迟。
- **门控过滤（filter_tool_catalog_for_gates）**：根据 `allowed_tools`（白名单）和 `disallowed_tools`（黑名单）筛掉不该出现的工具。
- 这里还保存了 `tool_catalog_for_event` 和 `base_url_for_event`，在 turn 结束时会随 `TurnComplete` 事件一起发出。

---

### 第 20 段：主 Turn 循环（带 panic 保护）（2807-2836）

```rust
// 2807-2809: 主 turn 循环。捕获 panic，防止引擎事件循环崩溃
use futures_util::FutureExt as _;
let turn_result = std::panic::AssertUnwindSafe(
    self.handle_deepseek_turn(     // ← 真正的 AI 对话循环
        &mut turn,
        tool_registry.as_ref(),
        tools,
        input_policy.mode,
        force_update_plan_first,
        input_policy.dynamic_active_tools,
    )
)
.catch_unwind()   // ← 捕获 panic！
.await;

// 2822-2836: 处理结果
let (status, error) = match turn_result {
    Ok(outcome) => outcome,        // 正常完成
    Err(panic) => {                // panic 了！
        let detail = crate::utils::panic_message(&*panic);
        crate::utils::record_caught_panic("engine-event-loop", &detail);
        (
            TurnOutcomeStatus::Failed,
            Some(format!(
                "The engine hit an internal error and stopped this turn: {detail}. \
                 Your session is intact — send your message again to retry. \
                 A crash report was saved to ~/.codewhale/crashes/."
            )),
        )
    }
};
```

**解读**：这是函数的**核心**——`handle_deepseek_turn` 才是真正执行 AI 对话循环的地方（发送 prompt、接收回复、执行工具调用、再发送结果...如此循环直到 AI 完成或达到 `max_steps`）。

但更重要的是外层的 `catch_unwind`。在 Rust 中，`panic` 会展开（unwind）调用栈。如果不加保护，引擎内部的 panic 会直接杀死整个事件循环任务，导致 UI 卡死。注释中提到的 #2583 和 #1269 就是这样的 bug。现在用 `AssertUnwindSafe` + `catch_unwind` 包裹后，即使内部 panic，引擎也能优雅地返回一个 `TurnOutcomeStatus::Failed`，并保存崩溃报告。

---

### 第 21 段：更新用量统计（2837-2841）

```rust
// 2838-2839: 把本轮 token 用量累加到会话总用量
self.session.total_usage.add(&turn.usage);
// 2840: 记录 goal 的用量
self.record_goal_usage_for_turn(&turn.usage, turn.elapsed());
```

**解读**：token 用量追踪——`turn.usage` 包含了本轮消耗的 input_tokens 和 output_tokens。累加到 `total_usage` 后，UI 就能显示"本次会话已花费 $X.XX"。goal 用量用于跟踪 `/goal` 任务是否超出 token 预算。

---

### 第 22 段：发送 TurnComplete 事件（2842-2854）

```rust
// 2842-2844: 先发送 goal 更新事件
self.emit_goal_updated().await;

// 2845-2854: 发送 TurnComplete 事件——这是 UI 解除"加载中"的信号
let _ = self
    .tx_event
    .send(Event::TurnComplete {
        usage: turn.usage,
        status,                     // Completed / Failed / Interrupted
        error,                      // 如果有错误，包含错误信息
        tool_catalog: tool_catalog_for_event,
        base_url: base_url_for_event,
    })
    .await;
```

**解读**：`TurnComplete` 是整个函数最重要的输出事件。UI 收到它后会：
1. 停止加载动画
2. 显示 token 用量和费用
3. 如果失败，显示错误信息
4. 重新启用输入框

---

### 第 23 段：事后快照（2856-2874）

```rust
// 2856-2874: 发送后快照（fire-and-forget，不阻塞）
if self.config.snapshots_enabled {
    let post_workspace = self.session.workspace.clone();
    let post_seq = self.turn_counter;
    let post_cap = self.config.snapshots_max_workspace_bytes;
    crate::utils::spawn_blocking_supervised("post-turn-snapshot", move || {
        post_turn_snapshot(
            &post_workspace,
            post_seq,
            post_cap,
            Some(&snapshot_prompt_post),
        );
    });
}
```

**解读**：与事前快照不同，事后快照使用 `spawn_blocking_supervised`（注意没有 `.await`）——**fire-and-forget**。因为此时 `TurnComplete` 已经发出，UI 已经解除了阻塞，没必要让用户等待快照完成。如果快照失败，有 supervised 任务管理器会记录日志。

---

### 第 24 段：跨轮次 Goal 延续（2876-2917）

```rust
// 2876-2882: 如果本轮成功完成，且 goal 仍然是 Active 状态，
// 且未超出预算，则自动发起下一轮
if status == TurnOutcomeStatus::Completed
    && let Some(continuation) = self.goal_continuation_if_active()
{
    // 2887-2890: 使用与本轮相同的配置重新派发消息
    let _ = self
        .tx_op
        .send(Op::SendMessage {
            content: continuation,                       // 继续提示（如"继续完成目标"）
            mode,                                        // 沿用相同模式
            provider,
            model: self.session.model.clone(),
            goal_objective: None,                        // ← 注意：传 None，防止无限嵌套
            goal_token_budget: None,
            goal_status: GoalStatus::Active,             // ← 保持 Active
            reasoning_effort: self.session.reasoning_effort.clone(),
            reasoning_effort_auto,
            auto_model,
            allow_shell,
            trust_mode,
            auto_approve,
            approval_mode,
            translation_enabled,
            show_thinking,
            allowed_tools: self.config.allowed_tools.clone(),
            dynamic_tools: dynamic_tools.clone(),
            hook_executor: self.config.hook_executor.clone(),
            verbosity: self.config.verbosity.clone(),
            provenance: UserInputProvenance::Runtime,    // ← 标记为"运行时发起"
        })
        .await;
}
// 2918: 函数结束
```

**解读**：这就是 **Goal 自动循环**机制。当用户通过 `/goal "重构整个项目"` 设置了一个目标后：

1. 第一轮执行完成
2. 引擎检查 goal 是否还是 Active（用户没暂停）+ 是否在预算内
3. 如果条件满足，自动通过 `tx_op` 通道发送一个新的 `Op::SendMessage`
4. 引擎的 op 处理循环收到后，会**再次调用 `handle_send_message`**
5. 如此循环直到 AI 自我报告完成、用户暂停、或预算用完

注意几个细节：
- `goal_objective: None`：不重复传目标，避免无限嵌套
- `provenance: UserInputProvenance::Runtime`：标记为运行时发起，这样 UI 可以区分"用户手动发的"和"自动继续的"
- 注释强调"Failed 或 Interrupted 的 turn 不会继续"——用户按 Esc 中断后，goal 循环就停了

---

## 总结

`handle_send_message` 是整个 CodeWhale 引擎的**中央调度器**，它的职责可以概括为：

| 阶段 | 行号 | 核心工作 |
|------|------|----------|
| **准备** | 2440-2505 | 计算策略、重置令牌、发送 TurnStarted、拍快照 |
| **验证** | 2507-2550 | 检查 API 客户端是否可用 |
| **组装** | 2552-2600 | 构造用户消息、同步 goal、更新会话配置 |
| **构建工具** | 2602-2805 | 构建工具注册表、子代理运行时、MCP、插件、工具目录 |
| **执行** | 2807-2836 | 调用 `handle_deepseek_turn`（带 panic 保护） |
| **收尾** | 2837-2874 | 更新用量、发送 TurnComplete、拍事后快照 |
| **延续** | 2876-2917 | Goal 自动循环（如果适用） |

这个函数体现了 CodeWhale 的多个设计哲学：
- **防御性编程**：每个可能失败的地方都有提前返回 + 错误事件
- **事件驱动架构**：引擎不直接操作 UI，而是通过 `tx_event` 发送事件
- **fire-and-forget**：不阻塞的操作用 spawn 而非 await
- **优雅降级**：子代理不可用时仍能正常工作

---

