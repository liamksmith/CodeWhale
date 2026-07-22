//! 延迟工具目录及内置高级工具辅助模块。
//!
//! 流式交互轮次循环负责控制工具的提供与执行时机。 本模块
//! 负责管理目录级别的策略，包括延迟加载、工具搜索、缺失工具建议，
//! 以及少量未通过常规运行时工具注册表注册的内置高级工具。
//! 
//! 该文件是 CodeWhale 工具目录系统的核心，负责以下职责：
//! 
//! 1. 定义哪些工具默认激活（DEFAULT_ACTIVE_NATIVE_TOOLS）
//! 2. 实现延迟加载策略——减少每轮发送给 LLM 的 tool schema 体积，保护 KV 前缀缓存
//! 3. 注入合成/高级工具（code_execution、js_execution、tool_search）
//! 4. 工具搜索与发现——BM25 和正则两种匹配引擎
//! 5. 模糊建议——基于编辑距离的工具名纠错
//! 6. 一致性校验——cross-check catalog 与 ToolRegistry
//! 7. 延迟工具水合——首次使用延迟工具时的 schema 反馈


use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::mcp::McpPool;                         // 判断一个工具名是否属于 MCP 工具（mcp__* 前缀）
use crate::model_profile::ToolSurfaceBudget;     // 紧凑模式下进一步缩减工具表面
use crate::models::Tool;                         // 核心类型
use crate::tools::spec::{ToolError, ToolResult, optional_str, optional_u64, required_str};
use crate::tui::app::AppMode;

use crate::dependencies::ExternalTool;           // Python/Node 解释器探测
use crate::regex_cache::compile_user_regex;      // 将用户正则查询编译为 regex::Regex

pub(super) const MULTI_TOOL_PARALLEL_NAME: &str = "multi_tool_use.parallel";
pub(super) const REQUEST_USER_INPUT_NAME: &str = "request_user_input";
pub(super) const CODE_EXECUTION_TOOL_NAME: &str = "code_execution";
const CODE_EXECUTION_TOOL_TYPE: &str = "code_execution_20250825";
pub(super) use crate::tools::js_execution::JS_EXECUTION_TOOL_NAME;
pub(super) const TOOL_SEARCH_NAME: &str = "tool_search";
const TOOL_SEARCH_TYPE: &str = "tool_search_20251119";
const LEGACY_TOOL_SEARCH_REGEX_NAME: &str = "tool_search_tool_regex";
const LEGACY_TOOL_SEARCH_BM25_NAME: &str = "tool_search_tool_bm25";
const TOOL_SEARCH_DEFAULT_MAX_RESULTS: usize = 20;
const TOOL_SEARCH_MAX_RESULTS_LIMIT: usize = 100;

/// 判断一个工具名是否属于工具搜索家族。包含新旧三个名字，确保遗留调用不会中断。
pub(super) fn is_tool_search_tool(name: &str) -> bool {
    matches!(
        name,
        TOOL_SEARCH_NAME | LEGACY_TOOL_SEARCH_REGEX_NAME | LEGACY_TOOL_SEARCH_BM25_NAME
    )
}

/// 28 个原生核心工具的权威列表。这些工具在所有模式下默认为"活跃"（不被延迟加载）
/// 列表的顺序无关——它仅用于成员测试（通过 HashSet 加速）。
pub(super) const DEFAULT_ACTIVE_NATIVE_TOOLS: &[&str] = &[
    "agent",
    "apply_patch",
    "edit_file",
    "exec_interact",
    "exec_shell",
    "exec_shell_interact",
    "exec_shell_wait",
    "exec_wait",
    "fetch_url",
    "file_search",
    "git_diff",
    "git_log",
    "git_show",
    "git_status",
    "grep_files",
    "list_dir",
    "read_file",
    "run_tests",
    "run_verifiers",
    "task_create",
    "task_list",
    "task_read",
    "update_plan",
    "wait_for_dev_server",
    "web_search",
    "work_update",
    "write_file",
];

/// 当核心工具在当前目录中不可见时，向模型提供的备选说明。
/// 包含工具名称、功能描述，以及当前不可用的具体原因。
const CORE_ACTION_TOOL_FALLBACKS: &[CoreActionToolFallback] = &[
    CoreActionToolFallback {
        name: "exec_shell",
        // "在工作区中执行 Shell 命令。"
        description: "Run shell commands in the workspace.",
        // "当前模型可见目录中不存在该工具。交互式 Agent 会话默认暴露 shell（除非 allow_shell = false）；非交互式和持久化配置需要显式设置 allow_shell = true。Plan 模式会隐藏 shell，命令工具的 allow/deny 门控也可能将其屏蔽。"
        unavailable_reason: "Not present in the current model-visible catalog. Interactive Agent sessions expose shell by default unless allow_shell = false; noninteractive and durable profiles require allow_shell = true. Plan mode hides shell, and command tool allow/deny gates can also block it.",
    },
    CoreActionToolFallback {
        name: "write_file",
        // "在工作区中创建或覆盖文件。"
        description: "Create or overwrite files in the workspace.",
        // "当前模型可见目录中不存在该工具。文件写入需要 Agent 或 Yolo 模式，且命令工具的 allow/deny 门控不能阻止 write_file。"
        unavailable_reason: "Not present in the current model-visible catalog. File writes require Agent or Yolo mode and no command tool allow/deny gate blocking write_file.",
    },
    CoreActionToolFallback {
        name: "edit_file",
        // "通过替换文本编辑现有文件。"
        description: "Edit existing files by replacing text.",
        // "当前模型可见目录中不存在该工具。文件编辑需要 Agent 或 Yolo 模式，且命令工具的 allow/deny 门控不能阻止 edit_file。"
        unavailable_reason: "Not present in the current model-visible catalog. File edits require Agent or Yolo mode and no command tool allow/deny gate blocking edit_file.",
    },
    CoreActionToolFallback {
        name: "apply_patch",
        // "对一个或多个工作区文件应用补丁。"
        description: "Apply a patch to one or more workspace files.",
        // "当前模型可见目录中不存在该工具。应用补丁需要 Agent 或 Yolo 模式、启用 apply_patch 特性，且命令工具的 allow/deny 门控不能阻止 apply_patch。"
        unavailable_reason: "Not present in the current model-visible catalog. Patches require Agent or Yolo mode, the apply_patch feature, and no command tool allow/deny gate blocking apply_patch.",
    },
];

#[derive(Debug, Clone, Copy)]
struct CoreActionToolFallback {
    name: &'static str,
    description: &'static str,
    unavailable_reason: &'static str,
}

/// Pre-computed lowercased haystack + name for each fallback; built once.
struct CachedFallback {
    fallback: CoreActionToolFallback,
    haystack: String,
    name_lower: String,
}

static CACHED_FALLBACKS: std::sync::OnceLock<Vec<CachedFallback>> = std::sync::OnceLock::new();

fn cached_fallbacks() -> &'static [CachedFallback] {
    CACHED_FALLBACKS.get_or_init(|| {
        CORE_ACTION_TOOL_FALLBACKS
            .iter()
            .map(|f| CachedFallback {
                fallback: *f,
                haystack: format!(
                    "{}\n{}\n{}",
                    f.name.to_lowercase(),
                    f.description.to_lowercase(),
                    f.unavailable_reason.to_lowercase(),
                ),
                name_lower: f.name.to_lowercase(),
            })
            .collect()
    })
}

/// 基于 [`DEFAULT_ACTIVE_NATIVE_TOOLS`] 的成员索引，在进程生命周期内构建一次。
/// 数组保持为*有序*迭代的真实来源（参见 [`tool_catalog_consistency_issues`] 和
/// `engine::default_active_native_tool_names`）；该集合仅用于加速 [`should_default_defer_tool`]
/// 中的热点成员检查，该检查在每次目录重建时（即每轮交互）对每个目录工具执行一次 ——
/// 将原本对数组的 O(n·m) 线性扫描降为 O(1) 的哈希查找。
static DEFAULT_ACTIVE_NATIVE_TOOLS_SET: std::sync::OnceLock<HashSet<&'static str>> =
    std::sync::OnceLock::new();

fn default_active_native_tools_set() -> &'static HashSet<&'static str> {
    DEFAULT_ACTIVE_NATIVE_TOOLS_SET
        .get_or_init(|| DEFAULT_ACTIVE_NATIVE_TOOLS.iter().copied().collect())
}

/// 判断一个工具是否应该默认延迟加载。
/// - 在参数always_load集合中工具的总是不会延迟加载。
/// - 搜索工具总是不会被延迟加载。
/// - 在DEFAULT_ACTIVE_NATIVE_TOOLS中的总是不会延迟加载
pub(super) fn should_default_defer_tool(name: &str, always_load: &HashSet<String>) -> bool {
    if always_load.contains(name) {
        return false;
    }

    if is_tool_search_tool(name) {
        return false;
    }

    // 仅成员资格测试（无顺序依赖）：基于 DEFAULT_ACTIVE_NATIVE_TOOLS 构建的辅助集合，
    // 其命中/未命中结果与原先的 `.iter().any(...)` 线性扫描完全一致。
    !default_active_native_tools_set().contains(name)
}

/// 遍历原生工具列表，为每个工具设置 defer_loading 标记。
pub(super) fn apply_native_tool_deferral(catalog: &mut [Tool], always_load: &HashSet<String>) {
    for tool in catalog {
        tool.defer_loading = Some(should_default_defer_tool(&tool.name, always_load));
    }
}

/// MCP 工具的延迟策略不同。5 个"MCP 元操作"工具（列出资源、读取资源、获取提示）始终保持加载——它们
/// 是发现其他 MCP 工具的入口。其他 MCP 工具默认延迟。
fn should_keep_mcp_tool_loaded(name: &str) -> bool {
    matches!(
        name,
        "list_mcp_resources"
            | "list_mcp_resource_templates"
            | "mcp_read_resource"
            | "read_mcp_resource"
            | "mcp_get_prompt"
    )
}

/// MCP 工具延迟的特殊规则：Yolo 模式下所有 MCP 工具都加载（不延迟），Agent/Plan 模式下仅保留元操作工具。
pub(super) fn apply_mcp_tool_deferral(
    catalog: &mut [Tool],
    mode: AppMode,
    always_load: &HashSet<String>,
) {
    for tool in catalog {
        if always_load.contains(&tool.name) {
            tool.defer_loading = Some(false);
            continue;
        }
        tool.defer_loading =
            Some(mode != AppMode::Yolo && !should_keep_mcp_tool_loaded(&tool.name));
    }
}

/// 从原生工具列表和 MCP 工具列表构建模型工具目录。
///
/// **目录头部稳定性不变量。** 目录的头部（所有非延迟工具）在模式切换（Plan ↔ Agent ↔ YOLO）时，
/// 对于两种模式共用的工具，必须保持字节级别完全一致。
/// 延迟工具激活时追加到尾部，且绝不会重排头部。此不变量对 DeepSeek 的 KV 前缀缓存至关重要：
/// 工具数组是不可变前缀的一部分，头部任何字节级别的变化都会导致下一轮交互触发完整的重预填充。
#[cfg(test)]
pub(super) fn build_model_tool_catalog(
    native_tools: Vec<Tool>,
    mcp_tools: Vec<Tool>,
    mode: AppMode,
    always_load: &HashSet<String>,
) -> Vec<Tool> {
    build_model_tool_catalog_with_surface(
        native_tools,
        mcp_tools,
        mode,
        always_load,
        ToolSurfaceBudget::Standard,
    )
}

pub(super) fn build_model_tool_catalog_with_surface(
    mut native_tools: Vec<Tool>,
    mut mcp_tools: Vec<Tool>,
    mode: AppMode,
    always_load: &HashSet<String>,
    surface_budget: ToolSurfaceBudget,
) -> Vec<Tool> {
    apply_native_tool_deferral(&mut native_tools, always_load);
    apply_mcp_tool_deferral(&mut mcp_tools, mode, always_load);
    // 如紧凑模式，进一步隐藏重工具
    apply_tool_surface_budget(&mut native_tools, surface_budget, always_load);
    apply_tool_surface_budget(&mut mcp_tools, surface_budget, always_load);
    // 关键：按名称对每个分区进行排序，以保证前缀缓存的稳定性（#263）。
    // 上游的 `to_api_tools()` 已经对注册表的 HashMap 输出进行了排序；
    // 但此目录是由调用者提供的 Vec 构建的，测试工具和（未来的）调用者重构可能不会预先排序。
    // 内置工具作为连续前缀保持在 MCP 工具之前，这样添加或移除 MCP 工具永远不会改变内置工具的位置。
    native_tools.sort_by(|a, b| a.name.cmp(&b.name));
    mcp_tools.sort_by(|a, b| a.name.cmp(&b.name));
    // 原生工具在前（连续前缀），MCP 在尾。这样增删 MCP 不会 shift 原生工具的位置。
    native_tools.extend(mcp_tools);
    native_tools
}

/// 仅 ToolSurfaceBudget::Compact 模式下生效。 将catalog中5个"重型"工具也强制延迟加载：
/// agent、run_tests、run_verifiers、task_create、web_search。
/// always_load 中的工具不受影响。
fn apply_tool_surface_budget(
    catalog: &mut [Tool],
    surface_budget: ToolSurfaceBudget,
    always_load: &HashSet<String>,
) {
    if !matches!(surface_budget, ToolSurfaceBudget::Compact) {
        return;
    }
    for tool in catalog {
        if always_load.contains(&tool.name) {
            continue;
        }
        if matches!(
            tool.name.as_str(),
            "agent" | "run_tests" | "run_verifiers" | "task_create" | "web_search"
        ) {
            tool.defer_loading = Some(true);
        }
    }
}

/// 检查高级工具[code_execution(Python)、js_execution(node.js)和tool_search(工具发现/检索工具)]
/// 并将它加入到catalog参数指定的vector中。
pub(super) fn ensure_advanced_tooling(
    catalog: &mut Vec<Tool>,
    mode: AppMode,
    always_load: &HashSet<String>,
) {
    // code_execution 依赖于本地安装的 Python 解释器（python3 / python / py -3）。
    // 在 v0.8.31 之前，该工具始终对外暴露，即使在 Windows 上 `python3` 不在 PATH 中，
    // 执行时也会失败。但由于它已出现在工具目录中，模型会将其视为可靠可用。
    // 现在我们在构建目录时进行预检测，仅当能够解析到有效的 Python 解释器时才对外暴露该工具。
    // 具体检测逻辑参见 `crate::dependencies::resolve_python_interpreter`。
    if mode != AppMode::Plan   // 1. plan模式下不注入
        && !catalog.iter().any(|t| t.name == CODE_EXECUTION_TOOL_NAME)  // 2. 已存在不重复注入
        && crate::dependencies::resolve_python_interpreter().is_some()         // 3. 本地确实能找到python解释器。
    {
        catalog.push(Tool {
            tool_type: Some(CODE_EXECUTION_TOOL_TYPE.to_string()),
            name: CODE_EXECUTION_TOOL_NAME.to_string(),
            // 描述："在本地沙箱化运行时中执行 Python 代码，以 JSON返回 stdout/stderr/return_code。
            description: "Execute Python code in a local sandboxed runtime and return stdout/stderr/return_code as JSON.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "code": { 
                        "type": "string", 
                        "description": "Python source code to execute." 
                    }
                },
                "required": ["code"]
            }),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(should_default_defer_tool(
                CODE_EXECUTION_TOOL_NAME,
                always_load,
            )),
            input_examples: None,
            strict: None,
            cache_control: None,
        });
    }

    // js_execution 与 code_execution 类似：以本地是否存在 Node.js
    // 作为门控条件，确保模型不会看到其实际无法使用的运行时。
    // 计划模式（Plan）在构造时就会隐藏 shell/exec 相关的交互界面（包括
    // 这两个解释器工具）；代理模式（Agent）/ YOLO 模式仅在 `resolve_node()`
    // 成功时才对外暴露该工具。
    if mode != AppMode::Plan
        && !catalog.iter().any(|t| t.name == JS_EXECUTION_TOOL_NAME)
        && crate::dependencies::resolve_node().is_some()
    {
        let mut tool = crate::tools::js_execution::js_execution_tool_definition();
        tool.defer_loading = Some(should_default_defer_tool(&tool.name, always_load));
        catalog.push(tool);
    }

    // tool_search
    // tool_search 是一个工具发现/检索工具，它的核心作用是让模型能够在运行时动态发现和激活那些原本被延迟加载
    //（defer_loading = true）的工具。
    // 为什么需要这个工具？
    // 在 deepseek-tui 中，为了节省 token 和保持前缀缓存（KV cache）的稳定性，大量工具默认是延迟加载的
    // （defer_loading = true）。这意味着：
    // - 在每轮对话的初始工具目录中，这些工具不会被暴露给模型
    // - 模型一开始"看不见"这些工具，也就无法调用它们
    // - 这样可以减少每次请求的 token 消耗，并保持工具列表前缀稳定（利于缓存）
    // 但问题来了：如果模型需要某个延迟加载的工具怎么办？
    // tool_search 就是解决这个问题的"入口"——它是一个始终加载（defer_loading = false）的工具，模型
    // 始终能看到它，通过它来搜索和激活其他延迟加载的工具。
    /*
    工作流程
    1. 模型初始只能看到工具目录中的"非延迟加载"工具
   （包括 tool_search 本身和一些核心工具）

    2. 当模型判断需要某个延迟加载的工具时（比如 code_execution），
    它调用 tool_search，传入查询关键词

    3. tool_search 在完整目录中搜索匹配的工具定义

    4. 匹配到的工具名称会被加入 active_tools 集合中
    （见 execute_tool_search 函数中的 active_tools.insert）

    5. 在下一轮对话中，这些被激活的工具就会出现在模型可见的工具列表中

    输入参数
    参数	类型	说明
    query	string（必填）	搜索查询字符串
    match	string（可选，默认 "bm25"）	匹配算法："bm25"（自然语言匹配）或 "regex"（正则表达式匹配）
    max_results	integer（可选，默认 20，最大 100）	返回的最大匹配数量

    搜索范围
    搜索会覆盖工具的：
    名称（name）
    描述（description）
    输入 schema（input_schema）
    搜索时会排除 tool_search 自身（避免递归），以及已经激活的工具。

    返回值
    工具返回一个 JSON 结构，包含两类引用：
    json
    {
    "type": "tool_search_tool_search_result",
    "tool_references": [
        { "type": "tool_reference", "tool_name": "code_execution" }
    ],
    "unavailable_tool_references": [
        { 
        "type": "unavailable_tool_reference", 
        "tool_name": "exec_shell", 
        "reason": "为什么这个工具当前不可用..."
        }
    ]
    }
    tool_references：成功匹配且可用的工具名列表，这些工具会在当前请求中被激活（加入 active_tools 集合）
    unavailable_tool_references：匹配到但当前不可用的核心工具（如 exec_shell），并附带不可用的原因说明

    为什么 unavailable_tool_references 很重要？
    在某些模式（如 Plan 模式）下，即使模型通过 tool_search 搜到了 exec_shell 这样的工具，它实际上也是不可用的（因为 Plan 模式禁止 shell 操作）。
    unavailable_tool_references 会明确告诉模型：
    - 这个工具存在，但当前不可用
    - 为什么不可用（模式限制、配置限制等）
    - 模型可以据此调整行为，而不是盲目尝试调用一个不可用的工具导致失败

    总结
    tool_search 是一个工具发现与激活机制，它的存在让系统可以在保持轻量初始工具列表的同时，仍然为模型提供按需发现和激活其他工具的能力。这兼顾了：
    - 性能（减少 token 消耗，保持前缀缓存稳定）
    - 灵活性（模型可以根据需要动态扩展可用工具集）
    - 可控性（即使工具被搜到，系统仍可通过 unavailable_tool_references 告知模型当前不可用及原因）
     */

    if !catalog.iter().any(|t| t.name == TOOL_SEARCH_NAME) {
        catalog.push(Tool {
            tool_type: Some(TOOL_SEARCH_TYPE.to_string()),
            name: TOOL_SEARCH_NAME.to_string(),
            // 查询已延迟加载的工具定义，并返回所有匹配项的工具引用。
            description: "Search deferred tool definitions and return matching tool references.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { 
                        "type": "string", 
                        // 工具检索查询语句
                        "description": "Search query for tool discovery." 
                    },  
                    "match": {
                        "type": "string",
                        "enum": ["bm25", "regex"],
                        "default": "bm25",
                        // 匹配算法：自然语言匹配采用 BM25 算法；正则表达式匹配则作用于工具名称、描述和 schema。
                        "description": "Matching algorithm: bm25 for natural-language matching, regex for a regular expression over tool names/descriptions/schema."
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": TOOL_SEARCH_MAX_RESULTS_LIMIT,
                        "default": TOOL_SEARCH_DEFAULT_MAX_RESULTS,
                        "description": "Maximum number of matching tool references to return."
                    }
                },
                "required": ["query"]
            }),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(false),
            input_examples: None,
            strict: None,
            cache_control: None,
        });
    }
}

/// 根据输入参数创建初始化活跃工具列表
/// 1. 非延迟工具 + tool_search 默认为活跃。
/// 2. 如果所有工具都被延迟了（极端情况），至少激活第一个工具，确保模型至少有工具可用。
pub(super) fn initial_active_tools(catalog: &[Tool]) -> HashSet<String> {
    let mut active = HashSet::new();
    for tool in catalog {
        if !tool.defer_loading.unwrap_or(false) || is_tool_search_tool(&tool.name) {
            active.insert(tool.name.clone());
        }
    }
    if active.is_empty()
        && !catalog.is_empty()
        && let Some(first) = catalog.first()
    {
        active.insert(first.name.clone());
    }
    active
}

// 遍历catalog，寻找名字在active集合中的工具。
// 并将其中始终加载的工具放前半段，将延迟加载的工具排后半段，合并成一个Vec返回。
fn active_tool_list_from_catalog(catalog: &[Tool], active: &HashSet<String>) -> Vec<Tool> {
    // 采用两遍遍历以保证前缀缓存的稳定性（#263）。
    // 始终加载的工具按其稳定的目录顺序排在前面；而最初为延迟状态、在对话中途通过 ToolSearch 激活的工具，
    // 则追加到尾部。否则，激活一个延迟工具会导致其后所有工具的字节偏移量发生变化，
    // 从而从该位置起破坏缓存的的前缀。
    let catalog_len = catalog.len();
    let mut head: Vec<Tool> = Vec::with_capacity(catalog_len);
    let mut tail: Vec<Tool> = Vec::with_capacity(catalog_len);
    for tool in catalog {
        if !active.contains(&tool.name) {
            continue;
        }
        if tool.defer_loading.unwrap_or(false) {
            tail.push(tool.clone());    // 中途被 tool_search 激活的延迟工具，追加到末尾。
        } else {
            head.push(tool.clone());    // 始终加载（非延迟）的工具，按目录原始顺序排列。
        }
    }
    head.extend(tail);
    head
}

/// 当 force_update_plan 为 true 时（引擎检测到明显的"做计划"请求），
/// 第一轮仅给模型 update_plan 一个工具——缩小工具表面，让模型专注规划。
/// DeepSeek reasoning 模型不支持显式 tool_choice 强制，所以用这种方法变通实现。
pub(super) fn active_tools_for_step(
    catalog: &[Tool],
    active: &HashSet<String>,
    force_update_plan: bool,
) -> Vec<Tool> {
    // DeepSeek reasoning 模型不支持显式 tool_choice 强制，所以用这种方法变通实现。
    // 对于明显的快速规划请求，我们将第一步的工具范围收窄为仅 update_plan。
    if force_update_plan {
        let forced: Vec<_> = catalog
            .iter()
            .filter(|tool| tool.name == "update_plan")
            .cloned()
            .collect();
        if !forced.is_empty() {
            return forced;
        }
    }

    active_tool_list_from_catalog(catalog, active)
}

/// 构造搜索用的 haystack：工具名 + 描述 + JSON schema 全部转小写后用换行拼接。
fn tool_search_haystack(tool: &Tool) -> String {
    format!(
        "{}\n{}\n{}",
        tool.name.to_lowercase(),
        tool.description.to_lowercase(),
        tool.input_schema.to_string().to_lowercase()
    )
}

/// 简单的线性扫描，检查工具名是否已在目录中。
fn catalog_contains_tool(catalog: &[Tool], name: &str) -> bool {
    catalog.iter().any(|tool| tool.name == name)
}

/// 正则搜索不可用的核心操作工具（fallback 列表）。
/// 先排除已在目录(catalog)中的，再用 regex 匹配 haystack。
fn unavailable_core_action_tools_with_regex(
    catalog: &[Tool],
    query: &str,
    max_results: usize,
) -> Result<Vec<CoreActionToolFallback>, ToolError> {
    if max_results == 0 {
        return Ok(Vec::new());
    }
    let regex = compile_user_regex(query)
        .map_err(|err| ToolError::invalid_input(format!("Invalid regex query: {err}")))?;
    Ok(cached_fallbacks()
        .iter()
        .filter(|cf| !catalog_contains_tool(catalog, cf.fallback.name))
        .filter(|cf| regex.is_match(&cf.haystack))
        .take(max_results)
        .map(|cf| cf.fallback)
        .collect())
}

/// BM25-like 搜索不可用的核心操作工具。简化版 BM25：
/// 
/// - 每个 term 在 haystack 中匹配得 1 分
/// - 每个 term 在工具名中匹配得 2 分（名匹配权重更高）
/// - 按 score 降序，得分相同按名字字母序
fn unavailable_core_action_tools_with_bm25_like(
    catalog: &[Tool],
    query: &str,
    max_results: usize,
) -> Vec<CoreActionToolFallback> {
    if max_results == 0 {
        return Vec::new();
    }
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|term| term.trim().to_lowercase())
        .filter(|term| !term.is_empty())
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(i64, CoreActionToolFallback)> = Vec::new();
    for cf in cached_fallbacks() {
        if catalog_contains_tool(catalog, cf.fallback.name) {
            continue;
        }
        let hay = &cf.haystack;
        let name = &cf.name_lower;
        let mut score = 0i64;
        for term in &terms {
            if hay.contains(term) {
                score += 1;
            }
            if name.contains(term) {
                score += 2;
            }
        }
        if score > 0 {
            scored.push((score, cf.fallback));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(b.1.name)));
    scored
        .into_iter()
        .take(max_results)
        .map(|(_, fallback)| fallback)
        .collect()
}

// 在目录(catalog)中按正则搜索工具。
fn discover_tools_with_regex(
    catalog: &[Tool],
    query: &str,
    max_results: usize,
) -> Result<Vec<String>, ToolError> {
    let regex = compile_user_regex(query)
        .map_err(|err| ToolError::invalid_input(format!("Invalid regex query: {err}")))?;

    let mut matches = Vec::new();
    for tool in catalog {
        if is_tool_search_tool(&tool.name) {    // 跳过 tool_search 自身（不自引）
            continue;
        }
        let hay = tool_search_haystack(tool);   // 匹配 tool_search_haystack
        if regex.is_match(&hay) {
            matches.push(tool.name.clone());
        }
        if matches.len() >= max_results {    // max_results 上限时提前 break
            break;
        }
    }
    Ok(matches)
}

/// 在目录中按 BM25-like 搜索工具。
/// 跳过 tool_search 自身。
fn discover_tools_with_bm25_like(catalog: &[Tool], query: &str, max_results: usize) -> Vec<String> {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|term| term.trim().to_lowercase())
        .filter(|term| !term.is_empty())
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(i64, String)> = Vec::new();
    for tool in catalog {
        if is_tool_search_tool(&tool.name) {
            continue;
        }
        let hay = tool_search_haystack(tool);
        let mut score = 0i64;
        for term in &terms {
            if hay.contains(term) {
                score += 1;
            }
            if tool.name.to_lowercase().contains(term) {
                score += 2;
            }
        }
        if score > 0 {
            scored.push((score, tool.name.clone()));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored
        .into_iter()
        .take(max_results)
        .map(|(_, name)| name)
        .collect()
}

fn edit_distance(a: &str, b: &str) -> usize {
    if a == b {
        return 0;
    }
    if a.is_empty() {
        return b.chars().count();
    }
    if b.is_empty() {
        return a.chars().count();
    }

    let b_chars: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b_chars.len()).collect();
    let mut curr = vec![0usize; b_chars.len() + 1];

    for (i, a_ch) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, b_ch) in b_chars.iter().enumerate() {
            let cost = if a_ch == *b_ch { 0 } else { 1 };
            let delete = prev[j + 1] + 1;
            let insert = curr[j] + 1;
            let substitute = prev[j] + cost;
            curr[j + 1] = delete.min(insert).min(substitute);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_chars.len()]
}

fn suggest_tool_names(catalog: &[Tool], requested: &str, limit: usize) -> Vec<String> {
    let requested = requested.trim().to_ascii_lowercase();
    if requested.is_empty() || limit == 0 {
        return Vec::new();
    }

    let mut candidates: Vec<(u8, usize, String)> = Vec::new();
    for tool in catalog {
        let candidate = tool.name.to_ascii_lowercase();
        let prefix_match = candidate.starts_with(&requested) || requested.starts_with(&candidate);
        let contains_match = candidate.contains(&requested) || requested.contains(&candidate);
        let distance = edit_distance(&candidate, &requested);
        let close_typo = distance <= 3;

        if !(prefix_match || contains_match || close_typo) {
            continue;
        }

        let rank = if prefix_match {
            0
        } else if contains_match {
            1
        } else {
            2
        };
        candidates.push((rank, distance, tool.name.clone()));
    }

    candidates.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    candidates.dedup_by(|a, b| a.2 == b.2);
    candidates
        .into_iter()
        .take(limit)
        .map(|(_, _, name)| name)
        .collect()
}

/// 这个函数回答一个问题："这个工具是不是由引擎在目录构建时合成出来的，而不是通过标准 ToolRegistry
/// 注册的？"
/// - 注册工具 通过ToolRegistry::register()正式注册，有handler有schema，是一等公民。
/// - 合成工具 在目录构建时动态构造，行为由特殊路径处理。
/// 
/// 它返回 true 的三类工具正好对应三种不经过 ToolRegistry::register() 的注入路径：
/// |──────────────────────────────┬──────────────────────────────┬──────────────────────────────┐
/// │ 检查条件                      │ 涵盖的工具                    │ 注入来源                     │ 
/// ├──────────────────────────────┼──────────────────────────────┼──────────────────────────────┤ 
/// │ is_tool_search_tool(name)    │ tool_search、`toolsearch     │ ensure_advanced_tooling      │ 
/// │                              │ tool_regex、tool_search_to   │ 第312-343行内联构造           │
/// │                              │ ol_bm25`                     │                              │ 
/// ├──────────────────────────────┼──────────────────────────────┼──────────────────────────────┤
/// │ `matches!(name,              │ code_execution、`js_execut   │ ensure_advanced_tooling      │  
/// │ CODE_EXECUTION_TOOL_NAME |   │ ion`                         │ 第272-310行内联构造或从       │ 
/// │ JS_EXECUTION_TOOL_NAME)`     │                              │ `js_execution_tool_definitio │ 
/// │                              │                              │ n()` 获取                    │ 
/// ├──────────────────────────────┼──────────────────────────────┼──────────────────────────────┤ 
/// │ McpPool::is_mcp_tool(name)   │ 所有 MCP 工具（mcp__*         │ McpPool                      │ 
/// │                              │ 前缀）                        │ 在目录构建时动态注入，handle  │ 
/// │                              │                              │ r 在 MCP 客户端侧            │ 
/// └──────────────────────────────┴──────────────────────────────┴──────────────────────────────┘ 
/// 它在整个文件中的唯一调用点位于 tool_catalog_consistency_issues 第728行
/// ``` rust
/// for tool in catalog { 
///   if is_synthetic_catalog_tool(&tool.name) {  
///     continue;   // 跳过——在 ToolRegistry 中，无需交叉检查
///   }
/// ...
/// }
/// ```
/// 这里的逻辑是： 合成工具天然不在 ToolRegistry 中，如果对它们做 "catalog 里有没有 handler"
/// 的检查，会100%误报。跳过它们，只对真正走注册流程的工具做一致性校验。 
fn is_synthetic_catalog_tool(name: &str) -> bool {
    is_tool_search_tool(name)
        || matches!(name, CODE_EXECUTION_TOOL_NAME | JS_EXECUTION_TOOL_NAME)
        || McpPool::is_mcp_tool(name)
}

/// 在每次构建工具目录后运行，检测目录（模型看到的工具列表）与注册表（实际有处理函数的工具）之间的
/// 交叉不一致，确保模型不会被暴露一个不存在的工具，也确保有 handler的核心工具不会意外从模型视野中消失。
/// 返回值： 字符串列表，每个元素是一条一致性问题描述。空列表表示一切正常。
pub(super) fn tool_catalog_consistency_issues(
    catalog: &[Tool],
    registry: &crate::tools::ToolRegistry,
) -> Vec<String> {
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

    for name in DEFAULT_ACTIVE_NATIVE_TOOLS {
        if registry_model_visible_names.contains(name) && !catalog_names.contains(name) {
            issues.push(format!(
                "registered core tool '{name}' is missing from the model/search catalog"
            ));
        }
    }

    issues.sort();
    issues
}

/// 模型请求了目录中不存在的工具时的人性化错误消息。
pub(super) fn missing_tool_error_message(tool_name: &str, catalog: &[Tool]) -> String {
    // 自测验证 A5 场景（#4092）：模型在清单中途(mid-checklist )有时会将每个列表条目作为独立的工具调用来发出，
    // 其名称可能是 `item`/`todo`/…… 等等。模糊建议在这种情况下会产生严重的误导（例如“您是不是想用：note, tts？”）；
    // 因此直接指明实际的修复方案。
    //
    // 特殊处理——如果模型试图调用 item/todo/checklist 等伪工具（模型幻觉，把 checklist 条目当成独立工具调用），
    // 直接告知正确的用法（用 work_update）。
    if matches!(
        tool_name,
        "item" | "items" | "todo" | "todos" | "checklist" | "checklist_item" | "plan_item"
    ) {
        /*
        “工具 '{tool_name}' 在当前工具目录中不可用。\
        清单条目并非独立的工具调用——请在一次 `work_update` 调用中写入整个列表，\
        使用 `todos` 数组，其中包含 {{content, status}} 对象。”
         */
        return format!(
            "Tool '{tool_name}' is not available in the current tool catalog. \
             Checklist entries are not separate tool calls — write the whole list \
             in one `work_update` call with a `todos` array of \
             {{content, status}} objects."
        );
    }
    // 1. 调用 suggest_tool_names 获取 3 个模糊建议
    let suggestions = suggest_tool_names(catalog, tool_name, 3);
    // 2. 判断是否为 shell 工具丢失（is_shell_tool_name），若是则追加 allow_shell 配置提示
    let shell_hint = if is_shell_tool_name(tool_name) {
        Some(shell_tool_allow_shell_hint())
    } else {
        None
    };
    // 3. 根据有无建议组合四种错误消息
    if suggestions.is_empty() {
        // 3.1 无建议 + 有 shell hint
        if let Some(shell_hint) = shell_hint {
            /*
            “工具 '{tool_name}' 在当前工具目录中不可用。\
                 {shell_hint}，或使用 {TOOL_SEARCH_NAME} 并附上简短查询。”
             */
            return format!(
                "Tool '{tool_name}' is not available in the current tool catalog. \
                 {shell_hint}, or use {TOOL_SEARCH_NAME} with a short query."
            );
        }
        // 3.2 无建议 + 无 shell hint
        /*
        “工具 '{tool_name}' 在当前工具目录中不可用。\
             请检查模式/特性标志，或使用 {TOOL_SEARCH_NAME} 并附上简短查询。”
         */
        return format!(
            "Tool '{tool_name}' is not available in the current tool catalog. \
             Verify mode/feature flags, or use {TOOL_SEARCH_NAME} with a short query."
        );
    }

    // 3.3 有建议 + 有 shell hint
    let suggestion_text = format!("Did you mean: {}?", suggestions.join(", "));
    if let Some(shell_hint) = shell_hint {
        /*
        “工具 '{tool_name}' 在当前工具目录中不可用。\
             {suggestion_text} {shell_hint}。\
             你也可以使用 {TOOL_SEARCH_NAME} 来发现可用的工具。”
         */
        return format!(
            "Tool '{tool_name}' is not available in the current tool catalog. \
             {suggestion_text} {shell_hint}. \
             You can also use {TOOL_SEARCH_NAME} to discover tools."
        );
    }

    // 3.4 有建议 + 无 shell hint
    /*
    “工具 '{tool_name}' 在当前工具目录中不可用。\
         {suggestion_text} 你也可以使用 {TOOL_SEARCH_NAME} 来发现可用的工具。”
     */
    format!(
        "Tool '{tool_name}' is not available in the current tool catalog. \
         {suggestion_text} You can also use {TOOL_SEARCH_NAME} to discover tools."
    )
}

/// 返回 shell 工具缺失时的针对性帮助文本，告知用户用 /config allow_shell true 解决。
fn shell_tool_allow_shell_hint() -> &'static str {
    /*
    “Shell 工具不可用，因为此会话或配置文件禁用了 shell 访问权限，
     通常是通过顶层配置 `allow_shell = false` 或 Plan 模式所致。
     交互式 Act 模式默认会暴露 shell 工具，并带有审批门控机制，除非被禁用。
     运行 `/config allow_shell true` 可为本会话启用，或添加 `--save` 以保留至未来会话；
     下一轮交互将重新暴露 shell 工具。”
     */
    "Shell tools are absent because this session or profile disabled shell access, \
     commonly via top-level `allow_shell = false` or Plan mode. \
     Interactive Act mode exposes shell by default with approval gating unless disabled. \
     Run `/config allow_shell true` for this session or add `--save` for future sessions; \
     the next turn will expose shell again"
}

/// 判断是否为 5 个 shell 工具之一。
fn is_shell_tool_name(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "exec_shell"
            | "exec_shell_wait"
            | "exec_shell_interact"
            | "task_shell_start"
            | "task_shell_wait"
    )
}

/// 测试用函数。检查工具是否延迟加载，若是则插入 active_tools 集合并返回 true。
#[cfg(test)]
pub(super) fn maybe_activate_requested_deferred_tool(
    tool_name: &str,
    catalog: &[Tool],
    active_tools: &mut HashSet<String>,
) -> bool {
    let Some(def) = catalog.iter().find(|def| def.name == tool_name) else {
        return false;
    };

    if !def.defer_loading.unwrap_or(false) || active_tools.contains(tool_name) {
        return false;
    }

    active_tools.insert(tool_name.to_string())
}

/// 运行时路径。 当模型调用一个延迟工具时：
/// - 检查：工具在目录中、标记为延迟、且 batch 开始时不在活跃集中
/// - 若是首次使用，记录到 hydrated_tools_this_batch
/// - 返回 Some(ToolResult)——包含详细 schema 反馈（不是执行结果！）
/// 
/// 在AI智能体（Agent）和编程语境中，hydrate（水合/填充）的核心意思是：
/// 将“不完整/轻量/序列化”的数据，补充、填充成“完整/可运行/内存中”的对象
/// 像泡方便面——干面饼（序列化数据） + 热水（运行时上下文） = 一碗能吃的面（活的对象）。
pub(super) fn maybe_hydrate_requested_deferred_tool(
    tool_name: &str,
    tool_input: &Value,
    catalog: &[Tool],
    active_tools_at_batch_start: &HashSet<String>,
    hydrated_tools_this_batch: &mut HashSet<String>,
) -> Option<ToolResult> {
    let def = catalog.iter().find(|def| def.name == tool_name)?;

    if !def.defer_loading.unwrap_or(false) || active_tools_at_batch_start.contains(tool_name) {
        return None;
    }

    hydrated_tools_this_batch.insert(tool_name.to_string());
    Some(deferred_tool_schema_hydration_result(def, tool_input))
}

/// 测试包装器，调用 maybe_hydrate_requested_deferred_tool 
/// 后将水合的工具合并入 active_tools。
#[cfg(test)]
pub(super) fn preflight_requested_deferred_tool(
    tool_name: &str,
    tool_input: &Value,
    catalog: &[Tool],
    active_tools: &mut HashSet<String>,
) -> Option<ToolResult> {
    let active_tools_at_batch_start = active_tools.clone();
    let mut hydrated_tools_this_batch = HashSet::new();
    let result = maybe_hydrate_requested_deferred_tool(
        tool_name,
        tool_input,
        catalog,
        &active_tools_at_batch_start,
        &mut hydrated_tools_this_batch,
    );
    active_tools.extend(hydrated_tools_this_batch);
    result
}

/// 延迟工具首次使用时返回的特殊响应。 这不是执行结果——而是告诉模型：
/// 1. 工具已加载（"Tool xxx was deferred and has now been loaded."）
/// 2. 未执行（"The tool was not executed. Retry with the loaded schema."）
/// 3. 完整 schema 信息：
///   - 期望字段（含类型和 required 标记）
///   - 收到的字段列表
///   - 缺失的必填字段
///   - 意外字段
///   - 可能的字段名纠正（见下文 likely_field_corrections）
/// 返回的 metadata包含结构化诊断：
/// event: "tool.schema_hydrated"、executed: false、retry_required: true、reason: "deferred_tool_first_use" 等。
fn deferred_tool_schema_hydration_result(tool: &Tool, tool_input: &Value) -> ToolResult {
    let expected = schema_fields(&tool.input_schema);
    let required = schema_required_fields(&tool.input_schema);
    let received = received_field_names(tool_input);
    let missing = required
        .iter()
        .filter(|field| !received.contains(field))
        .cloned()
        .collect::<Vec<_>>();
    let unexpected = received
        .iter()
        .filter(|field| !expected.iter().any(|expected| &expected.name == *field))
        .cloned()
        .collect::<Vec<_>>();
    let corrections = likely_field_corrections(&received, &expected, &tool.name);

    let mut lines = vec![
        format!("Tool `{}` was deferred and has now been loaded.", tool.name),
        String::new(),
        "The tool was not executed. Retry with the loaded schema.".to_string(),
        String::new(),
        "Expected fields:".to_string(),
    ];
    if expected.is_empty() {
        lines.push("  (none)".to_string());
    } else {
        for field in &expected {
            let required_marker = if required.contains(&field.name) {
                " required"
            } else {
                ""
            };
            lines.push(format!(
                "  {}: {}{}",
                field.name, field.kind, required_marker
            ));
        }
    }
    lines.push(String::new());
    lines.push("Received fields:".to_string());
    if received.is_empty() {
        lines.push("  (none)".to_string());
    } else {
        lines.push(format!("  {}", received.join(", ")));
    }
    if !missing.is_empty() {
        lines.push(String::new());
        lines.push("Missing required fields:".to_string());
        lines.push(format!("  {}", missing.join(", ")));
    }
    if !unexpected.is_empty() {
        lines.push(String::new());
        lines.push("Unexpected fields:".to_string());
        lines.push(format!("  {}", unexpected.join(", ")));
    }
    if !corrections.is_empty() {
        lines.push(String::new());
        lines.push("Likely corrections:".to_string());
        for correction in &corrections {
            lines.push(format!("  {correction}"));
        }
    }

    ToolResult::success(lines.join("\n")).with_metadata(json!({
        "event": "tool.schema_hydrated",
        "tool": tool.name,
        "executed": false,
        "retry_required": true,
        "reason": "deferred_tool_first_use",
        "deferred_tool_loaded": true,
        "tool_name": tool.name,
        "expected_fields": expected.iter().map(|field| field.name.clone()).collect::<Vec<_>>(),
        "received_fields": received,
        "missing_required_fields": missing,
        "unexpected_fields": unexpected,
        "likely_corrections": corrections,
    }))
}

/// 工具 schema 字段的简化表示。
#[derive(Debug, Clone)]
struct SchemaField {
    name: String,
    kind: String,
}

/// 从 JSON Schema 的 properties 中提取字段列表，按字段名排序。
fn schema_fields(schema: &Value) -> Vec<SchemaField> {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut fields = properties
        .iter()
        .map(|(name, spec)| SchemaField {
            name: name.clone(),
            kind: schema_type_label(spec),
        })
        .collect::<Vec<_>>();
    fields.sort_by(|a, b| a.name.cmp(&b.name));
    fields
}

/// 从 JSON Schema 的 required 数组提取必填字段名列表。
fn schema_required_fields(schema: &Value) -> Vec<String> {
    let mut required = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    required.sort();
    required
}

/// 构造字段类型的友好标签。如果字段有 enum 约束，显示为 "string (bm25 | regex)" 格式。
fn schema_type_label(spec: &Value) -> String {
    let Some(kind) = spec.get("type").and_then(Value::as_str) else {
        return "value".to_string();
    };
    if let Some(values) = spec.get("enum").and_then(Value::as_array) {
        let labels = values.iter().filter_map(Value::as_str).collect::<Vec<_>>();
        if !labels.is_empty() {
            return format!("{kind} ({})", labels.join(" | "));
        }
    }
    kind.to_string()
}

/// 从模型提供的 JSON 输入中提取所有字段名，排序后返回。
fn received_field_names(input: &Value) -> Vec<String> {
    let mut fields = input
        .as_object()
        .map(|object| object.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    fields.sort();
    fields
}

/// 智能字段名纠正系统。 分析模型传入的字段名，推断它可能想表达什么 
fn likely_field_corrections(
    received: &[String],
    expected: &[SchemaField],
    tool_name: &str,
) -> Vec<String> {
    let has_expected = |name: &str| expected.iter().any(|field| field.name == name);
    let has_received = |name: &str| received.iter().any(|field| field == name);
    let mut corrections = Vec::new();

    if has_received("old_string") && has_expected("search") {
        corrections.push("old_string -> search".to_string());
    } else if has_received("old_str") && has_expected("search") {
        corrections.push("old_str -> search".to_string());
    }
    if has_received("new_string") && has_expected("replace") {
        corrections.push("new_string -> replace".to_string());
    } else if has_received("new_str") && has_expected("replace") {
        corrections.push("new_str -> replace".to_string());
    } else if has_received("replacement") && has_expected("replace") {
        corrections.push("replacement -> replace".to_string());
    }
    if matches!(tool_name, "checklist_update" | "todo_update") && has_received("todos") {
        corrections.push(
            "Use work_update to replace the full list, or retry checklist_update/todo_update with id and status."
                .to_string(),
        );
    }
    // RLM source fields are easy to misname (#2659). rlm_open takes exactly one
    // of file_path / content / url / session_object; nudge common wrong names
    // toward those.
    // RLM 字段纠正特别详细——模型容易用 prompt、file、path、text、body、source 等非标准名称，逐个提示正确字段。
    if tool_name == "rlm_open" {
        for wrong in [
            "prompt",
            "resident_file",
            "text",
            "body",
            "path",
            "file",
            "source",
        ] {
            if has_received(wrong)
                && !has_received("file_path")
                && !has_received("content")
                && !has_received("url")
                && !has_received("session_object")
            {
                corrections.push(format!("{wrong} -> file_path (local file), content (inline text), url, or session_object"));
            }
        }
    }
    corrections
}

/// tool_search 工具的运行时执行逻辑。
pub(super) fn execute_tool_search(
    tool_name: &str,
    input: &serde_json::Value,
    catalog: &[Tool],
    active_tools: &mut HashSet<String>,
) -> Result<ToolResult, ToolError> {
    // 提取必填参数 query
    let query = required_str(input, "query")?;
    // 确定匹配算法：遗留工具名硬编码算法，新版从 match 参数读取，默认 "bm25"
    let match_kind = match tool_name {
        LEGACY_TOOL_SEARCH_REGEX_NAME => "regex",
        LEGACY_TOOL_SEARCH_BM25_NAME => "bm25",
        _ => optional_str(input, "match").unwrap_or("bm25"),
    };
    // 验证 match 算法仅允许 bm25 或 regex
    if !matches!(match_kind, "bm25" | "regex") {
        return Err(ToolError::invalid_input(format!(
            "Unsupported match algorithm '{match_kind}'. Expected one of: bm25, regex"
        )));
    }
    // 解析 max_results（默认 20，clamp 到 1..100）
    let max_results = usize::try_from(optional_u64(
        input,
        "max_results",
        TOOL_SEARCH_DEFAULT_MAX_RESULTS as u64,
    ))
    .unwrap_or(TOOL_SEARCH_DEFAULT_MAX_RESULTS)
    .clamp(1, TOOL_SEARCH_MAX_RESULTS_LIMIT);
    // 寻找工具
    let discovered = if match_kind == "regex" {
        discover_tools_with_regex(catalog, query, max_results)?
    } else {
        discover_tools_with_bm25_like(catalog, query, max_results)
    };
    // 剩余名额搜索不可用核心工具（unavailable_core_action_tools_*）
    let remaining_results = max_results.saturating_sub(discovered.len());
    let unavailable = if match_kind == "regex" {
        unavailable_core_action_tools_with_regex(catalog, query, remaining_results)?
    } else {
        unavailable_core_action_tools_with_bm25_like(catalog, query, remaining_results)
    };

    // 副作用： 将发现的工具名插入 active_tools——激活延迟工具
    for name in &discovered {
        active_tools.insert(name.clone());
    }

    // 构造响应：tool_references（可用的）+ unavailable_tool_references（存在但不可用的）
    let references = discovered
        .iter()
        .map(|name| json!({"type": "tool_reference", "tool_name": name}))
        .collect::<Vec<_>>();
    let unavailable_references = unavailable
        .iter()
        .map(|fallback| {
            json!({
                "type": "unavailable_tool_reference",
                "tool_name": fallback.name,
                "reason": fallback.unavailable_reason,
            })
        })
        .collect::<Vec<_>>();

    let payload = json!({
        "type": "tool_search_tool_search_result",
        "tool_references": references,
        "unavailable_tool_references": unavailable_references.clone(),
    });

    Ok(ToolResult {
        content: serde_json::to_string(&payload).unwrap_or_else(|_| payload.to_string()),
        success: true,
        metadata: Some(json!({
            "tool_references": discovered,
            "unavailable_tool_references": unavailable_references,
        })),
    })
}

/// Python 代码执行工具的运行时实现。
pub(super) async fn execute_code_execution_tool(
    input: &serde_json::Value,
    workspace: &Path,
) -> Result<ToolResult, ToolError> {
    // 提取 code
    let code = required_str(input, "code")?;

    // 解析在目录构建时缓存的本地已安装 Python 解释器。
    // 如果此时它不存在（某些情况下已注册但在启动到此调用之间消失了——并发卸载、
    // PATH 变更等），ExternalTool::tokio_command() 将返回 None，我们会快速失败并给出明确消息。
    //
    // 将代码写入临时文件并将其作为脚本执行，而不是通过 `-c "<code>"` 传递。原因如下：
    //   * `-c` 在 Windows 上存在长度限制（argv 限制）。
    //   * 通过 `-c` 传递带引号嵌套的多行代码容易出现解析错误。
    //   * 回溯信息中会引用真实文件名而非 `<string>`，因此模型能够正确解读行号。
    // 临时文件仅在此次执行期间存活；Drop 时会将其移除。我们使用 `.py` 扩展名，
    // 以便解释器中的任何 shebang 或编码检测逻辑能够正常运作。
    //
    // 创建临时目录、写入脚本
    let temp_dir = tempfile::tempdir()
        .map_err(|e| ToolError::execution_failed(format!("tempdir failed: {e}")))?;
    let script_path = temp_dir.path().join("code_execution.py");
    tokio::fs::write(&script_path, code)
        .await
        .map_err(|e| ToolError::execution_failed(format!("tempfile write failed: {e}")))?;
    // 构造命令 python <script_path> + current_dir(workspace)
    let mut cmd = crate::dependencies::Python::tokio_command().ok_or_else(|| {
        ToolError::execution_failed(
            "code_execution: Python interpreter became unavailable".to_string(),
        )
    })?;
    cmd.arg(&script_path).current_dir(workspace);

    // 收集输出: stdout、stderr、return_code
    let output = tokio::time::timeout(Duration::from_secs(120), cmd.output())
        .await
        .map_err(|_| ToolError::Timeout { seconds: 120 })
        .and_then(|res| res.map_err(|e| ToolError::execution_failed(e.to_string())))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let return_code = output.status.code().unwrap_or(-1);
    let success = output.status.success();
    
    // 构造响应: JSON 格式的结果，含 type: "code_execution_result"
    let payload = json!({
        "type": "code_execution_result",
        "stdout": stdout,
        "stderr": stderr,
        "return_code": return_code,
        "content": [],
    });

    Ok(ToolResult {
        content: serde_json::to_string(&payload).unwrap_or_else(|_| payload.to_string()),
        success,
        metadata: Some(payload),
    })
}
