//! Gherkin 验收测试：eval 冒烟测试。
//!
//! 验证二进制文件能够加载，并且 eval 测试框架在 Layer 4.2 注册表清理后
//! 能报告 shell 命令的步骤级成功。遵循经过验证的
//! `core_session_command_extraction.rs` 模式。
//!
//! 注意：这是一个 eval 冒烟测试，而非命令表面验证
//!（AT-004）测试。它确认二进制文件能正常启动并正确运行 eval。
//! 关于 AT-004 命令表面覆盖（help、palette、completion），请参考
//! command_palette.rs、widgets/mod.rs 和
//! commands/mod.rs 中的针对性单元测试。

use std::path::PathBuf;
use std::process::{Command, ExitStatus};

use cucumber::{World as _, given, then, when, writer::Stats as _};
use serde_json::Value;
use tempfile::TempDir;

const FEATURE_NAME: &str = "Eval 冒烟测试（二进制加载和 eval 步骤报告）";
const FEATURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/features/eval_smoke.feature"
);
const SMOKE_SCENARIO: &str = "二进制加载并通过 eval 报告步骤级成功";

#[derive(Debug, Default, cucumber::World)]
struct EvalSmokeWorld {
    _record_dir: Option<TempDir>,
    report: Option<Value>,
    exit_status: Option<ExitStatus>,
}

#[given("一个干净的 CodeWhale 评估工作区")]
fn clean_codewhale_evaluation_workspace(world: &mut EvalSmokeWorld) {
    world._record_dir = Some(TempDir::new().expect("评估 TempDir"));
}

#[when("评估测试框架运行一条 shell 命令")]
fn eval_harness_runs_shell_command(world: &mut EvalSmokeWorld) {
    let record_dir = world
        ._record_dir
        .as_ref()
        .expect("评估工作区应存在");

    let output = Command::new(codewhale_tui_binary())
        .args([
            "eval",
            "--json",
            "--shell-command",
            "echo eval-smoke-test",
            "--record",
        ])
        .arg(record_dir.path())
        .output()
        .expect("codewhale-tui eval 应能启动");

    // 捕获 stdout/stderr 用于诊断
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let report: Value = serde_json::from_str(&stdout).unwrap_or_else(|err| {
        panic!("eval --json 应输出有效的 JSON：{err}\nstdout：\n{stdout}\nstderr：\n{stderr}")
    });

    world.exit_status = Some(output.status);
    world.report = Some(report);
}

#[then("二进制文件不会崩溃退出")]
fn binary_exits_without_crashing(world: &mut EvalSmokeWorld) {
    let status = world
        .exit_status
        .expect("退出状态应已被捕获");

    // 评估测试框架在 `metrics.success` 为 false 时以退出码 1 退出
    //（main.rs 中的 run_eval 对非成功的场景使用 `bail!("...")`）。
    // 这是预期的行为：评估在离线状态下运行多步骤场景
    //（List、Read、Search、Edit、ApplyPatch、ExecShell），整体
    // metrics.success 反映所有步骤，而不仅仅是 ExecShell。ExecShell
    // 步骤本身会成功——参见 `json_report_contains_execution_steps`。
    //
    // 我们在此验证的内容：
    //   1. 进程已运行完毕（未被信号杀死）
    //   2. 产生了已知的退出码（而非崩溃/挂起）
    //   3. 步骤级成功由下一个 Gherkin 步骤验证。
    let exit_code = status.code().expect("进程应已终止");
    assert_no_signal_crash(&status);
    assert!(
        exit_code == 0 || exit_code == 1,
        "codewhale-tui eval 以意外退出码 {exit_code} 退出（期望 0 或 1）"
    );

    let report = world.report.as_ref().expect("评估报告应存在");
    let steps = report
        .get("steps")
        .and_then(|value| value.as_array())
        .expect("评估报告应包含 'steps' 数组");
    assert!(
        !steps.is_empty(),
        "评估报告应至少包含一个步骤"
    );
}

#[then("JSON 报告包含执行步骤")]
fn json_report_contains_execution_steps(world: &mut EvalSmokeWorld) {
    let report = world.report.as_ref().expect("评估报告应存在");
    let steps = report
        .get("steps")
        .and_then(|value| value.as_array())
        .expect("评估报告应包含 'steps' 数组");

    // 找到 ExecShell 步骤并验证其包含预期的输出
    let exec_step = steps
        .iter()
        .find(|step| step.get("kind").and_then(|v| v.as_str()) == Some("ExecShell"))
        .expect("评估报告应包含 ExecShell 步骤");

    let step_success = exec_step
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(
        step_success,
        "ExecShell 步骤应成功，实际：{exec_step:?}"
    );

    let output = exec_step
        .get("output")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        output.contains("eval-smoke-test"),
        "ExecShell 输出应包含 shell 命令的 echo，实际：{output}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn eval_smoke_binary_loads_and_reports_steps() {
    let writer = EvalSmokeWorld::cucumber()
        .fail_on_skipped()
        .with_default_cli()
        .filter_run(FEATURE_PATH, move |feature, _, scenario| {
            feature.name == FEATURE_NAME && scenario.name == SMOKE_SCENARIO
        })
        .await;
    assert_eq!(
        writer.failed_steps(),
        0,
        "场景失败：{SMOKE_SCENARIO}"
    );
    assert_eq!(
        writer.skipped_steps(),
        0,
        "场景跳过步骤：{SMOKE_SCENARIO}"
    );
    assert_eq!(
        writer.passed_steps(),
        4,
        "场景未运行：{SMOKE_SCENARIO}"
    );
}

/// 断言进程未被信号杀死（仅 Unix 检查）。
#[cfg(unix)]
fn assert_no_signal_crash(status: &ExitStatus) {
    use std::os::unix::process::ExitStatusExt;
    assert!(
        status.signal().is_none(),
        "codewhale-tui eval 被信号 {} 杀死（崩溃？）",
        status.signal().unwrap()
    );
}

/// 在 `ExitStatusExt` 不可用的非 Unix 平台上为空操作。
#[cfg(not(unix))]
fn assert_no_signal_crash(_status: &ExitStatus) {}

fn codewhale_tui_binary() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_codewhale-tui") {
        return PathBuf::from(path);
    }
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_codewhale-tui") {
        return PathBuf::from(path);
    }

    let mut path = std::env::current_exe().expect("当前测试可执行文件路径");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push(format!("codewhale-tui{}", std::env::consts::EXE_SUFFIX));
    path
}
