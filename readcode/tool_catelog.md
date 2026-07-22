# Chat Export

**Model:** deepseek-v4-pro
**Workspace:** D:\tmp\source\CodeWhale
**Date:** 2026-07-22 10:44:35

---

**You:**

读取crates\tui\src\core\engine\tool_catalog.rs文件，逐行分析ensure_advanced_tooling这个函数的作用

---

*Thinking:*

用户要求我读取 `crates/tui/src/core/engine/tool_catalog.rs` 文件，并逐行分析 `ensure_advanced_tooling` 函数的作用。让我先打开这个文件。

---

**Tool:**

• ▷ read done · 3 items
  ●●●  3 done, 0 running
▏ done: Reading crates/tui/src/core/engine/tool_catalog.rs
▏ done: Searching for `fn ensure_advanced_tooling`
▏ done: Reading crates/tui/src/core/engine/tool_catalog.rs

---

*Thinking:*

我需要继续读取文件的剩余部分，找到 `ensure_advanced_tooling` 函数。文件总共有 1099 行，我还需要继续读取。

---

*Thinking:*

函数 `ensure_advanced_tooling` 从第260行开始。让我读取该函数周围的完整代码。

---

*Thinking:*

我已经找到了 `ensure_advanced_tooling` 函数（第260-344行）。现在我可以提供逐行分析了。

让我仔细分析这个函数：

函数签名:
```rust
pub(super) fn ensure_advanced_tooling(
    catalog: &mut Vec<Tool>,
    mode: AppMode,
    always_load: &HashSet<String>,
)
```

这个函数的作用是：**确保高级/内置工具被添加到工具目录中**。这些工具不是通过常规运行时工具注册来的，而是在目录构建时条件性地注入。

函数分为三个部分：

1. **code_execution**（第265-296行）：Python代码执行工具
2. **js_execution**（第298-310行）：JavaScript代码执行工具
3. **tool_search**（第312-343行）：工具搜索工具

让我逐行详细分析。

---

**Assistant:**

我已经完整读取了 `ensure_advanced_tooling` 函数（第260-344行）。下面是逐行分析：

---

## 函数签名（第260-264行）

```rust
pub(super) fn ensure_advanced_tooling(
    catalog: &mut Vec<Tool>,
    mode: AppMode,
    always_load: &HashSet<String>,
)
```

- **可见性：** `pub(super)`，仅对父模块 `engine` 可见。
- **参数：**
  - `catalog`：可变引用，指向当前的模型可见工具列表。
  - `mode`：当前应用模式（Plan / Agent / Yolo）。
  - `always_load`：始终加载的工具名集合（来自用户配置）。
- **作用：** 在工具目录构建阶段，向 `catalog` 中**条件性地注入三个"高级"内置工具**——这些工具不走常规的 `ToolRegistry` 注册流程，而是在此处直接构造并推入目录。

---

## 第一部分：`code_execution` —— Python 代码执行工具（第265-296行）

| 行号 | 代码 | 分析 |
|------|------|------|
| 265-271 | 注释块 | 说明了历史背景：v0.8.31 之前，此工具总是被广告给模型，但在 Windows 上 `python3` 不在 PATH 中时，执行会失败。模型一旦看到工具就认为它可靠，这造成了不良体验。现在改为在目录构建时**探测本地是否有 Python 解释器**，有才广告。 |
| 272-274 | `if mode != AppMode::Plan && !catalog.iter().any(\|t\| t.name == CODE_EXECUTION_TOOL_NAME) && crate::dependencies::resolve_python_interpreter().is_some()` | **三重条件守卫：** (1) Plan 模式下不注入（Plan 模式隐藏所有执行表面）；(2) 目录中尚未存在同名工具（幂等保护，避免重复注入）；(3) 本地确实能找到 Python 解释器（`resolve_python_interpreter()` 返回 `Some`）。 |
| 276-295 | `catalog.push(Tool { ... })` | 若条件满足，构造一个 `Tool` 结构体并推入目录。字段含义： |
| 277 | `tool_type: Some(CODE_EXECUTION_TOOL_TYPE.to_string())` | 工具类型标记为 `"code_execution_20250825"`（带版本的内部类型标识）。 |
| 278 | `name: CODE_EXECUTION_TOOL_NAME.to_string()` | 工具名称为 `"code_execution"`。 |
| 279 | `description: ...` | 描述："在本地沙箱化运行时中执行 Python 代码，以 JSON 返回 stdout/stderr/return_code。" |
| 280-286 | `input_schema: json!({...})` | JSON Schema：唯一必填字段 `code`（Python 源代码字符串）。 |
| 287 | `allowed_callers: Some(vec!["direct".to_string()])` | 仅允许"直接"调用方——模型可以直接调用它，但子代理不能通过 `multi_tool_use.parallel` 调用。 |
| 288-291 | `defer_loading: Some(should_default_defer_tool(CODE_EXECUTION_TOOL_NAME, always_load))` | 延迟加载标记：检查该工具名是否在 `DEFAULT_ACTIVE_NATIVE_TOOLS` 列表或 `always_load` 集合中。`code_execution` 不在默认活跃列表中，所以**默认会被延迟加载**——模型需要先通过 `tool_search` 查询才能激活使用。 |
| 292-294 | `input_examples: None, strict: None, cache_control: None` | 无输入示例、无严格模式、无缓存控制（这些字段对高级工具不适用）。 |

---

## 第二部分：`js_execution` —— JavaScript 代码执行工具（第298-310行）

| 行号 | 代码 | 分析 |
|------|------|------|
| 298-302 | 注释块 | 与 `code_execution` 对称：只在本地有 Node.js 时才广告。Plan 模式同样隐藏。 |
| 303-305 | `if mode != AppMode::Plan && !catalog.iter().any(\|t\| t.name == JS_EXECUTION_TOOL_NAME) && crate::dependencies::resolve_node().is_some()` | 三重条件：(1) 非 Plan 模式；(2) 目录中尚未有 `js_execution`；(3) 本地可解析到 Node.js。 |
| 307 | `let mut tool = crate::tools::js_execution::js_execution_tool_definition()` | **与 `code_execution` 不同**：不是内联构造 JSON，而是从 `crate::tools::js_execution` 模块获取标准工具定义（该模块有完整的独立工具实现）。 |
| 308 | `tool.defer_loading = Some(should_default_defer_tool(&tool.name, always_load))` | 同样应用默认延迟加载策略。 |
| 309 | `catalog.push(tool)` | 推入目录。 |

> **注意差异：** `code_execution` 是内联构造的（相对简单），而 `js_execution` 委托给专门的 `crate::tools::js_execution` 模块获取定义——说明 JS 执行工具有更复杂的 schema 或实现逻辑。

---

## 第三部分：`tool_search` —— 工具搜索工具（第312-343行）

| 行号 | 代码 | 分析 |
|------|------|------|
| 312 | `if !catalog.iter().any(\|t\| t.name == TOOL_SEARCH_NAME)` | **唯一条件：** 仅检查是否已存在同名工具，没有模式或依赖探测条件。**所有模式下都会注入**（Plan / Agent / Yolo），因为工具搜索是只读元操作。 |
| 313-342 | `catalog.push(Tool { ... })` | 构造 `tool_search` 工具： |
| 314 | `tool_type: Some(TOOL_SEARCH_TYPE.to_string())` | 类型标记为 `"tool_search_20251119"`。 |
| 315 | `name: TOOL_SEARCH_NAME.to_string()` | 工具名为 `"tool_search"`。 |
| 316 | `description: ...` | 描述："搜索延迟加载的工具定义，返回匹配的工具引用。" |
| 317-336 | `input_schema: json!({...})` | JSON Schema，包含三个属性： |
|   | `query` (必填) | 搜索查询字符串。 |
|   | `match` (可选，默认 `"bm25"`) | 匹配算法：`"bm25"`（自然语言匹配）或 `"regex"`（正则匹配）。 |
|   | `max_results` (可选，默认20，最大100) | 最大返回结果数。 |
| 337 | `allowed_callers: Some(vec!["direct".to_string()])` | 仅直接调用。 |
| 338 | `defer_loading: Some(false)` | **关键差异：强制设为 `false`**——`tool_search` 永远不延迟加载，因为它本身就是发现和激活其他延迟工具的**入口**。如果它自己被延迟，模型将无法发现任何延迟工具。 |

---

## 总结

`ensure_advanced_tooling` 函数是工具目录的**"最后一公里"增强器**，在标准工具注册完成后被调用。它注入三个特殊工具：

| 工具 | 条件 | 延迟加载 | 构造方式 |
|------|------|---------|---------|
| `code_execution` | Agent/Yolo + Python可用 | 是（默认延迟） | 内联构造 |
| `js_execution` | Agent/Yolo + Node.js可用 | 是（默认延迟） | 外部模块 `js_execution_tool_definition()` |
| `tool_search` | 无特殊条件（所有模式） | **否（始终激活）** | 内联构造 |

核心设计思想：
1. **按需探测**：执行类工具不盲目广告，而是探测本地运行时可用性后才注入。
2. **模式感知**：Plan 模式完全隐藏执行类工具（但保留只读的 `tool_search`）。
3. **延迟加载**：执行类工具默认延迟，模型通过 `tool_search` 发现并激活它们——这减少了每轮对话的 tool catalog 体积，保护了 DeepSeek KV 前缀缓存的字节稳定性。
4. **幂等安全**：每个工具都先检查 `!catalog.iter().any(...)`，防止重复注入。

---

**You:**

解读crates\tui\src\core\engine\tool_catalog.rs中的tool_catalog_consistency_issues函数

---

*Thinking:*

用户要求解读 `tool_catalog_consistency_issues` 函数。先搜索它的位置。

---

**Tool:**

• ▷ read done · 2 items
  ●●  2 done, 0 running
▏ done: Searching for `fn tool_catalog_consistency_issues`
▏ done: Reading crates/tui/src/core/engine/tool_catalog.rs

---

*Thinking:*

函数从第712行开始。让我读取其完整内容，以及它调用的辅助函数。

---

*Thinking:*

现在我需要看一下 `is_synthetic_catalog_tool` 函数，因为它在一致性检查中被使用。让我也读一下这个函数和相关结构。

---

**Assistant:**

让我看看调用的辅助函数 `is_synthetic_catalog_tool`：

---

**Tool:**

• ▷ read done · Searching for `fn is_synthetic_catalog_tool`
▏ done: Searching for `fn is_synthetic_catalog_tool`

---

*Thinking:*

现在我已经收集了所有必要的信息来分析 `tool_catalog_consistency_issues` 函数。让我也检查一下 `ToolRegistry` 的 `to_api_tools` 方法和 `contains` 方法以及 `Tool` 结构体，以便更好地理解整个流程。

实际上，我已经有足够的信息来分析了。让我总结：

1. `tool_catalog_consistency_issues` 函数（第712-749行）
2. 辅助函数 `is_synthetic_catalog_tool`（第706-710行）

这个函数做了两个方向的检查：

**方向1（正向）：catalog → registry**
遍历目录中的每个工具，跳过合成工具（tool_search, code_execution, js_execution, MCP工具），然后检查每个目录工具是否在 ToolRegistry 中有对应的 handler 注册。如果没有，报告"目录广告了但无handler"。

**方向2（反向）：registry → catalog**
遍历 `DEFAULT_ACTIVE_NATIVE_TOOLS` 列表，检查每个核心工具是否同时在 registry（有注册handler）和 catalog（对模型可见）中。如果一个工具在 registry 中注册了但在 catalog 中缺失，报告"已注册但目录缺失"。

这是一个交叉验证，确保工具目录和工具注册表保持一致。

---

**Assistant:**

现在我有完整的图景了。下面是逐层分析：

---

## `tool_catalog_consistency_issues` 函数分析（第712-749行）

### 函数签名

```rust
pub(super) fn tool_catalog_consistency_issues(
    catalog: &[Tool],
    registry: &crate::tools::ToolRegistry,
) -> Vec<String>
```

**定位：** `pub(super)`，对 engine 模块内部可见。  
**返回值：** 字符串列表，每个元素是一条一致性问题描述。空列表表示一切正常。  
**用途：** 在每次构建工具目录后运行，检测目录（模型看到的工具列表）与注册表（实际有处理函数的工具）之间的**交叉不一致**，确保模型不会被广告一个不存在的工具，也确保有 handler 的核心工具不会意外从模型视野中消失。

---

### 逐段分析

#### 第一阶段：构建查找集合（第716-725行）

```rust
let catalog_names = catalog
    .iter()
    .map(|tool| tool.name.as_str())
    .collect::<HashSet<_>>();

let registry_api_tools = registry.to_api_tools();
let registry_model_visible_names = registry_api_tools
    .iter()
    .map(|tool| tool.name.as_str())
    .collect::<HashSet<_>>();

let mut issues = Vec::new();
```

| 行号 | 变量 | 说明 |
|------|------|------|
| 716-719 | `catalog_names` | 目录中所有工具名的 `HashSet<&str>`，用于 O(1) 成员测试。 |
| 720 | `registry_api_tools` | 从 `ToolRegistry` 获取 API 工具列表——这是 registry 注册的、对模型可见的工具子集（非所有内部工具，而是模型可调用的那些）。 |
| 721-724 | `registry_model_visible_names` | 上述列表中的工具名集合。 |
| 725 | `issues` | 累积诊断消息的容器。 |

---

#### 第二阶段：正向检查——catalog → registry（第727-737行）

```rust
for tool in catalog {
    if is_synthetic_catalog_tool(&tool.name) {
        continue;
    }
    if !registry.contains(&tool.name) {
        issues.push(format!(
            "catalog advertises '{}' but no registered handler exists",
            tool.name
        ));
    }
}
```

**遍历目录中的每个工具，验证其 handler 是否存在。**

跳过合成工具的原因在于 `is_synthetic_catalog_tool`（第706-710行）：

```rust
fn is_synthetic_catalog_tool(name: &str) -> bool {
    is_tool_search_tool(name)
        || matches!(name, CODE_EXECUTION_TOOL_NAME | JS_EXECUTION_TOOL_NAME)
        || McpPool::is_mcp_tool(name)
}
```

| 工具类别 | 为何跳过 |
|----------|----------|
| `tool_search` / 遗留 `tool_search_tool_regex` / `tool_search_tool_bm25` | 它们由 `ensure_advanced_tooling` 注入，handler 在引擎层硬编码处理，不在 `ToolRegistry` 中注册。 |
| `code_execution` / `js_execution` | 同上，由 `ensure_advanced_tooling` 内联注入，执行逻辑通过 `dependencies` 模块直接调用 Python/Node，不经过 `ToolRegistry`。 |
| MCP 工具（`McpPool::is_mcp_tool`） | MCP 工具的 handler 在 `McpPool` / MCP 客户端中管理，也不在标准 `ToolRegistry` 中注册。 |

> **核心逻辑：** 对于非合成工具，如果在目录中存在但 registry 中没有对应的 handler，就是 bug——模型会收到一个无法执行的工具，调用时必然失败。这检查的是**"幽灵工具"**。

---

#### 第三阶段：反向检查——registry → catalog（第739-745行）

```rust
for name in DEFAULT_ACTIVE_NATIVE_TOOLS {
    if registry_model_visible_names.contains(name) && !catalog_names.contains(name) {
        issues.push(format!(
            "registered core tool '{name}' is missing from the model/search catalog"
        ));
    }
}
```

**遍历默认活跃原生工具列表，反查是否存在注册了但目录中缺失的情况。**

`DEFAULT_ACTIVE_NATIVE_TOOLS`（第42-70行）包含 28 个核心工具：`agent`、`apply_patch`、`edit_file`、`exec_shell`、`web_search`、`write_file` 等。

| 条件 | 含义 |
|------|------|
| `registry_model_visible_names.contains(name)` | registry 中有这个工具（handler 就绪）。 |
| `!catalog_names.contains(name)` | 但目录中没有它（模型看不到）。 |

两个条件同时为真时报告不一致：工具已实现但对模型不可见——这可能是模式过滤、allow/deny 配置或目录构建 bug 导致的**"隐藏工具"**。

> 注意这里只检查 `DEFAULT_ACTIVE_NATIVE_TOOLS` 列表中的工具，不检查所有 registry 工具——因为某些工具被有意隐藏是有合理原因的（如 Plan 模式下的 shell 工具），但核心工具意外缺失就是 bug。

---

#### 第四阶段：排序输出（第747-748行）

```rust
issues.sort();
issues
```

对诊断消息排序后返回，保证输出确定性。

---

### 整体数据流示意

```
                    ┌─────────────────────┐
                    │    ToolRegistry      │
                    │  (handler 注册表)     │
                    │                      │
                    │ to_api_tools() ──────┼──→ registry_model_visible_names
                    │                      │
                    └──────────┬───────────┘
                               │
                    1. 正向检查：catalog 中的工具 registry 里有吗？
                               │
                    ┌──────────▼───────────┐
                    │   catalog (Vec<Tool>) │
                    │   (模型看到的工具)      │
                    │                      │
                    │  catalog_names       │
                    └──────────────────────┘
                               │
                    2. 反向检查：DEFAULT_ACTIVE_NATIVE_TOOLS
                       在 registry 中存在但在 catalog 中缺失？
                               │
                               ▼
                    ┌──────────────────────┐
                    │   Vec<String> issues │
                    │   (诊断报告，排序)     │
                    └──────────────────────┘
```

---

### 设计意图总结

| 检查方向 | 检测的 bug 类型 | 严重性 |
|----------|----------------|--------|
| catalog → registry | **幽灵工具**：模型看到但无法执行 | 高危（运行时会炸） |
| registry → catalog | **隐藏工具**：有实现但对模型不可见 | 中危（功能缺失） |
| 跳过合成工具 | 合成工具不走 registry，误报 | 必要的排除 |

这个函数本质上是**目录构建后的守护栅栏**——它在不影响性能热路径的前提下，用几行 O(n) 的集合操作捕获潜在的不一致，在开发和测试阶段及早暴露目录构建的 bug。

---

**You:**

is_synthetic_catalog_tool这个函数的作用是什么？函数名字里的synthetic应该怎么翻译？

---

*Thinking:*

用户问的是 `is_synthetic_catalog_tool` 函数的作用和 "synthetic" 这个词的翻译。之前我已经看过这个函数了，在第706-710行。让我再确认一下，然后给出分析。

这个函数非常简单：

```rust
fn is_synthetic_catalog_tool(name: &str) -> bool {
    is_tool_search_tool(name)
        || matches!(name, CODE_EXECUTION_TOOL_NAME | JS_EXECUTION_TOOL_NAME)
        || McpPool::is_mcp_tool(name)
}
```

关于 "synthetic" 的翻译：
- 字面意思是"合成的、人造的"
- 在这个上下文中，它的含义是这些工具不是通过常规的 ToolRegistry 注册流程（即不是"原生/标准注册"的工具），而是在目录构建时由 `ensure_advanced_tooling` 函数"人工/手动/特殊"注入的工具。
- 有几种可能的翻译：
  1. "合成工具" - 最直译
  2. "注入工具" - 强调它们是被额外注入的
  3. "非注册工具" - 强调它们不走 ToolRegistry
  4. "内置合成工具" - 结合语境

我认为在这个上下文中，"synthetic" 最准确的理解是：相对于通过 ToolRegistry 标准注册的"原生"工具而言，这些工具是合成/构建出来的——它们没有对应的 ToolRegistry handler，而是由引擎在目录构建阶段手动内联构造的。

翻译建议：**"合成工具"** 或者更易理解的 **"人工构造工具"**。我会建议用"合成"因为它在编程/计算机科学语境中有"非自然产生的、由系统构建的"含义，与这里的语义吻合。

---

**Assistant:**

之前读取的内容已包含该函数，无需再次读取。

---

## `is_synthetic_catalog_tool` 分析

### 源代码（第706-710行）

```rust
fn is_synthetic_catalog_tool(name: &str) -> bool {
    is_tool_search_tool(name)
        || matches!(name, CODE_EXECUTION_TOOL_NAME | JS_EXECUTION_TOOL_NAME)
        || McpPool::is_mcp_tool(name)
}
```

### 作用

这个函数回答一个问题：**"这个工具是不是由引擎在目录构建时合成出来的，而不是通过标准 ToolRegistry 注册的？"**

它返回 `true` 的三类工具正好对应三种不经过 `ToolRegistry::register()` 的注入路径：

| 检查条件 | 涵盖的工具 | 注入来源 |
|----------|-----------|---------|
| `is_tool_search_tool(name)` | `tool_search`、`tool_search_tool_regex`、`tool_search_tool_bm25` | `ensure_advanced_tooling` 第312-343行内联构造 |
| `matches!(name, CODE_EXECUTION_TOOL_NAME \| JS_EXECUTION_TOOL_NAME)` | `code_execution`、`js_execution` | `ensure_advanced_tooling` 第272-310行内联构造或从 `js_execution_tool_definition()` 获取 |
| `McpPool::is_mcp_tool(name)` | 所有 MCP 工具（`mcp__*` 前缀） | `McpPool` 在目录构建时动态注入，handler 在 MCP 客户端侧 |

它在整个文件中的唯一调用点位于 `tool_catalog_consistency_issues` 第728行：

```rust
for tool in catalog {
    if is_synthetic_catalog_tool(&tool.name) {
        continue;   // 跳过——不在 ToolRegistry 中，无需交叉检查
    }
    if !registry.contains(&tool.name) {
        issues.push(...);
    }
}
```

**这里的逻辑是：** 合成工具天然不在 `ToolRegistry` 中，如果对它们做 "catalog 里有没有 handler" 的检查，会100%误报。跳过它们，只对真正走注册流程的工具做一致性校验。

---

### "synthetic" 怎么翻译？

在这个语境中，`synthetic` 的精确含义是：**不是自然注册产生的，而是由系统在另一个路径上人工构造/组装出来的。**

几个候选翻译及评价：

| 翻译 | 评价 |
|------|------|
| **合成工具** | 最推荐。计算机科学中 "synthetic" 的惯用译法（如 synthetic benchmark → 合成基准测试、synthetic event → 合成事件），含义是"非自然产生、由系统构造"。 |
| 注入工具 | 强调"被额外注入"的行为，但丢失了"内联构造"这层含义。 |
| 虚拟工具 | 有歧义，容易和 virtualization 混淆。 |
| 非注册工具 | 过于直白，且"非注册"是结果而非本质——本质是"不走注册流程、被合成出来"。 |

**建议：翻译为"合成工具"。** 它准确传达了"由引擎在目录构建阶段手工内联构造、不出自 ToolRegistry 注册流程"这一核心语义，且与编程领域对 `synthetic` 的惯用译法一致。

### 类比理解

可以用数据库中的视图（view）来类比：

- **注册工具** ≈ 基表 —— 通过 `ToolRegistry::register()` 正式注册，有 handler 有 schema，是一等公民。
- **合成工具** ≈ 视图 —— 不在基表中存储，由引擎在查询时（目录构建时）动态构造，行为由特殊路径处理。

`is_synthetic_catalog_tool` 就是那个"这不是基表，是视图，别去基表里找它"的判断。

---

**You:**

逐行分析crates\tui\src\core\engine\tool_catalog.rs中的代码

---

*Thinking:*

用户要求逐行分析整个 `tool_catalog.rs` 文件。文件共 1189 行，我之前已经读取了部分。需要系统地读取完整文件。让我分批次读取，从上次中断的地方继续，同时也要重新确认前面部分。

我们已经读过的行：1-454 行和 710-959 行。还需要读取的部分：
- 455-709 行
- 960-1189 行

另外，之前只显示了 1-200 行的前 200 行被截断了，我需要确认 200 行之后的内容是否也被读取了。回顾之前：第一次读取了 1-200 行（truncated），第二次读取了 255-454 行。所以还缺少 201-254 行和 455-709 行以及 960-1189 行。

让我分批读取。

---

**Tool:**

• ▷ read done · 8 items
  ●●●●●●●●  8 done, 0 running
▏ done: Reading crates/tui/src/core/engine/tool_catalog.rs
▏ done: Reading crates/tui/src/core/engine/tool_catalog.rs
▏ done: Reading crates/tui/src/core/engine/tool_catalog.rs
▏ done: Reading crates/tui/src/core/engine/tool_catalog.rs
▏ done: Reading crates/tui/src/core/engine/tool_catalog.rs
▏ done: Reading crates/tui/src/core/engine/tool_catalog.rs
▏ done: Searching for `fn suggest_tool_names`
▏ done: Reading crates/tui/src/core/engine/tool_catalog.rs

---

*Thinking:*

文件已经从 1189 行增长到 1225 行了。我需要系统地读取剩余部分。让我继续从700行读起，然后分段读取。

现在已知：
- 第1-200行：已读（但截断了，我只有前200行）
- 第200-699行：已读（当前返回，但是response中包含了被截断的内容，我需要获取完整内容）
- 第700-1225行：未读

让我先读取第700行开始的内容，然后从第1行重新读取以确保完整性。

实际上response里显示了200-699行的一部分，但是中间被truncated了很大一部分（"... truncated 5694 chars from middle ..."）。我需要获取完整的。但是注意，我之前在之前的对话中已经读了1-200行和255-454行以及710-959行，所以我已经有了这些：

1. 第1-200行：完整（第一次读取）
2. 第255-454行：完整（第二次读取，ensure_advanced_tooling + 部分后续函数）
3. 第710-959行：完整（第三次读取，tool_catalog_consistency_issues + deferred_tool相关）

缺失的部分：
- 第201-254行（build_model_tool_catalog_with_surface, apply_tool_surface_budget）
- 第455-709行（active_tool_list_from_catalog, active_tools_for_step, tool_search相关，suggest_tool_names等）
- 第960-1225行（schema_fields, schema_required_fields, received_field_names, likely_field_corrections, 以及测试代码）

另外还需要重新读1-200行，确认完整内容（第一次读取1-200行也显示了truncated，但我记得它完整显示了，让我确认一下）。

实际上第一次读取1-200行输出显示了"shown_lines 1-200, truncated true, next_start_line 201"，内容完整显示了200行。

让我现在读取700-1225行，分段进行。

---

*Thinking:*

继续读取第900行之后的内容。

---

*Thinking:*

继续读取剩余部分（1100-1225行），这部分主要是 `execute_tool_search` 函数和 `#[cfg(test)]` 测试模块。

---

*Thinking:*

现在我已经读取了全部1225行文件。不过之前读取的中间部分有截断（第200-699行中间的5694字符被截断了）。让我重新读取缺失的行范围，即第455-709行区域中在截断区域的部分。

200-699行中被截断的部分是中间的内容，让我看看从200行继续的内容：已经显示了200-260行左右，然后中间截断了，然后显示了大约280-500行延续的内容。我需要填补200-454行中缺失的部分以及455-699行的完整内容。

让我重新读一下200-454行，看看截断的是什么。

实际上，从之前的输出可以看到：
- 200-247行：build_model_tool_catalog_with_surface（已显示）
- 249-268行：apply_tool_surface_budget（已显示）
- 270-434行：ensure_advanced_tooling（之前已经完全分析过）
- 436-450行：initial_active_tools（已显示）
- 452-473行：active_tool_list_from_catalog（已显示）
- 475-495行：active_tools_for_step（已显示）
- 497-505行：tool_search_haystack（部分显示）

缺失的主要是 suggest_tool_names 函数和相关工具搜索逻辑（大约在506-699行之间）。

让我读取这些缺失的行。

实际上用户要求"逐行分析整个文件"，考虑到文件已经被读过一遍，我现在应该做的就是按模块结构组织一份完整的逐行分析。不需要再读取了——我已经有所有1225行的内容。

让我按以下结构组织分析：
1. 模块文档和常量定义（1-100行）
2. 辅助函数：should_default_defer_tool、apply_native_tool_deferral、apply_mcp_tool_deferral（101-200行）
3. build_model_tool_catalog / build_model_tool_catalog_with_surface / apply_tool_surface_budget（201-268行）
4. ensure_advanced_tooling（270-434行）
5. initial_active_tools、active_tool_list_from_catalog、active_tools_for_step（436-495行）
6. 工具搜索核心：tool_search_haystack、discover_tools_*、suggest_tool_names 等（497-704行）
7. is_synthetic_catalog_tool（706-743行）
8. tool_catalog_consistency_issues（745-785行）
9. missing_tool_error_message、shell_tool_allow_shell_hint、is_shell_tool_name（787-854行）
10. 延迟工具水合：maybe_activate_requested_deferred_tool、maybe_hydrate_requested_deferred_tool、preflight_requested_deferred_tool（856-908行）
11. deferred_tool_schema_hydration_result、schema相关辅助函数（910-1095行）
12. execute_tool_search（1097-1166行）
13. execute_code_execution_tool（1168-1225行）

之前还有缺失的suggest_tool_names和discover_tools相关函数。让我补充读取460-699行。

---

*Thinking:*

现在让我读一下工具搜索函数discover_tools_with_regex和discover_tools_with_bm25_like，它们在中间被截断了。让我继续读取。

---

*Thinking:*

内容在665行附近被截断了。suggest_tool_names 函数可能在那附近。让我用 grep 查找其确切位置，然后读取。

---

*Thinking:*

Now I have all the information I need. Let me compose a comprehensive line-by-line analysis of the entire `tool_catalog.rs` file (1225 lines). I'll organize it by functional sections.

The file is organized as follows:

1. **Module doc & imports** (lines 1-41): Module-level documentation, constants, and imports
2. **Core data: DEFAULT_ACTIVE_NATIVE_TOOLS & fallbacks** (lines 42-127): The master list of native tools and the fallback mechanism
3. **Deferral logic** (lines 129-189): Deferred loading policies for native and MCP tools
4. **Catalog builders** (lines 191-268): Building the full tool catalog
5. **ensure_advanced_tooling** (lines 270-434): Injecting synthetic/built-in tools
6. **Active tool management** (lines 436-495): Determining which tools are active at the start of each turn
7. **Tool discovery/search** (lines 497-633): BM25-like and regex search over catalog
8. **Fuzzy suggestions** (lines 635-704): Edit-distance based tool name suggestions
9. **Consistency checking** (lines 706-785): is_synthetic_catalog_tool and tool_catalog_consistency_issues
10. **Missing tool error messages** (lines 787-854): User-friendly error messages for unknown tools
11. **Deferred tool hydration** (lines 856-908): Activating deferred tools on first use
12. **Schema hydration result** (lines 910-987): Generating detailed feedback when a deferred tool is first used
13. **Schema inspection helpers** (lines 989-1095): SchemaField, schema_fields, schema_required_fields, etc.
14. **execute_tool_search** (lines 1097-1166): Runtime execution of tool_search
15. **execute_code_execution_tool** (lines 1168-1225): Runtime execution of Python code_execution

Let me compose this comprehensive analysis now.

---

*Thinking:*

Now I have all 1225 lines of the file. Let me compile the comprehensive analysis. I have all the pieces needed.

Let me organize the complete line-by-line analysis. The file is 1225 lines total, and I have now read every line through multiple reads. Let me organize this into a coherent analysis.

---

**Assistant:**

现在我已经掌握了全部 1225 行的完整内容，下面按功能模块逐行分析。

---

# `tool_catalog.rs` 完整逐行分析

## 文件概览

该文件是 CodeWhale 工具目录系统的核心，负责以下职责：

1. **定义哪些工具默认激活**（`DEFAULT_ACTIVE_NATIVE_TOOLS`）
2. **实现延迟加载策略**——减少每轮发送给 LLM 的 tool schema 体积，保护 KV 前缀缓存
3. **注入合成/高级工具**（`code_execution`、`js_execution`、`tool_search`）
4. **工具搜索与发现**——BM25 和正则两种匹配引擎
5. **模糊建议**——基于编辑距离的工具名纠错
6. **一致性校验**——cross-check catalog 与 ToolRegistry
7. **延迟工具水合**——首次使用延迟工具时的 schema 反馈

---

## 一、模块文档与导入（第1-19行）

```rust
//! Deferred tool catalog and built-in advanced tool helpers.
//! ...
```

**第1-6行：** 模块级文档。声明本模块拥有"目录级策略"——延迟加载、工具搜索、缺失工具建议、合成工具注入。

**第8-19行：** `use` 导入。关键依赖包括：
| 导入 | 用途 |
|------|------|
| `McpPool` | 判断一个工具名是否属于 MCP 工具（`mcp__*` 前缀） |
| `ToolSurfaceBudget` | 紧凑模式下进一步缩减工具表面 |
| `Tool` / `ToolError` / `ToolResult` | 核心类型 |
| `ExternalTool` | Python/Node 解释器探测 |
| `compile_user_regex` | 将用户正则查询编译为 `regex::Regex` |

---

## 二、常量定义（第23-33行）

```rust
pub(super) const MULTI_TOOL_PARALLEL_NAME: &str = "multi_tool_use.parallel";
pub(super) const REQUEST_USER_INPUT_NAME: &str = "request_user_input";
pub(super) const CODE_EXECUTION_TOOL_NAME: &str = "code_execution";
const CODE_EXECUTION_TOOL_TYPE: &str = "code_execution_20250825";
pub(super) const TOOL_SEARCH_NAME: &str = "tool_search";
const TOOL_SEARCH_TYPE: &str = "tool_search_20251119";
const LEGACY_TOOL_SEARCH_REGEX_NAME: &str = "tool_search_tool_regex";
const LEGACY_TOOL_SEARCH_BM25_NAME: &str = "tool_search_tool_bm25";
const TOOL_SEARCH_DEFAULT_MAX_RESULTS: usize = 20;
const TOOL_SEARCH_MAX_RESULTS_LIMIT: usize = 100;
```

| 行 | 常量 | 说明 |
|----|------|------|
| 23 | `MULTI_TOOL_PARALLEL_NAME` | Anothropic 风格的多工具并行调用名，用于兼容 |
| 26 | `CODE_EXECUTION_TOOL_TYPE` | 带版本号的工具类型标识（`_20250825`），用于 API 层的类型标记 |
| 29 | `TOOL_SEARCH_TYPE` | 同理，版本化类型标识（`_20251119`） |
| 30-31 | `LEGACY_TOOL_SEARCH_*` | 旧版工具名（`tool_search_tool_regex` / `tool_search_tool_bm25`），保留用于向后兼容 |

---

## 三、`is_tool_search_tool`（第35-40行）

```rust
pub(super) fn is_tool_search_tool(name: &str) -> bool {
    matches!(name, TOOL_SEARCH_NAME | LEGACY_TOOL_SEARCH_REGEX_NAME | LEGACY_TOOL_SEARCH_BM25_NAME)
}
```

判断一个工具名是否属于工具搜索家族。包含新旧三个名字，确保遗留调用不会中断。

---

## 四、`DEFAULT_ACTIVE_NATIVE_TOOLS`（第42-70行）

```rust
pub(super) const DEFAULT_ACTIVE_NATIVE_TOOLS: &[&str] = &[
    "agent", "apply_patch", "edit_file", "exec_shell", ...
    "write_file",
];
```

**28 个核心工具**的权威列表。这些工具在所有模式下默认为"活跃"（不被延迟加载）。列表的顺序无关——它仅用于成员测试（通过 `HashSet` 加速）。

> 此数组是**有序迭代**的权威源，但 O(1) 成员测试通过下面的 `DEFAULT_ACTIVE_NATIVE_TOOLS_SET` 实现。

---

## 五、CoreActionToolFallback（第72-127行）

```rust
const CORE_ACTION_TOOL_FALLBACKS: &[CoreActionToolFallback] = &[
    CoreActionToolFallback { name: "exec_shell", ... },
    CoreActionToolFallback { name: "write_file", ... },
    CoreActionToolFallback { name: "edit_file", ... },
    CoreActionToolFallback { name: "apply_patch", ... },
];
```

**第72-100行：** 四个核心操作工具（shell、write、edit、patch）的回退定义。当用户在 `tool_search` 中搜索但搜不到这些工具时（它们被 allow/deny 或模式隐藏），引擎返回 `unavailable_tool_reference` 而非沉默——告诉模型"这个工具存在但当前不可用，以及原因"。

**第102-127行：** `CachedFallback` 结构体和 `cached_fallbacks()` 函数。预计算每个 fallback 的小写 haystack（name + description + unavailable_reason），避免每次搜索都重新格式化。

---

## 六、`should_default_defer_tool`（第136-157行）

```rust
static DEFAULT_ACTIVE_NATIVE_TOOLS_SET: std::sync::OnceLock<HashSet<&'static str>> = ...;

pub(super) fn should_default_defer_tool(name: &str, always_load: &HashSet<String>) -> bool {
    if always_load.contains(name) { return false; }
    if is_tool_search_tool(name) { return false; }
    !default_active_native_tools_set().contains(name)
}
```

**判断一个工具是否应该默认延迟加载。**

| 条件 | 返回 | 原因 |
|------|------|------|
| `always_load` 包含 | `false`（不延迟） | 用户显式要求永远加载 |
| 是 tool_search 家族 | `false`（不延迟） | tool_search 是发现其他延迟工具的入口，自身不能延迟 |
| 在 `DEFAULT_ACTIVE_NATIVE_TOOLS` 中 | `false`（不延迟） | 28 个核心工具默认立即可用 |
| 其他 | `true`（延迟） | 包括 MCP 工具、高级合成工具等 |

**第136-142行：** `OnceLock<HashSet>` 将数组转为 O(1) 查找集合，进程生命周期内仅构建一次。"membership-only test, no ordering dependency"。

---

## 七、`apply_native_tool_deferral`（第159-163行）

```rust
pub(super) fn apply_native_tool_deferral(catalog: &mut [Tool], always_load: &HashSet<String>) {
    for tool in catalog {
        tool.defer_loading = Some(should_default_defer_tool(&tool.name, always_load));
    }
}
```

遍历原生工具列表，为每个工具设置 `defer_loading` 标记。

---

## 八、`should_keep_mcp_tool_loaded` 与 `apply_mcp_tool_deferral`（第165-189行）

```rust
fn should_keep_mcp_tool_loaded(name: &str) -> bool {
    matches!(name, "list_mcp_resources" | "list_mcp_resource_templates"
                  | "mcp_read_resource" | "read_mcp_resource" | "mcp_get_prompt")
}
```

**第165-174行：** MCP 工具的延迟策略不同。5 个"MCP 元操作"工具（列出资源、读取资源、获取提示）始终保持加载——它们是发现其他 MCP 工具的入口。其他 MCP 工具默认延迟。

```rust
pub(super) fn apply_mcp_tool_deferral(catalog: &mut [Tool], mode: AppMode, always_load: &HashSet<String>) {
    for tool in catalog {
        if always_load.contains(&tool.name) { tool.defer_loading = Some(false); continue; }
        tool.defer_loading = Some(mode != AppMode::Yolo && !should_keep_mcp_tool_loaded(&tool.name));
    }
}
```

**第176-189行：** MCP 工具延迟的特殊规则：Yolo 模式下所有 MCP 工具都加载（不延迟），Agent/Plan 模式下仅保留元操作工具。

---

## 九、`build_model_tool_catalog_with_surface`（第201-247行）

```rust
pub(super) fn build_model_tool_catalog_with_surface(
    mut native_tools: Vec<Tool>,
    mut mcp_tools: Vec<Tool>,
    mode: AppMode,
    always_load: &HashSet<String>,
    surface_budget: ToolSurfaceBudget,
) -> Vec<Tool> {
    apply_native_tool_deferral(&mut native_tools, always_load);
    apply_mcp_tool_deferral(&mut mcp_tools, mode, always_load);
    apply_tool_surface_budget(&mut native_tools, surface_budget, always_load);
    apply_tool_surface_budget(&mut mcp_tools, surface_budget, always_load);
    native_tools.sort_by(|a, b| a.name.cmp(&b.name));
    mcp_tools.sort_by(|a, b| a.name.cmp(&b.name));
    native_tools.extend(mcp_tools);
    native_tools
}
```

| 行 | 操作 | 说明 |
|----|------|------|
| 233 | `apply_native_tool_deferral` | 标记原生工具延迟状态 |
| 234 | `apply_mcp_tool_deferral` | 标记 MCP 工具延迟状态 |
| 235-236 | `apply_tool_surface_budget` | 紧凑模式下进一步隐藏重工具 |
| 243-244 | `sort_by` | **关键：按名称排序。** 确保目录字节稳定——DeepSeek KV 前缀缓存要求工具数组不变。 |
| 245 | `native_tools.extend(mcp_tools)` | 原生工具在前（连续前缀），MCP 在尾。这样增删 MCP 不会 shift 原生工具的位置。 |

**第210-224行：** `#[cfg(test)] build_model_tool_catalog` 是测试用的简化版，硬编码 `ToolSurfaceBudget::Standard`。

---

## 十、`apply_tool_surface_budget`（第249-268行）

```rust
fn apply_tool_surface_budget(catalog: &mut [Tool], surface_budget: ToolSurfaceBudget, always_load: &HashSet<String>) {
    if !matches!(surface_budget, ToolSurfaceBudget::Compact) { return; }
    for tool in catalog {
        if always_load.contains(&tool.name) { continue; }
        if matches!(tool.name.as_str(), "agent" | "run_tests" | "run_verifiers" | "task_create" | "web_search") {
            tool.defer_loading = Some(true);
        }
    }
}
```

**仅 Compact 模式下生效。** 将 5 个"重型"工具也强制延迟加载：`agent`、`run_tests`、`run_verifiers`、`task_create`、`web_search`。`always_load` 中的工具不受影响。

---

## 十一、`ensure_advanced_tooling`（第270-434行）

> 前面已详细分析过。注入 `code_execution`（Python 可用时）、`js_execution`（Node 可用时）、`tool_search`（始终注入）。Plan 模式跳过执行类工具。详见前两次分析。

---

## 十二、`initial_active_tools`（第436-450行）

```rust
pub(super) fn initial_active_tools(catalog: &[Tool]) -> HashSet<String> {
    let mut active = HashSet::new();
    for tool in catalog {
        if !tool.defer_loading.unwrap_or(false) || is_tool_search_tool(&tool.name) {
            active.insert(tool.name.clone());
        }
    }
    if active.is_empty() && !catalog.is_empty() {
        if let Some(first) = catalog.first() {
            active.insert(first.name.clone());
        }
    }
    active
}
```

| 行 | 逻辑 | 说明 |
|----|------|------|
| 439 | `!defer_loading` 或 `is_tool_search_tool` | 非延迟工具 + tool_search 默认为活跃。`unwrap_or(false)`：未设置延迟标记的视为不延迟。 |
| 443-448 | 保底逻辑 | 如果所有工具都被延迟了（极端情况），至少激活第一个工具，确保模型至少有工具可用。 |

---

## 十三、`active_tool_list_from_catalog`（第452-473行）

```rust
fn active_tool_list_from_catalog(catalog: &[Tool], active: &HashSet<String>) -> Vec<Tool> {
    let mut head: Vec<Tool> = Vec::with_capacity(catalog_len);
    let mut tail: Vec<Tool> = Vec::with_capacity(catalog_len);
    for tool in catalog {
        if !active.contains(&tool.name) { continue; }
        if tool.defer_loading.unwrap_or(false) {
            tail.push(tool.clone());
        } else {
            head.push(tool.clone());
        }
    }
    head.extend(tail);
    head
}
```

**双遍构建（two-pass）——保护 KV 前缀缓存。**

- **Head（头）：** 始终加载（非延迟）的工具，按目录原始顺序排列。
- **Tail（尾）：** 中途被 `tool_search` 激活的延迟工具，追加到末尾。

> 如果激活的延迟工具插入中间，会 shift 后续所有工具在 JSON 中的字节偏移，导致 KV 缓存从该点开始全部失效。追加到尾部保证已缓存的前缀不受影响。

---

## 十四、`active_tools_for_step`（第475-495行）

```rust
pub(super) fn active_tools_for_step(catalog: &[Tool], active: &HashSet<String>, force_update_plan: bool) -> Vec<Tool> {
    if force_update_plan {
        let forced: Vec<_> = catalog.iter()
            .filter(|tool| tool.name == "update_plan")
            .cloned().collect();
        if !forced.is_empty() { return forced; }
    }
    active_tool_list_from_catalog(catalog, active)
}
```

**特殊模式：快速规划。** 当 `force_update_plan` 为 `true` 时（引擎检测到明显的"做计划"请求），第一轮仅给模型 `update_plan` 一个工具——缩小工具表面，让模型专注规划。DeepSeek reasoning 模型不支持显式 `tool_choice` 强制，所以用这种方法变通实现。

---

## 十五、工具搜索核心函数（第497-633行）

### `tool_search_haystack`（第497-504行）

```rust
fn tool_search_haystack(tool: &Tool) -> String {
    format!("{}\n{}\n{}", tool.name.to_lowercase(), tool.description.to_lowercase(), tool.input_schema.to_string().to_lowercase())
}
```

构造搜索用的 haystack：工具名 + 描述 + JSON schema 全部转小写后用换行拼接。

### `catalog_contains_tool`（第506-508行）

简单的线性扫描，检查工具名是否已在目录中。

### `unavailable_core_action_tools_with_regex`（第510-527行）

正则搜索不可用的核心操作工具（fallback 列表）。先排除已在目录中的，再用 regex 匹配 haystack。

### `unavailable_core_action_tools_with_bm25_like`（第529-572行）

BM25-like 搜索不可用的核心操作工具。**简化版 BM25：**
- 每个 term 在 haystack 中匹配得 1 分
- 每个 term 在工具名中匹配得 2 分（名匹配权重更高）
- 按 score 降序，得分相同按名字字母序

### `discover_tools_with_regex`（第574-596行）

在目录中按正则搜索工具。跳过 `tool_search` 自身（不自引），匹配 `tool_search_haystack`。达 `max_results` 上限时提前 break。

### `discover_tools_with_bm25_like`（第598-633行）

在目录中按 BM25-like 搜索。与 fallback 版逻辑相同：haystack 命中 +1，名字命中 +2。跳过 `tool_search` 自身。

---

## 十六、`edit_distance`（第635-663行）

标准 Levenshtein 编辑距离的迭代实现。使用两个行向量（`prev` / `curr`）交替计算，O(n·m) 时间，O(min(n,m)) 空间。

---

## 十七、`suggest_tool_names`（第665-704行）

```rust
fn suggest_tool_names(catalog: &[Tool], requested: &str, limit: usize) -> Vec<String> {
```

**三层匹配策略，按优先级排序：**

| 优先级 (rank) | 条件 | 示例 |
|---------------|------|------|
| 0 | 前缀匹配（候选以查询开头，或查询以候选开头） | 搜 `exec` → 命中 `exec_shell` |
| 1 | 包含匹配（互包含） | 搜 `shell` → 命中 `exec_shell_wait` |
| 2 | 相近拼写（编辑距离 ≤ 3） | 搜 `wriet` → 命中 `write_file` |

**排序规则（第693-697行）：** 先按 rank，再按编辑距离，再按名字。去重后取前 `limit` 个。

---

## 十八、`is_synthetic_catalog_tool`（第706-743行）

> 前面已详细分析。判断工具是否由引擎合成（非 ToolRegistry 注册）。包含三类：tool_search 家族、code_execution/js_execution、MCP 工具。

---

## 十九、`tool_catalog_consistency_issues`（第745-785行）

> 前面已详细分析。双向交叉校验：catalog → registry（幽灵工具检测）、registry → catalog（隐藏工具检测）。

---

## 二十、`missing_tool_error_message`（第787-835行）

```rust
pub(super) fn missing_tool_error_message(tool_name: &str, catalog: &[Tool]) -> String {
```

模型请求了目录中不存在的工具时的**人性化错误消息**。

**第792-801行：** 特殊处理——如果模型试图调用 `item`/`todo`/`checklist` 等伪工具（模型幻觉，把 checklist 条目当成独立工具调用），直接告知正确的用法（用 `work_update`）。

**第803-834行：** 通用路径：
1. 调用 `suggest_tool_names` 获取 3 个模糊建议
2. 判断是否为 shell 工具丢失（`is_shell_tool_name`），若是则追加 `allow_shell` 配置提示
3. 根据有无建议组合四种错误消息：
   - 无建议 + 有 shell hint
   - 无建议 + 无 shell hint
   - 有建议 + 有 shell hint
   - 有建议 + 无 shell hint

### `shell_tool_allow_shell_hint`（第837-843行）

返回 shell 工具缺失时的针对性帮助文本，告知用户用 `/config allow_shell true` 解决。

### `is_shell_tool_name`（第845-854行）

判断是否为 5 个 shell 工具之一。

---

## 二十一、延迟工具水合（第856-908行）

### `maybe_activate_requested_deferred_tool`（第856-871行，`#[cfg(test)]`）

测试用函数。检查工具是否延迟加载，若是则插入 `active_tools` 集合并返回 `true`。

### `maybe_hydrate_requested_deferred_tool`（第873-888行）

**运行时路径。** 当模型调用一个延迟工具时：
- 检查：工具在目录中、标记为延迟、且 batch 开始时不在活跃集中
- 若是首次使用，记录到 `hydrated_tools_this_batch`
- 返回 `Some(ToolResult)`——包含详细 schema 反馈（不是执行结果！）

### `preflight_requested_deferred_tool`（第890-908行，`#[cfg(test)]`）

测试包装器，调用 `maybe_hydrate_requested_deferred_tool` 后将水合的工具合并入 `active_tools`。

---

## 二十二、`deferred_tool_schema_hydration_result`（第910-987行）

```rust
fn deferred_tool_schema_hydration_result(tool: &Tool, tool_input: &Value) -> ToolResult {
```

**延迟工具首次使用时返回的特殊响应。** 这不是执行结果——而是告诉模型：

1. **工具已加载**（"Tool `xxx` was deferred and has now been loaded."）
2. **未执行**（"The tool was not executed. Retry with the loaded schema."）
3. **完整 schema 信息：**
   - 期望字段（含类型和 required 标记）
   - 收到的字段列表
   - 缺失的必填字段
   - 意外字段
   - **可能的字段名纠正**（见下文 `likely_field_corrections`）

返回的 `metadata`（第973-986行）包含结构化诊断：`event: "tool.schema_hydrated"`、`executed: false`、`retry_required: true`、`reason: "deferred_tool_first_use"` 等。

---

## 二十三、Schema 检查辅助函数（第989-1095行）

### `SchemaField`（第989-993行）

```rust
struct SchemaField { name: String, kind: String }
```

工具 schema 字段的简化表示。

### `schema_fields`（第995-1008行）

从 JSON Schema 的 `properties` 中提取字段列表，按字段名排序。

### `schema_required_fields`（第1010-1020行）

从 JSON Schema 的 `required` 数组提取必填字段名列表。

### `schema_type_label`（第1022-1033行）

构造字段类型的友好标签。如果字段有 `enum` 约束，显示为 `"string (bm25 | regex)"` 格式。

### `received_field_names`（第1035-1042行）

从模型提供的 JSON 输入中提取所有字段名，排序后返回。

### `likely_field_corrections`（第1044-1095行）

**智能字段名纠正系统。** 分析模型传入的字段名，推断它可能想表达什么：

| 条件 | 纠正 |
|------|------|
| 收到 `old_string`、期望 `search` | `old_string → search` |
| 收到 `old_str`、期望 `search` | `old_str → search` |
| 收到 `new_string`、期望 `replace` | `new_string → replace` |
| 收到 `new_str`、期望 `replace` | `new_str → replace` |
| 收到 `replacement`、期望 `replace` | `replacement → replace` |
| `checklist_update`/`todo_update` + 收到 `todos` | 建议用 `work_update` |
| `rlm_open` + 非标准源字段名 | 纠正为 `file_path`/`content`/`url`/`session_object` |

**第1071-1093行：** RLM 字段纠正特别详细——模型容易用 `prompt`、`file`、`path`、`text`、`body`、`source` 等非标准名称，逐个提示正确字段。

---

## 二十四、`execute_tool_search`（第1097-1166行）

```rust
pub(super) fn execute_tool_search(
    tool_name: &str, input: &Value, catalog: &[Tool], active_tools: &mut HashSet<String>,
) -> Result<ToolResult, ToolError> {
```

**`tool_search` 工具的运行时执行逻辑。**

| 行 | 逻辑 |
|----|------|
| 1103 | 提取必填参数 `query` |
| 1104-1108 | 确定匹配算法：遗留工具名硬编码算法，新版从 `match` 参数读取，默认 `"bm25"` |
| 1109-1113 | 验证 match 算法仅允许 `bm25` 或 `regex` |
| 1114-1120 | 解析 `max_results`（默认 20，clamp 到 1..100） |
| 1121-1125 | 调用 `discover_tools_with_regex` 或 `discover_tools_with_bm25_like` |
| 1126-1131 | 剩余名额搜索不可用工具（`unavailable_core_action_tools_*`） |
| 1133-1135 | **副作用：** 将发现的工具名插入 `active_tools`——激活延迟工具 |
| 1137-1156 | 构造响应：`tool_references`（可用的）+ `unavailable_tool_references`（存在但不可用的） |

**响应格式：**
```json
{
    "type": "tool_search_tool_search_result",
    "tool_references": [{"type": "tool_reference", "tool_name": "..."}],
    "unavailable_tool_references": [{"type": "unavailable_tool_reference", "tool_name": "...", "reason": "..."}]
}
```

---

## 二十五、`execute_code_execution_tool`（第1168-1225行）

```rust
pub(super) async fn execute_code_execution_tool(input: &Value, workspace: &Path) -> Result<ToolResult, ToolError> {
```

**Python 代码执行工具的运行时实现。**

| 行 | 步骤 | 说明 |
|----|------|------|
| 1172 | 提取 `code` | 必填参数 |
| 1174-1178 | 注释 | 解释器可能在构建目录后消失（并发卸载等），`tokio_command()` 会返回 `None` 并快速失败 |
| 1180-1188 | 注释 | **为何用临时文件而非 `-c`：** (1) Windows argv 长度限制；(2) 多行代码引号嵌套脆弱；(3) traceback 引用真实文件名 |
| 1189-1191 | 创建临时目录 | `tempfile::tempdir()` |
| 1192-1194 | 写入脚本 | `code_execution.py` |
| 1196-1201 | 构造命令 | `python <script_path>` + `current_dir(workspace)` |
| 1203-1206 | 执行 | `tokio::time::timeout(120s)` 超时保护 |
| 1208-1210 | 收集输出 | stdout、stderr、return_code |
| 1212-1218 | 构造响应 | JSON 格式的结果，含 `type: "code_execution_result"` |

---

## 全文件架构总图

```
tool_catalog.rs (1225行)
│
├── 常量与数据 (1-157)
│   ├── 工具名常量 (code_execution, tool_search, ...)
│   ├── DEFAULT_ACTIVE_NATIVE_TOOLS (28个核心工具)
│   ├── CoreActionToolFallback (4个回退定义 + 缓存)
│   ├── is_tool_search_tool
│   └── should_default_defer_tool (延迟判断)
│
├── 延迟应用 (159-189)
│   ├── apply_native_tool_deferral
│   ├── should_keep_mcp_tool_loaded
│   └── apply_mcp_tool_deferral
│
├── 目录构建 (201-268)
│   ├── build_model_tool_catalog / _with_surface
│   └── apply_tool_surface_budget (Compact模式)
│
├── 合成工具注入 (270-434)
│   └── ensure_advanced_tooling
│       ├── code_execution (Python)
│       ├── js_execution (Node.js)
│       └── tool_search (始终)
│
├── 活跃工具管理 (436-495)
│   ├── initial_active_tools
│   ├── active_tool_list_from_catalog (two-pass)
│   └── active_tools_for_step (force_update_plan)
│
├── 工具搜索引擎 (497-704)
│   ├── tool_search_haystack
│   ├── discover_tools_with_regex
│   ├── discover_tools_with_bm25_like
│   ├── unavailable_core_action_tools_with_regex / _bm25_like
│   ├── edit_distance
│   └── suggest_tool_names
│
├── 一致性检查 (706-785)
│   ├── is_synthetic_catalog_tool
│   └── tool_catalog_consistency_issues
│
├── 错误消息 (787-854)
│   ├── missing_tool_error_message
│   ├── shell_tool_allow_shell_hint
│   └── is_shell_tool_name
│
├── 延迟工具水合 (856-987)
│   ├── maybe_activate_requested_deferred_tool [test]
│   ├── maybe_hydrate_requested_deferred_tool
│   ├── preflight_requested_deferred_tool [test]
│   └── deferred_tool_schema_hydration_result
│
├── Schema辅助 (989-1095)
│   ├── SchemaField, schema_fields
│   ├── schema_required_fields, schema_type_label
│   ├── received_field_names
│   └── likely_field_corrections
│
├── 工具执行 (1097-1225)
│   ├── execute_tool_search
│   └── execute_code_execution_tool
│
└── [无测试模块 — 测试在独立文件]
```

---

这就是 `tool_catalog.rs` 全部 1225 行的完整逐行分析。核心设计原则贯穿始终：**KV 前缀缓存保护**（目录头部稳定、延迟工具追加尾部）、**按需探测**（Python/Node 运行时可用性）、**优雅降级与纠错**（不可用工具回退、字段名纠正、模糊建议）。

---

