//! 协议恢复契约测试。
//!
//! 这些测试旨在保持引擎对虚假工具调用包装器（助手文本中的 XML/Replit/markdown 伪调用）的警惕性。
//! 它们的工作是确保：
//!
//! 1. 已知的包装器标记仍然存在于 `core/engine.rs` 中，以便流式过滤器有内容可擦除。
//! 2. 遗留的基于文本的 `tool_parser` 将虚假包装器标记为剥离/状态记账，
//!    但不会将较新的 `<function_calls>` 包装器视为真实的工具调用 — 只有遗留的
//!    `[TOOL_CALL]` 和 `<invoke>` 形式曾经产生过结构化调用，
//!    并且没有任何东西应静默地重新启用基于文本的执行。
//! 3. 结束标记列表与开始标记列表保持相同长度，
//!    以便过滤器逻辑不会永远卡在工具调用模式中。
//!
//! 关键是模型输出中的协议漂移应该是可见的（我们仍然剥离它并发出状态通知），
//! 而不是静默地转化为工具执行。

use std::fs;

#[path = "../src/core/tool_parser.rs"]
#[allow(dead_code)]
mod tool_parser;

// `engine.rs` 已被拆分为 `core/engine/` 下的子模块。
// 下面测试所断言的协议擦除字符串现在分布在 `engine.rs` 和多个 `engine/*.rs` 文件中。
// 我们在编译时包含每个文件，以便贡献者将标记移动到同级子模块时
// 不会静默地破坏这些回归检查。
const ENGINE_SOURCES: &[&str] = &[
    include_str!("../src/core/engine.rs"),
    include_str!("../src/core/engine/streaming.rs"),
    include_str!("../src/core/engine/turn_loop.rs"),
    include_str!("../src/core/engine/dispatch.rs"),
    include_str!("../src/core/engine/tool_setup.rs"),
    include_str!("../src/core/engine/tool_execution.rs"),
    include_str!("../src/core/engine/tool_catalog.rs"),
    include_str!("../src/core/engine/context.rs"),
    include_str!("../src/core/engine/approval.rs"),
    include_str!("../src/core/engine/lsp_hooks.rs"),
];

fn any_engine_source_contains(needle: &str) -> bool {
    ENGINE_SOURCES.iter().any(|src| src.contains(needle))
}

const EXPECTED_START_MARKERS: &[&str] = &[
    "[TOOL_CALL]",
    "<codewhale:tool_call",
    "<tool_call",
    "<invoke ",
    "<function_calls>",
];

const EXPECTED_END_MARKERS: &[&str] = &[
    "[/TOOL_CALL]",
    "</codewhale:tool_call>",
    "</tool_call>",
    "</invoke>",
    "</function_calls>",
];

#[test]
fn engine_keeps_known_fake_wrapper_start_markers() {
    for marker in EXPECTED_START_MARKERS {
        let needle = format!("\"{marker}\"");
        assert!(
            any_engine_source_contains(&needle),
            "no engine source file still mentions start marker `{marker}` — \
             protocol scrubbing may have regressed. Searched for {needle:?} \
             across engine.rs and engine/* submodules."
        );
    }
}

#[test]
fn engine_keeps_known_fake_wrapper_end_markers() {
    for marker in EXPECTED_END_MARKERS {
        let needle = format!("\"{marker}\"");
        assert!(
            any_engine_source_contains(&needle),
            "no engine source file still mentions end marker `{marker}` — \
             protocol scrubbing may have regressed. Searched for {needle:?} \
             across engine.rs and engine/* submodules."
        );
    }
}

#[test]
fn engine_marker_counts_stay_paired() {
    // 未来的贡献者可能会悄悄删除一个结束标记，导致过滤器能够进入工具调用模式而无法退出。
    // 将计数锁定为当前常量所声明的值。
    assert_eq!(EXPECTED_START_MARKERS.len(), EXPECTED_END_MARKERS.len());
    assert!(any_engine_source_contains("TOOL_CALL_START_MARKERS"));
    assert!(any_engine_source_contains("TOOL_CALL_END_MARKERS"));
}

#[test]
fn engine_emits_compact_fake_wrapper_notice() {
    assert!(
        any_engine_source_contains("FAKE_WRAPPER_NOTICE"),
        "no engine source file references the protocol-recovery notice constant"
    );
    assert!(
        any_engine_source_contains("API tool channel"),
        "the protocol-recovery notice should mention the API tool channel"
    );
}

#[test]
fn legacy_parser_extracts_bracket_tool_call() {
    let result = tool_parser::parse_tool_calls(
        "intro [TOOL_CALL]\n{\"tool\":\"x\",\"args\":{}}\n[/TOOL_CALL]",
    );
    assert_eq!(result.tool_calls.len(), 1);
    assert_eq!(result.tool_calls[0].name, "x");
    assert_eq!(result.clean_text, "intro");
}

#[test]
fn legacy_parser_extracts_invoke_block() {
    let result = tool_parser::parse_tool_calls(
        "before <invoke name=\"do_thing\"><parameter name=\"k\">v</parameter></invoke> after",
    );
    assert_eq!(result.tool_calls.len(), 1);
    assert_eq!(result.tool_calls[0].name, "do_thing");
}

#[test]
fn legacy_parser_does_not_execute_function_calls_wrapper() {
    // 较新的 `<function_calls>` 包装器是那种出现在非 DeepSeek 工具调用泄漏中的伪造形式。
    // 遗留的文本解析器绝不能将其转换为结构化工具调用
    // （引擎的过滤器仍然将其从可见文本中剥离，模型应使用 API 工具通道代替）。
    let raw = "narrative <function_calls>\n{\"name\":\"x\",\"input\":{}}\n</function_calls> end";
    let result = tool_parser::parse_tool_calls(raw);
    assert!(
        result.tool_calls.is_empty(),
        "function_calls wrapper must not be parsed as a real tool call: {:?}",
        result.tool_calls
    );
}

#[test]
fn legacy_parser_marker_helper_flags_fake_wrappers_without_enabling_execution() {
    // `has_tool_call_markers` 现在还会标记伪造的包装器，以便引擎可以
    // 从可见文本中擦除它们并保持推理占位符记账。
    // 解析器仍然不能将这些包装器转换为可执行的调用。
    assert!(tool_parser::has_tool_call_markers(
        "noise [TOOL_CALL]x[/TOOL_CALL]"
    ));
    assert!(tool_parser::has_tool_call_markers(
        "noise <invoke name=\"x\"></invoke>"
    ));
    assert!(tool_parser::has_tool_call_markers(
        "noise <function_calls>{}</function_calls>"
    ));
    assert!(
        tool_parser::parse_tool_calls("noise <function_calls>{}</function_calls>")
            .tool_calls
            .is_empty()
    );
}

#[test]
fn engine_source_file_still_exists_and_is_non_trivial() {
    // 完整性检查，使上面的 `include_str!` 有意义 — 如果引擎模块移动了，
    // 此测试必须随之更新。
    let metadata = fs::metadata("src/core/engine.rs").expect("engine.rs must exist next to tests");
    assert!(
        metadata.len() > 10_000,
        "engine.rs is unexpectedly small ({} bytes); did the file move?",
        metadata.len()
    );
}
