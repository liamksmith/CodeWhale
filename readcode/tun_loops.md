# Chat Export

**Model:** deepseek-v4-pro
**Workspace:** D:\tmp\source\CodeWhale
**Date:** 2026-07-20 23:28:23

---

*System:*

模型已切换：deepseek-v4-flash → deepseek-v4-pro

---

**You:**

我是rust新手，你来逐行分析一下crates\tui\src\core\engine\turn_loop.rs

---

*Thinking:*

用户是Rust新手，想要我逐行分析 `crates\tui\src\core\engine\turn_loop.rs` 这个文件。让我先读取这个文件，然后逐行分析。

---

**Assistant:**

好的！让我先把这个文件读出来。

---

**Tool:**

• ▷ read done · Reading crates/tui/src/core/engine/turn_loop.rs
▏ done: Reading crates/tui/src/core/engine/turn_loop.rs

---

*Thinking:*

文件很大，3807行。让我继续读取剩余部分。我需要分段读取，然后逐行分析。考虑到文件很大，我需要分多段读取。先从501行继续。

由于文件很大（3807行），全部逐行分析会非常长。让我先完整读完这个文件，然后给出一个结构化的分析，重点解释关键段落，而不是真的逐3807行去解释。

让我先读完整个文件再决定策略。

---

**Assistant:**

[interrupted] 文件有 3807 行，我先把它完整读完。

---

**You:**

我是rust新手，你来逐行分析一下crates\tui\src\core\engine\turn_loop.rs中的handle_deepseek_turn函数，它在文件中的253行到2646行。

---

*Thinking:*

用户要我逐行分析 `handle_deepseek_turn` 函数，从第253行到第2646行。这是一个非常大的函数（约2400行）。让我分段读取这个函数。

首先，让我从253行附近开始读取。

---

**Assistant:**

好的，让我分段读取这个函数。它很长（约2400行），我一段一段来。

---

**Tool:**

• ▷ read done · Reading crates/tui/src/core/engine/turn_loop.rs
▏ done: Reading crates/tui/src/core/engine/turn_loop.rs

---

*Thinking:*

继续读取。

---

**Assistant:**

继续读取：

---

**Tool:**

• ▷ read done · Reading crates/tui/src/core/engine/turn_loop.rs
▏ done: Reading crates/tui/src/core/engine/turn_loop.rs

---

*Thinking:*

继续读取，函数非常长。让我用更大的步长读取。

---

**Assistant:**

继续读取下一段：

---

**Tool:**

• ▷ read done · 10 items
  ●●●●●●●●●●  10 done, 0 running
▏ done: Reading crates/tui/src/core/engine/turn_loop.rs
▏ done: Reading crates/tui/src/core/engine/turn_loop.rs
▏ done: Reading crates/tui/src/core/engine/turn_loop.rs
▏ done: Reading crates/tui/src/core/engine/turn_loop.rs
▏ done: Reading crates/tui/src/core/engine/turn_loop.rs
▏ done: Reading crates/tui/src/core/engine/turn_loop.rs
▏ done: Reading crates/tui/src/core/engine/turn_loop.rs
▏ done: Reading crates/tui/src/core/engine/turn_loop.rs
▏ done: Reading crates/tui/src/core/engine/turn_loop.rs
▏ done: Reading crates/tui/src/core/engine/turn_loop.rs

---

*Thinking:*

继续读取。

---

*Thinking:*

继续。

---

*Thinking:*

继续读取。

---

*Thinking:*

继续。

---

*Thinking:*

继续。

---

*Thinking:*

继续。

---

*Thinking:*

继续读取，快到尾声了。

---

*Thinking:*

继续读取，接近尾声。

---

*Thinking:*

继续最后一段。

---

*Thinking:*

好的，现在我已经完整读取了 `handle_deepseek_turn` 函数（253-2646行）。这是一个非常长且复杂的函数。让我来逐段分析它给 Rust 新手看。

这是 CodeWhale TUI 引擎中"核心中的核心"——它是代理引擎的一个主要 turn 循环。我需要对 Rust 新手详细讲解这段代码，但也不要太过冗长。

让我组织一下分析方案：

1. 函数签名（253-261）
2. 初始化阶段（262-302）
3. 外层 loop（304-2637）
   - 取消检查 + steer 接收（305-327）
   - 子代理完成事件（329-334）
   - 系统提示刷新 + 步数检查（336-345）
   - 上下文压缩（347-430）
   - 上下文溢出恢复（432-461）
   - LSP 诊断（463-466）
   - 分层上下文检查点（468-471）
   - 请求构建（473-619）
   - 流式请求发送（621-656）
   - 流式事件处理（660-1137）
   - 透明重试（1139-1201）
   - 内容块构建（1203-1294）
   - 无工具调用分支（1296-1529）
   - 工具执行（1531-2606）
     - 审批流程
     - 并行/串行执行
     - 结果处理
   - 错误跟踪 + 步进（2604-2636）
4. 返回（2638-2646）

让我现在写出逐段分析。这是一篇很长的分析，我尽量讲清楚每个 Rust 语言特性的用法。

---

**Assistant:**

好的，我已经读取了完整的 `handle_deepseek_turn` 函数（253-2646行）。下面我逐段分析这个函数。

---

## `handle_deepseek_turn` 逐行分析

这是 CodeWhale TUI 代理引擎中**最核心的函数**——它负责执行一个完整的"回合"（turn）：请求模型 → 流式接收响应 → 解析工具调用 → 执行工具 → 把结果返回模型 → 循环往复，直到模型不再调用工具为止。

---

### 一、函数签名（253–261）

```rust
pub(super) async fn handle_deepseek_turn(
    &mut self,
    turn: &mut TurnContext,
    tool_registry: Option<&crate::tools::ToolRegistry>,
    tools: Option<Vec<Tool>>,
    mode: AppMode,
    force_update_plan_first: bool,
    dynamic_active_tools: Vec<&'static str>,
) -> (TurnOutcomeStatus, Option<String>) {
```

**Rust 知识点：**

| 语法 | 含义 |
|------|------|
| `pub(super)` | 只对**父模块**可见（不暴露给外部 crate） |
| `async fn` | 这是异步函数，调用者用 `.await` 等待它完成 |
| `&mut self` | 可变借用自身——可以修改 `Engine` 的字段 |
| `&mut TurnContext` | 可变借用 turn 上下文——可以修改步数、token 计数等 |
| `Option<&T>` | 可选引用。`Option<Vec<T>>` 是可能为空的 Vec |
| `Vec<&'static str>` | 包含**静态生命周期**字符串切片（编译时确定的字符串）的 Vec |
| `-> (A, B)` | 返回一个**元组**：状态码和可选的错误消息 |

**功能：** 这是整个 turn 循环的入口。参数含义：
- `self`：Engine 自身的可变引用
- `turn`：该回合的上下文（步数、token 用量等）
- `tool_registry`：可选的工具注册表（运行时工具集合）
- `tools`：可选的可用于当前回合的工具列表（从模型搜索得到）
- `mode`：当前应用模式（Agent / Plan / YOLO）
- `force_update_plan_first`：是否强制先调用 `update_plan`
- `dynamic_active_tools`：动态激活的额外工具

---

### 二、初始化阶段（262–302）

```rust
crate::tui::notifications::set_taskbar_progress_busy();
crate::tui::notifications::start_title_animation("CodeWhale");
```
通知任务栏：开始忙状态 + 标题栏动画。

```rust
let client = self
    .deepseek_client
    .clone()
    .expect("DeepSeek client should be configured");
```
**Rust 知识点：**
- `clone()`：DeepSeek API 客户端是一个可克隆对象，clone 得到独立副本（通常包含 `Arc` 内部共享状态）
- `expect(msg)`：`Option` 的方法——如果是 `Some` 取出值，如果是 `None` 则 panic 并显示 msg

```rust
let mut consecutive_tool_error_steps = 0u32;
let mut turn_error: Option<String> = None;
let mut context_recovery_attempts = 0u8;
let mut goal_continuations_this_turn = 0u32;
let mut stream_retry_attempts: u32 = 0;
```
**Rust 知识点：**
- `let mut`：可变变量绑定。Rust 默认变量不可变
- `0u32`：**类型后缀**——`u32` 是 32 位无符号整数
- `Option<String>`：Rust 没有 `null`，用 `Option` 表示"可能没有值"

```rust
let mut tool_catalog = tools.unwrap_or_default();
if !tool_catalog.is_empty() {
    ensure_advanced_tooling(&mut tool_catalog, mode, &self.config.tools_always_load);
}
```
**Rust 知识点：**
- `unwrap_or_default()`：如果是 `None`，返回 `Vec::new()`（即默认值）
- `!tool_catalog.is_empty()`：`!` 是逻辑非运算符
- `&mut tool_catalog`：传递**可变引用**，允许函数修改它的内容

```rust
let mut active_tool_names = initial_active_tools(&tool_catalog);
active_tool_names.extend(
    dynamic_active_tools
        .into_iter()
        .map(std::string::ToString::to_string),
);
```
**Rust 知识点：**
- `.into_iter()`：消耗 Vec 并返回迭代器（所有权转移）
- `.map(...)`：对每个元素应用转换函数
- `std::string::ToString::to_string`：将 `&str` 转换为 `String` 的方法引用
- `.extend(...)`：将迭代器中的所有元素追加到 Vec

---

### 三、外层主循环 `loop { ... }`（304–2637）

```rust
loop {
```
**Rust 知识点：**
- `loop { }` 是**无限循环**。退出靠 `break`、`return` 或 panic。没有条件判断。

#### 3.1 取消检查（305–308）

```rust
if self.cancel_token.is_cancelled() {
    let _ = self.tx_event.send(Event::status("Request cancelled")).await;
    return (TurnOutcomeStatus::Interrupted, None);
}
```
**Rust 知识点：**
- `let _ = ...`：丢弃返回值。Rust 编译器会警告未使用的 `Result`，`let _ =` 显式表示"我知道，有意丢弃"
- `.await`：异步等待。函数必须用 `async fn` 声明才能使用
- `return (A, B)`：提前从函数返回一个元组

#### 3.2 引导输入处理（310–327）

```rust
while let Ok(steer) = self.rx_steer.try_recv() {
```
**Rust 知识点：**
- `while let Ok(x) = ...`：模式匹配循环。只要 `try_recv()` 返回 `Ok`，就继续循环；返回 `Err` 时停止
- `try_recv()`：非阻塞接收——立即返回，不等待

```rust
let steer = steer.trim().to_string();
if steer.is_empty() {
    continue;
}
```
**Rust 知识点：**
- `trim()`：去除首尾空白，返回 `&str`
- `continue`：跳过当前循环迭代的剩余部分，进入下一次迭代

```rust
self.session
    .working_set
    .observe_user_message(&steer, &self.session.workspace);
```
**Rust 知识点：**
- 链式字段访问：`self.session.working_set` 访问嵌套结构体字段
- `&steer`：借用一个 `String` 的引用（`&String`），自动解引用为 `&str`

#### 3.3 子代理完成事件（329–334）

```rust
self.drain_subagent_completion_events("queued").await;
```
清空子代理的完成通知，让父代理能立即看到子代理的结果。

#### 3.4 系统提示刷新 + 步数检查（336–345）

```rust
self.refresh_system_prompt();

if turn.at_max_steps() {
    break;
}
```
**Rust 知识点：**
- `break`：跳出最内层循环（这里跳出外层 `loop`）

#### 3.5 上下文自动压缩（347–430）

```rust
if self.config.compaction.enabled
    && should_compact(
        &self.session.messages,
        &self.config.compaction,
        Some(&self.session.workspace),
        Some(&compaction_pins),
        Some(&compaction_paths),
    )
{
```
**Rust 知识点：**
- 多条件 `if`：`&&` 短路求值
- `Some(&...)`：构造 `Option<&T>`，传入可选引用

```rust
let compaction_id = format!("compact_{}", &uuid::Uuid::new_v4().to_string()[..8]);
```
**Rust 知识点：**
- `format!`：宏，类似于 `println!` 但返回 `String`
- `[..8]`：**字符串切片**——取前 8 个字节（Unicode 注意：不能切在多字节字符中间）

```rust
match compact_messages_safe(...).await {
    Ok(result) => { ... }
    Err(err) => { ... }
}
```
**Rust 知识点：**
- `match`：Rust 的模式匹配。每个分支称为一个"臂"（arm）
- `Ok(result)` 和 `Err(err)` 解构 `Result` 枚举

#### 3.6 上下文溢出恢复（432–461）

```rust
if let Some(input_budget) = context_input_budget_for_route(...) {
    let estimated_input = self.estimated_input_tokens();
    if estimated_input > input_budget {
        if context_recovery_attempts >= MAX_CONTEXT_RECOVERY_ATTEMPTS {
            ... return (TurnOutcomeStatus::Failed, turn_error);
        }
        if self.recover_context_overflow(...).await {
            context_recovery_attempts = context_recovery_attempts.saturating_add(1);
            continue;
        }
    }
}
```
**Rust 知识点：**
- `if let Some(x) = ...`：条件模式匹配——只在值是 `Some` 时执行体
- `saturating_add(1)`：**饱和加法**。达到最大值时不 panic，保持最大值不变
- `continue`：跳过当前迭代，回到 `loop` 开头——这就是"重试"的实现方式

---

### 四、请求构建（463–619）

```rust
let force_update_plan_this_step = force_update_plan_first && !turn.has_tool_calls();
```
**Rust 知识点：**
- `&&`：短路逻辑与

```rust
let mut active_tools = if tool_catalog.is_empty() {
    None
} else {
    Some(active_tools_for_step(...))
};
```
**Rust 知识点：**
- `if` 是**表达式**（不是语句），可以返回值。这里根据条件返回 `None` 或 `Some(...)`

```rust
if self.config.strict_tool_mode
    && let Some(tools) = active_tools.as_mut()
{
    crate::tools::schema_sanitize::prepare_tools_for_strict_mode(tools);
}
```
**Rust 知识点：**
- `if ... && let Some(x) = ...`：**let chains**（Rust 1.65+ 特性）。在 `if` 条件中嵌入模式匹配
- `as_mut()`：`Option` 的方法，返回 `Option<&mut T>`——可变引用版本

#### 前缀缓存稳定性检查（490–593）

```rust
if let Some(pm) = self.session.prefix_stability.as_mut() {
    match pm.check_and_update(&system_text, tools_ref) {
        Err(change) => { ... }
        Ok(_) => { ... }
    }
}
```
**Rust 知识点：**
- `as_mut()`：从 `&mut Option<T>` 得到 `Option<&mut T>`
- `match` 的 `Err(change)`——将错误值绑定到变量 `change`
- `Ok(_)`：`_` 是**通配符**，忽略匹配到的值

#### 构造 MessageRequest（595–619）

```rust
let request = MessageRequest {
    model: self.session.model.clone(),
    messages: self.messages_with_turn_metadata(),
    max_tokens: effective_max_output_tokens_for_route(...),
    system: self.session.system_prompt.clone(),
    tools: active_tools.clone(),
    tool_choice: if active_tools.is_some() {
        if self.config.strict_tool_mode {
            Some(json!("required"))
        } else {
            Some(json!({ "type": "auto" }))
        }
    } else {
        None
    },
    ...
};
```
**Rust 知识点：**
- 结构体字面量语法：`Struct { field1: value1, field2: value2, ... }`
- `json!()`：`serde_json` 宏，将 JSON 字面量转换为 `serde_json::Value`
- 嵌套 `if` 表达式——多层嵌套的表达式求值，最后返回一个值

---

### 五、流式请求发送（621–656）

```rust
let stream_result = tokio::select! {
    biased;
    () = self.cancel_token.cancelled() => {
        return (TurnOutcomeStatus::Interrupted, None);
    }
    result = client.create_message_stream(stream_request.clone()) => result,
};
```

**Rust 知识点：**
- `tokio::select!`：**宏**。同时等待多个异步 Future，哪个先完成就执行哪个分支
- `biased;`：宣告偏序模式——按书写顺序检查（而非随机），取消检查优先于请求
- `() = ...`：等待一个返回 `()` 的 Future（取消信号）
- `result = ... => result`：等待请求流，把结果绑定到 `result` 并作为 `select!` 的值

```rust
let stream = match stream_result {
    Ok(s) => {
        context_recovery_attempts = 0;
        s
    }
    Err(e) => {
        ...
        if is_context_length_error_message(&message)
            && context_recovery_attempts < MAX_CONTEXT_RECOVERY_ATTEMPTS
            && self.recover_context_overflow(...).await
        {
            context_recovery_attempts = context_recovery_attempts.saturating_add(1);
            continue;
        }
        ...
        return (TurnOutcomeStatus::Failed, turn_error);
    }
};
```

**Rust 知识点：**
- `match` 分支中可以用 `;` 分隔多条语句，最后一条不带 `;` 作为返回值
- `continue` 在 `match` 内部会回到最内层的 `loop`

---

### 六、流事件处理（660–1137）

这是函数中最长的一段，处理来自 DeepSeek API 的 SSE（Server-Sent Events）流。

#### 初始化状态变量（660–716）

```rust
let mut content_blocks: Vec<ContentBlock> = Vec::new();
let mut current_text_raw = String::new();
let mut current_thinking = String::new();
let mut tool_uses: Vec<ToolUseState> = Vec::new();
let mut usage = Usage { input_tokens: 0, output_tokens: 0, ..Usage::default() };
```

**Rust 知识点：**
- `Vec::new()`：创建空 Vec
- `String::new()`：创建空字符串
- `Usage { ..Usage::default() }`：**结构体更新语法**。用 `default()` 填充未显式指定的字段

```rust
let mut current_tool_indices: std::collections::HashMap<u32, usize> =
    std::collections::HashMap::new();
```

**Rust 知识点：**
- `HashMap<u32, usize>`：键为 `u32`（block_index），值为 `usize`（在 tool_uses 中的位置）
- `usize`：平台相关的无符号整数类型（64位系统上=u64）

```rust
let mut any_content_received = false;
let mut transparent_stream_retries = 0u32;
```

布尔值 `false` 和字面量 `0u32`——Rust 要求显式类型注解时用后缀。

#### 内部流循环（718–1137）

```rust
loop {
    let stream_event = tokio::select! {
        biased;
        () = self.cancel_token.cancelled() => { ... }
        event = stream.next() => { event }
    };
    ...
}
```

**Rust 知识点：**
- **嵌套 loop**：外层的 `loop` 是整个 turn 的步骤循环，内层的 `loop` 是流事件的消费循环
- `stream.next()`：异步迭代器的 `next()` 方法，返回 `Option<StreamEvent>`——流结束时返回 `None`

```rust
let event = match stream_event {
    Some(Ok(event)) => event,
    Some(Err(e)) => { ... }
    None => break,
};
```

**Rust 知识点：**
- `Some(Ok(event))`：嵌套模式匹配——解构两层 `Option` 和 `Result`
- `None => break`：流结束了，break 跳出内层流循环

**透明重试逻辑（724–878）：** 当流因网络问题中断且尚未收到任何内容时，会使用完全相同的请求重新建立一个流。`stream_read_error_retryable` 判断是否可重试，`deepseek_client.create_message_stream()` 创建新流。

#### 流事件匹配（881–1137）

```rust
match event {
    StreamEvent::MessageStart { message } => {
        usage = message.usage;
    }
    StreamEvent::ContentBlockStart { index, content_block } => match content_block {
        ContentBlockStart::Text { text } => { ... }
        ContentBlockStart::Thinking { thinking } => { ... }
        ContentBlockStart::ToolUse { id, name, input, caller } => { ... }
        ContentBlockStart::ServerToolUse { id, name, input } => { ... }
    },
    StreamEvent::ContentBlockDelta { index, delta } => match delta {
        Delta::TextDelta { text } => { ... }
        Delta::InputJsonDelta { partial_json } => { ... }
    },
    StreamEvent::ContentBlockStop { index } => { ... }
    StreamEvent::MessageDelta { usage: delta_usage, .. } => { ... }
    StreamEvent::MessageStop | StreamEvent::Ping => {}
    StreamEvent::Error { error } => { ... }
}
```

**Rust 知识点：**
- **嵌套 match**：外层匹配事件类型，内层匹配内容块类型
- `MessageStop | Ping => {}`：**或模式**——两个变体共享同一个处理（什么都不做）
- `usage: delta_usage`：解构时**重命名**字段绑定

#### Thinking 块签名追踪（#3014）

```rust
current_thinking_signature = Some(signature);
```

对于 Anthropic 兼容的签名 thinking，签名必须原样回传。

#### 流结束后的透明重试（1139–1201）

```rust
let stream_died_with_nothing = stream_errors > 0
    && tool_uses.is_empty()
    && current_text_visible.trim().is_empty()
    && current_thinking.trim().is_empty()
    && !pending_message_complete;
```

**Rust 知识点：**
- 多条件的布尔表达式——用缩进提高可读性是 Rust 社区的惯用做法

```rust
if stream_died_with_nothing || sleep_resume_pending {
    if stream_retry_attempts < MAX_STREAM_RETRIES {
        stream_retry_attempts = stream_retry_attempts.saturating_add(1);
        turn_error = None;
        continue;
    }
    ...
} else if stream_errors == 0 {
    stream_retry_attempts = 0;
}
```

这里的 `continue` 会回到**外层** `loop` 的开头——重新发请求。

---

### 七、无工具调用分支（1296–1529）

```rust
if has_sendable_assistant_content {
    self.add_session_message(Message {
        role: "assistant".to_string(),
        content: content_blocks,
    })
    .await;
}
```

将模型的文本/thinking 输出保存为 assistant 消息。

```rust
if tool_uses.is_empty() {
    // 处理 pending steers → continue
    // 处理 shell completions
    // 处理 sub-agent completions → continue
    // 处理 REPL blocks → continue
    // 处理 goal continuations → continue
    // 处理 thinking-only 状态 → break
}
```

`continue` 回到外层 `loop` 重新请求模型；`break` 结束整个 turn。

---

### 八、工具执行阶段（1531–2606）

这是函数最复杂的部分之一。

#### 8.1 工具执行计划（1564–1945）

```rust
let mut plans: Vec<ToolExecutionPlan> = Vec::with_capacity(tool_uses.len());
for (index, tool) in tool_uses.iter_mut().enumerate() {
    ...
}
```

**Rust 知识点：**
- `Vec::with_capacity(n)`：预分配容量，减少后续 push 时的内存重分配
- `.iter_mut()`：可变迭代器——可以修改 Vec 中的元素
- `.enumerate()`：给迭代器附加索引 `(index, item)`

**工具权限判定（多层叠加）：**

1. **Plan 模式阻塞**（1600–1614）：Plan 模式下阻止 `exec_shell` 等写工具
2. **输入解析错误**（1617–1626）：模型生成的 JSON 参数解析失败
3. **Hook 前置处理**（1681–1728）：`tool_call_before` hook 可以修改参数、要求审批、或拒绝
4. **注册表审批规则**（1730–1765）：MCP 工具、注册表工具各自的审批要求
5. **Plan 模式写保护**（1767–1774）
6. **Ask 规则**（1784–1821）：`exec_shell_ask_rule_decision` 和 `file_tool_ask_rule_decision`
7. **自动审查策略**（1823–1856）：发布等敏感操作强制审批
8. **仓库法则**（1858–1893）：`.codewhale/constitution.json` 保护不变量
9. **延迟工具加载**（1895–1927）：如果模型调用了尚未加载的工具，自动加载

```rust
plans.push(ToolExecutionPlan {
    index, id: tool_id, name: tool_name, ...
});
```

**Rust 知识点：**
- `index,` 是 `index: index,` 的**简写**——当字段名与变量名相同时可以省略冒号

#### 8.2 并行/串行批处理（1964–2003）

```rust
let batches = plan_tool_execution_batches(plans);
```

将工具执行计划分组：只读工具可并行执行；写工具或需审批的工具必须串行执行。

```rust
if !parallel_chunks.is_empty() {
    // 发送并行执行状态
} else if plan_count > 1 {
    // 发送串行执行状态
}
```

#### 8.3 执行循环（2008–2595）

```rust
for batch in batches {
    let (parallel_allowed, plans) = match batch {
        ToolExecutionBatch::Parallel(plans) => (true, plans),
        ToolExecutionBatch::Serial(plan) => (false, vec![*plan]),
    };
    ...
}
```

**Rust 知识点：**
- 解构枚举：`ToolExecutionBatch::Parallel(plans)` 取出内部数据
- `vec![*plan]`：`*` **解引用**——从 `Box<ToolExecutionPlan>` 取出 `ToolExecutionPlan`，然后 `vec![...]` 创建包含一个元素的 Vec

**并行执行（2049–2157）：**

```rust
let mut tool_tasks = FuturesUnordered::new();
let shell_permits = Arc::new(tokio::sync::Semaphore::new(MAX_PARALLEL_SHELL_EXEC));
```

**Rust 知识点：**
- `FuturesUnordered`：一个异步任务集合，并行执行所有任务并按完成顺序产出结果
- `Arc::new(Semaphore::new(n))`：`Arc` 是**原子引用计数**智能指针，允许多线程共享；`Semaphore` 是信号量，限制并发数量

```rust
tool_tasks.push(async move {
    let _shell_permit = if plan.name == "exec_shell" {
        shell_permits.acquire_owned().await.ok()
    } else {
        None
    };
    ...
    ToolExecOutcome { index, id, name, input, started_at, result }
});
```

**Rust 知识点：**
- `async move { ... }`：创建异步闭包。`move` 将所有捕获的变量**所有权移入**闭包
- `acquire_owned().await.ok()`：异步获取信号量许可，`.ok()` 将 `Result` 转为 `Option`

```rust
while let Some(outcome) = tool_tasks.next().await {
    let index = outcome.index;
    outcomes[index] = Some(outcome);
}
```

按完成顺序收集结果写入 `outcomes` Vec。

**串行执行中的审批流程（2299–2388）：**

```rust
let (result_override, context_override, approval_stamp) = if plan.approval_required {
    ...
    match self.await_tool_approval(&tool_id).await {
        Ok(ApprovalResult::Approved) => (None, None, Some(ToolApprovalStamp::ApprovedByUser)),
        Ok(ApprovalResult::Denied) => (Some(Err(ToolError::permission_denied(...))), None, None),
        Ok(ApprovalResult::RetryWithPolicy(policy)) => (...),
        Err(err) => (Some(Err(err)), None, None),
    }
} else {
    (None, None, None)
};
```

**Rust 知识点：**
- 解构元组返回三个值：`(result_override, context_override, approval_stamp)`
- `ToolApprovalStamp::ApprovedByUser`：枚举变体

**工具执行与结果收集（2407–2590）：**

```rust
let mut result = if let Some(result_override) = result_override {
    result_override
} else {
    Engine::execute_tool_with_lock(...).await
};
```

成功时：
- 将工具结果写入 session 消息
- 运行 LSP 诊断 hook（136 号 issue）
- 附加 hook 上下文（3026 号 issue）

失败时：
- 记录错误类别（Network / RateLimit / Timeout / Tool）
- 便于后续判断是否需要发送降级提示

---

### 九、错误降级提示（2618–2634）

```rust
if step_error_count > 0 {
    consecutive_tool_error_steps = consecutive_tool_error_steps.saturating_add(1);
    if let Some(hint) = tool_error_degradation_runtime_hint(...) {
        self.add_session_message(...).await;
    }
} else {
    consecutive_tool_error_steps = 0;
}

turn.next_step();
```

**Rust 知识点：**
- `saturating_add(1)`：不会溢出，到达 `u32::MAX` 后保持
- 连续工具错误达到阈值时，向模型注入提示消息："不要再重复同样的调用"

---

### 十、返回（2638–2646）

```rust
if self.cancel_token.is_cancelled() {
    return (TurnOutcomeStatus::Interrupted, None);
}
if let Some(err) = turn_error {
    return (TurnOutcomeStatus::Failed, Some(err));
}
(TurnOutcomeStatus::Completed, None)
```

**Rust 知识点：**
- 最后的表达式不带 `;` 即作为**函数返回值**（隐式 return）
- `if let Some(err) = turn_error` 解构 Option，如果有错误就提前返回

---

### 总结：数据流全景

```
handle_deepseek_turn()
│
├── 初始化 (变量/客户端/recovery计数器)
│
└── loop {                        ← 外层步骤循环
    │
    ├── 取消检查 / steer处理 / 子代理完成
    ├── 上下文压缩 / 溢出恢复
    ├── 构建 MessageRequest
    │
    ├── 发送流式请求 (tokio::select!)
    │   └── loop {                ← 内层流事件循环
    │       ├── ContentBlockStart (Text/Thinking/ToolUse)
    │       ├── ContentBlockDelta (文本增量/JSON增量)
    │       ├── ContentBlockStop  (完成块)
    │       └── Error / 透明重试
    │   }
    │
    ├── 无工具调用? → 处理 steer/completions/REPL/goal → break 或 continue
    │
    └── 工具执行
        ├── for each tool_use: 构建 ToolExecutionPlan (多层权限判定)
        ├── 分批 (并行只读 / 串行写入+审批)
        └── for each batch:
            ├── 并行: FuturesUnordered
            └── 串行: 审批 → 快照 → 执行 → 结果 → LSP
        ├── goal 更新通知
        └── 错误降级提示
        └── turn.next_step() → continue (回到 loop 开头)
}

return (TurnOutcomeStatus, Option<String>)
```

这个函数说白了就是：**请求模型 → 解析响应 → 执行工具 → 返回结果 → 循环**，直到模型说"我完成了"（不再调用工具、不触发 continue），然后 break 出 loop 返回最终状态。

---

