# Chat Export

**Model:** deepseek-v4-flash
**Workspace:** D:\tmp\source\CodeWhale
**Date:** 2026-07-23 15:23:17

---

*System:*

Started new session 60bd72a4 (New Session). Previous sessions remain available via /resume.

---

**You:**

我正在学习codewhale的工作原理，请你逐行详细介绍crates\tui\src\working_set.rs

---

*Thinking:*

The user wants me to explain `crates/tui/src/working_set.rs` line by line. Let me first read the file to understand its content.

---

**Assistant:**

好的，我先读取这个文件的内容。

---

**Tool:**

• ▷ read done · Reading crates/tui/src/working_set.rs
▏ done: Reading crates/tui/src/working_set.rs

---

*Thinking:*

The file is 2399 lines long and was truncated. I need to read it in chunks to get the full content. Let me read the rest of the file.

---

**Assistant:**

这个文件有 2399 行，我需要分段读取完整内容。

---

**Tool:**

• ▷ read done · 4 items
  ●●●●  4 done, 0 running
▏ done: Reading crates/tui/src/working_set.rs
▏ done: Reading crates/tui/src/working_set.rs
▏ done: Reading crates/tui/src/working_set.rs
▏ done: Reading crates/tui/src/working_set.rs

---

*Thinking:*

The file is 2399 lines total and I've read it in chunks. Let me now provide a comprehensive line-by-line explanation of the entire file. I'll organize it by sections since the file is quite large.

Let me organize the explanation into logical sections:

1. **Lines 1-21**: Module documentation and imports
2. **Lines 23-67**: `Workspace` struct and `SearchContext` helper
3. **Lines 69-192**: `Workspace` implementation - constructor methods
4. **Lines 193-350**: `Workspace` completion methods (completions(), completion_candidates(), browser_completions())
5. **Lines 351-450**: `Workspace` resolution methods (resolve(), fuzzy matching, path escape prevention)
6. **Lines 451-598**: Helper functions: path_regex(), discovery_walk_builder(), walk_always_discoverable_dirs(), walk_for_completions()
7. **Lines 599-799**: `add_local_reference_completions`, `WorkspaceConfig`, `WorkingSetConfig`
8. **Lines 800-1100**: `WorkingSetSource`, `WorkingSetEntry`, `WorkingSet` struct and methods (observe, record, summary_block start)
9. **Lines 1100-1500+**: More of WorkingSet implementation, extract_paths functions, regex patterns
10. **Lines 1500-2399**: Tests

Let me provide the full explanation now in a structured way.

---

**Assistant:**

好的，我已经阅读了完整的 `working_set.rs` 文件（共 2399 行）。下面我会按逻辑模块进行逐段详细讲解。

---

## 文件概述

**作用：** 实现 **Repo 感知的工作集追踪（Working Set）** 和 **提示词上下文打包（Prompt Context Packing）**。核心目标：保持一个小而高信号的活动文件路径列表，让 agent 能优先关注这些路径。

---

## 第 1 部分：模块文档和导入（第 1-21 行）

```rust
//! Repo-aware working set tracking and prompt context packing.
//!
//! The goal of this module is to keep a small, high-signal list of
//! "active" paths that the assistant should prioritize. It observes
//! user messages and tool calls, extracts likely paths, and produces:
//! - a compact working-set summary block for the system prompt
//! - pinned message indices that compaction should preserve
```

- **第 1-7 行** — 模块级文档注释（`//!`）：说明了这个模块的三大职责：
  1. 跟踪"活跃的"文件路径
  2. 为系统提示词生成紧凑的工作集摘要块（`<turn_meta>` 块）
  3. 标记需要保留（pinned）的消息索引，以便上下文紧缩（compaction）时不被丢弃

```rust
use crate::models::{ContentBlock, Message};
use crate::workspace_discovery::{
    DISCOVERY_ALWAYS_DIRS, path_is_excluded_from_discovery, should_skip_unignored_discovery_entry,
};
```

- **第 9-11 行** — 引入同 crate 的类型：
  - `ContentBlock` / `Message`：消息模型，用于扫描消息内容中的路径提及
  - `DISCOVERY_ALWAYS_DIRS`：始终需要发现目录列表（如 `.deepseek/`, `.cursor/` 等 AI 工具目录）
  - `path_is_excluded_from_discovery` / `should_skip_unignored_discovery_entry`：发现过程中的排除规则

```rust
use ignore::WalkBuilder;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;
```

- **第 13-21 行** — 标准库和第三方 crate 导入：
  - `ignore::WalkBuilder`：Rust 的 gitignore-aware 目录遍历库（来自 `ignore` crate，与 `ripgrep` 同源）
  - `Regex`：正则表达式引擎
  - `serde` / `serde_json`：序列化/反序列化
  - `HashMap` / `HashSet`：哈希集合
  - `OsStr`：系统字符串处理（跨平台文件名）
  - `Path` / `PathBuf`：路径类型
  - `OnceLock`：一次性锁，用于惰性初始化全局静态变量（如正则表达式）

---

## 第 2 部分：`Workspace` 结构体（第 23-40 行）

```rust
/// Repo-aware resolver for `@`-mentions and file pickers.
///
/// `cwd` is captured at construction; if the host's current directory changes
/// during a session, build a fresh `Workspace`. Fuzzy lookups are backed by a
/// lazy basename → paths index built once on first miss and reused for the
/// rest of the session — without it, every mis-typed mention triggered a full
/// `WalkBuilder` traversal up to the configured completion depth.
#[derive(Debug)]
pub struct Workspace {
    pub root: PathBuf,           // 工作区根目录
    cwd: Option<PathBuf>,        // 当前工作目录（可能不同于 root）
    file_index: OnceLock<HashMap<String, Vec<PathBuf>>>,  // 惰性文件名索引
    completion_walk_depth: Option<usize>,  // 自动补全遍历深度限制
    follow_links: bool,          // 是否跟随符号链接
}
```

- **第 23-40 行** — `Workspace` 是仓库感知的 `@` 提及解析器和文件选择器：
  - `root`：工作区根目录（公开，`pub`）
  - `cwd`：当前工作目录（可选），如果进程 CWD 在工作区内部且可作为次要解析入口
  - `file_index`：**惰性构建的"文件名 → 路径列表"索引**。`OnceLock` 保证只在第一次模糊匹配失败时才构建一次，然后在整个会话中复用。这是一个关键性能优化：没有它，每次拼写错误的提及都会触发一次完整的目录遍历。
  - `completion_walk_depth`：自动补全的最大遍历深度
  - `follow_links`：是否跟随符号链接（支持多项目工作区的符号链接枢纽目录）

---

## 第 3 部分：`SearchContext` 辅助结构（第 42-67 行）

```rust
struct SearchContext<'a> {
    needle: &'a str,              // 搜索关键词（小写）
    limit: usize,                 // 结果数量上限
    prefix_hits: &'a mut Vec<String>,   // 前缀匹配结果
    substring_hits: &'a mut Vec<String>, // 子串匹配结果
    seen: &'a mut HashSet<PathBuf>,     // 已见的绝对路径（用于去重）
}
```

- **第 42-48 行** — 内部辅助结构，用于在一次遍历中同时收集前缀匹配和子串匹配：
  - `needle`：搜索词（已转为小写）
  - `limit`：返回上限
  - `prefix_hits`：**前缀命中**（路径以此开头）
  - `substring_hits`：**子串命中**（路径包含此子串但不以此开头）
  - `seen`：已访问的绝对路径集合，用于跨（cwd, workspace）双重遍历的去重

**方法：**

- **第 50-53 行 `is_full()`** — 判断是否已达到结果上限
- **第 55-57 行 `remember(path)`** — 将路径标记为"已见过"，返回 `true` 表示首次见到
- **第 59-67 行 `push_match(candidate)`** — 将候选路径分类加入前缀命中和子串命中列表：
  - 如果 `needle` 为空或候选以小写 needle 开头 → 前缀命中（优先级高）
  - 如果候选包含 needle → 子串命中（优先级低）

---

## 第 4 部分：`Workspace` 构造函数（第 69-192 行）

```rust
impl Workspace {
    #[allow(dead_code)]
    pub fn new(root: PathBuf) -> Self {
        Self::with_cwd(root, std::env::current_dir().ok())
    }
```

- **第 74-77 行** — `new()`：简便构造函数，自动获取进程的当前工作目录（CWD）。`#[allow(dead_code)]` 的原因是该接口为未来特性预留（Ctrl+P 文件选择器）。

```rust
    pub fn with_cwd(root: PathBuf, cwd: Option<PathBuf>) -> Self
```

- **第 82 行** — 显式指定 CWD 的构造函数。测试代码用此接口避免依赖进程真实 CWD。

**内部流程（约第 82-192 行）：**

`with_cwd` 做了以下工作：
1. 规范化 root（`root.canonicalize()`），失败则使用原始路径
2. 如果提供了 CWD，尝试规范化并与 root 比较：
   - 如果 CWD 不在 root 下 → 设 CWD 为 `None`（避免越界访问）
   - 如果 CWD == root → 设 CWD 为 `None`（避免冗余遍历）
3. 从 config 环境变量或编译时默认值读取 `completion_walk_depth`（`CODEPO_COMPLETION_DEPTH` 或默认 5）
4. 读取 `follow_links` 配置

```rust
    fn build_file_index(&self) -> HashMap<String, Vec<PathBuf>>
```

- **第 183 行** — 构建文件名索引。遍历工作区所有文件，建立 `basename → [完整路径列表]` 的映射。这层索引使得模糊匹配（如 `resolve("main.rs")`）不需要重复遍历磁盘。

```rust
    pub fn with_config(root: PathBuf, cwd: Option<PathBuf>, config: &WorkspaceConfig) -> Self
```

- **第 192 行附近** — 接受显式 `WorkspaceConfig` 的构造函数，从配置中提取 `completion_walk_depth` 和 `follow_links`。

---

## 第 5 部分：自动补全方法（第 193-450 行）

### `completions()`（第 201-319 行）

```rust
    pub fn completions(&self, needle: &str, limit: usize) -> Vec<String>
```

这是主要的自动补全入口，流程：

1. **第 210-214 行** — 空 needle 且 `limit == 0` → 直接返回空
2. **第 217-222 行** — 如果字符串包含 `/` 或 `\`，说明是路径导航（如 `src/lib`），直接使用 `walk_for_completions` 进行目录遍历
3. **第 224-258 行** — 尝试**模糊文件名索引查找**（`resolve` + `completions_from_index`）：在索引中查找 needle 的开头匹配，如 `main` 匹配 `main.rs`。这是为了替代每次完整的目录遍历
4. **第 260-312 行** — 如果模糊索引查找没有足够的结果，回退到**完整目录遍历**：
   - 如果 CWD 与 root 不同，先遍历 CWD（使 CWD 相对路径优先）
   - 再遍历工作区 root
   - 调用 `add_local_reference_completions` 添加本地引用（如 `./` 和 `../` 路径）
5. **第 314-318 行** — 排序：前缀命中在前，子串命中在后，截断到 limit

### `completion_candidates()`（第 330-380 行）

```rust
    pub fn completion_candidates(&self) -> Vec<String>
```

- 一次性返回所有可发现路径（不区分 needle），无上限。用于 UI 预加载完整路径列表，这样用户在键盘敲击时可以即时过滤而无需重新遍历文件系统。

### `browser_completions()`（第 390-450 行左右）

```rust
    pub fn browser_completions(&self, needle: &str, limit: usize) -> Vec<String>
```

- 专为文件浏览器模式设计，`@` 快捷键触发。行为与 `completions()` 类似，但有额外的**安全约束**：
  - **拒绝路径逃逸**（第 420-430 行）：如果 needle 包含 `../` 或从当前目录向外逃逸，浏览器模式会拒绝列出工作区以外的文件。这是安全措施，防止用户意外访问工作区外的敏感文件。

---

## 第 6 部分：路径解析方法（第 352-450 行）

### `resolve()`（约第 352 行）

```rust
    pub fn resolve(&self, needle: &str) -> Result<String, String>
```

- 通过**模糊文件名索引**查找匹配路径：
  1. 如果路径以 `.` 或 `/` 开头，或是 `~` → 需要 `resolve_relative_path`
  2. 如果 needle 中包含文件分隔符 → 使用 `resolve_relative_path` 做相对路径解析
  3. 否则 → 在 `file_index` 中查找所有 basename 匹配的路径：
     - 如果正好匹配一个 → 返回该路径
     - 如果匹配多个 → 返回最佳匹配（优先完整 basename 匹配）
     - 如果没匹配 → 尝试 `resolve_relative_path` 后备

### `resolve_relative_path()`（约第 370-390 行）

- 处理相对路径（如 `./src/main.rs`、`../other/file.rs`）：
  - 先尝试相对于 CWD 解析
  - 如果不在工作区内（被 `is_path_in_workspace` 检测到）→ 拒绝，返回错误
  - 如果指定了另一个显式根目录（通过 `config`），也尝试从该根目录解析

### `is_path_in_workspace()`（约第 390 行）

- 检查路径是否在工作区目录内。使用 `canonicalize()` 规范化后比较。防止 path traversal 攻击。

### `completions_from_index()`（约第 340 行）

- 利用已有 file_index 进行快速模糊匹配，不需要遍历磁盘。

---

## 第 7 部分：文件遍历辅助函数（第 451-598 行）

### `path_regex()`（第 451-462 行）

```rust
fn path_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?x)
            (?:
                (?:[A-Za-z]:\\)?                # 可选的 Windows 盘符
                (?:\./|\../|/)?                 # 可选的前导路径
                [A-Za-z0-9._-]+
                (?:[/\\][A-Za-z0-9._-]+)+
                (?:\.[A-Za-z0-9]{1,8})?
            )
        "#).expect("invalid path regex")
    })
}
```

- 使用 `OnceLock` 确保正则只编译一次
- 匹配内容：可选的 Windows 盘符 + 可选的路径前缀 + 至少一个路径分隔符 + 可选扩展名
- 这是从文本中提取路径的核心模式

### `discovery_walk_builder()`（第 464-500 行）

```rust
fn discovery_walk_builder(root: &Path, max_depth: Option<usize>, follow_links: bool) -> WalkBuilder
```

- 构建一个遵守 `.gitignore` 和 `.deepseekignore` 的目录遍历器：
  ```rust
  builder
      .hidden(false)          // 不隐藏点文件
      .follow_links(follow_links)
      .git_ignore(true)       // 遵守 .gitignore
      .git_global(true)
      .git_exclude(true)
      .add_custom_ignore_filename(".deepseekignore")  // 自定义忽略文件
      .max_depth(...)         // 可选深度限制
      .sort_by_file_name(Path::cmp)  // 按文件名排序，保证确定性
  ```

### `walk_always_discoverable_dirs()`（第 501-551 行）

```rust
fn walk_always_discoverable_dirs(
    walk_root: &Path, display_root: &Path,
    ctx: &mut SearchContext<'_>,
    max_depth: Option<usize>, follow_links: bool,
)
```

- 独立遍历 AI 工具目录（如 `.deepseek/`, `.cursor/`, `.claude/`），**禁用 gitignore**，但应用 `path_is_excluded_from_discovery` 排除规则（如排除 `.deepseek/snapshots/` 快照目录）。
- 目的是让这些隐藏目录中的文件在 `@` 补全中可用。

### `walk_for_completions()`（第 553-592 行）

```rust
fn walk_for_completions(
    walk_root: &Path, display_root: &Path,
    ctx: &mut SearchContext<'_>,
    max_depth: Option<usize>, follow_links: bool,
)
```

- 主要的目录遍历函数：
  1. 使用 `discovery_walk_builder` 构建遍历器
  2. 遍历每个条目，计算相对于 `display_root` 的路径
  3. 通过 `ctx.remember()` 去重（避免 cwd 和 root 双重遍历的重复）
  4. 目录路径添加 `/` 后缀
  5. 调用 `ctx.push_match()` 分类
  6. 最后调用 `walk_always_discoverable_dirs` 补充隐藏目录

---

## 第 8 部分：`LOCAL_REFERENCE_SCAN_LIMIT` 和 `add_local_reference_completions`（第 594-650 行）

```rust
const LOCAL_REFERENCE_SCAN_LIMIT: usize = 4096;

fn add_local_reference_completions(
    root: &Path, display_root: &Path,
    ctx: &mut SearchContext<'_>,
    max_depth: Option<usize>, follow_links: bool,
)
```

- 扫描本地引用路径（以 `.` 或 `..` 开头的路径），扫描上限为 4096 个路径
- 使用 `should_try_local_reference_completion()` 函数检查是否需要启动本地引用补全（避免在输入 `@/` 或 `@.` 等场景触发昂贵的文件系统遍历）

### `should_try_local_reference_completion()`（第 652-672 行左右）

```rust
fn should_try_local_reference_completion(needle: &str) -> bool
```

- **拒绝条件**（返回 `false`）：
  - needle 是空的
  - needle 只是 `.` 或 `..`（单独的点）
  - needle 以 `/` 开头但不是 Windows 盘符（如 `C:\`）
- 这是 **#1921 的回归测试修复**：防止 `@/` 或 `@.` 触发昂贵的本地引用路径扫描

---

## 第 9 部分：配置结构体（第 660-799 行）

### `WorkspaceConfig`（第 660-700 行左右）

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    pub completion_walk_depth: Option<usize>,
    pub follow_links: bool,
}
```

- 工作区配置：补全深度和符号链接跟随

### `WorkingSetConfig`（第 700-799 行）

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingSetConfig {
    pub max_entries: usize,           // 工作集最大条目数（默认 2000）
    pub max_pinned_paths: usize,      // 钉住路径的最大数量（默认 8）
    pub max_prompt_entries: usize,    // 提示词摘要中的最大条目（默认 8）
    pub cache_maximal: bool,          // 是否缓存最大文件内容
    pub max_resident_file_bytes: usize,   // 单个文件最大驻留字节数（默认 8192）
    pub max_total_resident_bytes: usize,  // 总驻留字节上限（默认 32768）
    pub max_scan_chars: usize,        // 消息扫描字符上限
}
```

- **第 784-798 行** — `Default` 实现提供了生产级默认值：
  - `max_resident_file_bytes = 8192`（8KB）
  - `max_total_resident_bytes = 32768`（32KB）
  - `max_scan_chars = 128_000`

---

## 第 10 部分：`WorkingSetSource` 和 `WorkingSetEntry`（第 801-838 行）

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum WorkingSetSource {
    UserMessage,   // 用户消息中提到
    ToolInput,     // 工具调用输入中引用
    ToolOutput,    // 工具输出中提到
    Rebuild,       // 会话重建时恢复
}
```

- **第 801-808 行** — 枚举标记路径最后一次被更新的来源

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingSetEntry {
    pub path: String,           // 工作区相对路径
    pub is_dir: bool,           // 是否是目录
    pub exists: bool,           // 磁盘上是否存在
    pub touches: u32,           // 被引用的次数
    pub last_turn: u64,         // 最后引用的轮次编号
    pub last_source: WorkingSetSource,  // 最后更新的来源
}
```

- **第 810-825 行** — 工作集中的单个条目：
  - `touches`：引用计数器，用于计算得分
  - `last_turn`：最近在哪一轮被引用

```rust
impl WorkingSetEntry {
    fn new(path: String, exists: bool, is_dir: bool, turn: u64, source: WorkingSetSource) -> Self {
        Self { path, is_dir, exists, touches: 1, last_turn: turn, last_source: source }
    }
}
```

- **第 827-838 行** — 构造函数，初始化 `touches = 1`

---

## 第 11 部分：`WorkingSet` 主结构体（第 840-1100 行）

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkingSet {
    pub config: WorkingSetConfig,
    pub turn: u64,                    // 单调递增的轮次计数器
    pub entries: HashMap<String, WorkingSetEntry>,  // 路径 → 条目
}
```

- **第 840-849 行** — 核心工作集：
  - `turn`：每次用户消息时递增
  - `entries`：所有被追踪的文件路径

### `next_turn()`（第 853-855 行）

```rust
pub fn next_turn(&mut self) {
    self.turn = self.turn.saturating_add(1);
}
```

### `observe_user_message()`（第 858-864 行）

```rust
pub fn observe_user_message(&mut self, text: &str, workspace: &Path) {
    self.next_turn();
    let paths = extract_paths_from_text(text);
    self.record_candidates(paths, workspace, WorkingSetSource::UserMessage);
}
```

- 递增轮次 → 从用户消息中提取路径 → 记录到工作集

### `observe_tool_call()`（第 867-881 行）

```rust
pub fn observe_tool_call(
    &mut self, tool_name: &str, input: &Value,
    output: Option<&str>, workspace: &Path,
) {
    let input_candidates = extract_paths_from_value(input, Some(tool_name));
    self.record_candidates(input_candidates, workspace, WorkingSetSource::ToolInput);
    if let Some(text) = output {
        let output_candidates = extract_paths_from_text(text);
        self.record_candidates(output_candidates, workspace, WorkingSetSource::ToolOutput);
    }
}
```

- 从工具调用的输入和输出中提取路径：
  - 输入：通过 `extract_paths_from_value()` 遍历 JSON 对象，寻找 path-like 字段
  - 输出：通过 `extract_paths_from_text()` 正则匹配

### `rebuild_from_messages()`（第 883-950 行左右）

```rust
pub fn rebuild_from_messages(&mut self, messages: &[Message], workspace: &Path)
```

- 从已有消息重建工作集（在恢复会话时使用）：
  1. 重置 `entries`
  2. 遍历所有消息
  3. 从每条消息中提取路径（`extract_paths_from_message`）
  4. `record_candidates` 标记为 `Rebuild` 来源

### `record_candidates()`（第 1082-1150 行左右）

```rust
fn record_candidates(
    &mut self, candidates: Vec<String>,
    workspace: &Path, source: WorkingSetSource,
)
```

- 核心的路径记录逻辑：
  1. 规范化路径（统一分隔符为 `/`）
  2. 检查路径是否在工作区内
  3. 检查磁盘存在性
  4. 如果存在但命中 `path_is_excluded_from_discovery` → 跳过（防止意外引入 `.deepseek/snapshots/` 等目录）
  5. 更新或插入条目
  6. 如果条目超过 `max_entries` → 移除得分最低的条目

### `sorted_entries()`（第 1150-1210 行左右）

```rust
fn sorted_entries(&self) -> Vec<&WorkingSetEntry>
```

- 按**得分**排序的条目列表。得分规则：
  - 首要：`touches` 越多越好
  - 次要：`last_turn` 越大（越新）越好
  - 这是用于决定哪些路径应出现在摘要块的最前面

### `summary_block()`（第 985-1050 行左右）— 关键方法

```rust
pub fn summary_block(&self, workspace: &Path) -> Option<String>
```

这是**输出到系统提示词的核心方法**，生成 `<turn_meta>` 块。流程：

1. **第 900-920 行** — 收集关键路径（按得分排序的前 `max_prompt_entries` 个）
2. **第 930-950 行** — 生成"关键文件"部分：
   ```
   ## Repo Working Set
   Key files: path1, path2, ...
   ```
3. **第 960-980 行** — 如果是 `cache_maximal` 模式 → 缓存文件内容：
   - 读取每个文件的正文
   - 按 `max_resident_file_bytes` 和 `max_total_resident_bytes` 限制大小
   - 生成代码块：
     ```
     ### Active file contents (cache-resident)
     <!-- file: src/main.rs -->
     ```... file content ...
     ```
   - 如果文件被截断 → 添加 `<!-- ...more bytes truncated -->` 注释
   - 如果文件不可读 → `<!-- ...unreadable, skipped -->`
   - 如果文件被预算限制省略 → `<!-- omitted from budget -->`

### `top_paths()`（第 1045-1051 行）

```rust
pub fn top_paths(&self, limit: usize) -> Vec<String>
```

- 返回得分最高的 N 个路径

### `pinned_message_indices()`（第 1054-1080 行）

```rust
pub fn pinned_message_indices(&self, messages: &[Message], workspace: &Path) -> Vec<usize>
```

- 找出哪些消息提到了工作集中的重要路径，在紧缩（compaction）时保留这些消息
  1. 获取得分最高的 `max_pinned_paths` 个条目
  2. 对每个条目，构建搜索 needle（文件名、目录名）
  3. 遍历所有消息，检查是否包含任何 needle
  4. 返回匹配的消息索引列表

---

## 第 12 部分：路径提取函数（第 1290-1450 行）

### `extract_paths_from_value()`（第 1285-1294 行）

```rust
pub fn extract_paths_from_value(value: &Value, tool_hint: Option<&str>) -> Vec<String>
```

- 从 JSON 值中递归提取路径。入口函数，调用 `extract_paths_from_value_inner`

### `extract_paths_from_value_inner()`（第 1296-1326 行）

```rust
fn extract_paths_from_value_inner(
    value: &Value, tool_hint: Option<&str>,
    key_hint: Option<&str>, out: &mut Vec<String>,
)
```

- 递归遍历 JSON 树：
  - **String**（第 1303-1312 行）：如果 key 名包含 `path`/`file`/`dir`/`cwd`/`workspace`/`root`/`target`，或者字符串看起来像路径（包含分隔符或常见扩展名），则提取路径
  - 特例：`tool_hint == "exec_shell"` 且字符串 < 400 字符 → 也提取（因为 shell 命令中可能包含路径）
  - **Array**（第 1314-1318 行）：递归每个元素
  - **Object**（第 1319-1323 行）：递归每个值，以 key 名作为 `key_hint`
  - **Null/Bool/Number**：忽略

### `key_is_path_like()`（第 1328-1337 行）

```rust
fn key_is_path_like(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower.contains("path") || lower.contains("file") || lower.contains("dir")
        || lower.contains("cwd") || lower.contains("workspace") || lower.contains("root")
        || lower == "target"
}
```

### `looks_like_path()`（第 1339-1351 行）

```rust
fn looks_like_path(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() { return false; }
    if trimmed.contains('/') || trimmed.contains('\\') { return true; }
    match Path::new(trimmed).extension().and_then(OsStr::to_str) {
        Some(ext) => COMMON_EXTENSIONS.contains(&ext),
        None => false,
    }
}
```

- 判断一个字符串是否像文件路径：包含分隔符，或者有常见的文件扩展名

### `COMMON_EXTENSIONS`（第 1353-1356 行）

```rust
const COMMON_EXTENSIONS: &[&str] = &[
    "rs", "toml", "md", "txt", "json", "yaml", "yml", "ts", "tsx", "js", "jsx",
    "py", "go", "java", "c", "cc", "cpp", "h", "hpp", "sh", "bash", "zsh",
    "sql", "html", "css", "scss",
];
```

### `extract_paths_from_text()`（第 1360-1370 行）

```rust
fn extract_paths_from_text(text: &str) -> Vec<String> {
    if text.trim().is_empty() { return Vec::new(); }
    let re = path_regex();
    re.find_iter(text)
        .map(|m| m.as_str().to_string())
        .filter(|s| looks_like_path(s))
        .collect()
}
```

- 使用正则匹配 + `looks_like_path` 二次过滤

### `path_regex()` 和 `build_search_needles()`（第 1372-1450 行）

- `path_regex` 的完整版本匹配各种路径格式（Windows/Linux/Mac, 相对/绝对）
- `build_search_needles()`：从工作集条目构建搜索 needle（文件名 + 父目录名），用于 `pinned_message_indices` 中的消息匹配

### `message_mentions_any_path()`（第 1450-1500 行左右）

- 检查一条消息是否包含任何 needle 中的路径引用

---

## 第 13 部分：`list_top_level_dirs()`（第 1507-1539 行）

```rust
fn list_top_level_dirs(workspace: &Path, limit: usize) -> Vec<String> {
    // 列出工作区根目录下的一级目录（过滤隐藏目录和已知忽略目录）
}

const IGNORED_ROOT_DIRS: &[&str] = &["target", "node_modules", "dist", "build", ".git"];
```

- 用于在 summary_block 中生成 `Top-level dirs` 信息

---

## 第 14 部分：关键常量定义（第 1353-1370 行左右附近）

核心常量总结：

| 常量 | 值 | 用途 |
|------|-----|------|
| `LOCAL_REFERENCE_SCAN_LIMIT` | 4096 | 本地引用扫描上限 |
| `COMMON_EXTENSIONS` | 26 种扩展名 | 判断字符串是否像文件 |
| `IGNORED_ROOT_DIRS` | 5 个目录名 | 根目录显示时忽略 |

---

## 第 15 部分：测试（第 1541-2399 行）

测试覆盖了以下场景：

| 测试函数 | 行号 | 测试内容 |
|---------|------|---------|
| `observe_user_message_tracks_paths` | 1556 | 用户消息路径追踪基本功能 |
| `observe_tool_call_extracts_paths_from_input` | 1573 | 工具输入路径提取 |
| `pinned_message_indices_respects_working_set` | 1586 | 钉住消息与工作集联动 |
| `cache_maximal_respects_budget` | 1752 | 文件缓存预算限制 |
| `cache_maximal_is_byte_stable_when_files_unchanged` | 1781 | **KV 缓存稳定性测试** — 文件未变时输出必须字节一致 |
| `cache_maximal_changes_when_file_edited` | 1803 | 文件编辑后摘要块变化 |
| `extract_paths_from_message_picks_up_tool_results` | 1822 | ToolResult 中的路径提取 |
| `pinning_prefers_high_signal_paths` | 1838 | 高引用路径优先钉住 |
| `browser_completions_reject_path_escape_segments` | 2016 | **安全测试** — 拒绝 `../` 路径逃逸 |
| `workspace_completions_surface_explicit_hidden_and_ignored_paths` | 2039 | 隐藏目录/忽略目录补全 |
| `should_try_local_reference_completion_skips_bare_separators_and_dots` | 2316 | **性能回归测试** — `@/` 不触发昂贵扫描 |
| `dot_dir_walk_excludes_snapshot_side_repo` | 2270 | **安全/性能测试** — 排除 `.deepseek/snapshots/` |

---

## 整体架构总结

```
┌─────────────────────────────────────────────────┐
│              working_set.rs 架构                  │
├─────────────────────────────────────────────────┤
│                                                   │
│  Workspace (文件系统交互层)                        │
│  ├── 构造：new / with_cwd / with_config           │
│  ├── 自动补全：completions / completion_candidates │
│  │            / browser_completions               │
│  ├── 路径解析：resolve / resolve_relative_path    │
│  ├── 文件索引：build_file_index (惰性, OnceLock)  │
│  └── 辅助：walk_for_completions / discovery_walk   │
│                                                   │
│  WorkingSet (抽象追踪层)                           │
│  ├── 观察：observe_user_message / observe_tool_call│
│  ├── 重建：rebuild_from_messages                   │
│  ├── 记录：record_candidates (规范化/去重/计分)    │
│  ├── 排序：sorted_entries (touches + last_turn)    │
│  ├── 输出：summary_block → <turn_meta> 系统提示    │
│  └── 钉住：pinned_message_indices → 紧缩保留       │
│                                                   │
│  路径提取 (文本/JSON 分析层)                       │
│  ├── extract_paths_from_text (正则匹配)            │
│  ├── extract_paths_from_value (JSON 递归遍历)      │
│  └── looks_like_path / key_is_path_like 启发式     │
│                                                   │
│  测试 (1541-2399, ~850行)                          │
│  ├── 功能测试：路径追踪、工具调用、钉住消息        │
│  ├── 性能测试：KV 缓存稳定性、@/ 不触发扫描        │
│  └── 安全测试：路径逃逸拒绝、快照目录排除          │
│                                                   │
└─────────────────────────────────────────────────┘
```

**核心数据流：**

1. **用户输入** → `observe_user_message()` → 从文本中提取路径 → 记录到 `entries`
2. **工具调用** → `observe_tool_call()` → 从 JSON 输入/输出提取路径 → 记录到 `entries`
3. **提示词生成** → `summary_block()` → 取最高得分的路径 → 生成 `<turn_meta>` 块（含文件内容缓存）
4. **上下文紧缩** → `pinned_message_indices()` → 标记包含重要路径的消息保留

这就是 `working_set.rs` 的完整结构——它通过 **机会主义路径提取** 和 **计分排序**，在不引入显式用户配置的情况下，动态地让 agent 知道用户当前最关心的文件是什么。

---

