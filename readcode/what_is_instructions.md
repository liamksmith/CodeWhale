# Chat Export

**Model:** deepseek-v4-flash
**Workspace:** D:\tmp\source\CodeWhale
**Date:** 2026-07-24 16:28:19

---

*System:*

Started new session e4b1e81e (New Session). Previous sessions remain available via /resume.

---

**You:**

在项目中查找EngineConfig::instructions有关的代码和文档，并回答一下问题：
1. 设计它是为了解决什么问题？
2. 它从何而来：即用户怎么设置它？
3. 它如何影响智能体的行为?
EngineConfig::instructions的定义在文件crates\tui\src\core\engine.rs的第261行。

---

*Thinking:*

用户想了解 `EngineConfig::instructions` 的设计意图、用户如何设置它、以及它如何影响智能体的行为。需要从源码和相关文档入手查找。

我先看看 `crates/tui/src/core/engine.rs` 第261行附近的定义，以及整个 `instructions` 的使用情况。

---

**Tool:**

• ▷ read done · 29 items
  ●●●●●●●●●●●●●●●●●●●●●●●●●●●●●  29 done, 0 running
▏ done: Reading crates/tui/src/core/engine.rs
▏ done: Searching for `instructions`
▏ done: Searching for `InstructionSource`
▏ done: Searching for `instructions`
▏ done: Searching for `\.instructions`
▏ done: Searching for `instructions`
▏ done: Reading crates/tui/src/prompts.rs
▏ done: Reading crates/tui/src/prompts.rs
▏ done: Searching for `\.instructions\b.*=|instructions.*config|config.*instruct
▏       ions`
▏ done: Searching for `load_config|engine_config|from_config`
▏ done: Searching for `configured_instruction_sources`
▏ done: Searching for `instructions.*Path|instructions.*Vec|pub instructions`
▏ done: Reading crates/tui/src/tui/ui.rs
▏ done: Searching for `instructions_paths|fn instructions`
▏ done: Searching for `instructions_paths`
▏ done: Searching for `instructions`
▏ done: Reading crates/tui/src/config.rs
▏ done: Searching for `pub instructions`
▏ done: Reading crates/tui/src/config.rs
▏ done: Searching for `instructions`
▏ done: Reading config.example.toml
▏ done: Searching for `render_instructions_block|instructions_block|instructions
▏       .*source`
▏ done: Reading crates/tui/src/prompts.rs
▏ done: Reading crates/tui/src/prompts.rs
▏ done: Reading crates/tui/src/core/engine.rs
▏ done: Reading crates/tui/src/config.rs
▏ done: Searching for `fn has_instructions`
▏ done: Reading crates/tui/src/main.rs
▏ done: Reading crates/tui/src/runtime_threads.rs

---

*Thinking:*

好的，现在我需要进一步了解 `InstructionSource` 的定义、`instructions` 如何从配置中加载、以及它如何被注入到系统提示中。

让我查看：
1. `InstructionSource` 的定义（在 `crates/tui/src/prompts.rs`）
2. 配置加载部分 - 用户如何设置 `instructions`
3. 系统提示渲染部分 - `instructions` 如何被注入

---

*Thinking:*

配置文件中没有 `instructions` 字段。让我搜索一下配置加载部分，看看用户如何设置 instructions，以及 `InstructionSource` 的完整定义。

---

*Thinking:*

现在我需要找到用户如何设置 `instructions` — 即从 TOML 配置文件到 `EngineConfig` 的路径。让我搜索实际的配置解析/加载代码。

---

*Thinking:*

关键线索找到了：`crates/tui/src/tui/ui.rs` 第1474行，`instructions: configured_instruction_sources(config)`。让我看看 `configured_instruction_sources` 函数，以及用户如何在配置文件中设置 `instructions`。

---

*Thinking:*

让我看看 `configured_instruction_sources` 函数的完整代码，以及用户配置中的 `instructions` 字段是如何定义的。我需要找到 `instructions_paths()` 方法。

---

*Thinking:*

`instructions_paths()` 这个函数不在 `crates/config` 中。让我在 `crates/tui` 中搜索它，可能在 `Config` 的扩展方法中。

---

*Thinking:*

Let me look at the `instructions_paths()` function in `crates/tui/src/config.rs` and the config field for `instructions` in the Config struct.

---

*Thinking:*

Now let me see the context around line 1904 to see the TOML config field's docs, and also where `instructions` gets loaded from config.

---

*Thinking:*

现在我还需要查看 config.example.toml 中关于 instructions 的完整文档，以及看看 `instructions` 在系统提示中的具体渲染位置，以完整回答第三个问题。

---

*Thinking:*

Now let me see where the instructions block is rendered in the prompt composition flow to understand how it's positioned.

---

*Thinking:*

Now I have a comprehensive understanding. Let me also look at the `instructions` check in the project context to understand the relationship between `instructions` and project context.

---

*Thinking:*

Let me also look at the project_context to see `has_instructions` to understand the relationship between instructions and project context.

---

*Thinking:*

Now I have a complete picture. Let me also check the embedder path — `RuntimeThread` or `main.rs` — where the embedder can inject inline instructions.

---

*Thinking:*

Now I have all the information needed to answer the user's three questions comprehensively. Let me synthesize everything.

1. **设计目的（设计它是为了解决什么问题？）**
   - Issue #454 跟踪的问题：允许用户为智能体注入自定义的系统提示指令
   - 核心问题：用户/项目需要一种方式将额外的规则、指南、偏好注入到智能体的系统提示中，而不必修改 CodeWhale 自身的提示词模板
   - 具体用例：仓库级别的规则（如 AGENTS.md）、全局偏好（如编码规范、安全检查要求等）
   - 它解决了"用户如何在系统提示层面告诉智能体应该做什么"这个通用问题

2. **从何而来（用户怎么设置它）**
   - 用户在 `config.toml` 中设置 `instructions = [...路径...]` 数组
   - 也可以在项目级别 `.codewhale/config.toml` 中设置，项目级配置会完全替换用户级配置（而非合并）
   - 路径支持 `~` 和环境变量扩展
   - 可以为空数组 `instructions = []` 来清除项目仓库中的用户级配置
   - 非 TUI 场景下，嵌入器（如 main.rs 的 CLI 调度器）还可以通过 `--append-system-prompt` CLI 参数注入内联指令（`InstructionSource::Inline`）
   - 每条路径对应的文件在渲染时读取，缺失时会跳过（带警告），不会导致启动失败
   - 每条文件上限 100 KiB，超过会被截断

3. **如何影响智能体的行为**
   - `instructions` 被渲染为 `<instructions source="path/to/file">...内容...</instructions>` XML 块注入到系统提示中
   - 渲染位置在系统提示的**第 6a 步**，位于 volatile boundary 之后——这意味着它在"谁的指令优先"层级中位于较高优先级
   - 在提示的层级结构中：Constitution（宪章）> 项目法律和 instructions > 偏好设置 > 记忆和历史交接（这正是测试 `project_instructions_outrank_memory_in_whose_word_wins` 所验证的）
   - 它以声明式的方式告诉智能体"你应该知道这些规则"，直接影响智能体的决策、行为模式和输出风格
   - 对于 Fleet Profile，`role.instructions` 字段充当子智能体的指令覆盖
   - 注意：项目上下文（project_context）也有自己的 `instructions` 字段，但在 `EngineConfig.instructions` 中是分开的——前者来自 `has_instructions()` 检查，用于决定是否将项目上下文作为项目指令注入

现在让我组织答案。

---

**Assistant:**

现在我已经完整了解了 `EngineConfig::instructions` 的整个生命周期。下面给出完整回答。

---

## 1. 设计目的是为了解决什么问题？

`EngineConfig::instructions` 设计用于解决 **issue #454** 追踪的问题——**用户/项目需要一种标准化的方式，将自定义规则、指南和偏好注入到智能体的系统提示中**，而无需修改 CodeWhale 本身的提示词模板。

具体来说，它解决了三个层面的需求：

- **按仓库定制行为**：不同项目可能有不同的编码规范、测试要求或安全策略。例如一个项目要求所有代码必须通过 `cargo clippy` 才能提交，而另一个项目要求优先写测试。通过 `instructions` 可以在每个仓库层面声明这些规则。
- **全局偏好层叠**：用户可能有一份全局的编码规范（如 `~/.codewhale/global.md`），而每个仓库有其自己的补充规则。`instructions` 允许多层规则按声明顺序拼接。
- **嵌入器场景**：非 TUI 场景（如 CLI 调度器、自动化任务）需要在运行时动态生成指令（如模板化渲染后的规则），而无需将内容暂存到磁盘文件。`InstructionSource::Inline` 变体就是为此而生。

在 `instructions` 出现之前，用户如果想"告诉智能体一些事情"，没有标准渠道——要么通过记忆（memory）这种持久化但非结构化的方式，要么通过聊天中自然语言告知，但缺乏持久性和确定性。

---

## 2. 从何而来：用户如何设置它？

用户可以通过 **两种主要方式** 设置 `instructions`：

### 方式一：配置文件 `config.toml`

在 `config.toml` 中设置 `instructions` 数组：

```toml
instructions = ["./AGENTS.md", "~/.codewhale/global.md"]
```

每条路径：
- 支持 `~` 和环境变量展开（通过 `expand_path`）
- 渲染时从磁盘读取，上限 **100 KiB**，超过会被截断
- 文件缺失会跳过（并输出 tracing 警告），不会导致启动失败

### 方式二：项目级配置覆盖

在项目工作区的 `.codewhale/config.toml` 中可以设置项目级 `instructions`。这条规则很重要：

> **项目级配置完全替换用户级配置，而不是合并。** 如果希望在项目中也保留全局文件，需要在项目数组中显式列出它。显式空数组 `instructions = []` 会清除用户级配置。

### 方式三：CLI 嵌入器注入（高级）

在 `main.rs` 中，CLI 调度器通过 `--append-system-prompt` 参数将内联字符串注入为 `InstructionSource::Inline`：

```rust
// main.rs 第7820行
if let Some(ref extra) = append_system_prompt {
    instrs.push(crate::prompts::InstructionSource::Inline {
        name: "cli:append-system-prompt".into(),
        content: extra.clone(),
    });
}
```

### 数据流

```
config.toml 中的 instructions = [...]
        ↓
crates/tui/src/config.rs:3673
  instructions_paths() 解析为 Vec<PathBuf>
        ↓
crates/tui/src/tui/ui.rs:1558
  configured_instruction_sources() 转换为 Vec<InstructionSource>
        ↓
crates/tui/src/core/engine.rs:288
  EngineConfig.instructions = Vec<InstructionSource>
        ↓
crates/tui/src/prompts.rs:1350-1353
  render_instructions_block() → <instructions source="...">...内容...</instructions>
```

---

## 3. 它如何影响智能体的行为？

### 注入方式

`instructions` 被渲染为 **XML 标记块** 注入到系统提示的固定位置（第 6a 步）：

```xml
<instructions source="/path/to/file.md">
实际规则内容...
</instructions>
```

每个源文件独立渲染为一个 `<instructions>` 块，按声明顺序拼接。比如：

```xml
<instructions source="~/global.md">全局规则</instructions>

<instructions source="./repo-rules.md">仓库规则</instructions>
```

### 在系统提示中的位置

```
系统提示结构（简化）:
├── Constitution (宪章)
├── 模式提示 (Agent/Yolo/Plan)
├── 项目上下文 (Project Context Pack)
├── 技能 (Skills)
├── 上下文管理 (压缩/工作集)
├── ── volatile boundary ──         ← 以下是 volatile 部分
├── 环境块 (workspace 等)
├── 6a. <instructions source="..."> ← 这里
├── 6b. <user_memory>
├── 6c. <session_goal>
├── 交接块 (handoff)
└── ...
```

放置在 volatile boundary 之后是有意设计的：instructions 文件是工作区作用域的，不同会话之间可能不同。放在这里可以避免因 instructions 变化而破坏前面静态部分的 DeepSeek 前缀缓存。

### 优先级：谁的指令优先？

代码中有专门的测试（`project_instructions_outrank_memory_in_whose_word_wins`）验证了 `instructions` 在**"谁的指令优先"层级**中的位置：

```
1. 当前用户请求（本轮）
2. 本宪章（Constitution）
3. 项目法律和 instructions ← instructions 在这里
4. 用户全局偏好
5. 记忆和历史交接（Memory & Handoffs）
```

即 `instructions` 的优先级高于用户偏好设置和记忆，但低于宪章和当前用户请求。这意味着：

- **宪章**（Constitution）定义了不可逾越的边界
- **instructions** 可以覆盖和补充用户的偏好设置与记忆中的规则
- 用户本轮说的任何话（当前请求）覆盖一切

### 实际行为影响

通过这种方式注入的系统提示内容会**直接影响智能体的全部决策**——因为对于 LLM 来说，系统提示中的内容就是"你应当遵守的规则"。典型的用法包括：

- **编码规范**：`"所有 Rust 代码必须通过 cargo clippy -D warnings"`
- **安全策略**：`"禁止使用 unsafe 代码块"`
- **测试要求**：`"每次变更必须附带测试"`
- **输出格式**：`"回答时始终使用中文"`
- **工作流程约束**：`"编辑后必须运行 cargo test"`

### 注意：与 Fleet Profile 的关系

`FleetRole` 结构体也有自己的 `instructions: Option<String>` 字段（在 `crates/config/src/lib.rs` 第 1212 行）。当 Fleet Manager 创建子智能体时，这个字段会作为该角色的指令覆盖注入到子智能体的提示中——这是 `instructions` 机制在**子智能体委派场景**中的延伸应用。

---

