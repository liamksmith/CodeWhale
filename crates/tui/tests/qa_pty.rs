//! 通过真实伪终端驱动的端到端 TUI 场景。
//!
//! 每个场景在密封的工作区 + 密封的 `$HOME` 中启动 `deepseek-tui`，
//! 通过 PTY 发送脚本化输入，并在解析的终端帧和工作区文件系统上断言。
//! 设计和使用方法见 `support/qa_harness/README.md`。
//!
//! 这些测试目前仅在 Unix 上启用。Windows ConPTY 行为（#923、#765、#802）
//! 需要在场景上线前进行单独的审计。

#![cfg(unix)]

#[path = "support/qa_harness/mod.rs"]
mod qa_harness;

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use qa_harness::harness::{Harness, make_sealed_workspace};
use qa_harness::keys;

const BOOT_TIMEOUT: Duration = Duration::from_secs(15);
const KEY_TIMEOUT: Duration = Duration::from_secs(5);
const COMPOSER_READY_TEXT: &str = "Write a task";
static QA_PTY_TEST_LOCK: Mutex<()> = Mutex::new(());

fn qa_pty_test_lock() -> MutexGuard<'static, ()> {
    QA_PTY_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

fn boot_minimal() -> anyhow::Result<(qa_harness::harness::SealedWorkspace, Harness)> {
    let ws = make_sealed_workspace()?;
    spawn_minimal(ws)
}

fn boot_minimal_without_retry() -> anyhow::Result<(qa_harness::harness::SealedWorkspace, Harness)> {
    let ws = make_sealed_workspace()?;
    std::fs::write(
        ws.home().join(".deepseek").join("config.toml"),
        "[retry]\nenabled = false\n",
    )?;
    spawn_minimal(ws)
}

fn spawn_minimal(
    ws: qa_harness::harness::SealedWorkspace,
) -> anyhow::Result<(qa_harness::harness::SealedWorkspace, Harness)> {
    let h = Harness::builder(Harness::cargo_bin("codewhale-tui"))
        .cwd(ws.workspace())
        .clear_env()
        .seal_home(ws.home())
        // 提供一个存根密钥，以便跳过引导屏幕，TUI 直接启动到编辑器。
        // 测试工具从不发出真实请求——我们只需要二进制文件认为密钥存在。
        .env("DEEPSEEK_API_KEY", "ci-test-key-not-real")
        // 强制使用已知的 Base URL，使诊断/模型探测永远不会逃出
        // 沙箱。127.0.0.1:1 会立即拒绝连接。
        .env("DEEPSEEK_BASE_URL", "http://127.0.0.1:1")
        .env("RUST_LOG", "warn")
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--no-project-config",
            "--skip-onboarding",
        ])
        .size(40, 140)
        .spawn()?;
    Ok((ws, h))
}

fn write_skill(root: std::path::PathBuf, name: &str, description: &str) -> anyhow::Result<()> {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\nUse {name}.\n"),
    )?;
    Ok(())
}

fn first_non_blank_row(frame: &qa_harness::Frame) -> Option<u16> {
    (0..frame.rows()).find(|&row| !frame.row(row).trim().is_empty())
}

fn assert_viewport_starts_at_top(frame: &qa_harness::Frame) {
    let dump = frame.debug_dump();
    let first_row = first_non_blank_row(frame).expect("expected visible frame text");
    assert_eq!(
        first_row, 0,
        "视口内容已漂移到第 0 行以下:\n{dump}"
    );
    assert!(
        frame.row(0).contains("Plan")
            || frame.row(0).contains("Act")
            || frame.row(0).contains("Agent")
            || frame.row(0).contains("Operate")
            || frame.row(0).contains("Yolo")
            || frame.row(0).contains("DeepSeek"),
        "第 0 行应包含标题内容:\n{dump}"
    );
}

/// 冒烟测试：二进制文件启动到备用屏幕，绘制编辑器，标题显示项目标签。
/// 如果此测试失败，说明测试工具本身就有问题，不用考虑任何场景。
#[test]
fn smoke_boot_paints_composer() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let (_ws, mut h) = boot_minimal()?;

    h.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;

    let f = h.frame();
    assert!(
        f.any_visible_text(),
        "启动后应存在非空帧:\n{}",
        f.debug_dump()
    );

    let _ = h.shutdown();
    Ok(())
}

/// v0.8.61 启动回归：调度器端配置写入器产生 camelCase 键加上
/// `[features.enabled]`，而 TUI 配置读取器只接受 snake_case 和
/// 扁平的 `[features]` 布尔值。这在 TUI 日志初始化之前就失败了，
/// 从外观看起来像是交互式启动崩溃。通过真实 PTY 引导并证明
/// 早期初始化能到达信任提示并接受输入。
#[test]
fn interactive_init_accepts_input_with_dispatcher_written_config() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let ws = make_sealed_workspace()?;
    std::fs::write(
        ws.home().join(".codewhale").join("config.toml"),
        r#"
provider = "zai"
fallbackProviders = []
apiKey = "deepseek-test-key"
defaultTextModel = "deepseek-v4-pro"
authMode = "api_key"

[providers.zai]
apiKey = "zai-test-key"
authMode = "api_key"

[providers.zai.httpHeaders]

[features.enabled]
shell_tool = true
subagents = true
web_search = true
"#,
    )?;

    let mut h = Harness::builder(Harness::cargo_bin("codewhale-tui"))
        .cwd(ws.workspace())
        .clear_env()
        .seal_home(ws.home())
        .env("RUST_LOG", "warn")
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--no-project-config",
        ])
        .size(40, 140)
        .spawn()?;

    h.wait_for_text("Press Enter to continue", BOOT_TIMEOUT)?;
    h.send(keys::key::enter())?;
    h.wait_for_text("Choose your language", BOOT_TIMEOUT)?;
    h.send(keys::key::enter())?;
    h.wait_for_text("Trust Workspace", BOOT_TIMEOUT)?;
    h.send(keys::key::ch('2'))?;
    assert_eq!(h.wait_for_exit(KEY_TIMEOUT), Some(0));
    Ok(())
}

/// #1085 回归：轮次通过错误路径退出后，终端原点/滚动区域状态
/// 不得在 TUI 上方留下空白行。
#[test]
fn viewport_origin_stays_row_zero_after_failed_turn() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let (_ws, mut h) = boot_minimal_without_retry()?;
    h.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;
    assert_viewport_starts_at_top(h.frame());

    h.send(keys::key::text("trigger a failed turn"))?;
    h.wait_for_idle(Duration::from_millis(200), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;
    h.wait_for(
        |frame| {
            frame.contains("Turn failed")
                || frame.contains("Connection refused")
                || frame.contains("error")
        },
        Duration::from_secs(15),
    )?;
    h.wait_for_idle(Duration::from_millis(300), Duration::from_secs(3))?;
    assert_viewport_starts_at_top(h.frame());

    let _ = h.shutdown();
    Ok(())
}

/// 验证测试工具实际能看到按键——输入一个字符并观察它出现在编辑器中。
/// 这是在将其用于真实场景之前最基础的健全性检查。
#[test]
fn smoke_keystroke_reaches_composer() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let (_ws, mut h) = boot_minimal()?;
    h.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;

    h.send(keys::key::text("hello-from-pty"))?;
    h.wait_for_text("hello-from-pty", KEY_TIMEOUT)?;

    let _ = h.shutdown();
    Ok(())
}

/// 回归测试：`/skills` 应反映与斜杠菜单和模型可见技能块相同的
/// 合并发现集，而不仅仅是第一个选定的技能目录。
#[test]
fn skills_menu_shows_local_and_global_skills() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let ws = make_sealed_workspace()?;
    write_skill(ws.user_skills_dir(), "global-alpha", "Global alpha skill")?;
    write_skill(
        ws.workspace().join(".agents").join("skills"),
        "workspace-beta",
        "Workspace beta skill",
    )?;

    let mut h = Harness::builder(Harness::cargo_bin("codewhale-tui"))
        .cwd(ws.workspace())
        .clear_env()
        .seal_home(ws.home())
        .env("DEEPSEEK_API_KEY", "ci-test-key-not-real")
        .env("DEEPSEEK_BASE_URL", "http://127.0.0.1:1")
        .env("RUST_LOG", "warn")
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--no-project-config",
            "--skip-onboarding",
        ])
        .size(40, 140)
        .spawn()?;

    h.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;
    h.send(keys::key::text("/skills"))?;
    h.wait_for_text("/skills", KEY_TIMEOUT)?;
    h.wait_for_idle(Duration::from_millis(300), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;
    h.wait_for_text("Available skills", KEY_TIMEOUT)?;
    h.wait_for_text("global-alpha", KEY_TIMEOUT)?;
    h.wait_for_text("workspace-beta", KEY_TIMEOUT)?;

    let f = h.frame();
    let dump = f.debug_dump();
    assert!(f.contains("global-alpha"), "全局技能缺失:\n{dump}");
    assert!(
        f.contains("workspace-beta"),
        "工作区技能缺失:\n{dump}"
    );

    let _ = h.shutdown();
    Ok(())
}

// ===========================================================================
// #1073 — 粘贴带有尾部换行符的多行文本不得自动提交
// ===========================================================================

/// 括号粘贴路径：终端将负载包裹在 `ESC[200~ … ESC[201~` 中，
/// crossterm 传递一个 `Event::Paste(text)`，TUI 的括号路径将其插入编辑器。
/// 尾部的 `\n` 应使编辑器持有文本，而不是开始一轮对话。
#[test]
fn paste_bracketed_with_trailing_newline_does_not_autosubmit() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let (_ws, mut h) = boot_minimal()?;
    h.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;

    // 约 200 个字符，匹配原始报告。尾部换行符是历史上触发自动提交的负载。
    let payload = "first line of the multi-line paste body\n\
         second line continuing the paragraph until the end\n\
         third line that finishes with a trailing newline character\n";
    h.paste(payload)?;
    h.wait_for_idle(Duration::from_millis(300), Duration::from_secs(2))?;

    let f = h.frame();
    let dump = f.debug_dump();

    // 自动提交会用 "working / thinking" 状态芯片替换编辑器并清除编辑器文本。
    // 任一信号都表明 bug 已触发。
    assert!(
        !f.contains("Working") && !f.contains("thinking") && !f.contains("Thinking"),
        "带有尾部换行符的括号粘贴自动提交了:\n{dump}"
    );
    assert!(
        f.contains("first line") || f.contains("third line"),
        "粘贴的文本应在编辑器中可见:\n{dump}"
    );

    let _ = h.shutdown();
    Ok(())
}

/// 非括号粘贴路径：终端不包裹负载，因此 crossterm 将字节视为普通按键。
/// TUI 的 `paste_burst` 检测器应识别快速流并将其视为单个粘贴，但历史上
/// 突发的尾部 `\r`（Enter）会泄漏出去并触发提交，而突发刷新将文本排入
/// 现在为空的编辑器。
///
/// 这是来自 #1073 的 Windows / PowerShell 复现。
#[test]
fn paste_unbracketed_with_trailing_newline_does_not_autosubmit() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let (_ws, mut h) = boot_minimal()?;
    h.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;
    // 让启动完全稳定下来，使输入处理已经就绪。
    h.wait_for_idle(Duration::from_millis(300), Duration::from_secs(3))?;

    let payload = "first line of the multi-line paste body\n\
         second line continuing the paragraph until the end\n\
         third line that finishes with a trailing newline character\n";
    h.paste_unbracketed(payload)?;
    h.wait_for_idle(Duration::from_millis(400), Duration::from_secs(3))?;

    let f = h.frame();
    let dump = f.debug_dump();
    eprintln!("=== 非括号粘贴后 ===\n{dump}");

    // 自动提交的可见信号：文本出现在编辑器的上方对话记录中
    //（作为用户消息发送）。编辑器通常也会被重置，但 #1073 报告
    // 除了自动提交外还有残留文本，因此检查对话记录更可靠。
    let count = dump.matches("first line").count();
    assert!(
        count <= 1,
        "'first line' 出现 {count} 次——已自动提交到对话记录和编辑器中:\n{dump}"
    );
    // 粘贴的文本应在某处可见。
    assert!(
        f.contains("first line"),
        "粘贴的文本应在屏幕某处可见:\n{dump}"
    );

    let _ = h.shutdown();
    Ok(())
}
