//! 可见核心命令表面的 Gherkin 验收覆盖。

use cucumber::{World as _, given, then, when, writer::Stats as _};
use tempfile::TempDir;

use crate::commands::{self, CommandResult};
use crate::config::{ApiProvider, Config};
use crate::test_support::{EnvVarGuard, lock_test_env};
use crate::tui::app::{App, TuiOptions};
use crate::tui::history::HistoryCell;

const FEATURE_NAME: &str = "核心命令可见表面";
const FEATURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/features/core_command_surfaces.feature"
);
const INFORMATIONAL_SCENARIO: &str =
    "核心信息类命令写入可见的对话消息";
const STATE_SCENARIO: &str = "核心状态类命令报告可见的变更";
const CLEAR_SCENARIO: &str = "Clear 用可见的确认信息替换之前的对话";
const PERSISTENT_WORK_SCENARIO: &str = "持久化工作命令报告可见的调度请求";

#[derive(Default, cucumber::World)]
struct CoreCommandWorld {
    tmpdir: Option<TempDir>,
    app: Option<Box<App>>,
    home_path: Option<std::path::PathBuf>,
    last_message: Option<String>,
    last_result_is_error: Option<bool>,
}

impl std::fmt::Debug for CoreCommandWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreCommandWorld")
            .field("has_tmpdir", &self.tmpdir.is_some())
            .field("has_app", &self.app.is_some())
            .field("home_path", &self.home_path)
            .field("last_message", &self.last_message)
            .field("last_result_is_error", &self.last_result_is_error)
            .finish()
    }
}

#[given("一个 CodeWhale 核心命令工作区")]
fn core_command_workspace(world: &mut CoreCommandWorld) {
    let tmpdir = TempDir::new().expect("核心命令 TempDir");
    let mut app = create_test_app_with_tmpdir(&tmpdir);
    app.ui_locale = crate::localization::Locale::En;
    app.api_provider = ApiProvider::Deepseek;
    app.model = "deepseek-v4-pro".to_string();
    app.auto_model = false;
    app.model_ids_passthrough = false;

    world.home_path = Some(tmpdir.path().join("home"));
    world.app = Some(Box::new(app));
    world.tmpdir = Some(tmpdir);
}

#[given("一个带一条可见用户消息的 CodeWhale 核心命令工作区")]
fn core_command_workspace_with_one_visible_user_message(world: &mut CoreCommandWorld) {
    core_command_workspace(world);
    let app = world.app.as_deref_mut().expect("app 应存在");
    app.add_message(HistoryCell::User {
        content: "记住鲸鱼迁徙".to_string(),
    });
}

#[when(regex = r#"^用户运行核心命令 "([^"]+)"$"#)]
fn user_runs_core_command(world: &mut CoreCommandWorld, command: String) {
    let result = execute_isolated(world, &command);
    record_visible_result(world, result);
}

#[then(regex = r#"^消息窗口应包含 "([^"]+)"$"#)]
fn message_window_should_include(world: &mut CoreCommandWorld, expected: String) {
    let visible = visible_message_window(world);

    assert!(
        visible.contains(&expected),
        "消息窗口应包含 {expected:?}\n可见对话：\n{visible}"
    );
}

#[then(regex = r#"^消息窗口不应包含 "([^"]+)"$"#)]
fn message_window_should_not_include(world: &mut CoreCommandWorld, forbidden: String) {
    let visible = visible_message_window(world);

    assert!(
        !visible.contains(&forbidden),
        "消息窗口不应包含 {forbidden:?}\n可见对话：\n{visible}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn core_informational_commands_write_visible_transcript_messages() {
    run_scenario(INFORMATIONAL_SCENARIO, 11).await;
}

#[tokio::test(flavor = "current_thread")]
async fn core_state_commands_report_visible_changes() {
    run_scenario(STATE_SCENARIO, 8).await;
}

#[tokio::test(flavor = "current_thread")]
async fn clear_replaces_prior_transcript_with_visible_confirmation() {
    run_scenario(CLEAR_SCENARIO, 4).await;
}

#[tokio::test(flavor = "current_thread")]
async fn persistent_work_commands_report_visible_dispatch_requests() {
    run_scenario(PERSISTENT_WORK_SCENARIO, 7).await;
}

async fn run_scenario(name: &'static str, expected_steps: usize) {
    let writer = CoreCommandWorld::cucumber()
        .fail_on_skipped()
        .with_default_cli()
        .filter_run(FEATURE_PATH, move |feature, _, scenario| {
            feature.name == FEATURE_NAME && scenario.name == name
        })
        .await;
    assert_eq!(writer.failed_steps(), 0, "场景失败：{name}");
    assert_eq!(writer.skipped_steps(), 0, "场景跳过步骤：{name}");
    assert_eq!(
        writer.passed_steps(),
        expected_steps,
        "场景未运行：{name}"
    );
}

fn create_test_app_with_tmpdir(tmpdir: &TempDir) -> App {
    let options = TuiOptions {
        model: "deepseek-v4-pro".to_string(),
        workspace: tmpdir.path().to_path_buf(),
        config_path: None,
        config_profile: None,
        allow_shell: false,
        use_alt_screen: true,
        use_mouse_capture: false,
        use_bracketed_paste: true,
        max_subagents: 1,
        skills_dir: tmpdir.path().join("skills"),
        memory_path: tmpdir.path().join("memory.md"),
        notes_path: tmpdir.path().join("notes.txt"),
        mcp_config_path: tmpdir.path().join("mcp.json"),
        use_memory: false,
        start_in_agent_mode: false,
        skip_onboarding: true,
        yolo: false,
        resume_session_id: None,
        initial_input: None,
    };
    App::new(options, &Config::default())
}

fn execute_isolated(world: &mut CoreCommandWorld, command: &str) -> CommandResult {
    let home = world
        .home_path
        .as_ref()
        .expect("测试 home 应存在")
        .clone();
    std::fs::create_dir_all(&home).expect("创建隔离测试 home");

    let _lock = lock_test_env();
    let _home = EnvVarGuard::set("HOME", &home);
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", home.join(".codewhale"));

    let app = world.app.as_deref_mut().expect("app 应存在");
    commands::user_registry::reload(Some(&app.workspace));
    commands::execute(command, app)
}

fn record_visible_result(world: &mut CoreCommandWorld, result: CommandResult) {
    world.last_result_is_error = Some(result.is_error);
    world.last_message = result.message.clone();

    if let Some(message) = result.message {
        let app = world.app.as_deref_mut().expect("app 应存在");
        app.add_message(HistoryCell::System { content: message });
    }
}

fn visible_message_window(world: &CoreCommandWorld) -> String {
    let app = world.app.as_deref().expect("app 应存在");
    app.history
        .iter()
        .filter_map(|cell| match cell {
            HistoryCell::User { content }
            | HistoryCell::Assistant { content, .. }
            | HistoryCell::System { content }
            | HistoryCell::Thinking { content, .. } => Some(content.as_str()),
            HistoryCell::Error { message, .. } => Some(message.as_str()),
            HistoryCell::ArchivedContext { summary, .. } => Some(summary.as_str()),
            HistoryCell::Tool(_) | HistoryCell::SubAgent(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
