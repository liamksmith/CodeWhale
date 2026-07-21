use super::*;
use crate::fleet::roster::FleetRoster;
use crate::tools::{AgentToolSurfaceOptions, ToolRegistryBuilder};
use crate::worker_profile::ShellPolicy;
use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
use std::collections::HashSet;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::{Builder as TempDirBuilder, tempdir};

fn make_assignment() -> SubAgentAssignment {
    SubAgentAssignment::new("prompt".to_string(), Some("worker".to_string()))
}

fn make_snapshot(status: SubAgentStatus) -> SubAgentResult {
    SubAgentResult {
        name: "agent_test".to_string(),
        agent_id: "agent_test".to_string(),
        context_mode: "fresh".to_string(),
        fork_context: false,
        workspace: None,
        git_branch: None,
        agent_type: SubAgentType::General,
        assignment: make_assignment(),
        model: "deepseek-v4-flash".to_string(),
        nickname: None,
        status,
        worker_status: None,
        parent_run_id: None,
        spawn_depth: 0,
        result: None,
        steps_taken: 0,
        checkpoint: None,
        needs_input: None,
        duration_ms: 0,
        from_prior_session: false,
    }
}

fn make_worker_spec(worker_id: &str, workspace: PathBuf) -> AgentWorkerSpec {
    let tool_profile =
        AgentWorkerToolProfile::Explicit(vec!["read_file".to_string(), "grep_files".to_string()]);
    let mut runtime_profile = WorkerRuntimeProfile::for_role(SubAgentType::Explore);
    runtime_profile.tools =
        ToolScope::Explicit(vec!["read_file".to_string(), "grep_files".to_string()]);
    runtime_profile.model = ModelRoute::Fixed("deepseek-v4-flash".to_string());
    runtime_profile.max_spawn_depth = DEFAULT_MAX_SPAWN_DEPTH.saturating_sub(1);
    AgentWorkerSpec {
        worker_id: worker_id.to_string(),
        run_id: worker_id.to_string(),
        parent_run_id: None,
        session_name: Some(worker_id.to_string()),
        objective: "inspect the repo".to_string(),
        role: Some("explorer".to_string()),
        agent_type: SubAgentType::Explore,
        model: "deepseek-v4-flash".to_string(),
        workspace,
        git_branch: None,
        context_mode: "fresh".to_string(),
        fork_context: false,
        tool_profile,
        runtime_profile,
        max_steps: 8,
        spawn_depth: 1,
        max_spawn_depth: DEFAULT_MAX_SPAWN_DEPTH,
    }
}

#[test]
fn headless_worker_record_tracks_lifecycle_without_tui_projection() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = SubAgentManager::new(tmp.path().to_path_buf(), 4);
    manager.register_worker(make_worker_spec(
        "agent_worker_contract",
        tmp.path().to_path_buf(),
    ));

    manager.record_worker_event(
        "agent_worker_contract",
        AgentWorkerStatus::Queued,
        Some(SUBAGENT_QUEUED_LAUNCH_REASON.to_string()),
        None,
        None,
    );
    manager.record_worker_progress(
        "agent_worker_contract",
        "step 1: requesting model response".to_string(),
    );
    manager.record_worker_progress(
        "agent_worker_contract",
        "step 1: running tool 'read_file'".to_string(),
    );

    let mut result = make_snapshot(SubAgentStatus::Completed);
    result.agent_id = "agent_worker_contract".to_string();
    result.name = "agent_worker_contract".to_string();
    result.result = Some("worker summary".to_string());
    result.steps_taken = 1;
    manager.complete_worker_from_result("agent_worker_contract", &result);

    let record = manager
        .get_worker_record("agent_worker_contract")
        .expect("worker record");
    assert_eq!(record.status, AgentWorkerStatus::Completed);
    assert_eq!(record.spec.run_id, "agent_worker_contract");
    assert_eq!(record.actor_kind, "subagent");
    assert_eq!(record.spec.agent_type, SubAgentType::Explore);
    assert_eq!(
        record.spec.tool_profile,
        AgentWorkerToolProfile::Explicit(vec!["read_file".to_string(), "grep_files".to_string()])
    );
    assert_eq!(record.spec.runtime_profile.role, SubAgentType::Explore);
    assert!(!record.spec.runtime_profile.permissions.write);
    assert_eq!(
        record.spec.runtime_profile.tools,
        ToolScope::Explicit(vec!["read_file".to_string(), "grep_files".to_string()])
    );
    assert_eq!(
        record.spec.runtime_profile.model,
        ModelRoute::Fixed("deepseek-v4-flash".to_string())
    );
    assert_eq!(record.result_summary.as_deref(), Some("worker summary"));
    assert_eq!(record.steps_taken, 1);
    assert_eq!(record.follow_up.tool, "handle_read");
    assert_eq!(record.follow_up.agent_id.as_str(), "agent_worker_contract");
    assert_eq!(record.recommended_action.action, "verify_self_report");
    assert_eq!(
        record.recommended_action.tool.as_deref(),
        Some("handle_read")
    );
    assert!(record.takeover.supported);
    assert!(
        record
            .takeover
            .instructions
            .contains("transcript_handle with handle_read")
    );
    assert_eq!(record.usage.status, "unknown");
    assert_eq!(record.verification.status, "self_report_only");
    assert!(
        record
            .artifacts
            .iter()
            .any(|artifact| artifact.kind == "transcript")
    );
    let statuses: Vec<_> = record.events.iter().map(|event| event.status).collect();
    assert!(statuses.contains(&AgentWorkerStatus::Queued));
    assert!(statuses.contains(&AgentWorkerStatus::ModelWait));
    assert!(statuses.contains(&AgentWorkerStatus::RunningTool));
    assert!(statuses.contains(&AgentWorkerStatus::Completed));
    assert!(
        record
            .events
            .iter()
            .any(|event| event.tool_name.as_deref() == Some("read_file"))
    );
}

#[test]
fn worker_record_usage_accumulates_provider_tokens() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = SubAgentManager::new(tmp.path().to_path_buf(), 4);
    manager.register_worker(make_worker_spec("agent_usage", tmp.path().to_path_buf()));

    manager.record_worker_usage(
        "agent_usage",
        &Usage {
            input_tokens: 100,
            output_tokens: 25,
            prompt_cache_hit_tokens: Some(70),
            prompt_cache_miss_tokens: Some(30),
            ..Usage::default()
        },
    );
    manager.record_worker_usage(
        "agent_usage",
        &Usage {
            input_tokens: 40,
            output_tokens: 10,
            ..Usage::default()
        },
    );

    let record = manager
        .get_worker_record("agent_usage")
        .expect("worker record");
    assert_eq!(record.usage.status, "reported");
    assert_eq!(record.usage.input_tokens, Some(140));
    assert_eq!(record.usage.output_tokens, Some(35));
    assert_eq!(record.usage.total_tokens, Some(175));
    assert_eq!(record.usage.token_budget, None);
    assert!(
        record.usage.note.contains("175 tokens"),
        "usage note includes reported total: {}",
        record.usage.note
    );
}

#[test]
fn token_budget_scope_is_shared_across_nested_workers_and_blocks_when_spent() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let mut manager =
        SubAgentManager::new(workspace.clone(), 4).with_default_token_budget(Some(100));

    manager.register_worker(make_worker_spec("agent_root", workspace.clone()));
    let root_scope = manager
        .resolve_spawn_budget_scope("agent_root", None, None)
        .expect("root budget resolves")
        .expect("root budget present");
    manager.attach_budget_scope("agent_root", root_scope);
    manager.record_worker_usage(
        "agent_root",
        &Usage {
            input_tokens: 40,
            output_tokens: 10,
            ..Usage::default()
        },
    );

    let mut child_spec = make_worker_spec("agent_child", workspace);
    child_spec.parent_run_id = Some("agent_root".to_string());
    let child_scope = manager
        .resolve_spawn_budget_scope("agent_child", Some("agent_root"), None)
        .expect("child inherits budget")
        .expect("child budget present");
    assert_eq!(child_scope.scope_id, "agent_root");
    assert_eq!(child_scope.limit, 100);
    assert_eq!(child_scope.spent, 50);
    manager.register_worker(child_spec);
    manager.attach_budget_scope("agent_child", child_scope);
    manager.record_worker_usage(
        "agent_child",
        &Usage {
            input_tokens: 30,
            output_tokens: 20,
            ..Usage::default()
        },
    );

    let root = manager.get_worker_record("agent_root").expect("root");
    let child = manager.get_worker_record("agent_child").expect("child");
    assert_eq!(root.usage.budget_spent_tokens, Some(100));
    assert_eq!(child.usage.budget_spent_tokens, Some(100));
    assert_eq!(root.usage.budget_remaining_tokens, Some(0));
    assert_eq!(child.usage.budget_remaining_tokens, Some(0));
    assert_eq!(root.usage.status, "budget_exhausted");

    let err = manager
        .resolve_spawn_budget_scope("agent_grandchild", Some("agent_child"), None)
        .expect_err("spent shared budget blocks further child spawn");
    assert!(
        err.to_string().contains("token budget exhausted"),
        "actionable exhaustion error: {err}"
    );

    let override_scope = manager
        .resolve_spawn_budget_scope("agent_override", Some("agent_child"), Some(20))
        .expect("explicit override starts new scope")
        .expect("override budget present");
    assert_eq!(override_scope.scope_id, "agent_override");
    assert_eq!(override_scope.limit, 20);
    assert_eq!(override_scope.spent, 0);
}

#[test]
fn agent_worker_profile_derives_from_parent_without_escalation() {
    let mut runtime = stub_runtime();
    runtime.worker_profile = WorkerRuntimeProfile::for_role(SubAgentType::Explore);
    runtime.spawn_depth = 1;
    runtime.max_spawn_depth = DEFAULT_MAX_SPAWN_DEPTH;
    let tool_profile =
        AgentWorkerToolProfile::Explicit(vec!["read_file".to_string(), "write_file".to_string()]);

    let profile = worker_profile_for_spawn(
        &runtime,
        &SubAgentType::Implementer,
        &tool_profile,
        "deepseek-v4-pro",
        Some(ModelRoute::Fixed("deepseek-v4-pro".to_string())),
    );

    assert_eq!(profile.role, SubAgentType::Implementer);
    assert!(
        !profile.permissions.write,
        "child cannot gain write permission from a read-only parent profile"
    );
    assert_eq!(profile.shell, ShellPolicy::ReadOnly);
    assert_eq!(profile.max_spawn_depth, DEFAULT_MAX_SPAWN_DEPTH - 1);
    assert_eq!(
        profile.model,
        ModelRoute::Fixed("deepseek-v4-pro".to_string())
    );
    assert_eq!(
        profile.tools,
        ToolScope::Explicit(vec!["read_file".to_string(), "write_file".to_string()])
    );
}

#[test]
fn subagent_progress_displays_shell_tools_as_bash() {
    assert_eq!(subagent_progress_tool_display_name("exec_shell"), "Bash");
    assert_eq!(subagent_progress_tool_display_name("exec_wait"), "Bash");
    assert_eq!(
        subagent_progress_tool_display_name("task_shell_wait"),
        "Bash"
    );
    assert_eq!(
        subagent_progress_tool_display_name("read_file"),
        "read_file"
    );
}

#[test]
fn agent_progress_preserves_event_channel_headroom_under_load() {
    let (tx, mut rx) = mpsc::channel(40);
    for _ in 0..8 {
        tx.try_send(Event::status("filler")).expect("fill channel");
    }
    assert_eq!(tx.capacity(), 32);

    emit_agent_progress(
        Some(&tx),
        "agent_busy",
        "step 1: requesting model response".to_string(),
        None,
        1,
    );
    assert_eq!(
        tx.capacity(),
        32,
        "routine progress should preserve reserved event-channel headroom"
    );

    emit_agent_progress(
        Some(&tx),
        "agent_waiting",
        "waiting for user input".to_string(),
        None,
        1,
    );
    assert_eq!(
        tx.capacity(),
        31,
        "high-value progress should still reach the UI when headroom is reserved"
    );

    for _ in 0..8 {
        assert!(matches!(rx.try_recv(), Ok(Event::Status { .. })));
    }
    assert!(matches!(
        rx.try_recv(),
        Ok(Event::AgentProgress { id, status, .. })
            if id == "agent_waiting" && status == "waiting for user input"
    ));
    assert!(rx.try_recv().is_err());
}

#[test]
fn agent_progress_uses_small_event_channels_without_headroom_reservation() {
    let (tx, mut rx) = mpsc::channel(8);

    emit_agent_progress(
        Some(&tx),
        "agent_small_channel",
        "step 1: requesting model response".to_string(),
        None,
        1,
    );

    assert_eq!(tx.capacity(), 7);
    assert!(matches!(
        rx.try_recv(),
        Ok(Event::AgentProgress { id, status, .. })
            if id == "agent_small_channel" && status == "step 1: requesting model response"
    ));
}

#[test]
fn headless_worker_records_persist_with_subagent_state() {
    let tmp = tempdir().expect("tempdir");
    let state_path = tmp.path().join("subagents.v1.json");
    let mut manager =
        SubAgentManager::new(tmp.path().to_path_buf(), 4).with_state_path(state_path.clone());
    manager.register_worker(make_worker_spec(
        "agent_persisted",
        tmp.path().to_path_buf(),
    ));

    let mut result = make_snapshot(SubAgentStatus::Failed("boom".to_string()));
    result.agent_id = "agent_persisted".to_string();
    result.name = "agent_persisted".to_string();
    result.steps_taken = 3;
    manager.complete_worker_from_result("agent_persisted", &result);
    manager
        .persist_state()
        .expect("persist state")
        .join()
        .expect("persist thread");

    let mut loaded = SubAgentManager::new(tmp.path().to_path_buf(), 4).with_state_path(state_path);
    loaded.load_state().expect("load state");

    let record = loaded.get_worker_record("agent_persisted").expect("record");
    assert_eq!(record.spec.run_id, "agent_persisted");
    assert_eq!(record.follow_up.agent_id, "agent_persisted");
    assert!(record.takeover.supported);
    assert_eq!(record.status, AgentWorkerStatus::Failed);
    assert_eq!(record.error.as_deref(), Some("boom"));
    assert_eq!(record.steps_taken, 3);
    assert!(
        record
            .events
            .iter()
            .any(|event| event.status == AgentWorkerStatus::Failed)
    );
}

fn init_subagent_git_repo() -> tempfile::TempDir {
    let dir = tempdir().expect("tempdir");

    let init = Command::new("git")
        .arg("init")
        .current_dir(dir.path())
        .output()
        .expect("git init should run");
    assert!(
        init.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let autocrlf = Command::new("git")
        .args(["config", "core.autocrlf", "false"])
        .current_dir(dir.path())
        .output()
        .expect("git config core.autocrlf should run");
    assert!(
        autocrlf.status.success(),
        "git config core.autocrlf failed: {}",
        String::from_utf8_lossy(&autocrlf.stderr)
    );

    let commit = Command::new("git")
        .args([
            "-c",
            "user.name=codewhale Tests",
            "-c",
            "user.email=tests@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-m",
            "init",
        ])
        .current_dir(dir.path())
        .output()
        .expect("git commit should run");
    assert!(
        commit.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&commit.stderr)
    );

    dir
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git command should run");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn text_message(role: &str, text: &str) -> Message {
    Message {
        role: role.to_string(),
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            cache_control: None,
        }],
    }
}

fn make_checkpoint(agent_id: &str, steps_taken: u32, messages: Vec<Message>) -> SubAgentCheckpoint {
    build_subagent_checkpoint(agent_id, "test_checkpoint", &messages, steps_taken, true)
}

fn message_text(message: &Message) -> &str {
    match message.content.first() {
        Some(ContentBlock::Text { text, .. }) => text.as_str(),
        other => panic!("expected text content block, got {other:?}"),
    }
}

async fn delayed_chat_client(
    first_delay: Duration,
    response_text: &str,
) -> (
    DeepSeekClient,
    Arc<AtomicUsize>,
    Arc<std::sync::Mutex<Vec<Value>>>,
) {
    let calls = Arc::new(AtomicUsize::new(0));
    let bodies = Arc::new(std::sync::Mutex::new(Vec::new()));
    let response_text = response_text.to_string();
    let app = Router::new().route(
        "/{*path}",
        post({
            let calls = Arc::clone(&calls);
            let bodies = Arc::clone(&bodies);
            move |Json(body): Json<Value>| {
                let calls = Arc::clone(&calls);
                let bodies = Arc::clone(&bodies);
                let response_text = response_text.clone();
                async move {
                    let attempt = calls.fetch_add(1, Ordering::SeqCst) + 1;
                    bodies
                        .lock()
                        .expect("request body recorder mutex poisoned")
                        .push(body);
                    if attempt == 1 {
                        tokio::time::sleep(first_delay).await;
                    }
                    Json(json!({
                        "id": format!("chatcmpl-test-{attempt}"),
                        "model": "deepseek-v4-flash",
                        "choices": [{
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": response_text
                            },
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": 1,
                            "completion_tokens": 1,
                            "total_tokens": 2
                        }
                    }))
                }
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake chat server");
    let addr = listener.local_addr().expect("fake chat server addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let config = crate::config::Config {
        api_key: Some("test-key".to_string()),
        base_url: Some(format!("http://{addr}/v1")),
        ..crate::config::Config::default()
    };
    let client = DeepSeekClient::new(&config).expect("fake chat client");
    (client, calls, bodies)
}

async fn transient_header_timeout_then_success_chat_client(
    response_text: &str,
) -> (DeepSeekClient, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let response_text = response_text.to_string();
    let app = Router::new().route(
        "/{*path}",
        post({
            let calls = Arc::clone(&calls);
            move |Json(_body): Json<Value>| {
                let calls = Arc::clone(&calls);
                let response_text = response_text.clone();
                async move {
                    let attempt = calls.fetch_add(1, Ordering::SeqCst) + 1;
                    if attempt == 1 {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "error": {
                                    "message": "SSE stream request did not receive response headers after 45s"
                                }
                            })),
                        )
                            .into_response();
                    }
                    Json(json!({
                        "id": format!("chatcmpl-test-{attempt}"),
                        "model": "deepseek-v4-flash",
                        "choices": [{
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": response_text
                            },
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": 1,
                            "completion_tokens": 1,
                            "total_tokens": 2
                        }
                    }))
                    .into_response()
                }
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake transient chat server");
    let addr = listener.local_addr().expect("fake chat server addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let config = crate::config::Config {
        api_key: Some("test-key".to_string()),
        base_url: Some(format!("http://{addr}/v1")),
        ..crate::config::Config::default()
    };
    let client = DeepSeekClient::new(&config).expect("fake transient chat client");
    (client, calls)
}

async fn always_rate_limited_chat_client() -> (DeepSeekClient, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/{*path}",
        post({
            let calls = Arc::clone(&calls);
            move |Json(_body): Json<Value>| {
                let calls = Arc::clone(&calls);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::TOO_MANY_REQUESTS,
                        [("Retry-After", "0")],
                        Json(json!({
                            "error": {
                                "message": "test provider rate limit"
                            }
                        })),
                    )
                        .into_response()
                }
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake rate-limited chat server");
    let addr = listener.local_addr().expect("fake chat server addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let config = crate::config::Config {
        api_key: Some("test-key".to_string()),
        base_url: Some(format!("http://{addr}/v1")),
        retry: Some(crate::config::RetryConfig {
            enabled: Some(false),
            max_retries: Some(0),
            initial_delay: Some(0.0),
            max_delay: Some(0.0),
            exponential_base: Some(1.0),
        }),
        ..crate::config::Config::default()
    };
    let client = DeepSeekClient::new(&config).expect("fake rate-limited chat client");
    (client, calls)
}

fn estimate_tool_description_tokens_conservative(text: &str) -> usize {
    text.chars().count().div_ceil(3)
}

#[test]
fn test_agent_type_from_str() {
    assert_eq!(
        SubAgentType::from_str("general"),
        Some(SubAgentType::General)
    );
    assert_eq!(
        SubAgentType::from_str("explore"),
        Some(SubAgentType::Explore)
    );
    assert_eq!(SubAgentType::from_str("PLAN"), Some(SubAgentType::Plan));
    assert_eq!(
        SubAgentType::from_str("code-review"),
        Some(SubAgentType::Review)
    );
    assert_eq!(
        SubAgentType::from_str("worker"),
        Some(SubAgentType::General)
    );
    assert_eq!(
        SubAgentType::from_str("default"),
        Some(SubAgentType::General)
    );
    assert_eq!(
        SubAgentType::from_str("explorer"),
        Some(SubAgentType::Explore)
    );
    assert_eq!(SubAgentType::from_str("awaiter"), Some(SubAgentType::Plan));
    assert_eq!(SubAgentType::from_str("invalid"), None);
}

#[test]
fn test_agent_type_implementer_aliases() {
    // #404 — Implementer 接受模型在用户说"构建此功能"时很可能使用的明显别名
    // 当用户说"构建此功能"时模型很可能使用的明显别名。
    for alias in ["implementer", "implement", "implementation", "builder"] {
        assert_eq!(
            SubAgentType::from_str(alias),
            Some(SubAgentType::Implementer),
            "alias {alias} should resolve to Implementer"
        );
    }
    // 不区分大小写。
    assert_eq!(
        SubAgentType::from_str("IMPLEMENTER"),
        Some(SubAgentType::Implementer)
    );
}

#[test]
fn test_agent_type_verifier_aliases() {
    // #404 — Verifier 接受与 Reviewer 不同的 test/validate 别名，
    // Reviewer 用于*评估*代码而非*执行*代码。
    for alias in ["verifier", "verify", "verification", "validator", "tester"] {
        assert_eq!(
            SubAgentType::from_str(alias),
            Some(SubAgentType::Verifier),
            "alias {alias} should resolve to Verifier"
        );
    }
    assert_eq!(
        SubAgentType::from_str("VERIFY"),
        Some(SubAgentType::Verifier)
    );
}

#[test]
fn test_agent_type_round_trips_via_as_str() {
    // 每个类型都应序列化为可通过 from_str 往返还原的字符串，
    // 在新增角色时捕获遗漏的变体。
    //
    for t in [
        SubAgentType::General,
        SubAgentType::Explore,
        SubAgentType::Plan,
        SubAgentType::Review,
        SubAgentType::Implementer,
        SubAgentType::Verifier,
        SubAgentType::Custom,
    ] {
        let label = t.as_str();
        let back = SubAgentType::from_str(label)
            .unwrap_or_else(|| panic!("as_str label {label:?} doesn't round-trip via from_str"));
        assert_eq!(back, t, "round-trip failed for {t:?} via {label:?}");
    }
}

#[test]
fn test_implementer_and_verifier_have_distinct_prompts() {
    // 添加这些类型的全部意义在于它们带有不同的姿态。
    // 防御性检查：捕获复制粘贴后两个新变体与 General 使用相同提示的常见错误
    // 导致两个新变体与 General 使用相同提示。
    let implementer = SubAgentType::Implementer.system_prompt();
    let verifier = SubAgentType::Verifier.system_prompt();
    let general = SubAgentType::General.system_prompt();
    assert_ne!(
        implementer, general,
        "Implementer prompt must differ from General"
    );
    assert_ne!(
        verifier, general,
        "Verifier prompt must differ from General"
    );
    assert_ne!(
        implementer, verifier,
        "Implementer and Verifier must differ"
    );
    // 合理性检查：每个提示都应提及角色的定义性动词，以便
    // 模型有明确的方向。
    assert!(
        implementer.to_lowercase().contains("implement")
            || implementer.to_lowercase().contains("write the code"),
        "Implementer prompt should reference its role: {implementer}"
    );
    assert!(
        verifier.to_lowercase().contains("verif")
            || verifier.to_lowercase().contains("test suite")
            || verifier.to_lowercase().contains("validation"),
        "Verifier prompt should reference its role: {verifier}"
    );
}

#[test]
fn test_agent_type_prompts_include_shared_output_contract_once() {
    for (agent_type, marker) in [
        (SubAgentType::General, "general-purpose sub-agent"),
        (SubAgentType::Explore, "exploration sub-agent"),
        (SubAgentType::Plan, "planning sub-agent"),
        (SubAgentType::Review, "code review sub-agent"),
        (SubAgentType::Implementer, "implementation sub-agent"),
        (SubAgentType::Verifier, "verification sub-agent"),
        (SubAgentType::Custom, "custom sub-agent"),
    ] {
        let prompt = agent_type.system_prompt();
        assert!(prompt.contains(marker));
        assert_eq!(
            prompt.matches("## Output contract (mandatory)").count(),
            1,
            "{agent_type:?} prompt should include the shared output contract exactly once"
        );
        assert!(prompt.contains("### SUMMARY") && prompt.contains("### BLOCKERS"));
    }
}

#[test]
fn explore_prompt_orients_before_searching() {
    let prompt = SubAgentType::Explore.system_prompt();
    assert!(prompt.contains("role: `explore`"));
    assert!(prompt.contains("AGENTS.md/README"));
    assert!(prompt.contains("workspace/project root"));
    assert!(prompt.contains("compressed reconnaissance"));
}

#[test]
fn explore_prompt_is_quick_bounded_and_read_only() {
    let prompt = SubAgentType::Explore.system_prompt();
    assert!(prompt.contains("Default to `EFFORT: quick`"));
    assert!(prompt.contains("3-5 tool calls"));
    assert!(prompt.contains("strictly read-only"));
    assert!(prompt.contains("ALREADY_KNOWN"));
    assert!(prompt.contains("STOP_CONDITION"));
    assert!(prompt.contains("Return partial findings"));
}

#[test]
fn implementer_prompt_is_not_forced_into_explorer_cap() {
    let prompt = SubAgentType::Implementer.system_prompt();
    assert!(prompt.contains("not limited to an explorer-style 3-5 tool-call cap"));
    assert!(prompt.contains("Checkpoint before expanding scope"));
    assert!(!prompt.contains("Default to `EFFORT: quick`"));
}

#[test]
fn review_and_verifier_prompts_stop_after_decisive_evidence() {
    let review = SubAgentType::Review.system_prompt();
    let verifier = SubAgentType::Verifier.system_prompt();
    assert!(review.contains("stop after decisive evidence"));
    assert!(verifier.contains("stop after decisive pass/fail evidence"));
}

#[test]
fn agent_description_explains_background_child_and_transcript_handle() {
    let tmp = tempdir().expect("tempdir");
    let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 1);
    let tool = AgentTool::new(manager, stub_runtime());
    let description = tool.description();

    assert!(description.contains("Start, inspect, peek at, or cancel focused child agent tasks"));
    assert!(description.contains("runs or queues"));
    assert!(description.contains("provider rate-limit"));
    assert!(description.contains("background"));
    assert!(description.contains("transcript_handle"));
    assert!(
        estimate_tool_description_tokens_conservative(description) <= 1024,
        "agent description exceeds the conservative 1024-token budget"
    );
}

#[test]
fn new_session_tools_use_single_agent_name() {
    let manager = Arc::new(RwLock::new(SubAgentManager::new(PathBuf::from("."), 1)));
    assert_eq!(AgentTool::new(manager, stub_runtime()).name(), "agent");
}

#[test]
fn test_parse_spawn_request_accepts_message_and_agent_type_aliases() {
    let input = json!({
        "message": "Find references to Foo",
        "agent_type": "explorer"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(parsed.prompt, "Find references to Foo");
    assert_eq!(parsed.agent_type, SubAgentType::Explore);
    assert_eq!(parsed.assignment.role.as_deref(), Some("explorer"));
}

#[test]
fn test_parse_spawn_request_accepts_objective_and_role_alias() {
    let input = json!({
        "objective": "Coordinate and wait",
        "role": "awaiter"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(parsed.prompt, "Coordinate and wait");
    assert_eq!(parsed.agent_type, SubAgentType::Plan);
    assert_eq!(parsed.assignment.role.as_deref(), Some("awaiter"));
}

#[test]
fn test_parse_spawn_request_accepts_items_payload() {
    let input = json!({
        "items": [
            {"type": "text", "text": "Analyze module"},
            {"type": "mention", "name": "drive", "path": "app://drive"}
        ],
        "agent_name": "explorer"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert!(parsed.prompt.contains("Analyze module"));
    assert!(parsed.prompt.contains("[mention:$drive](app://drive)"));
    assert_eq!(parsed.agent_type, SubAgentType::Explore);
}

#[test]
fn test_parse_spawn_request_accepts_fork_context() {
    let input = json!({
        "prompt": "continue from here",
        "fork_context": true
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert!(parsed.fork_context);

    let input = json!({
        "prompt": "continue from here",
        "inherit_context": true
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert!(parsed.fork_context);
}

#[test]
fn test_parse_spawn_request_accepts_model_strength() {
    let input = json!({
        "prompt": "scan parser references",
        "type": "explore",
        "model_strength": "faster"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(parsed.agent_type, SubAgentType::Explore);
    assert_eq!(parsed.model_strength, SubAgentModelStrength::Faster);

    let input = json!({
        "prompt": "apply a release fix",
        "modelStrength": "same"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(parsed.model_strength, SubAgentModelStrength::Same);
}

#[test]
fn explore_subagent_defaults_to_faster_model_strength() {
    // type: "explore" 无 model_strength 且无 model 时默认使用 Faster：
    // 有界限的只读查找正是廉价兄弟任务。
    let input = json!({
        "prompt": "find every caller of normalize_model_name",
        "type": "explore"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(parsed.agent_type, SubAgentType::Explore);
    assert_eq!(parsed.model_strength, SubAgentModelStrength::Faster);

    // 显式 model_strength："same" 对 explore 也优先。
    let input = json!({
        "prompt": "explore but stay capable",
        "type": "explore",
        "model_strength": "same"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(parsed.agent_type, SubAgentType::Explore);
    assert_eq!(parsed.model_strength, SubAgentModelStrength::Same);

    // 显式模型固定子代理（下游 Fixed 路由）并禁用
    // explore→faster 默认值，因此 model_strength 回退到 Same。
    let input = json!({
        "prompt": "explore on a specific model",
        "type": "explore",
        "model": "GLM-5.2"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(parsed.agent_type, SubAgentType::Explore);
    assert_eq!(parsed.model_strength, SubAgentModelStrength::Same);
}

#[test]
fn non_explore_subagents_keep_default_same_model_strength() {
    // 非 explore 角色即使没有模型也保持保守的 Same 默认值。
    for role in ["general", "plan", "review", "implementer"] {
        let input = json!({
            "prompt": "do some work",
            "type": role
        });
        let parsed = parse_spawn_request(&input).expect("spawn request should parse");
        assert_eq!(
            parsed.model_strength,
            SubAgentModelStrength::Same,
            "role {role:?} should default to Same"
        );
    }
}

#[test]
fn test_parse_spawn_request_accepts_child_thinking() {
    let input = json!({
        "prompt": "scan parser references",
        "thinking": "off"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(
        parsed.thinking,
        SubAgentThinking::Effort(ReasoningEffort::Off)
    );

    let input = json!({
        "prompt": "design a fix",
        "reasoning_effort": "max"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(
        parsed.thinking,
        SubAgentThinking::Effort(ReasoningEffort::Max)
    );

    let input = json!({
        "prompt": "classify complexity",
        "reasoningEffort": "auto"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(parsed.thinking, SubAgentThinking::Auto);
}

#[test]
fn test_parse_spawn_request_rejects_invalid_model_strength() {
    let input = json!({
        "prompt": "scan parser references",
        "model_strength": "automatic"
    });
    let err = parse_spawn_request(&input).expect_err("invalid model_strength should fail");
    assert!(
        err.to_string()
            .contains("model_strength must be one of: same, faster")
    );
}

#[test]
fn test_parse_spawn_request_rejects_invalid_child_thinking() {
    let input = json!({
        "prompt": "scan parser references",
        "thinking": "forever"
    });
    let err = parse_spawn_request(&input).expect_err("invalid thinking should fail");
    assert!(
        err.to_string()
            .contains("thinking must be one of: inherit, auto, off, low, medium, high, max")
    );
}

#[test]
fn test_parse_spawn_request_accepts_session_name_for_agent() {
    let input = json!({
        "name": "review.parser",
        "prompt": "inspect parser",
        "fork_context": true,
        "max_depth": 0
    });
    let parsed = parse_spawn_request(&input).expect("agent request should parse");
    assert_eq!(parsed.session_name.as_deref(), Some("review.parser"));
    assert!(parsed.fork_context);
    assert_eq!(parsed.max_depth, Some(0));
}

#[test]
fn test_parse_spawn_request_rejects_invalid_session_name() {
    let input = json!({
        "name": "bad name",
        "prompt": "inspect parser"
    });
    let err = parse_spawn_request(&input).expect_err("space in name should fail");
    assert!(err.to_string().contains("name must not contain whitespace"));
}

#[test]
fn test_parse_spawn_request_rejects_out_of_range_max_depth() {
    let ceiling = codewhale_config::MAX_SPAWN_DEPTH_CEILING;
    let input = json!({
        "name": "review.parser",
        "prompt": "inspect parser",
        "max_depth": ceiling + 1
    });
    let err = parse_spawn_request(&input).expect_err("max_depth should be capped at schema range");
    assert!(
        err.to_string()
            .contains(&format!("max_depth must be between 0 and {ceiling}"))
    );
}

fn fleet_roster_with(id: &str, profile: codewhale_config::FleetProfile) -> FleetRoster {
    let tmp = tempdir().expect("tempdir");
    let config = codewhale_config::FleetConfigToml {
        profiles: std::collections::BTreeMap::from([(id.to_string(), profile)]),
        ..Default::default()
    };
    FleetRoster::load(&config, tmp.path())
}

fn custom_fleet_profile(role: &str) -> codewhale_config::FleetProfile {
    codewhale_config::FleetProfile {
        slot: codewhale_config::FleetSlot::from_name(role),
        role: codewhale_config::FleetRole {
            name: role.to_string(),
            description: None,
            instructions: None,
        },
        ..Default::default()
    }
}

#[test]
fn test_parse_spawn_request_accepts_profile_and_normalizes() {
    let input = json!({
        "prompt": "review the diff",
        "profile": "  Reviewer  "
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(parsed.profile.as_deref(), Some("reviewer"));
    assert!(!parsed.agent_type_explicit);
    assert!(!parsed.model_strength_explicit);

    let parsed = parse_spawn_request(&json!({"prompt": "x", "fleet_profile": "Scout"}))
        .expect("fleet_profile alias should parse");
    assert_eq!(parsed.profile.as_deref(), Some("scout"));

    let parsed = parse_spawn_request(&json!({"prompt": "x", "roster_profile": "BUILDER"}))
        .expect("roster_profile alias should parse");
    assert_eq!(parsed.profile.as_deref(), Some("builder"));
}

#[test]
fn test_parse_spawn_request_rejects_invalid_profile_token() {
    for bad in [
        "rev iewer",
        "rev\"iewer",
        "rev'iewer",
        "rev`iewer",
        "rev=er",
    ] {
        let err = parse_spawn_request(&json!({"prompt": "x", "profile": bad}))
            .expect_err("invalid profile token should fail");
        assert!(
            err.to_string()
                .contains("profile must be a bare roster member id"),
            "{bad}: {err}"
        );
    }
}

#[test]
fn test_apply_spawn_profile_unknown_lists_available_members() {
    let roster = FleetRoster::built_ins_only();
    let mut request =
        parse_spawn_request(&json!({"prompt": "x", "profile": "warlock"})).expect("parse");
    let err = apply_spawn_profile(&mut request, &roster).expect_err("unknown profile should fail");
    let message = err.to_string();
    assert!(
        message.contains("Unknown fleet role/profile 'warlock'"),
        "{message}"
    );
    for member in [
        "manager",
        "scout",
        "builder",
        "reviewer",
        "verifier",
        "synthesizer",
        "general",
    ] {
        assert!(message.contains(member), "missing {member}: {message}");
    }
}

#[test]
fn test_apply_spawn_profile_rejects_conflicting_explicit_type() {
    let roster = FleetRoster::built_ins_only();
    let mut request = parse_spawn_request(&json!({
        "prompt": "x",
        "profile": "reviewer",
        "type": "implementer"
    }))
    .expect("parse");
    let err = apply_spawn_profile(&mut request, &roster).expect_err("type conflict should fail");
    let message = err.to_string();
    assert!(
        message.contains("profile 'reviewer' implies type review"),
        "{message}"
    );
    assert!(
        message.contains("conflicting explicit type 'implementer'"),
        "{message}"
    );
}

#[test]
fn test_apply_spawn_profile_accepts_agreeing_explicit_type() {
    let roster = FleetRoster::built_ins_only();
    let mut request = parse_spawn_request(&json!({
        "prompt": "x",
        "profile": "reviewer",
        "type": "review"
    }))
    .expect("parse");
    let member = apply_spawn_profile(&mut request, &roster)
        .expect("agreeing type should pass")
        .expect("member resolved");
    assert_eq!(member.id, "reviewer");
    assert_eq!(request.agent_type, SubAgentType::Review);
    assert_eq!(request.assignment.role.as_deref(), Some("reviewer"));
}

#[test]
fn test_apply_spawn_profile_scout_yields_explore_type_and_faster_route() {
    let roster = FleetRoster::built_ins_only();
    let mut request = parse_spawn_request(&json!({"prompt": "map the parser", "profile": "scout"}))
        .expect("parse");
    let member = apply_spawn_profile(&mut request, &roster)
        .expect("scout should resolve")
        .expect("member resolved");
    assert_eq!(request.agent_type, SubAgentType::Explore);
    assert_eq!(
        spawn_model_route(&request, Some(&member)),
        ModelRoute::Faster,
        "scout's fast loadout routes to the faster sibling"
    );
}

#[test]
fn test_apply_spawn_profile_synthesizer_yields_plan_type() {
    let roster = FleetRoster::built_ins_only();
    let mut request =
        parse_spawn_request(&json!({"prompt": "merge findings", "profile": "synthesizer"}))
            .expect("parse");
    apply_spawn_profile(&mut request, &roster).expect("synthesizer should resolve");
    assert_eq!(request.agent_type, SubAgentType::Plan);
}

#[test]
fn test_spawn_model_route_profile_precedence() {
    let mut profile = custom_fleet_profile("reviewer");
    profile.model = Some("deepseek-v4-pro".to_string());
    profile.loadout = codewhale_config::FleetLoadout::Fast;
    let roster = fleet_roster_with("auditor", profile);
    let member = roster.get("auditor").expect("member").clone();

    // 成员模型固定优先于装备。
    let request =
        parse_spawn_request(&json!({"prompt": "x", "profile": "auditor"})).expect("parse");
    assert_eq!(
        spawn_model_route(&request, Some(&member)),
        ModelRoute::Fixed("deepseek-v4-pro".to_string())
    );

    // 显式 model_strength 优先于成员模型固定。
    let request = parse_spawn_request(&json!({
        "prompt": "x",
        "profile": "auditor",
        "model_strength": "same"
    }))
    .expect("parse");
    assert_eq!(
        spawn_model_route(&request, Some(&member)),
        ModelRoute::Inherit
    );

    // 显式模型优先于成员模型固定：请求的路由让位，
    // 配置模型路径修正显式 ID。
    let request = parse_spawn_request(&json!({
        "prompt": "x",
        "profile": "auditor",
        "model": "deepseek-v4-flash"
    }))
    .expect("parse");
    let requested_route = spawn_model_route(&request, Some(&member));
    assert_eq!(
        assignment_model_route(Some("deepseek-v4-flash"), requested_route),
        ModelRoute::Fixed("deepseek-v4-flash".to_string())
    );

    // 无模型固定时，装备决定：fast -> Faster，其他装备继承
    // 而非自动降级为廉价兄弟。
    let mut fast = custom_fleet_profile("scout");
    fast.loadout = codewhale_config::FleetLoadout::Fast;
    let roster = fleet_roster_with("recon", fast);
    let request = parse_spawn_request(&json!({"prompt": "x", "profile": "recon"})).expect("parse");
    assert_eq!(
        spawn_model_route(&request, roster.get("recon")),
        ModelRoute::Faster
    );

    let mut strong = custom_fleet_profile("builder");
    strong.loadout = codewhale_config::FleetLoadout::Custom("strong".to_string());
    let roster = fleet_roster_with("architect", strong);
    assert_eq!(
        spawn_model_route(&request, roster.get("architect")),
        ModelRoute::Inherit
    );
}

#[test]
fn test_child_max_spawn_depth_profile_hint_only_narrows() {
    // 档案提示缩小继承的预算...
    assert_eq!(child_max_spawn_depth_for_spawn(3, 1, None, Some(1)), 2);
    // ...但从不扩大它。
    assert_eq!(child_max_spawn_depth_for_spawn(2, 0, None, Some(6)), 2);
    // 显式请求与提示取最小值。
    assert_eq!(child_max_spawn_depth_for_spawn(2, 0, Some(3), Some(1)), 1);
    // 仅显式请求保留其现有的扩大到上限的语义。
    assert_eq!(child_max_spawn_depth_for_spawn(2, 0, Some(3), None), 3);
    assert_eq!(
        child_max_spawn_depth_for_spawn(
            2,
            0,
            Some(codewhale_config::MAX_SPAWN_DEPTH_CEILING),
            None
        ),
        codewhale_config::MAX_SPAWN_DEPTH_CEILING
    );
    // 既无请求也无提示：继承不变。
    assert_eq!(child_max_spawn_depth_for_spawn(5, 2, None, None), 5);
}

#[test]
fn test_apply_spawn_profile_depth_hint_flows_from_member() {
    let mut profile = custom_fleet_profile("scout");
    profile.delegation.max_spawn_depth = Some(1);
    let roster = fleet_roster_with("recon", profile);
    let mut request =
        parse_spawn_request(&json!({"prompt": "x", "profile": "recon", "max_depth": 3}))
            .expect("parse");
    let member = apply_spawn_profile(&mut request, &roster)
        .expect("resolve")
        .expect("member resolved");
    let effective = child_max_spawn_depth_for_spawn(
        DEFAULT_MAX_SPAWN_DEPTH,
        1,
        request.max_depth,
        member.profile.delegation.max_spawn_depth,
    );
    assert_eq!(
        effective, 2,
        "hint 1 caps the requested 3 at spawn_depth 1 + 1"
    );
}

#[test]
fn test_apply_spawn_profile_appends_instruction_overlay() {
    let mut profile = custom_fleet_profile("reviewer");
    profile.role.description = Some("Security-focused reviewer.".to_string());
    profile.role.instructions = Some("Check unsafe blocks first.".to_string());
    let roster = fleet_roster_with("auditor", profile);
    let mut request =
        parse_spawn_request(&json!({"prompt": "audit the crate", "profile": "auditor"}))
            .expect("parse");
    apply_spawn_profile(&mut request, &roster).expect("resolve");
    assert!(
        request.prompt.starts_with("audit the crate"),
        "{}",
        request.prompt
    );
    assert!(
        request.prompt.contains("Fleet profile: auditor"),
        "{}",
        request.prompt
    );
    assert!(
        request
            .prompt
            .contains("Profile description:\nSecurity-focused reviewer."),
        "{}",
        request.prompt
    );
    assert!(
        request
            .prompt
            .contains("Profile instructions:\nCheck unsafe blocks first."),
        "{}",
        request.prompt
    );
    // 账目标识保留原始任务；覆盖内容仅限提示。
    assert_eq!(request.assignment.objective, "audit the crate");
}

#[tokio::test]
async fn session_projection_exposes_forked_prefix_cache_contract() {
    let mut snapshot = make_snapshot(SubAgentStatus::Running);
    snapshot.name = "fanout_review".to_string();
    snapshot.context_mode = "forked".to_string();
    snapshot.fork_context = true;

    let ctx = ToolContext::new(".");
    let projection = subagent_session_projection(snapshot, false, &ctx, None).await;

    assert_eq!(projection.name, "fanout_review");
    assert_eq!(projection.context_mode, "forked");
    assert_eq!(projection.run_id, "agent_test");
    assert_eq!(projection.follow_up.tool, "handle_read");
    assert_eq!(projection.follow_up.agent_id, "agent_test");
    assert!(projection.takeover.supported);
    assert_eq!(projection.usage.status, "unknown");
    assert_eq!(projection.verification.status, "self_report_only");
    assert!(projection.fork_context);
    assert_eq!(projection.prefix_cache.mode, "forked");
    assert_eq!(
        projection.prefix_cache.parent_prefix,
        "preserved_byte_identical_when_available"
    );
    assert_eq!(projection.transcript_handle.kind, "var_handle");
    assert_eq!(projection.transcript_handle.name, "transcript");
}

#[tokio::test]
async fn terminal_session_projection_prefers_full_transcript_handle() {
    let mut snapshot = make_snapshot(SubAgentStatus::Completed);
    snapshot.result = Some("done".to_string());

    let ctx = ToolContext::new(".");
    let full_handle = {
        let mut store = ctx.runtime.handle_store.lock().await;
        store.insert_json(
            "agent:agent_test",
            "full_transcript",
            json!({
                "kind": "subagent_full_transcript",
                "agent_id": "agent_test",
                "messages": [
                    {
                        "role": "assistant",
                        "content": [
                            { "type": "text", "text": "complete child output" }
                        ]
                    }
                ]
            }),
        )
    };

    let projection = subagent_session_projection(snapshot, false, &ctx, None).await;

    assert_eq!(projection.transcript_handle, full_handle);
    assert_eq!(projection.transcript_handle.name, "full_transcript");
}

#[tokio::test]
async fn interrupted_projection_exposes_checkpoint_metadata_and_messages() {
    let mut snapshot = make_snapshot(SubAgentStatus::Interrupted(
        "API call timed out after 10ms".to_string(),
    ));
    let checkpoint = make_checkpoint(
        &snapshot.agent_id,
        1,
        vec![text_message("user", "inspect checkpoint recovery")],
    );
    snapshot.steps_taken = checkpoint.steps_taken;
    snapshot.checkpoint = Some(checkpoint.clone());

    let ctx = ToolContext::new(".");
    let projection = subagent_session_projection(snapshot, false, &ctx, None).await;

    assert_eq!(projection.status, "waiting_for_user");
    assert!(projection.terminal);
    assert!(projection.continuable);
    assert!(projection.needs_continuation);
    assert!(!projection.timed_out_with_checkpoint);
    assert_eq!(
        projection
            .checkpoint
            .as_ref()
            .expect("checkpoint projected")
            .continuation_handle,
        checkpoint.continuation_handle
    );
    assert_eq!(
        projection
            .snapshot
            .checkpoint
            .as_ref()
            .map(|cp| cp.message_count),
        Some(1)
    );
    assert_eq!(
        projection
            .checkpoint
            .as_ref()
            .and_then(|cp| cp.messages.first())
            .map(message_text),
        Some("inspect checkpoint recovery")
    );

    let timed_out_projection =
        subagent_session_projection(projection.snapshot.clone(), true, &ctx, None).await;
    assert!(timed_out_projection.needs_continuation);
    assert!(timed_out_projection.timed_out);
    assert!(timed_out_projection.timed_out_with_checkpoint);
}

#[test]
fn test_delegate_defaults_to_fork_context() {
    let input = with_default_fork_context(json!({ "prompt": "review current work" }), true);
    let parsed = parse_spawn_request(&input).expect("delegate request should parse");
    assert!(parsed.fork_context);

    let input = with_default_fork_context(
        json!({ "prompt": "fresh exploration", "fork_context": false }),
        true,
    );
    let parsed = parse_spawn_request(&input).expect("delegate override should parse");
    assert!(!parsed.fork_context);
}

#[test]
fn spawn_request_parses_token_budget_override() {
    let parsed = parse_spawn_request(&json!({
        "prompt": "fan out safely",
        "token_budget": 12_345
    }))
    .expect("token budget parses");
    assert_eq!(parsed.token_budget, Some(12_345));

    let parsed = parse_spawn_request(&json!({
        "prompt": "fleet-shaped alias",
        "max_tokens": 4_000
    }))
    .expect("max_tokens alias parses");
    assert_eq!(parsed.token_budget, Some(4_000));

    let err = parse_spawn_request(&json!({
        "prompt": "bad budget",
        "token_budget": 0
    }))
    .expect_err("zero budget is invalid in tool input");
    assert!(
        err.to_string().contains("must be greater than zero"),
        "clear token budget error: {err}"
    );
}

#[test]
fn forked_subagent_messages_preserve_parent_prefix_then_append_task() {
    let parent_system = SystemPrompt::Text("parent system".to_string());
    let parent_message = Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "parent turn".to_string(),
            cache_control: None,
        }],
    };
    let fork_context = SubAgentForkContext {
        system: Some(parent_system.clone()),
        messages: vec![parent_message.clone()],
        structured_state_block: Some("## Fork State\n- Mode: `AGENT`".to_string()),
    };

    let assignment = SubAgentAssignment::new("inspect parser".to_string(), Some("worker".into()));
    let messages = build_initial_subagent_messages(
        "inspect parser",
        &assignment,
        &SubAgentType::General,
        Some(&fork_context),
    );

    assert_eq!(
        subagent_request_system_prompt("child system", Some(&fork_context)),
        parent_system
    );
    assert_eq!(messages.first(), Some(&parent_message));
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[1].role, "system");
    assert!(message_text(&messages[1]).contains("<codewhale:fork_state>"));
    assert_eq!(messages[2].role, "system");
    assert!(message_text(&messages[2]).contains("<codewhale:subagent_context>"));
    assert_eq!(messages[3].role, "user");
    assert!(message_text(&messages[3]).contains("inspect parser"));
}

#[test]
fn fresh_subagent_messages_keep_existing_single_turn_shape() {
    let assignment = SubAgentAssignment::new("list files".to_string(), None);
    let messages =
        build_initial_subagent_messages("list files", &assignment, &SubAgentType::Explore, None);

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "user");
    assert!(message_text(&messages[0]).contains("list files"));
}

#[test]
fn test_parse_spawn_request_rejects_text_and_items_together() {
    let input = json!({
        "prompt": "Analyze module",
        "items": [{"type": "text", "text": "dup"}]
    });
    let err = parse_spawn_request(&input).expect_err("text+items should fail");
    assert!(err.to_string().contains("either prompt text or items"));
}

#[test]
fn test_parse_spawn_request_rejects_invalid_role() {
    let input = json!({
        "prompt": "do work",
        "role": "unknown role"
    });
    let err = parse_spawn_request(&input).expect_err("invalid role should fail");
    assert!(
        err.to_string()
            .contains("role must be a bare roster member id"),
        "{err}"
    );
}

#[test]
fn test_parse_spawn_request_accepts_fleet_role_token_for_runtime_resolution() {
    let input = json!({
        "prompt": "do work",
        "role": "release_lead"
    });
    let parsed = parse_spawn_request(&input).expect("fleet role token should parse");
    assert_eq!(parsed.agent_type, SubAgentType::General);
    assert!(!parsed.agent_type_explicit);
    assert_eq!(parsed.assignment.role.as_deref(), Some("release_lead"));
    assert_eq!(parsed.profile.as_deref(), Some("release_lead"));
}

#[test]
fn test_parse_spawn_request_accepts_full_role_vocabulary() {
    // #2649 回归：SubAgentType::from_str 接受的角色也必须
    // 通过第二个 normalize_role_alias 验证，而不是被过时的提示拒绝。
    //
    for (role, expected_type, expected_role) in [
        ("general", SubAgentType::General, "worker"),
        ("general-purpose", SubAgentType::General, "worker"),
        ("general_purpose", SubAgentType::General, "worker"),
        ("worker", SubAgentType::General, "worker"),
        ("default", SubAgentType::General, "default"),
        ("explore", SubAgentType::Explore, "explorer"),
        ("exploration", SubAgentType::Explore, "explorer"),
        ("explorer", SubAgentType::Explore, "explorer"),
        ("plan", SubAgentType::Plan, "awaiter"),
        ("planning", SubAgentType::Plan, "awaiter"),
        ("planner", SubAgentType::Plan, "awaiter"),
        ("awaiter", SubAgentType::Plan, "awaiter"),
        ("review", SubAgentType::Review, "reviewer"),
        ("code-review", SubAgentType::Review, "reviewer"),
        ("code_review", SubAgentType::Review, "reviewer"),
        ("reviewer", SubAgentType::Review, "reviewer"),
        ("implementer", SubAgentType::Implementer, "implementer"),
        ("implement", SubAgentType::Implementer, "implementer"),
        ("implementation", SubAgentType::Implementer, "implementer"),
        ("builder", SubAgentType::Implementer, "implementer"),
        ("verifier", SubAgentType::Verifier, "verifier"),
        ("verify", SubAgentType::Verifier, "verifier"),
        ("verification", SubAgentType::Verifier, "verifier"),
        ("validator", SubAgentType::Verifier, "verifier"),
        ("tester", SubAgentType::Verifier, "verifier"),
        ("custom", SubAgentType::Custom, "custom"),
    ] {
        assert_eq!(
            SubAgentType::from_str(role),
            Some(expected_type.clone()),
            "from_str should accept role alias {role:?}"
        );
        assert_eq!(
            normalize_role_alias(role),
            Some(expected_role),
            "normalize_role_alias should accept role alias {role:?}"
        );

        let input = json!({ "prompt": "do work", "role": role });
        let parsed = parse_spawn_request(&input)
            .unwrap_or_else(|e| panic!("role {role:?} should parse, got {e}"));
        assert_eq!(parsed.agent_type, expected_type, "type for role {role:?}");
        assert_eq!(
            parsed.assignment.role.as_deref(),
            Some(expected_role),
            "canonical role for {role:?}"
        );
    }
}

#[test]
fn test_invalid_role_error_lists_real_aliases() {
    // 格式良好的舰队角色令牌解析后在名册解析时明确失败，
    // 同时提示真实名册成员和类型别名（#4177）。
    let roster = FleetRoster::built_ins_only();
    let input = json!({ "prompt": "do work", "role": "nonsense" });
    let mut request = parse_spawn_request(&input).expect("fleet role token should parse");
    let err = apply_spawn_profile(&mut request, &roster)
        .expect_err("unknown fleet role should fail at runtime resolution")
        .to_string();
    assert!(
        err.contains("Unknown fleet role/profile 'nonsense'"),
        "{err}"
    );
    assert!(err.contains("scout"), "hint should list scout: {err}");
    assert!(err.contains("reviewer"), "hint should list reviewer: {err}");
    assert!(err.contains("verifier"), "hint should list verifier: {err}");
    assert!(err.contains("custom"), "hint should list custom: {err}");
    assert!(
        err.contains("general-purpose"),
        "hint should list general-purpose: {err}"
    );
    assert!(
        err.contains("code_review"),
        "hint should list code_review: {err}"
    );
}

fn schema_property_description<'a>(schema: &'a Value, property: &str) -> &'a str {
    schema["properties"][property]["description"]
        .as_str()
        .unwrap_or_else(|| panic!("missing description for schema property {property:?}"))
}

#[test]
fn subagent_tool_schemas_advertise_real_type_and_role_vocabulary() {
    let tmp = tempdir().expect("tempdir");
    let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 1);
    let agent_schema = AgentTool::new(manager, stub_runtime()).input_schema();

    let description = schema_property_description(&agent_schema, "type");
    for alias in [
        "general",
        "explore",
        "plan",
        "review",
        "implementer",
        "verifier",
        "custom",
    ] {
        assert!(
            description.contains(alias),
            "type description should list accepted type {alias:?}: {description}"
        );
    }
    assert!(agent_schema["properties"].get("role").is_none());
    assert!(agent_schema["properties"].get("max_depth").is_some());
    let model_strength = schema_property_description(&agent_schema, "model_strength");
    assert!(
        model_strength.contains("type=explore") && model_strength.contains("faster"),
        "model_strength description should teach explore/faster routing: {model_strength}"
    );
    let thinking = schema_property_description(&agent_schema, "thinking");
    assert!(
        thinking.contains("inherit") && thinking.contains("model_strength=faster"),
        "thinking description should teach child thinking control: {thinking}"
    );
    assert!(agent_schema["properties"].get("model").is_some());
    let worktree = schema_property_description(&agent_schema, "worktree");
    assert!(
        worktree.contains("git worktree") && worktree.contains("parallel edit"),
        "worktree description should teach isolated parallel edits: {worktree}"
    );
    assert!(agent_schema["properties"].get("worktree_branch").is_some());
    assert!(agent_schema["properties"].get("worktree_path").is_some());
}

#[test]
fn agent_tool_prompt_schema_prefers_structured_briefs() {
    let tmp = tempdir().expect("tempdir");
    let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 1);
    let agent_schema = AgentTool::new(manager, stub_runtime()).input_schema();
    let prompt = schema_property_description(&agent_schema, "prompt");
    assert!(prompt.contains("Subagent Brief"));
    assert!(prompt.contains("QUESTION"));
    assert!(prompt.contains("STOP_CONDITION"));
    assert!(prompt.contains("ALREADY_KNOWN"));
}

#[test]
fn agent_tool_schema_advertises_status_peek_cancel_actions() {
    let tmp = tempdir().expect("tempdir");
    let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 1);
    let agent_schema = AgentTool::new(manager, stub_runtime()).input_schema();

    let action = schema_property_description(&agent_schema, "action");
    assert!(action.contains("status"));
    assert!(action.contains("peek"));
    assert!(action.contains("cancel"));
    assert!(agent_schema["properties"].get("agent_id").is_some());
}

#[tokio::test]
async fn agent_tool_status_returns_running_child_projection() {
    let tmp = tempdir().expect("tempdir");
    let manager = Arc::new(RwLock::new(SubAgentManager::new(
        tmp.path().to_path_buf(),
        2,
    )));
    let agent_id = "agent_status_probe".to_string();
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "probe".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx,
        tmp.path().to_path_buf(),
        manager.read().await.current_session_boot_id.clone(),
    );
    agent.status = SubAgentStatus::Running;
    {
        let mut manager_guard = manager.write().await;
        manager_guard.agents.insert(agent_id.clone(), agent);
        manager_guard.register_worker(make_worker_spec(&agent_id, tmp.path().to_path_buf()));
        manager_guard
            .record_worker_progress(&agent_id, "step 1: requesting model response".to_string());
    }

    let tool = AgentTool::new(Arc::clone(&manager), stub_runtime());
    let context = ToolContext::new(tmp.path());
    let result = tool
        .execute(json!({"action": "status", "agent_id": agent_id}), &context)
        .await
        .expect("status action succeeds");

    assert_eq!(result.metadata.as_ref().unwrap()["action"], json!("status"));
    assert!(result.content.contains("agent_status_probe"));
    assert!(result.content.contains("running"));
    assert!(result.content.contains("transcript_handle"));
}

#[tokio::test]
async fn agent_tool_status_reconciles_stale_single_agent_projection() {
    let tmp = tempdir().expect("tempdir");
    let inner = SubAgentManager::new(tmp.path().to_path_buf(), 2)
        .with_running_heartbeat_timeout(Duration::from_secs(30));
    let current_boot = inner.session_boot_id().to_string();
    let manager = Arc::new(RwLock::new(inner));
    let agent_id = "agent_stale_single_status".to_string();
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "probe stale single status".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx,
        tmp.path().to_path_buf(),
        current_boot,
    );
    agent.status = SubAgentStatus::Running;
    agent.last_activity_at = Instant::now() - Duration::from_secs(31);
    agent.task_handle = Some(tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }));
    {
        let mut manager_guard = manager.write().await;
        manager_guard.agents.insert(agent_id.clone(), agent);
        manager_guard.register_worker(make_worker_spec(&agent_id, tmp.path().to_path_buf()));
    }

    let tool = AgentTool::new(Arc::clone(&manager), stub_runtime());
    let context = ToolContext::new(tmp.path());
    let result = tool
        .execute(json!({"action": "status", "agent_id": agent_id}), &context)
        .await
        .expect("status action succeeds");

    let metadata = result.metadata.as_ref().expect("status metadata");
    assert_eq!(metadata["action"], json!("status"));
    assert_eq!(metadata["status"], json!("cancelled"));
    assert_eq!(metadata["terminal"], json!(true));
    assert_eq!(metadata["agent_id"], json!("agent_stale_single_status"));
    assert!(result.content.contains("agent_stale_single_status"));
    assert!(result.content.contains("cancelled"));
    assert!(result.content.contains("Auto-cancelled"));
    assert_eq!(manager.read().await.running_count(), 0);
}

#[tokio::test]
async fn agent_tool_cancel_stops_running_child() {
    let tmp = tempdir().expect("tempdir");
    let manager = Arc::new(RwLock::new(SubAgentManager::new(
        tmp.path().to_path_buf(),
        2,
    )));
    let agent_id = "agent_cancel_probe".to_string();
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "cancel".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx,
        tmp.path().to_path_buf(),
        manager.read().await.current_session_boot_id.clone(),
    );
    agent.status = SubAgentStatus::Running;
    {
        let mut manager_guard = manager.write().await;
        manager_guard.agents.insert(agent_id.clone(), agent);
        manager_guard.register_worker(make_worker_spec(&agent_id, tmp.path().to_path_buf()));
    }

    let tool = AgentTool::new(Arc::clone(&manager), stub_runtime());
    let context = ToolContext::new(tmp.path());
    let result = tool
        .execute(json!({"action": "cancel", "agent_id": agent_id}), &context)
        .await
        .expect("cancel action succeeds");

    assert_eq!(result.metadata.as_ref().unwrap()["action"], json!("cancel"));
    assert!(result.content.contains("cancelled"));
    let snapshot = manager
        .read()
        .await
        .get_result("agent_cancel_probe")
        .expect("agent remains listed");
    assert_eq!(snapshot.status, SubAgentStatus::Cancelled);
}

#[test]
fn test_parse_spawn_request_rejects_conflicting_type_and_role() {
    let input = json!({
        "prompt": "inspect internals",
        "type": "explore",
        "role": "worker"
    });
    let err = parse_spawn_request(&input).expect_err("conflicting type+role should fail");
    assert!(
        err.to_string()
            .contains("Conflicting type/agent_type and role/agent_role")
    );
}

#[test]
fn test_build_allowed_tools_independent_of_allow_shell() {
    // v0.6.6：allow_shell 不再在 build_allowed_tools 层级过滤——
    // 注册表构建器控制 shell 工具注册。
    // 对于默认的 General 代理，
    // 两次调用均返回 None（完全继承）。
    let with_shell = build_allowed_tools(&SubAgentType::General, None, true).unwrap();
    let without_shell = build_allowed_tools(&SubAgentType::General, None, false).unwrap();
    assert!(with_shell.is_none());
    assert!(without_shell.is_none());
}

#[test]
fn test_allowed_tools_are_deduplicated() {
    let tools = build_allowed_tools(
        &SubAgentType::Custom,
        Some(vec![
            "read_file".to_string(),
            "read_file".to_string(),
            "  ".to_string(),
            "grep_files".to_string(),
        ]),
        true,
    )
    .unwrap();
    assert_eq!(
        tools,
        Some(vec!["read_file".to_string(), "grep_files".to_string()])
    );
}

#[test]
fn test_custom_agent_requires_allowed_tools() {
    let err = build_allowed_tools(&SubAgentType::Custom, None, true).unwrap_err();
    assert!(err.to_string().contains("requires"));
}

#[test]
fn role_posture_blocks_writes_and_shell_for_read_only_roles() {
    // #3217：只读角色绝不可运行 write/edit/patch 工具，
    // 无论父级自动批准与否，但始终可以读取。
    for role in [
        SubAgentType::Explore,
        SubAgentType::Review,
        SubAgentType::Plan,
        SubAgentType::Verifier,
    ] {
        assert!(
            !role_posture_permits(&role, ApprovalRequirement::Suggest),
            "{role:?} must not run write/edit/patch tools"
        );
        assert!(
            role_posture_permits(&role, ApprovalRequirement::Auto),
            "{role:?} can still read"
        );
    }

    // 可写角色保留写入权限。
    for role in [SubAgentType::Implementer, SubAgentType::General] {
        assert!(
            role_posture_permits(&role, ApprovalRequirement::Suggest),
            "{role:?} writes"
        );
    }

    // 仅 Full-shell 角色可运行 shell（Required）工具。
    for role in [
        SubAgentType::Verifier,
        SubAgentType::Implementer,
        SubAgentType::General,
    ] {
        assert!(
            role_posture_permits(&role, ApprovalRequirement::Required),
            "{role:?} has full shell"
        );
    }
    for role in [
        SubAgentType::Plan,
        SubAgentType::Explore,
        SubAgentType::Review,
    ] {
        assert!(
            !role_posture_permits(&role, ApprovalRequirement::Required),
            "{role:?} must not run shell tools"
        );
    }

    // Custom 由其显式的 allowed_tools 列表管理，因此姿态检查允许它
    //（允许列表是该角色的权威来源）。
    assert!(role_posture_permits(
        &SubAgentType::Custom,
        ApprovalRequirement::Suggest
    ));
    assert!(role_posture_permits(
        &SubAgentType::Custom,
        ApprovalRequirement::Required
    ));
}

#[test]
fn test_build_assignment_prompt_includes_metadata() {
    let assignment = SubAgentAssignment::new(
        "Inspect parser behavior".to_string(),
        Some("explorer".to_string()),
    );
    let prompt = build_assignment_prompt(
        "Inspect parser behavior",
        &assignment,
        &SubAgentType::Explore,
    );
    assert!(prompt.contains("Assignment metadata"));
    assert!(prompt.contains("resolved_type: explore"));
    assert!(prompt.contains("role: explorer"));
}

#[test]
fn subagent_model_strength_defaults_to_parent_even_when_parent_auto_model() {
    let mut runtime = stub_runtime().with_auto_model(true);
    runtime.model = "deepseek-v4-pro".to_string();

    for prompt in ["implement the release fix", "say hello"] {
        let route = fallback_subagent_assignment_route(
            &runtime,
            None,
            ModelRoute::Inherit,
            SubAgentThinking::Inherit,
            prompt,
        );
        assert_eq!(route.model_route, ModelRoute::Inherit);
        assert_eq!(route.model, "deepseek-v4-pro", "prompt {prompt:?}");
    }
}

#[test]
fn subagent_model_strength_faster_uses_known_family_sibling() {
    let mut runtime = stub_runtime().with_auto_model(true);
    runtime.model = "deepseek-v4-pro".to_string();

    let route = fallback_subagent_assignment_route(
        &runtime,
        None,
        ModelRoute::Faster,
        SubAgentThinking::Inherit,
        "inspect one file",
    );
    assert_eq!(route.model_route, ModelRoute::Faster);
    assert_eq!(route.model, "deepseek-v4-flash");
    assert_eq!(route.reasoning_effort.as_deref(), Some("off"));
}

#[test]
fn subagent_model_strength_explicit_model_wins_over_faster() {
    let runtime = stub_runtime().with_auto_model(true);

    let route = fallback_subagent_assignment_route(
        &runtime,
        Some("deepseek-v4-pro".to_string()),
        ModelRoute::Faster,
        SubAgentThinking::Inherit,
        "inspect one file",
    );
    assert_eq!(
        route.model_route,
        ModelRoute::Fixed("deepseek-v4-pro".to_string())
    );
    assert_eq!(route.model, "deepseek-v4-pro");
}

#[test]
fn explicit_child_thinking_overrides_faster_default_off() {
    let mut runtime = stub_runtime().with_reasoning_effort(Some("max".to_string()), false);
    runtime.model = "deepseek-v4-pro".to_string();

    let route = fallback_subagent_assignment_route(
        &runtime,
        None,
        ModelRoute::Faster,
        SubAgentThinking::Effort(ReasoningEffort::High),
        "inspect one file",
    );
    assert_eq!(route.model, "deepseek-v4-flash");
    assert_eq!(route.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(route.tuning.reasoning_effort, Some(ReasoningEffort::High));
}

#[test]
fn explicit_child_auto_thinking_resolves_from_child_prompt() {
    let runtime = stub_runtime().with_reasoning_effort(Some("off".to_string()), false);

    let route = fallback_subagent_assignment_route(
        &runtime,
        None,
        ModelRoute::Inherit,
        SubAgentThinking::Auto,
        "debug this release failure",
    );
    assert_eq!(route.reasoning_effort.as_deref(), Some("max"));
}

#[tokio::test]
async fn route_resolution_matrix_uses_explicit_model_strength_routes() {
    let mut runtime = stub_runtime()
        .with_auto_model(false)
        .with_reasoning_effort(Some("max".to_string()), false);
    runtime.model = "deepseek-v4-pro".to_string();

    struct RouteCase {
        agent_type: SubAgentType,
        configured_model: Option<&'static str>,
        requested_route: ModelRoute,
        prompt: &'static str,
        expected_route: ModelRoute,
        expected_model: &'static str,
        expected_reasoning: Option<&'static str>,
        expected_tuning_effort: Option<ReasoningEffort>,
    }

    let cases = vec![
        RouteCase {
            agent_type: SubAgentType::Explore,
            configured_model: None,
            requested_route: ModelRoute::Inherit,
            prompt: "inspect the parser and report what changed",
            expected_route: ModelRoute::Inherit,
            expected_model: "deepseek-v4-pro",
            expected_reasoning: Some("max"),
            expected_tuning_effort: Some(ReasoningEffort::Max),
        },
        RouteCase {
            agent_type: SubAgentType::Explore,
            configured_model: None,
            requested_route: ModelRoute::Faster,
            prompt: "inspect the parser and report what changed",
            expected_route: ModelRoute::Faster,
            expected_model: "deepseek-v4-flash",
            expected_reasoning: Some("off"),
            expected_tuning_effort: Some(ReasoningEffort::Off),
        },
        RouteCase {
            agent_type: SubAgentType::General,
            configured_model: None,
            requested_route: ModelRoute::Inherit,
            prompt: "synthesize the release blocker fix",
            expected_route: ModelRoute::Inherit,
            expected_model: "deepseek-v4-pro",
            expected_reasoning: Some("max"),
            expected_tuning_effort: Some(ReasoningEffort::Max),
        },
        RouteCase {
            agent_type: SubAgentType::Implementer,
            configured_model: Some("deepseek-v4-flash"),
            requested_route: ModelRoute::Inherit,
            prompt: "apply the narrow code edit",
            expected_route: ModelRoute::Fixed("deepseek-v4-flash".to_string()),
            expected_model: "deepseek-v4-flash",
            expected_reasoning: Some("max"),
            expected_tuning_effort: Some(ReasoningEffort::Max),
        },
    ];

    for case in cases {
        let route = resolve_subagent_assignment_route(
            &runtime,
            case.configured_model.map(str::to_string),
            case.prompt,
            &case.agent_type,
            case.requested_route.clone(),
            SubAgentThinking::Inherit,
        )
        .await;
        assert_eq!(
            route.model_route, case.expected_route,
            "{:?}",
            case.agent_type
        );
        assert_eq!(route.model, case.expected_model, "{:?}", case.agent_type);
        assert_eq!(
            route.reasoning_effort.as_deref(),
            case.expected_reasoning,
            "{:?}",
            case.agent_type
        );
        assert_eq!(
            route.tuning.reasoning_effort, case.expected_tuning_effort,
            "{:?}",
            case.agent_type
        );
        assert_eq!(
            route.tuning.max_output_tokens,
            Some(SUBAGENT_RESPONSE_MAX_TOKENS),
            "{:?}",
            case.agent_type
        );
    }
}

#[test]
fn subagent_auto_reasoning_resolves_to_distinct_v4_tiers() {
    let runtime = stub_runtime().with_reasoning_effort(Some("high".to_string()), true);

    assert_eq!(
        fallback_subagent_assignment_route(
            &runtime,
            None,
            ModelRoute::Inherit,
            SubAgentThinking::Inherit,
            "quick lookup",
        )
        .reasoning_effort,
        Some("high".to_string())
    );
    assert_eq!(
        fallback_subagent_assignment_route(
            &runtime,
            None,
            ModelRoute::Inherit,
            SubAgentThinking::Inherit,
            "debug this release failure"
        )
        .reasoning_effort,
        Some("max".to_string())
    );
}

#[test]
fn test_subagent_tool_registry_reports_unavailable_tools() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    runtime.allow_shell = false;
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::Explore,
        Some(vec!["read_file".to_string(), "missing_tool".to_string()]),
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );
    assert_eq!(
        registry.unavailable_allowed_tools(),
        vec!["missing_tool".to_string()]
    );
}

#[test]
fn test_subagent_tools_respect_nested_agent_depth_budget() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    runtime.spawn_depth = 1;
    runtime.max_spawn_depth = 2;
    let registry = SubAgentToolRegistry::new(
        runtime.clone(),
        SubAgentType::Explore,
        None,
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );
    let tools = registry.tools_for_model(&SubAgentType::Explore);
    let names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(
        names.contains(&"agent"),
        "child should keep the single agent launcher while depth budget remains; tools: {names:?}"
    );
    assert!(registry.is_tool_allowed("agent"));

    runtime.spawn_depth = 2;
    let capped = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::Explore,
        None,
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );
    let capped_tools = capped.tools_for_model(&SubAgentType::Explore);
    let capped_names: Vec<_> = capped_tools.iter().map(|t| t.name.as_str()).collect();
    assert!(
        !capped_names.contains(&"agent"),
        "child should lose agent launcher at configured depth cap; tools: {capped_names:?}"
    );
    assert!(!capped.is_tool_allowed("agent"));
}

fn tool_names(tools: Vec<Tool>) -> HashSet<String> {
    tools.into_iter().map(|tool| tool.name).collect()
}

fn enabled_agent_surface_options() -> AgentToolSurfaceOptions {
    let mut options = AgentToolSurfaceOptions::new(ShellPolicy::Full);
    options.apply_patch_enabled = true;
    options.web_search_enabled = true;
    options.memory_tool_enabled = true;
    options.goal_state = Some(crate::tools::goal::new_shared_goal_state());
    options
}

fn disabled_feature_agent_surface_options() -> AgentToolSurfaceOptions {
    let mut options = AgentToolSurfaceOptions::new(ShellPolicy::Full);
    options.goal_state = Some(crate::tools::goal::new_shared_goal_state());
    options
}

#[test]
fn subagent_general_catalog_matches_parent_agent_surface_when_features_enabled() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime =
        stub_runtime().with_agent_tool_surface_options(enabled_agent_surface_options());
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    let todo_list = crate::tools::todo::new_shared_todo_list();
    let plan_state = crate::tools::plan::new_shared_plan_state();

    let parent_registry = ToolRegistryBuilder::new()
        .with_full_agent_surface_options(
            Some(runtime.client.clone()),
            runtime.model.clone(),
            runtime.manager.clone(),
            runtime.clone(),
            runtime.agent_tool_surface_options.clone(),
            todo_list.clone(),
            plan_state.clone(),
        )
        .build(runtime.context.clone());
    let child_registry =
        SubAgentToolRegistry::new(runtime, SubAgentType::General, None, todo_list, plan_state);

    let parent_names = tool_names(parent_registry.to_api_tools());
    let child_names = tool_names(child_registry.tools_for_model(&SubAgentType::General));
    assert_eq!(
        child_names, parent_names,
        "default General sub-agent catalog must stay in parity with the parent Agent surface"
    );
}

#[test]
fn subagent_feature_gates_match_parent_agent_surface() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime =
        stub_runtime().with_agent_tool_surface_options(disabled_feature_agent_surface_options());
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    let todo_list = crate::tools::todo::new_shared_todo_list();
    let plan_state = crate::tools::plan::new_shared_plan_state();

    let parent_registry = ToolRegistryBuilder::new()
        .with_full_agent_surface_options(
            Some(runtime.client.clone()),
            runtime.model.clone(),
            runtime.manager.clone(),
            runtime.clone(),
            runtime.agent_tool_surface_options.clone(),
            todo_list.clone(),
            plan_state.clone(),
        )
        .build(runtime.context.clone());
    let child_registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::Implementer,
        None,
        todo_list,
        plan_state,
    );

    let parent_names = tool_names(parent_registry.to_api_tools());
    let child_names = tool_names(child_registry.tools_for_model(&SubAgentType::Implementer));
    for name in [
        "apply_patch",
        "web_search",
        "fetch_url",
        "web.run",
        "wait_for_dev_server",
        "remember",
    ] {
        assert!(
            !parent_names.contains(name),
            "{name} should be parent-gated"
        );
        assert!(!child_names.contains(name), "{name} should be child-gated");
    }
}

#[test]
fn explore_catalog_inherits_web_but_hides_write_shell_and_fim_tools() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime =
        stub_runtime().with_agent_tool_surface_options(enabled_agent_surface_options());
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    runtime.context.auto_approve = true;
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::Explore,
        None,
        crate::tools::todo::new_shared_todo_list(),
        crate::tools::plan::new_shared_plan_state(),
    );

    let names = tool_names(registry.tools_for_model(&SubAgentType::Explore));
    for name in ["web_search", "fetch_url", "web.run", "wait_for_dev_server"] {
        assert!(names.contains(name), "Explore should inherit {name}");
    }
    for name in [
        "write_file",
        "edit_file",
        "apply_patch",
        "fim_edit",
        "exec_shell",
        "task_shell_start",
    ] {
        assert!(!names.contains(name), "Explore must hide {name}");
    }
}

#[test]
fn implementer_catalog_inherits_patch_and_fim_when_enabled() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime =
        stub_runtime().with_agent_tool_surface_options(enabled_agent_surface_options());
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::Implementer,
        None,
        crate::tools::todo::new_shared_todo_list(),
        crate::tools::plan::new_shared_plan_state(),
    );

    let names = tool_names(registry.tools_for_model(&SubAgentType::Implementer));
    for name in ["apply_patch", "fim_edit", "write_file", "edit_file"] {
        assert!(
            names.contains(name),
            "Implementer should inherit write-capable tool {name}"
        );
    }
}

#[tokio::test]
async fn plan_parent_profile_narrows_even_implementer_child_to_read_only() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let mut runtime =
        stub_runtime().with_agent_tool_surface_options(enabled_agent_surface_options());
    runtime.context = ToolContext::new(workspace.clone());
    runtime.context.auto_approve = true;
    runtime.allow_shell = false;
    runtime.worker_profile = WorkerRuntimeProfile::for_role(SubAgentType::Plan);
    runtime.agent_tool_surface_options.shell_policy = ShellPolicy::None;

    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::Implementer,
        None,
        crate::tools::todo::new_shared_todo_list(),
        crate::tools::plan::new_shared_plan_state(),
    );

    let names = tool_names(registry.tools_for_model(&SubAgentType::Implementer));
    assert!(names.contains("agent"), "Plan children may still delegate");
    for name in [
        "write_file",
        "edit_file",
        "apply_patch",
        "fim_edit",
        "exec_shell",
        "task_shell_start",
    ] {
        assert!(
            !names.contains(name),
            "Plan parent profile must hide child capability {name}"
        );
    }

    let err = registry
        .execute(
            "agent_test",
            "write_file",
            json!({"path": "plan-parent-write.txt", "content": "denied"}),
        )
        .await
        .expect_err("Plan parent profile must block writes even for implementer children");
    assert!(
        err.to_string().contains("not permitted"),
        "expected posture rejection, got: {err}"
    );
    assert!(!workspace.join("plan-parent-write.txt").exists());
}

#[tokio::test]
async fn api_timeout_preserves_checkpoint_and_returns_needs_input_without_parking() {
    let tmp = tempdir().expect("tempdir");
    let manager = Arc::new(RwLock::new(SubAgentManager::new(
        tmp.path().to_path_buf(),
        2,
    )));
    let agent_id = "agent_checkpoint_timeout".to_string();
    let (task_input_tx, task_input_rx) = mpsc::unbounded_channel();
    let agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "Inspect checkpoint behavior".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec![]),
        task_input_tx,
        tmp.path().to_path_buf(),
        "boot_test".to_string(),
    );
    {
        let mut manager = manager.write().await;
        manager.agents.insert(agent_id.clone(), agent);
        manager.register_worker(make_worker_spec(&agent_id, tmp.path().to_path_buf()));
    }

    let (client, calls, _bodies) =
        delayed_chat_client(Duration::from_millis(80), "resumed answer").await;
    let mut runtime = stub_runtime().with_step_api_timeout(Duration::from_millis(50));
    runtime.client = client;
    runtime.manager = Arc::clone(&manager);
    runtime.context = ToolContext::new(tmp.path());
    let (mailbox, mut mailbox_rx) =
        crate::tools::subagent::mailbox::Mailbox::new(tokio_util::sync::CancellationToken::new());
    runtime.mailbox = Some(mailbox);

    let task = SubAgentTask {
        manager_handle: Arc::clone(&manager),
        runtime: runtime.clone(),
        agent_id: agent_id.clone(),
        agent_type: SubAgentType::General,
        prompt: "Inspect checkpoint behavior".to_string(),
        assignment: make_assignment(),
        allowed_tools: Some(vec![]),
        fork_context: false,
        started_at: Instant::now(),
        max_steps: 3,
        token_budget: None,
        input_rx: task_input_rx,
        launch_gate: None,
    };
    let task_handle = tokio::spawn(run_subagent_task(task));

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if calls.load(Ordering::SeqCst) >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("first timed-out API attempt should reach the test server");

    let interrupted_envelope = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            for env in mailbox_rx.drain() {
                if let MailboxMessage::Interrupted {
                    agent_id: id,
                    reason,
                } = env.message
                {
                    return (id, reason);
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("API timeout should publish an Interrupted mailbox lifecycle event");
    assert_eq!(interrupted_envelope.0, agent_id);
    assert!(
        interrupted_envelope.1.contains("API call timed out"),
        "reason should carry the timeout context: {}",
        interrupted_envelope.1
    );

    tokio::time::timeout(Duration::from_secs(5), task_handle)
        .await
        .expect("sub-agent task must not park waiting for checkpoint input")
        .expect("sub-agent task should finish");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "needs-input interruption must not park for continuation or issue a second API request"
    );

    let interrupted = {
        let manager = manager.read().await;
        manager
            .get_result(&agent_id)
            .expect("agent should stay registered")
    };
    assert!(matches!(interrupted.status, SubAgentStatus::Interrupted(_)));
    let checkpoint = interrupted
        .checkpoint
        .as_ref()
        .expect("timeout should preserve checkpoint");
    assert!(checkpoint.continuable);
    assert_eq!(checkpoint.steps_taken, 1);
    assert!(
        checkpoint
            .messages
            .iter()
            .any(|message| message_text(message).contains("Inspect checkpoint behavior")),
        "checkpoint should preserve local child prompt: {checkpoint:?}"
    );
    assert!(interrupted.needs_input.is_some());

    let ctx = runtime.context.clone();
    let worker_record = {
        let manager = manager.read().await;
        manager.get_worker_record(&agent_id)
    };
    let projection =
        subagent_session_projection(interrupted.clone(), false, &ctx, worker_record).await;
    assert_eq!(projection.status, "waiting_for_user");
    assert!(projection.continuable);
    assert!(projection.needs_continuation);
    assert!(projection.checkpoint.is_some());
    assert!(
        projection
            .needs_input
            .as_ref()
            .expect("needs_input should be projected")
            .question
            .contains("Re-dispatch this worker"),
        "projection should tell the parent how to wake/re-dispatch: {:?}",
        projection.needs_input
    );
    assert_eq!(
        projection
            .worker_record
            .as_ref()
            .expect("worker record")
            .status,
        AgentWorkerStatus::WaitingForUser
    );
    assert_eq!(
        projection
            .worker_record
            .as_ref()
            .expect("worker record")
            .recommended_action
            .action,
        "inspect_or_replace"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "projection inspection must not respawn the child implicitly"
    );
}

#[test]
fn transient_provider_classifier_matches_sse_header_timeout() {
    let err = anyhow::anyhow!("SSE stream request did not receive response headers after 45s");

    assert!(is_transient_subagent_provider_error(&err));
}

#[test]
fn transient_provider_classifier_matches_structured_rate_limit() {
    let err = anyhow::Error::new(crate::llm_client::LlmError::RateLimited {
        message: "please slow down".to_string(),
        retry_after: Some(Duration::from_secs(2)),
    })
    .context("Responses API request failed");

    assert!(is_transient_subagent_provider_error(&err));
}

#[tokio::test]
async fn subagent_retries_transient_provider_header_timeout_before_succeeding() {
    let tmp = tempdir().expect("tempdir");
    let manager = Arc::new(RwLock::new(SubAgentManager::new(
        tmp.path().to_path_buf(),
        2,
    )));
    let agent_id = "agent_transient_provider_retry".to_string();
    let (task_input_tx, task_input_rx) = mpsc::unbounded_channel();
    let agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "Inspect transient provider recovery".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec![]),
        task_input_tx,
        tmp.path().to_path_buf(),
        "boot_test".to_string(),
    );
    {
        let mut manager = manager.write().await;
        manager.agents.insert(agent_id.clone(), agent);
        manager.register_worker(make_worker_spec(&agent_id, tmp.path().to_path_buf()));
    }

    let (client, calls) =
        transient_header_timeout_then_success_chat_client("recovered answer").await;
    let mut runtime = stub_runtime().with_step_api_timeout(Duration::from_secs(5));
    runtime.client = client;
    runtime.manager = Arc::clone(&manager);
    runtime.context = ToolContext::new(tmp.path());

    let task = SubAgentTask {
        manager_handle: Arc::clone(&manager),
        runtime,
        agent_id: agent_id.clone(),
        agent_type: SubAgentType::General,
        prompt: "Inspect transient provider recovery".to_string(),
        assignment: make_assignment(),
        allowed_tools: Some(vec![]),
        fork_context: false,
        started_at: Instant::now(),
        max_steps: 3,
        token_budget: None,
        input_rx: task_input_rx,
        launch_gate: None,
    };

    tokio::time::timeout(
        Duration::from_secs(10),
        tokio::spawn(run_subagent_task(task)),
    )
    .await
    .expect("sub-agent task should finish")
    .expect("sub-agent join should succeed");

    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "one transient provider failure should be retried exactly once"
    );
    let snapshot = {
        let manager = manager.read().await;
        manager
            .get_result(&agent_id)
            .expect("agent should stay registered")
    };
    assert_eq!(snapshot.status, SubAgentStatus::Completed);
    assert_eq!(snapshot.result.as_deref(), Some("recovered answer"));
}

#[tokio::test]
async fn subagent_rate_limit_exhaustion_interrupts_with_checkpoint() {
    let tmp = tempdir().expect("tempdir");
    let manager = Arc::new(RwLock::new(SubAgentManager::new(
        tmp.path().to_path_buf(),
        2,
    )));
    let agent_id = "agent_rate_limited_checkpoint".to_string();
    let (task_input_tx, task_input_rx) = mpsc::unbounded_channel();
    let agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "Inspect rate-limit recovery".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec![]),
        task_input_tx,
        tmp.path().to_path_buf(),
        "boot_test".to_string(),
    );
    {
        let mut manager = manager.write().await;
        manager.agents.insert(agent_id.clone(), agent);
        manager.register_worker(make_worker_spec(&agent_id, tmp.path().to_path_buf()));
    }

    let (client, calls) = always_rate_limited_chat_client().await;
    let mut runtime = stub_runtime().with_step_api_timeout(Duration::from_secs(5));
    runtime.client = client;
    runtime.manager = Arc::clone(&manager);
    runtime.context = ToolContext::new(tmp.path());

    let task = SubAgentTask {
        manager_handle: Arc::clone(&manager),
        runtime,
        agent_id: agent_id.clone(),
        agent_type: SubAgentType::General,
        prompt: "Inspect rate-limit recovery".to_string(),
        assignment: make_assignment(),
        allowed_tools: Some(vec![]),
        fork_context: false,
        started_at: Instant::now(),
        max_steps: 3,
        token_budget: None,
        input_rx: task_input_rx,
        launch_gate: None,
    };

    tokio::time::timeout(
        Duration::from_secs(10),
        tokio::spawn(run_subagent_task(task)),
    )
    .await
    .expect("sub-agent task should finish")
    .expect("sub-agent join should succeed");

    assert_eq!(
        calls.load(Ordering::SeqCst),
        SUBAGENT_TRANSIENT_PROVIDER_MAX_RETRIES.saturating_add(1) as usize,
        "rate-limit retries should be owned by the sub-agent retry loop"
    );
    let snapshot = {
        let manager = manager.read().await;
        manager
            .get_result(&agent_id)
            .expect("agent should stay registered")
    };
    let SubAgentStatus::Interrupted(reason) = &snapshot.status else {
        panic!("expected interrupted sub-agent, got {:?}", snapshot.status);
    };
    assert!(
        reason.contains("rate-limited provider response"),
        "reason should name the provider rate limit: {reason}"
    );
    let checkpoint = snapshot
        .checkpoint
        .as_ref()
        .expect("rate-limit interruption should preserve checkpoint");
    assert_eq!(checkpoint.reason, "api_rate_limited");
    assert!(checkpoint.continuable);
    assert!(snapshot.needs_input.is_some());
}

#[tokio::test]
async fn spawn_duplicate_session_name_error_names_conflicting_agent() {
    // #2656：重复名称错误必须标识冲突的代理，以便模型可以确定性恢复
    //（重用 ID 或选择新名称）。
    let manager = Arc::new(RwLock::new(SubAgentManager::new(PathBuf::from("."), 5)));
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut existing = SubAgent::new(
        "test_agent_existing".to_string(),
        SubAgentType::Explore,
        "scan".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    existing.session_name = "researcher".to_string();
    existing.status = SubAgentStatus::Running;
    let existing_id = existing.id.clone();
    {
        let mut guard = manager.write().await;
        guard.agents.insert(existing_id.clone(), existing);
    }

    let err = {
        let mut guard = manager.write().await;
        guard
            .spawn_background_with_assignment_options(
                manager.clone(),
                stub_runtime(),
                SubAgentType::Explore,
                "new work".to_string(),
                make_assignment(),
                Some(vec!["read_file".to_string()]),
                SubAgentSpawnOptions {
                    name: Some("researcher".to_string()),
                    ..Default::default()
                },
            )
            .expect_err("duplicate session name must error")
    };
    let msg = err.to_string();
    assert!(
        msg.contains(&existing_id),
        "names the conflicting agent_id: {msg}"
    );
    assert!(
        msg.contains("running"),
        "includes the conflicting status: {msg}"
    );
    // #3020：经过的时间让父级区分活跃的工作器
    // 与过时的早期派生。
    assert!(
        msg.contains("started ") && msg.contains(" ago"),
        "includes elapsed time since spawn: {msg}"
    );
}

#[tokio::test]
async fn test_running_count_counts_only_agents_with_live_task_handles() {
    let mut manager = SubAgentManager::new(PathBuf::from("."), 1);
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        "test_agent_3".to_string(),
        SubAgentType::Explore,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    agent.status = SubAgentStatus::Running;
    let handle = tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(60)).await;
    });
    agent.task_handle = Some(handle);
    let agent_id = agent.id.clone();
    manager.agents.insert(agent.id.clone(), agent);

    assert_eq!(manager.running_count(), 1);
    manager
        .agents
        .get_mut(&agent_id)
        .and_then(|agent| agent.task_handle.take())
        .expect("live task handle")
        .abort();
}

#[test]
fn test_running_count_ignores_running_status_without_task_handle() {
    let mut manager = SubAgentManager::new(PathBuf::from("."), 1);
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        "test_agent_4".to_string(),
        SubAgentType::Explore,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    agent.status = SubAgentStatus::Running;
    manager.agents.insert(agent.id.clone(), agent);

    assert_eq!(manager.running_count(), 0);
}

#[tokio::test]
async fn test_running_count_counts_running_agents_until_status_reconciles() {
    let mut manager = SubAgentManager::new(PathBuf::from("."), 1);
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        "test_agent_5".to_string(),
        SubAgentType::Explore,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    agent.status = SubAgentStatus::Running;
    let finished_handle = tokio::spawn(async {});
    while !finished_handle.is_finished() {
        tokio::task::yield_now().await;
    }
    agent.task_handle = Some(finished_handle);
    manager.agents.insert(agent.id.clone(), agent);

    assert_eq!(manager.running_count(), 1);
}

#[tokio::test]
async fn admission_limit_counts_queued_and_running_workers_separately() {
    let mut manager = SubAgentManager::new(PathBuf::from("."), 2).with_admission_limit(4);
    let mut handles = Vec::new();

    for (agent_id, queued) in [
        ("agent_admit_a", false),
        ("agent_admit_b", false),
        ("agent_admit_c", true),
        ("agent_admit_d", true),
    ] {
        let (input_tx, _input_rx) = mpsc::unbounded_channel();
        let mut agent = SubAgent::new(
            agent_id.to_string(),
            SubAgentType::Explore,
            "prompt".to_string(),
            make_assignment(),
            "deepseek-v4-flash".to_string(),
            Some("Blue".to_string()),
            Some(vec!["read_file".to_string()]),
            input_tx,
            PathBuf::from("."),
            "boot_test".to_string(),
        );
        agent.status = SubAgentStatus::Running;
        agent.task_handle = Some(tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
        }));
        handles.push(agent_id.to_string());
        manager.agents.insert(agent_id.to_string(), agent);
        manager.register_worker(make_worker_spec(agent_id, PathBuf::from(".")));
        if queued {
            manager.record_worker_event(
                agent_id,
                AgentWorkerStatus::Queued,
                Some(SUBAGENT_QUEUED_LAUNCH_REASON.to_string()),
                None,
                None,
            );
        }

        if manager.admitted_count() < 4 {
            manager
                .check_admission_capacity()
                .expect("admission remains below total ceiling");
        }
    }

    assert_eq!(manager.admitted_count(), 4);
    assert_eq!(manager.active_count(), 2);
    assert_eq!(manager.queued_count(), 2);
    let err = manager
        .check_admission_capacity()
        .expect_err("admission ceiling rejects fifth worker");
    let msg = err.to_string();
    assert!(
        msg.contains("max_admitted 4") && msg.contains("running 2") && msg.contains("queued 2"),
        "error distinguishes running vs queued counts: {msg}"
    );

    for agent_id in handles {
        manager
            .agents
            .get_mut(&agent_id)
            .and_then(|agent| agent.task_handle.take())
            .expect("live task handle")
            .abort();
    }
}

#[tokio::test]
async fn cleanup_auto_cancels_stale_running_agent_and_releases_slot() {
    let mut manager = SubAgentManager::new(PathBuf::from("."), 1)
        .with_running_heartbeat_timeout(Duration::from_millis(1));
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        "test_agent_stale".to_string(),
        SubAgentType::Explore,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    agent.task_handle = Some(tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }));
    let agent_id = agent.id.clone();
    manager.agents.insert(agent_id.clone(), agent);
    tokio::time::sleep(Duration::from_millis(5)).await;

    assert_eq!(
        manager.running_count(),
        0,
        "stale running agents must not keep the concurrency slot occupied"
    );
    assert_eq!(manager.cleanup(Duration::from_secs(60 * 60)), 1);

    let snapshot = manager
        .get_result(&agent_id)
        .expect("agent should remain inspectable");
    assert_eq!(snapshot.status, SubAgentStatus::Cancelled);
    assert_eq!(manager.running_count(), 0);
    assert!(
        snapshot
            .result
            .as_deref()
            .unwrap_or_default()
            .contains("Auto-cancelled")
    );
}

#[tokio::test]
async fn status_projection_reconciles_stale_running_agent() {
    let mut inner = SubAgentManager::new(PathBuf::from("."), 1)
        .with_running_heartbeat_timeout(Duration::from_millis(1));
    let current_boot = inner.session_boot_id().to_string();
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        "test_agent_status_stale".to_string(),
        SubAgentType::Explore,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        current_boot,
    );
    agent.task_handle = Some(tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }));
    inner.agents.insert(agent.id.clone(), agent);
    tokio::time::sleep(Duration::from_millis(5)).await;

    let manager = Arc::new(RwLock::new(inner));
    let context = ToolContext::new(".");
    let result =
        inspect_agent_from_input(&json!({"action": "status"}), manager, &context, false, None)
            .await
            .expect("status projection should succeed");
    let payload: serde_json::Value =
        serde_json::from_str(&result.content).expect("status payload should be json");
    let agent = payload["agents"]
        .as_array()
        .and_then(|agents| agents.first())
        .expect("stale current-session agent should remain inspectable");

    assert_eq!(payload["count"], 1);
    assert_eq!(agent["agent_id"], "test_agent_status_stale");
    assert_eq!(agent["status"], "cancelled");
    assert_eq!(agent["terminal"], true);
    assert_eq!(agent["snapshot"]["status"], "Cancelled");
    assert!(
        agent["snapshot"]["result"]
            .as_str()
            .unwrap_or_default()
            .contains("Auto-cancelled")
    );
}

#[tokio::test]
async fn cleanup_keeps_recent_running_agent() {
    let mut manager = SubAgentManager::new(PathBuf::from("."), 1)
        .with_running_heartbeat_timeout(Duration::from_secs(300));
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        "test_agent_recent".to_string(),
        SubAgentType::Explore,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    agent.last_activity_at = Instant::now();
    agent.task_handle = Some(tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }));
    let agent_id = agent.id.clone();
    manager.agents.insert(agent_id.clone(), agent);

    assert_eq!(manager.running_count(), 1);
    assert_eq!(manager.cleanup(Duration::from_secs(60 * 60)), 0);
    assert_eq!(
        manager.get_result(&agent_id).expect("agent").status,
        SubAgentStatus::Running
    );
    manager
        .agents
        .get_mut(&agent_id)
        .and_then(|agent| agent.task_handle.take())
        .expect("live task handle")
        .abort();
}

#[tokio::test]
async fn touch_refreshes_stale_running_agent_heartbeat() {
    // 使用一个明显大于 touch() 和下面 cleanup() 断言之间同步工作的关键路径的心跳超时。
    // 使用 1ms 超时时，测试在负载较重的 CI 运行器上不稳定（尤其是 Windows，
    // 其调度器可能将此线程取消调度超过 1ms）：刚 touch 的代理会在 cleanup() 运行前
    // 回退到过时阈值并被回收，导致 cleanup() 返回 1 而非 0。
    // 50ms 的超时既保持对过期逻辑的测试，又消除了时序竞争。
    //
    //
    let mut manager = SubAgentManager::new(PathBuf::from("."), 1)
        .with_running_heartbeat_timeout(Duration::from_millis(50));
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        "test_agent_touched".to_string(),
        SubAgentType::Explore,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    agent.task_handle = Some(tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }));
    let agent_id = agent.id.clone();
    manager.agents.insert(agent_id.clone(), agent);
    // 睡眠远超 50ms 心跳超时，以便即使在粗粒度 OS 定时器下
    // 计时器提前触发，代理也可靠过期。
    tokio::time::sleep(Duration::from_millis(150)).await;

    assert_eq!(manager.running_count(), 0);
    assert!(manager.touch(&agent_id));
    assert_eq!(manager.running_count(), 1);
    assert_eq!(manager.cleanup(Duration::from_secs(60 * 60)), 0);
    manager
        .agents
        .get_mut(&agent_id)
        .and_then(|agent| agent.task_handle.take())
        .expect("live task handle")
        .abort();
}

#[test]
fn test_persist_and_reload_marks_running_agent_as_interrupted() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let state_path = default_state_path(tmp.path()).expect("default state path");

    let mut manager = SubAgentManager::new(workspace.clone(), 2).with_state_path(state_path);
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let running = SubAgent::new(
        "test_agent_9_running".to_string(),
        SubAgentType::General,
        "work".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    let running_id = running.id.clone();
    manager.agents.insert(running_id.clone(), running);
    manager
        .persist_state()
        .expect("persist state")
        .join()
        .expect("persist thread");

    let mut reloaded = SubAgentManager::new(workspace, 2)
        .with_state_path(default_state_path(tmp.path()).expect("default state path"));
    reloaded.load_state().expect("load state");
    let snapshot = reloaded
        .get_result(&running_id)
        .expect("reloaded agent should exist");
    assert!(matches!(
        snapshot.status,
        SubAgentStatus::Interrupted(ref message)
            if message.contains(SUBAGENT_RESTART_REASON)
    ));
}

#[test]
fn persist_and_reload_preserves_checkpoint_for_interrupted_running_agent() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let state_path = default_state_path(tmp.path()).expect("default state path");

    let mut manager = SubAgentManager::new(workspace.clone(), 2).with_state_path(state_path);
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut running = SubAgent::new(
        "test_agent_checkpoint_reload".to_string(),
        SubAgentType::General,
        "work".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    running.checkpoint = Some(make_checkpoint(
        &running.id,
        2,
        vec![
            text_message("user", "initial task"),
            text_message("assistant", "partial progress"),
        ],
    ));
    let running_id = running.id.clone();
    manager.agents.insert(running_id.clone(), running);
    manager
        .persist_state()
        .expect("persist state")
        .join()
        .expect("persist thread");

    let mut reloaded = SubAgentManager::new(workspace, 2)
        .with_state_path(default_state_path(tmp.path()).expect("default state path"));
    reloaded.load_state().expect("load state");
    let snapshot = reloaded
        .get_result(&running_id)
        .expect("reloaded agent should exist");

    assert!(matches!(snapshot.status, SubAgentStatus::Interrupted(_)));
    let checkpoint = snapshot.checkpoint.expect("checkpoint should reload");
    assert!(checkpoint.continuable);
    assert_eq!(checkpoint.steps_taken, 2);
    assert_eq!(checkpoint.messages.len(), 2);
    assert_eq!(message_text(&checkpoint.messages[1]), "partial progress");
}

#[cfg(unix)]
#[test]
fn load_state_rejects_symlinked_state_file() {
    let tmp = tempdir().expect("tempdir");
    let target = tmp.path().join("outside-state.json");
    let link = tmp.path().join(SUBAGENT_STATE_FILE);
    std::fs::write(
        &target,
        serde_json::json!({
            "schema_version": SUBAGENT_STATE_SCHEMA_VERSION,
            "agents": [],
            "workers": []
        })
        .to_string(),
    )
    .expect("write target");
    std::os::unix::fs::symlink(&target, &link).expect("symlink state");

    let mut manager = SubAgentManager::new(tmp.path().to_path_buf(), 1).with_state_path(link);
    let err = manager
        .load_state()
        .expect_err("symlinked state should fail");
    assert!(format!("{err:#}").contains("must not traverse symlinks"));
}

#[test]
fn persist_state_rejects_state_path_outside_workspace() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    let outside_state = tmp.path().join("outside-state.json");
    std::fs::create_dir_all(&workspace).expect("mkdir workspace");

    let manager = SubAgentManager::new(workspace, 1).with_state_path(outside_state);
    let err = manager
        .persist_state()
        .expect_err("outside state path should fail");

    assert!(format!("{err:#}").contains("must stay within workspace"));
}

#[cfg(unix)]
#[test]
fn persist_state_rejects_symlinked_state_directory() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    let outside = tmp.path().join("outside-state");
    let codewhale_dir = workspace.join(".codewhale");
    let state_dir = codewhale_dir.join("state");
    std::fs::create_dir_all(&codewhale_dir).expect("mkdir codewhale");
    std::fs::create_dir_all(&outside).expect("mkdir outside");
    std::os::unix::fs::symlink(&outside, &state_dir).expect("symlink state dir");

    let err = default_state_path(&workspace)
        .expect_err("symlinked state directory should fail before manager construction");
    assert!(
        format!("{err:#}").contains("must stay within workspace")
            || format!("{err:#}").contains("must not traverse symlinks")
    );
}

#[test]
fn test_interrupted_status_name_and_summary() {
    let snapshot = make_snapshot(SubAgentStatus::Interrupted(
        SUBAGENT_RESTART_REASON.to_string(),
    ));
    assert_eq!(subagent_status_name(&snapshot.status), "interrupted");
    assert!(summarize_subagent_result(&snapshot).contains(SUBAGENT_RESTART_REASON));
}

// === v0.6.6 — 子代理权限统一 ===

#[test]
fn build_allowed_tools_general_returns_none_for_full_inheritance() {
    // 默认行为：无显式列表的 General 代理继承父级的完整注册表
    //（None 表示无缩小）。
    let result = build_allowed_tools(&SubAgentType::General, None, true).unwrap();
    assert!(
        result.is_none(),
        "General with no explicit_tools should default to full inheritance (None), got {result:?}"
    );
}

#[test]
fn build_allowed_tools_explore_returns_none_for_full_inheritance() {
    // 按类型的允许列表现在是建议性的——除非传递显式列表，
    // Explore 也获得完整工具面。
    let result = build_allowed_tools(&SubAgentType::Explore, None, true).unwrap();
    assert!(
        result.is_none(),
        "Explore with no explicit_tools should default to full inheritance"
    );
}

#[test]
fn build_allowed_tools_custom_requires_explicit_list() {
    // Custom 是唯一需要显式 allowed_tools 的类型。
    let err = build_allowed_tools(&SubAgentType::Custom, None, true).unwrap_err();
    assert!(
        err.to_string().contains("Custom sub-agent requires"),
        "got: {err}"
    );
}

#[test]
fn build_allowed_tools_explicit_list_returned_as_some() {
    let explicit = vec!["read_file".to_string(), "list_dir".to_string()];
    let result = build_allowed_tools(&SubAgentType::Custom, Some(explicit.clone()), true).unwrap();
    assert_eq!(result, Some(explicit));
}

#[test]
fn build_allowed_tools_explicit_list_dedupes_and_trims() {
    let explicit = vec![
        "read_file".to_string(),
        "  read_file  ".to_string(), // trim + dedupe
        "list_dir".to_string(),
        "".to_string(), // skip empty
    ];
    let result = build_allowed_tools(&SubAgentType::Custom, Some(explicit), true).unwrap();
    assert_eq!(
        result,
        Some(vec!["read_file".to_string(), "list_dir".to_string()])
    );
}

#[test]
fn parse_spawn_request_extracts_cwd_when_present() {
    let input = json!({
        "prompt": "build feature A",
        "cwd": ".worktrees/feature-a"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(
        parsed.cwd.as_ref().map(|p| p.to_string_lossy().to_string()),
        Some(".worktrees/feature-a".to_string())
    );
}

#[test]
fn parse_spawn_request_accepts_worktree_isolation() {
    let input = json!({
        "prompt": "build feature A",
        "worktree": true,
        "worktree_branch": "codex/agent-feature-a",
        "worktree_path": "feature-a",
        "worktree_base": "HEAD"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    let worktree = parsed.worktree.expect("worktree request");
    assert_eq!(worktree.branch.as_deref(), Some("codex/agent-feature-a"));
    assert_eq!(worktree.base_ref.as_deref(), Some("HEAD"));
    assert_eq!(
        worktree
            .path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string()),
        Some("feature-a".to_string())
    );
}

#[test]
fn parse_spawn_request_accepts_cwd_with_worktree_isolation() {
    let input = json!({
        "prompt": "build feature A",
        "cwd": ".worktrees/manual",
        "worktree": true
    });
    let parsed = parse_spawn_request(&input).expect("cwd and worktree may be combined");
    assert!(parsed.worktree.is_some());
    assert!(parsed.cwd.is_some());
}

#[test]
fn git_repo_root_finds_repo_from_direct_cwd() {
    let repo = init_subagent_git_repo();
    let root = git_repo_root(repo.path()).expect("direct repo cwd should resolve");
    assert_eq!(
        root.canonicalize().expect("canonical root"),
        repo.path().canonicalize().expect("canonical repo")
    );
}

#[test]
fn git_repo_root_discovers_one_level_nested_repo_from_harness() {
    let repo = init_subagent_git_repo();
    let harness = tempdir().expect("harness dir");
    let nested = harness.path().join("CodeWhale");
    Command::new("git")
        .args([
            "clone",
            repo.path().to_str().unwrap(),
            nested.to_str().unwrap(),
        ])
        .output()
        .expect("clone nested repo");
    let root = git_repo_root(harness.path()).expect("harness cwd should discover nested repo");
    assert_eq!(
        root.canonicalize().expect("canonical root"),
        nested.canonicalize().expect("canonical nested")
    );
}

#[test]
fn git_repo_root_reports_attempted_paths_when_no_repo_found() {
    let repo_root = git_repo_root(&std::env::current_dir().expect("current dir"))
        .expect("test should run inside the checkout");
    let harness = TempDirBuilder::new()
        .prefix(".codewhale-no-repo-")
        .tempdir_in(repo_root.parent().expect("repo parent"))
        .expect("empty harness outside checkout");
    let empty = harness
        .path()
        .join("isolated")
        .join("a")
        .join("b")
        .join("c")
        .join("d")
        .join("empty");
    std::fs::create_dir_all(&empty).expect("empty nested dir");
    let expected = empty.canonicalize().expect("canonical empty dir");
    let err = git_repo_root(&empty).expect_err("missing repo should fail cleanly");
    let message = err.to_string();
    assert!(
        message.contains("Tried:") && message.contains(expected.to_string_lossy().as_ref()),
        "expected friendly attempted-path error, got: {message}"
    );
}

#[test]
fn parse_spawn_request_cwd_absent_yields_none() {
    let input = json!({ "prompt": "no cwd" });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert!(parsed.cwd.is_none());
}

#[test]
fn parse_spawn_request_cwd_empty_string_yields_none() {
    let input = json!({ "prompt": "empty cwd", "cwd": "   " });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert!(parsed.cwd.is_none(), "whitespace-only cwd should be None");
}

#[test]
fn create_isolated_worktree_creates_branch_checkout_outside_parent_repo() {
    let repo = init_subagent_git_repo();
    let worktree_home = tempdir().expect("worktree home");
    let request = SubAgentWorktreeRequest {
        branch: Some("codex/agent-isolated-test".to_string()),
        path: Some(worktree_home.path().join("isolated")),
        base_ref: None,
    };

    let path = create_isolated_worktree(
        repo.path(),
        &request,
        Some("isolated-test"),
        &SubAgentType::Implementer,
    )
    .expect("worktree should be created");

    assert!(path.exists(), "worktree path should exist");
    assert!(
        !path.starts_with(repo.path()),
        "generated worktree must be outside the parent checkout"
    );
    assert_eq!(
        current_git_branch(&path).as_deref(),
        Some("codex/agent-isolated-test")
    );
}

#[test]
fn create_isolated_worktree_rejects_invalid_branch_as_input() {
    let repo = init_subagent_git_repo();
    let worktree_home = tempdir().expect("worktree home");
    let request = SubAgentWorktreeRequest {
        branch: Some("bad branch name".to_string()),
        path: Some(worktree_home.path().join("isolated")),
        base_ref: None,
    };

    let err = create_isolated_worktree(
        repo.path(),
        &request,
        Some("isolated-test"),
        &SubAgentType::Implementer,
    )
    .expect_err("invalid branch should fail");

    assert!(
        err.to_string().contains("Invalid worktree_branch"),
        "unexpected error: {err}"
    );
}

fn init_git_repo_at(path: &std::path::Path) {
    let init = Command::new("git")
        .arg("init")
        .current_dir(path)
        .output()
        .expect("git init should run");
    assert!(init.status.success(), "git init failed");
    let commit = Command::new("git")
        .args([
            "-c",
            "user.name=codewhale Tests",
            "-c",
            "user.email=tests@example.com",
            "commit",
            "--allow-empty",
            "-m",
            "init",
        ])
        .current_dir(path)
        .output()
        .expect("git commit should run");
    assert!(commit.status.success(), "git commit failed");
}

#[test]
fn create_isolated_worktree_discovers_nested_repo_from_harness_parent() {
    let harness = tempdir().expect("harness");
    let nested = harness.path().join("CodeWhale");
    std::fs::create_dir_all(&nested).expect("nested checkout dir");
    init_git_repo_at(&nested);
    let worktree_home = tempdir().expect("worktree home");
    let request = SubAgentWorktreeRequest {
        branch: Some("codex/agent-harness-nested".to_string()),
        path: Some(worktree_home.path().join("isolated")),
        base_ref: None,
    };

    let path = create_isolated_worktree(
        harness.path(),
        &request,
        Some("harness-nested"),
        &SubAgentType::Explore,
    )
    .expect("harness parent should discover nested repo");

    assert!(path.exists(), "worktree path should exist");
    assert_eq!(
        current_git_branch(&path).as_deref(),
        Some("codex/agent-harness-nested")
    );
}

#[test]
fn create_isolated_worktree_reports_friendly_error_when_no_repo_found() {
    let harness = tempdir().expect("harness");
    std::fs::create_dir_all(harness.path().join("not-a-repo")).expect("mkdir");
    let worktree_home = tempdir().expect("worktree home");
    let request = SubAgentWorktreeRequest {
        branch: Some("codex/agent-missing".to_string()),
        path: Some(worktree_home.path().join("isolated")),
        base_ref: None,
    };

    let err = create_isolated_worktree(harness.path(), &request, None, &SubAgentType::General)
        .expect_err("missing repo should fail with friendly error");

    let message = err.to_string();
    assert!(
        message.contains("requires a git repository") && message.contains("Tried:"),
        "expected actionable discovery error, got: {message}"
    );
}

#[test]
fn create_isolated_worktree_rejects_ambiguous_nested_repos() {
    let harness = tempdir().expect("harness");
    for name in ["RepoA", "RepoB"] {
        let nested = harness.path().join(name);
        std::fs::create_dir_all(&nested).expect("nested dir");
        init_git_repo_at(&nested);
    }
    let worktree_home = tempdir().expect("worktree home");
    let request = SubAgentWorktreeRequest {
        branch: Some("codex/agent-ambiguous".to_string()),
        path: Some(worktree_home.path().join("isolated")),
        base_ref: None,
    };

    let err = create_isolated_worktree(harness.path(), &request, None, &SubAgentType::General)
        .expect_err("multiple nested repos should fail deterministically");

    let message = err.to_string();
    assert!(
        message.contains("Multiple git repositories found"),
        "expected ambiguity diagnostic, got: {message}"
    );
}

#[test]
fn build_subagent_system_prompt_appends_role_when_set() {
    let assignment = SubAgentAssignment::new("p".to_string(), Some("worker".to_string()));
    let prompt = build_subagent_system_prompt(&SubAgentType::General, &assignment);
    assert!(
        prompt.contains("You are operating in the role of `worker`."),
        "expected role line present, got: {}",
        &prompt[prompt.len().saturating_sub(160)..]
    );
    // 共享的后台工作器/调用者框架跟在角色行之后。
    assert!(prompt.contains("background sub-agent"));
}

#[test]
fn build_subagent_system_prompt_skips_role_when_none() {
    let assignment = SubAgentAssignment::new("p".to_string(), None);
    let prompt = build_subagent_system_prompt(&SubAgentType::General, &assignment);
    assert!(!prompt.contains("You are operating in the role of"));
}

#[test]
fn build_subagent_system_prompt_skips_role_when_blank() {
    let assignment = SubAgentAssignment::new("p".to_string(), Some("   ".to_string()));
    let prompt = build_subagent_system_prompt(&SubAgentType::General, &assignment);
    assert!(!prompt.contains("You are operating in the role of"));
}

#[test]
fn subagent_done_sentinel_format_is_well_formed() {
    let res = make_snapshot(SubAgentStatus::Completed);
    let sentinel = subagent_done_sentinel("agent_xyz", &res, false);
    assert!(sentinel.starts_with("<codewhale:subagent.done>"));
    assert!(sentinel.ends_with("</codewhale:subagent.done>"));

    // 内部 JSON 解析并携带预期字段。
    let inner = sentinel
        .trim_start_matches("<codewhale:subagent.done>")
        .trim_end_matches("</codewhale:subagent.done>");
    let parsed: serde_json::Value = serde_json::from_str(inner).expect("inner JSON parses");
    assert_eq!(parsed["agent_id"], "agent_xyz");
    assert_eq!(parsed["status"], "completed");
    assert_eq!(parsed["agent_type"], "general");
    assert_eq!(parsed["summary_location"], "previous_line");
    // issue #2652：完整（未截断）的摘要被标记为此。
    assert_eq!(parsed["summary_kind"], "complete");
    assert!(parsed.get("details").is_none());
    assert!(parsed.get("result_clipped").is_none());
    assert!(parsed.get("summary_complete").is_none());
    assert!(parsed.get("next_action").is_none());
    assert!(parsed.get("summary").is_none());
    assert!(parsed.get("duration_ms").is_none());
    assert!(parsed.get("steps").is_none());
}

#[test]
fn subagent_done_sentinel_keeps_large_result_out_of_metadata() {
    let mut res = make_snapshot(SubAgentStatus::Completed);
    res.result = Some("x".repeat(2048));
    let sentinel = subagent_done_sentinel("agent_big", &res, false);
    let inner = sentinel
        .trim_start_matches("<codewhale:subagent.done>")
        .trim_end_matches("</codewhale:subagent.done>");
    let parsed: serde_json::Value = serde_json::from_str(inner).expect("inner JSON parses");
    assert_eq!(parsed["agent_id"], "agent_big");
    assert_eq!(parsed["summary_location"], "previous_line");
    assert_eq!(parsed["summary_kind"], "complete");
    assert!(parsed.get("result_clipped").is_none());
    assert!(parsed.get("summary_complete").is_none());
    assert!(parsed.get("next_action").is_none());
    assert!(
        !inner.contains(&"x".repeat(128)),
        "sentinel should not duplicate large result text"
    );
}

#[test]
fn subagent_done_sentinel_marks_truncated_summaries() {
    // issue #2652：当子摘要被长度限制时，哨兵必须标明
    // summary_kind:"truncated"，以便父级引导验证。
    let res = make_snapshot(SubAgentStatus::Completed);
    let sentinel = subagent_done_sentinel("agent_trunc", &res, true);
    let inner = sentinel
        .trim_start_matches("<codewhale:subagent.done>")
        .trim_end_matches("</codewhale:subagent.done>");
    let parsed: serde_json::Value = serde_json::from_str(inner).expect("inner JSON parses");
    assert_eq!(parsed["summary_kind"], "truncated");
}

#[test]
fn stamp_subagent_summary_appends_note_when_short() {
    // issue #2652：简短（完整）的摘要获取软性自报告备注，
    // 且不被标记为截断。
    let (stamped, truncated) = stamp_subagent_summary("All tests pass.");
    assert!(!truncated);
    assert!(stamped.starts_with("All tests pass."));
    assert!(
        stamped.contains("[Sub-agent self-report"),
        "short summary gets the provenance note"
    );
    assert!(
        !stamped.contains("[Sub-agent summary truncated"),
        "short summary must not get the truncation footer"
    );
}

#[test]
fn stamp_subagent_summary_truncates_when_over_budget() {
    // issue #2652：超出预算的摘要使用现有的 [Output truncated ...] 词汇进行
    // 头尾截断，诚实地说明没有检索句柄，
    // 并被标记为截断。
    let big = "a".repeat(SUBAGENT_SUMMARY_CHAR_BUDGET + 5_000);
    let (stamped, truncated) = stamp_subagent_summary(&big);
    assert!(truncated);
    assert!(
        stamped.contains("[Sub-agent summary truncated"),
        "long summary gets the truncation footer"
    );
    assert!(
        stamped.contains("not in the spillover store"),
        "footer is honest about the missing retrieve handle"
    );
    assert!(
        !stamped.contains("[Sub-agent self-report"),
        "truncated summary must not also get the self-report note"
    );
    // 头部和尾部切片存在；中间的一段预算长度的 'a' 已移除。
    //
    assert!(stamped.contains(&"a".repeat(SUBAGENT_SUMMARY_HEAD_CHARS)));
    assert!(stamped.contains(&"a".repeat(SUBAGENT_SUMMARY_TAIL_CHARS)));
    assert!(
        stamped.chars().filter(|c| *c == 'a').count() < big.chars().count(),
        "truncation removed middle characters"
    );
}

#[test]
fn subagent_failed_sentinel_format_is_well_formed() {
    let sentinel = subagent_failed_sentinel("agent_zzz", "boom");
    let inner = sentinel
        .trim_start_matches("<codewhale:subagent.done>")
        .trim_end_matches("</codewhale:subagent.done>");
    let parsed: serde_json::Value = serde_json::from_str(inner).expect("inner JSON parses");
    assert_eq!(parsed["agent_id"], "agent_zzz");
    assert_eq!(parsed["status"], "failed");
    assert_eq!(parsed["error_location"], "previous_line");
    assert!(parsed.get("details").is_none());
    assert!(parsed.get("next_action").is_none());
    // 保持精简——错误文本位于上一行，而非哨兵中。
    assert!(parsed.get("error").is_none());
}

#[test]
fn annotated_failure_message_composes_class_tag_and_model_hint() {
    // #3884：失败记录器组合 subagent_failure_message（添加类标签+完整链）与
    // annotate_child_model_error（添加模型可用性提示）。
    // 固定 mailbox/update_failed 调用点实际执行的组合，
    // 而非仅隔离的辅助函数。
    let err = anyhow::Error::new(crate::llm_client::LlmError::AuthorizationError(
        "The model `gpt-5.5-codex` does not exist or you do not have access".to_string(),
    ))
    .context("Responses API request failed");

    let provider = crate::config::ApiProvider::OpenaiCodex;
    let route = ModelRoute::Fixed("gpt-5.5-codex".to_string());
    let annotated = annotate_child_model_error(
        &subagent_failure_message(&err),
        "gpt-5.5-codex",
        provider,
        &route,
    );

    // 来自 subagent_failure_message 的类标签。
    assert!(annotated.starts_with("[auth]"), "{annotated}");
    // 完整链保留。
    assert!(
        annotated.contains("Responses API request failed"),
        "{annotated}"
    );
    assert!(annotated.contains("does not exist"), "{annotated}");
    // 模型可用性提示触发，因为真实的提供者文本现在到达分类器
    //（当仅记录被屏蔽的外部上下文字符串时无法做到）。
    //
    assert!(annotated.contains("gpt-5.5-codex"), "{annotated}");
    assert!(
        annotated.contains("child model override")
            || annotated.contains("child-agent model config"),
        "{annotated}"
    );
    // #4049：失败现在命名提供者和路由来源。
    assert!(annotated.contains(provider.display_name()), "{annotated}");
    assert!(annotated.contains("route:"), "{annotated}");
    assert!(annotated.contains("explicit model id"), "{annotated}");
}

#[test]
fn subagent_failure_message_preserves_error_chain() {
    // #3884：anyhow 错误上的 to_string() 仅打印最外层上下文
    //（"Responses API request failed"），掩盖了源 LlmError 携带的
    // HTTP 状态和主体详情。失败消息必须遍历链并添加错误类前缀。
    //
    let err = anyhow::Error::new(crate::llm_client::LlmError::InvalidRequest {
        status: 400,
        message: "model `gpt-5.5-codex` is not supported on this endpoint".to_string(),
    })
    .context("Responses API request failed");

    let message = subagent_failure_message(&err);
    assert!(message.starts_with("[invalid_request]"), "{message}");
    assert!(
        message.contains("Responses API request failed"),
        "{message}"
    );
    assert!(message.contains("Invalid request (400)"), "{message}");
    assert!(
        message.contains("not supported on this endpoint"),
        "{message}"
    );

    // 速率限制也进行分类——来自报告的扇出失败形状。
    let err = anyhow::Error::new(crate::llm_client::LlmError::RateLimited {
        message: "please slow down".to_string(),
        retry_after: None,
    })
    .context("Responses API request failed");
    let message = subagent_failure_message(&err);
    assert!(message.starts_with("[rate_limited]"), "{message}");
    assert!(message.contains("please slow down"), "{message}");

    // 链中没有 LlmError 的普通错误无标签通过，
    // 但仍完全链接。
    let err = anyhow::anyhow!("boom").context("outer");
    let message = subagent_failure_message(&err);
    assert_eq!(message, "outer: boom");
}

#[test]
fn annotate_child_model_error_adds_actionable_hint() {
    // #2653：通过命名模型和恢复路径，裸的 provider 403 变为可操作，
    // 而不相关的错误原样通过。
    let provider = crate::config::ApiProvider::Moonshot;
    let inherit = ModelRoute::Inherit;
    let auth = annotate_child_model_error("403 Forbidden", "kimi-k2", provider, &inherit);
    assert!(auth.contains("kimi-k2"), "names the model: {auth}");
    assert!(
        auth.contains("child model override"),
        "names the recovery path: {auth}"
    );
    assert!(
        auth.contains("403 Forbidden"),
        "preserves the original: {auth}"
    );
    // #4049：在提示中命名提供者和路由来源。
    assert!(auth.contains(provider.display_name()), "{auth}");
    assert!(auth.contains("inherited from the parent"), "{auth}");

    // 不相关的错误仍完全不变地通过
    //（网络故障时不添加提供者/路由噪声）。
    let unrelated =
        annotate_child_model_error("connection reset by peer", "kimi-k2", provider, &inherit);
    assert_eq!(unrelated, "connection reset by peer");

    // #3020：分类为 Internal（非 Authorization/State）的提供者拒绝
    // 仍通过原始文本匹配获得提示。
    let not_exist = annotate_child_model_error("Model Not Exist", "kimi-k2", provider, &inherit);
    assert!(
        not_exist.contains("child-agent model config"),
        "DeepSeek-style rejection gets the hint: {not_exist}"
    );

    let openai_style = annotate_child_model_error(
        "The model `gpt-5.5-nano` does not exist or you do not have access to it.",
        "gpt-5.5-nano",
        crate::config::ApiProvider::OpenaiCodex,
        &ModelRoute::Fixed("gpt-5.5-nano".to_string()),
    );
    assert!(
        openai_style.contains("child-agent model config"),
        "OpenAI-style rejection gets the hint: {openai_style}"
    );
}

#[test]
fn child_launch_error_names_provider_model_and_route_source() {
    // #4049：模型未找到的子代理启动失败必须命名使用的提供者、
    // 请求的模型以及产生它的路由，以便父级（和用户）可以判断
    // 是提供者上下文丢失、请求了错误的模型，
    // 还是需要调整覆盖设置。
    let err = anyhow::Error::new(crate::llm_client::LlmError::ModelError(
        "Model \"deepseek-v4-pro\" not found".to_string(),
    ));
    let provider = crate::config::ApiProvider::Deepseek;
    let route = ModelRoute::Fixed("deepseek-v4-pro".to_string());
    let annotated = annotate_child_model_error(
        &subagent_failure_message(&err),
        "deepseek-v4-pro",
        provider,
        &route,
    );
    assert!(
        annotated.contains(provider.display_name()),
        "provider: {annotated}"
    );
    assert!(annotated.contains("deepseek-v4-pro"), "model: {annotated}");
    assert!(
        annotated.contains("route:"),
        "route label present: {annotated}"
    );
    assert!(
        annotated.contains("explicit model id"),
        "route source: {annotated}"
    );

    // 路由标签区分反映继承路由与固定路由。
    let inherited = annotate_child_model_error(
        &subagent_failure_message(&err),
        "deepseek-v4-pro",
        provider,
        &ModelRoute::Inherit,
    );
    assert!(
        inherited.contains("inherited from the parent"),
        "inherit route source: {inherited}"
    );
}

#[test]
fn subagent_runtime_default_max_depth_is_three() {
    // 合理性检查常量——未经测试就修改它意味着文档过时。
    assert_eq!(DEFAULT_MAX_SPAWN_DEPTH, 3);
}

#[test]
fn would_exceed_depth_at_boundary() {
    // depth=2, max=3 → 下一次派生（depth 3）允许（允许相等）。
    // depth=3, max=3 → 下一次派生（depth 4）超出。
    let runtime = stub_runtime();
    let mut at_max = runtime.clone();
    at_max.spawn_depth = 3;
    at_max.max_spawn_depth = 3;
    assert!(
        at_max.would_exceed_depth(),
        "depth 3 + max 3 → next would be 4, exceeds"
    );

    let mut below_max = runtime;
    below_max.spawn_depth = 2;
    below_max.max_spawn_depth = 3;
    assert!(
        !below_max.would_exceed_depth(),
        "depth 2 + max 3 → next is 3, allowed"
    );
}

#[test]
fn clamp_child_max_spawn_depth_enforces_absolute_ceiling() {
    let ceiling = codewhale_config::MAX_SPAWN_DEPTH_CEILING;
    // 深层子级重新提供 max_depth 不能将上限推过天花板——
    // 这是递归环限制绕过修复。一旦达到天花板，
    // 结果上限等于天花板，因此 would_exceed_depth 阻止。
    assert_eq!(clamp_child_max_spawn_depth(ceiling, 5), ceiling);
    assert_eq!(clamp_child_max_spawn_depth(ceiling - 1, 5), ceiling);
    // 天花板以下的较小请求仍然被遵循（更少的环数）。
    assert_eq!(clamp_child_max_spawn_depth(1, 2), 3);
    // 饱和加法不会溢出为巨大的上限。
    assert_eq!(clamp_child_max_spawn_depth(u32::MAX, 5), ceiling);

    // 端到端：上限通过天花板钳位设置的运行时
    // 无法再派生另一层。
    let mut rt = stub_runtime();
    rt.spawn_depth = ceiling;
    rt.max_spawn_depth = clamp_child_max_spawn_depth(rt.spawn_depth, 5);
    assert!(
        rt.would_exceed_depth(),
        "at the ceiling, a further spawn must be blocked regardless of max_depth"
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn rate_limit_pause_blocks_subagent_spawn() {
    let _guard = crate::retry_status::test_guard();
    // 即使下面的断言 panic 也要丢弃清除窗口：此状态是进程全局的，
    // 泄漏的 30 秒暂停会使每个并发运行中且工作器发出模型请求的测试停滞。
    //
    let _clear = ClearRateLimitOnDrop;
    crate::retry_status::clear();
    crate::retry_status::clear_rate_limit();
    crate::retry_status::note_rate_limit(Duration::from_secs(30));

    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 2);

    let err = spawn_subagent_from_input(
        json!({"prompt": "inspect the retry gate"}),
        Arc::clone(&manager),
        runtime,
    )
    .await
    .expect_err("active provider rate-limit pause must refuse new sub-agent work");

    assert!(
        err.to_string().contains("rate-limiting"),
        "error should name the provider throttle: {err}"
    );
    assert!(
        manager.read().await.list().is_empty(),
        "refused spawn must not register or launch a worker"
    );
}

#[test]
fn child_runtime_increments_depth_and_preserves_auto_approve() {
    let mut parent = stub_runtime();
    parent.spawn_depth = 1;
    parent.context.auto_approve = false; // parent in suggest mode
    let child = parent.child_runtime();
    assert_eq!(child.spawn_depth, 2, "child depth = parent + 1");
    assert_eq!(child.step_api_timeout, DEFAULT_STEP_API_TIMEOUT);
    assert!(
        !child.context.auto_approve,
        "child must inherit parent approval state"
    );
    assert!(!parent.context.auto_approve);

    parent.context.auto_approve = true;
    let auto_child = parent.child_runtime();
    assert!(
        auto_child.context.auto_approve,
        "auto-approved parents should still create auto-approved children"
    );
}

#[test]
fn child_and_background_runtimes_preserve_step_api_timeout() {
    let timeout = Duration::from_secs(7);
    let parent = stub_runtime().with_step_api_timeout(timeout);

    let child = parent.child_runtime();
    assert_eq!(child.step_api_timeout, timeout);

    let background = parent.background_runtime();
    assert_eq!(background.step_api_timeout, timeout);
}

#[tokio::test]
async fn subagent_registry_blocks_approval_tools_without_parent_auto_approve() {
    let mut runtime = stub_runtime();
    runtime.context.auto_approve = false;
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::General,
        Some(vec!["exec_shell".to_string()]),
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );

    let err = registry
        .execute("agent_test", "exec_shell", json!({"command": "echo hi"}))
        .await
        .expect_err("approval-gated child tool should be blocked");

    assert!(
        err.to_string().contains("requires approval"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn implementer_delegation_allows_suggest_write_without_parent_auto_approve() {
    // Issue #1828：implementer 代理无法写入文件，即使它们的工作就是落地代码变更，
    // 因为当父级以 suggest 模式运行时，注册表阻止了每个需要批准的工具。
    // 加固的门控（#1833）将 Suggest 级别工具（write_file、edit_file、apply_patch）
    // 委托给可写入角色。
    //
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(workspace.clone());
    runtime.context.auto_approve = false;
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::Implementer,
        None,
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );

    let result = registry
        .execute(
            "agent_test",
            "write_file",
            json!({"path": "delegated.txt", "content": "hello"}),
        )
        .await
        .expect("delegated write should be allowed for implementer");

    let written = std::fs::read_to_string(workspace.join("delegated.txt"))
        .expect("file should exist after delegated write");
    assert_eq!(written, "hello");
    assert!(
        !result.contains("requires approval"),
        "successful write should not look like an approval error: {result}"
    );
}

#[tokio::test]
async fn workflow_accept_edits_allows_general_file_write_without_parent_auto_approve() {
    // 工作流派生的子级接受可写姿态（包括 general）的 Suggest 级别文件编辑，
    // 而 shell 工具仍需要父级自动批准。
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(workspace.clone());
    runtime.context.auto_approve = false;
    runtime.accept_edits = true;
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::General,
        None,
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );

    let result = registry
        .execute(
            "agent_test",
            "write_file",
            json!({"path": "workflow_edit.txt", "content": "from workflow"}),
        )
        .await
        .expect("workflow accept_edits should allow general write");
    let written =
        std::fs::read_to_string(workspace.join("workflow_edit.txt")).expect("file should exist");
    assert_eq!(written, "from workflow");
    assert!(!result.contains("requires approval"), "{result}");

    let err = registry
        .execute("agent_test", "exec_shell", json!({"command": "echo hi"}))
        .await
        .expect_err("shell must still require parent auto-approve");
    assert!(
        err.to_string().contains("requires approval"),
        "unexpected: {err}"
    );
}

#[tokio::test]
async fn general_delegation_still_blocks_suggest_write_without_parent_auto_approve() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(workspace.clone());
    runtime.context.auto_approve = false;
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::General,
        None,
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );

    let err = registry
        .execute(
            "agent_test",
            "write_file",
            json!({"path": "general.txt", "content": "ok"}),
        )
        .await
        .expect_err("general agent should not silently gain write permission");
    let msg = err.to_string();
    assert!(
        msg.contains("not delegated to general sub-agents"),
        "general writes should be rejected with a role-aware message: {msg}"
    );

    assert!(
        !workspace.join("general.txt").exists(),
        "general write must not land without parent auto-approve"
    );
}

#[tokio::test]
async fn explore_role_still_blocks_suggest_writes_without_parent_auto_approve() {
    // 只读立场（explore、plan、review、verifier）不得通过委托获得写入能力——
    // 否则请求"只是看看代码"的父级
    // 可能会发现文件在背后被修改。
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    runtime.context.auto_approve = false;
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::Explore,
        None,
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );

    let err = registry
        .execute(
            "agent_test",
            "write_file",
            json!({"path": "should_not_appear.txt", "content": "denied"}),
        )
        .await
        .expect_err("explore agents must not write");
    let msg = err.to_string();
    assert!(
        msg.contains("explore") && msg.contains("not permitted"),
        "explore writes should be rejected with a role-aware message: {msg}"
    );
    assert!(
        !tmp.path().join("should_not_appear.txt").exists(),
        "file must not have been written"
    );
}

#[tokio::test]
async fn explore_role_blocks_writes_even_under_parent_auto_approve() {
    // #3217：权威的按角色姿态关闭自动批准绕过——
    // 只读角色即使在父会话自动批准时
    // 也不能修改工作区。
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    runtime.context.auto_approve = true;
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::Explore,
        None,
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );

    let err = registry
        .execute(
            "agent_test",
            "write_file",
            json!({"path": "nope.txt", "content": "denied"}),
        )
        .await
        .expect_err("explore must not write even under parent auto-approve");
    assert!(
        err.to_string().contains("not permitted"),
        "expected posture rejection, got: {err}"
    );
    assert!(
        !tmp.path().join("nope.txt").exists(),
        "file must not have been written under auto-approve"
    );
}

#[tokio::test]
async fn delegated_write_role_still_blocks_required_tools() {
    // Required 级别工具（exec_shell 等）无论角色如何
    // 仍需要父级自动批准。Implementer 可以写文件，
    // 但不能仅仅因为自己是"写"角色就绕过 shell 批准。
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    runtime.context.auto_approve = false;
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::Implementer,
        Some(vec!["exec_shell".to_string()]),
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );

    let err = registry
        .execute("agent_test", "exec_shell", json!({"command": "echo hi"}))
        .await
        .expect_err("Required-level shell must still need parent auto-approve");
    assert!(
        err.to_string().contains(
            "cannot run inside this sub-agent unless the parent session is auto-approved"
        ),
        "expected Required-level approval message, got: {err}"
    );
}

#[tokio::test]
async fn auto_approved_parent_runs_required_tools_in_subagent() {
    // 基线：当父运行时自动批准时，每个批准类都被允许
    //（与委托加固前相同）。
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    runtime.context.auto_approve = true;
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::General,
        None,
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );

    // 使用 interactive=true 调用 exec_shell 是我们通过单独的终端接管守卫阻止的；
    // 选择更简单的 write-file 路径来断言当 auto_approve 设置时
    // 批准门控已关闭。
    registry
        .execute(
            "agent_test",
            "write_file",
            json!({"path": "auto.txt", "content": "auto"}),
        )
        .await
        .expect("auto-approved parent should allow writes");
}

#[test]
fn subagent_request_budget_allows_large_write_file_arguments() {
    assert_eq!(
        SUBAGENT_RESPONSE_MAX_TOKENS, 16_384,
        "non-streaming sub-agent tool calls need enough output budget for large write_file arguments"
    );
}

#[test]
fn truncated_subagent_tool_calls_return_model_visible_errors() {
    let tool_uses = vec![(
        "toolu_write".to_string(),
        "write_file".to_string(),
        json!({"path": "report.md", "content": "partial"}),
    )];

    let results = truncated_response_tool_results(&tool_uses);

    assert_eq!(results.len(), 1);
    match &results[0] {
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
            ..
        } => {
            assert_eq!(tool_use_id, "toolu_write");
            assert_eq!(is_error, &Some(true));
            assert!(content.contains("truncated by max_tokens"));
            assert!(content.contains("write_file"));
            assert!(content.contains("smaller writes"));
        }
        other => panic!("expected tool error result, got {other:?}"),
    }
}

#[test]
fn truncated_subagent_text_response_returns_model_visible_error() {
    let results = truncated_response_text_retry_message();

    assert_eq!(results.len(), 1);
    match &results[0] {
        ContentBlock::Text { text, .. } => {
            assert!(text.contains("truncated by max_tokens"));
            assert!(text.contains("No complete tool call was available"));
            assert!(text.contains("Retry with a shorter response"));
        }
        other => panic!("expected text retry message, got {other:?}"),
    }
}

#[test]
fn consecutive_truncated_subagent_responses_are_capped() {
    let mut consecutive = 0;

    for _ in 0..MAX_CONSECUTIVE_TRUNCATED_SUBAGENT_RESPONSES {
        record_truncated_subagent_response(&mut consecutive).expect("within truncation cap");
    }

    let err = record_truncated_subagent_response(&mut consecutive)
        .expect_err("one more truncation should stop the sub-agent");
    assert!(err.to_string().contains("truncated by max_tokens"));
    assert!(err.to_string().contains("consecutive"));

    reset_truncated_subagent_responses(&mut consecutive);
    record_truncated_subagent_response(&mut consecutive).expect("reset should allow recovery");
    assert_eq!(consecutive, 1);
}

#[test]
fn child_cancellation_cascades_from_parent() {
    let parent = stub_runtime();
    let child = parent.child_runtime();
    assert!(!child.cancel_token.is_cancelled());
    parent.cancel_token.cancel();
    assert!(
        child.cancel_token.is_cancelled(),
        "parent cancel() must propagate to child via child_token()"
    );
}

#[test]
fn mailbox_propagates_through_child_runtime_chain() {
    use crate::tools::subagent::mailbox::Mailbox;
    let parent_token = CancellationToken::new();
    let (mailbox, _rx) = Mailbox::new(parent_token.clone());

    let mut parent = stub_runtime();
    parent.cancel_token = parent_token;
    parent.mailbox = Some(mailbox);

    let child = parent.child_runtime();
    let grandchild = child.child_runtime();
    assert!(parent.mailbox.is_some());
    assert!(child.mailbox.is_some(), "child inherits parent mailbox");
    assert!(
        grandchild.mailbox.is_some(),
        "grandchild inherits via the cloned Arc inside Mailbox"
    );
}

#[test]
fn subagent_rejects_interactive_shell_terminal_takeover() {
    let err = reject_subagent_terminal_takeover(
        "exec_shell",
        &serde_json::json!({
            "command": "python3 -i",
            "interactive": true
        }),
    )
    .expect_err("sub-agents must not inherit the parent terminal");

    let msg = err.to_string();
    assert!(msg.contains("cannot use exec_shell with interactive=true"));
    assert!(msg.contains("parent TUI terminal"));

    reject_subagent_terminal_takeover(
        "exec_shell",
        &serde_json::json!({
            "command": "cargo check",
            "interactive": false
        }),
    )
    .expect("non-interactive shell remains allowed");
    reject_subagent_terminal_takeover(
        "exec_shell",
        &serde_json::json!({
            "command": "cargo test",
            "background": true
        }),
    )
    .expect("background shell remains allowed");
}

#[tokio::test]
async fn mailbox_close_as_cancel_propagates_to_grandchild_runtime() {
    use crate::tools::subagent::mailbox::Mailbox;
    let parent_token = CancellationToken::new();
    let (mailbox, _rx) = Mailbox::new(parent_token.clone());

    let mut parent = stub_runtime();
    parent.cancel_token = parent_token;
    parent.mailbox = Some(mailbox.clone());

    let child = parent.child_runtime();
    let grandchild = child.child_runtime();
    assert!(!grandchild.cancel_token.is_cancelled());

    // 通过*任何*克隆关闭邮箱——原始克隆或存储在运行时上的克隆。
    // 取消必须一直传播到孙级。
    mailbox.close();
    assert!(parent.cancel_token.is_cancelled());
    assert!(child.cancel_token.is_cancelled());
    assert!(
        grandchild.cancel_token.is_cancelled(),
        "close-as-cancel must propagate across max_spawn_depth=3"
    );
}

#[tokio::test]
async fn mailbox_orders_messages_from_parent_and_child_runtimes() {
    use crate::tools::subagent::mailbox::{Mailbox, MailboxMessage};
    let parent_token = CancellationToken::new();
    let (mailbox, mut rx) = Mailbox::new(parent_token.clone());

    let mut parent = stub_runtime();
    parent.cancel_token = parent_token;
    parent.mailbox = Some(mailbox);
    let child = parent.child_runtime();

    // 交错发送来自两个运行时；序列号保持单调递增。
    parent
        .mailbox
        .as_ref()
        .unwrap()
        .send(MailboxMessage::progress("parent_a", "step 1"));
    child
        .mailbox
        .as_ref()
        .unwrap()
        .send(MailboxMessage::progress("child_b", "step 1"));
    parent
        .mailbox
        .as_ref()
        .unwrap()
        .send(MailboxMessage::progress("parent_a", "step 2"));

    let drained = rx.drain();
    assert_eq!(drained.len(), 3);
    assert_eq!(drained[0].seq, 1);
    assert_eq!(drained[1].seq, 2);
    assert_eq!(drained[2].seq, 3);
    // 验证跨发布者的顺序保持。
    match (
        &drained[0].message,
        &drained[1].message,
        &drained[2].message,
    ) {
        (
            MailboxMessage::Progress { agent_id: a, .. },
            MailboxMessage::Progress { agent_id: b, .. },
            MailboxMessage::Progress { agent_id: c, .. },
        ) => {
            assert_eq!(a, "parent_a");
            assert_eq!(b, "child_b");
            assert_eq!(c, "parent_a");
        }
        other => panic!("unexpected message order: {other:?}"),
    }
}

#[test]
fn persisted_empty_allowed_tools_loads_as_full_inheritance() {
    // 向后兼容：使用空 Vec 持久化的 v0.6.5 会话
    //（或未缩小的 v0.6.6 会话）在重启时应加载为 None，
    // 表示完全继承。
    let dir = tempdir().unwrap();
    let state_path = dir.path().join("subagents.v1.json");
    let payload = serde_json::json!({
        "schema_version": SUBAGENT_STATE_SCHEMA_VERSION,
        "agents": [{
            "id": "agent_test",
            "agent_type": "general",
            "prompt": "p",
            "assignment": { "objective": "p" },
            "status": "Completed",
            "result": null,
            "steps_taken": 0,
            "duration_ms": 0,
            "allowed_tools": [],
            "updated_at_ms": 0
        }]
    });
    std::fs::write(&state_path, payload.to_string()).unwrap();

    let mut manager = SubAgentManager::new(dir.path().to_path_buf(), 5).with_state_path(state_path);
    manager.load_state().expect("load should succeed");
    let agent = manager.agents.get("agent_test").expect("loaded agent");
    assert!(
        agent.allowed_tools.is_none(),
        "empty Vec on disk → None (full inheritance)"
    );
}

#[test]
fn persisted_non_empty_allowed_tools_loads_as_narrow() {
    // 另一种向后兼容：使用显式缩小列表持久化的 v0.6.5 会话
    // 在重新加载时保留该列表。
    let dir = tempdir().unwrap();
    let state_path = dir.path().join("subagents.v1.json");
    let payload = serde_json::json!({
        "schema_version": SUBAGENT_STATE_SCHEMA_VERSION,
        "agents": [{
            "id": "agent_narrow",
            "agent_type": "custom",
            "prompt": "p",
            "assignment": { "objective": "p" },
            "status": "Completed",
            "result": null,
            "steps_taken": 0,
            "duration_ms": 0,
            "allowed_tools": ["read_file", "list_dir"],
            "updated_at_ms": 0
        }]
    });
    std::fs::write(&state_path, payload.to_string()).unwrap();

    let mut manager = SubAgentManager::new(dir.path().to_path_buf(), 5).with_state_path(state_path);
    manager.load_state().expect("load should succeed");
    let agent = manager.agents.get("agent_narrow").expect("loaded agent");
    assert_eq!(
        agent.allowed_tools.as_deref(),
        Some(&["read_file".to_string(), "list_dir".to_string()][..]),
        "non-empty Vec → Some(list), narrow scope preserved"
    );
}

/// 构建一个最小的 SubAgentRuntime，用于测试纯运行时辅助函数
///（深度、取消、child_runtime）。不构造真实的 HTTP 客户端——
/// 调用 runtime.client 会失败，但此处测试的辅助函数不会调用它。
///
fn stub_runtime() -> SubAgentRuntime {
    use tokio_util::sync::CancellationToken;

    let workspace = std::env::temp_dir().join("codewhale-test-stub");
    let context = ToolContext::new(workspace.clone());
    SubAgentRuntime {
        client: stub_client(),
        api_config: None,
        model: "deepseek-v4-flash".to_string(),
        auto_model: false,
        reasoning_effort: None,
        reasoning_effort_auto: false,
        role_models: std::collections::HashMap::new(),
        fleet_roster: std::sync::Arc::new(crate::fleet::roster::FleetRoster::built_ins_only()),
        context,
        allow_shell: true,
        accept_edits: false,
        agent_tool_surface_options: AgentToolSurfaceOptions::new(ShellPolicy::Full),
        worker_profile: WorkerRuntimeProfile::for_role(SubAgentType::General),
        event_tx: None,
        manager: new_shared_subagent_manager(workspace, 5),
        spawn_depth: 0,
        max_spawn_depth: DEFAULT_MAX_SPAWN_DEPTH,
        cancel_token: CancellationToken::new(),
        mailbox: None,
        parent_agent_id: None,
        parent_completion_tx: None,
        fork_context: None,
        parent_mode: crate::tui::app::AppMode::Agent,
        mcp_pool: None,
        step_api_timeout: DEFAULT_STEP_API_TIMEOUT,
        tool_timeout: DEFAULT_TOOL_TIMEOUT,
        speech_output_dir: None,
        todos: crate::tools::todo::new_shared_todo_list(),
    }
}

/// 最小的桩客户端。下面的测试辅助函数仅检查结构体字段
///（depth、cancel_token、context）；它们不调用网络。
/// 我们需要*某个* DeepSeekClient，因为 SubAgentRuntime.client 不是
/// Option<...>。Config::default() 就足够了——DeepSeekClient::new
/// 仅验证 API 密钥字段存在，而非密钥有效。
fn stub_runtime_for_provider(provider: &str) -> SubAgentRuntime {
    let mut runtime = stub_runtime();
    runtime.client = stub_client_for_provider(provider);
    runtime
}

fn stub_client_for_provider(provider: &str) -> DeepSeekClient {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut providers = crate::config::ProvidersConfig::default();
    match provider {
        "moonshot" => {
            providers.moonshot = crate::config::ProviderConfig {
                api_key: Some("test-key".to_string()),
                ..Default::default()
            };
        }
        "openrouter" => {
            providers.openrouter = crate::config::ProviderConfig {
                api_key: Some("test-key".to_string()),
                ..Default::default()
            };
        }
        "zai" => {
            providers.zai = crate::config::ProviderConfig {
                api_key: Some("test-key".to_string()),
                ..Default::default()
            };
        }
        // OpenAI Codex（ChatGPT 后端）。测试快速通道推理规则：
        // GPT-5.5 子级保持在 GPT-5.5 并解析为 Low 推理。
        "openai-codex" => {
            providers.openai_codex = crate::config::ProviderConfig {
                api_key: Some("test-key".to_string()),
                ..Default::default()
            };
        }
        // Ollama 无需密钥（本地运行时）；根据需要扩展每个提供者。
        "ollama" => {}
        "sakana" => {
            providers.sakana = crate::config::ProviderConfig {
                api_key: Some("test-key".to_string()),
                ..Default::default()
            };
        }
        other => panic!("extend stub_client_for_provider for provider {other}"),
    }
    let config = crate::config::Config {
        api_key: Some("test-key".to_string()),
        provider: Some(provider.to_string()),
        providers: Some(providers),
        ..crate::config::Config::default()
    };
    DeepSeekClient::new(&config).expect("stub client should construct")
}

fn stub_client() -> DeepSeekClient {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let config = crate::config::Config {
        api_key: Some("test-key".to_string()),
        ..crate::config::Config::default()
    };
    DeepSeekClient::new(&config).expect("stub client should construct")
}

// ---- #4193：交互式 TUI 进程内派生遵循配置文件的固定提供者 ----

/// 包含两个完全配置的提供者的 Config，每个在不同的主机上，以便
/// 测试可以证明子客户端实际重新指向：deepseek 是会话路由，
/// zai 是固定路由。使用提供者作用域的密钥/基础 URL
///（根 api_key 故意未设置），以便 deepseek_api_key/deepseek_base_url
/// 独立解析每个提供者。
fn cross_provider_config() -> crate::config::Config {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut custom = std::collections::HashMap::new();
    custom.insert(
        "lm-studio".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            api_key: Some("lm-studio-key".to_string()),
            base_url: Some("http://127.0.0.1:1234/v1".to_string()),
            model: Some("qwen-2.5-7b".to_string()),
            ..Default::default()
        },
    );
    let providers = crate::config::ProvidersConfig {
        deepseek: crate::config::ProviderConfig {
            api_key: Some("session-key".to_string()),
            base_url: Some("https://session-provider.example.com/v1".to_string()),
            ..Default::default()
        },
        zai: crate::config::ProviderConfig {
            api_key: Some("pinned-key".to_string()),
            base_url: Some("https://pinned-provider.example.com/v1".to_string()),
            ..Default::default()
        },
        custom,
        ..crate::config::ProvidersConfig::default()
    };
    crate::config::Config {
        provider: Some("deepseek".to_string()),
        providers: Some(providers),
        ..crate::config::Config::default()
    }
}

/// 在 deepseek 上运行的会话运行时，注入跨提供者的 Config，
/// 完全像引擎通过 with_api_config 配线那样。
fn cross_provider_runtime() -> SubAgentRuntime {
    let config = cross_provider_config();
    let client = DeepSeekClient::new(&config).expect("session client builds");
    let mut runtime = stub_runtime().with_api_config(config);
    runtime.client = client;
    runtime
}

/// 名册成员，其配置文件显式固定 provider（+ 任意 model），
/// 镜像磁盘上的 [fleet] 配置文件形状。
fn member_pinning_provider(provider: &str, model: &str) -> crate::fleet::profile::AgentProfile {
    let mut profile = custom_fleet_profile("worker");
    profile.provider = Some(provider.to_string());
    profile.model = Some(model.to_string());
    crate::fleet::profile::AgentProfile {
        id: format!("{provider}-worker"),
        display_name: Some(format!("{provider} worker")),
        description: None,
        profile,
        source: std::path::PathBuf::from(format!("{provider}-worker.toml")),
        origin: crate::fleet::roster::ProfileOrigin::Workspace,
    }
}

#[test]
fn spawn_child_client_targets_profile_pinned_provider() {
    // 会话在 DeepSeek 上运行；名册成员固定到 Z.ai。
    // 进程内子级必须向 Z.ai 客户端（Z.ai 基础 URL + 凭证）发出请求，
    // 而非共享的会话 DeepSeek 客户端（#4193 验收标准）。
    let runtime = cross_provider_runtime();
    assert_eq!(
        runtime.client.api_provider(),
        crate::config::ApiProvider::Deepseek,
        "precondition: session is on DeepSeek"
    );

    let member = member_pinning_provider("zai", "glm-4.6");
    let child_client = child_client_for_member(&runtime, Some(&member))
        .expect("pinned-provider client builds when its creds are configured");

    assert_eq!(
        child_client.api_provider(),
        crate::config::ApiProvider::Zai,
        "child client must target the profile-pinned provider (#4193)"
    );
    assert!(
        child_client
            .base_url()
            .contains("pinned-provider.example.com"),
        "child must talk to the pinned provider's endpoint, got {}",
        child_client.base_url()
    );
    assert!(
        !child_client
            .base_url()
            .contains("session-provider.example.com"),
        "child must NOT reuse the session provider's endpoint (the #4093 misroute)"
    );
}

#[test]
fn spawn_child_client_targets_custom_profile_provider() {
    // #3965：LM Studio 和其他用户命名的 OpenAI 兼容提供者位于
    // [providers.<name>] 表中。配置文件固定必须保留该名称，
    // 以便子客户端解析自定义表，而不是拒绝它
    // 或静默继承 DeepSeek 会话客户端。
    let runtime = cross_provider_runtime();
    assert_eq!(
        runtime.client.api_provider(),
        crate::config::ApiProvider::Deepseek,
        "precondition: session is on DeepSeek"
    );

    let member = member_pinning_provider("lm-studio", "qwen-2.5-7b");
    let child_client = child_client_for_member(&runtime, Some(&member))
        .expect("custom provider client builds from the named provider table");

    assert_eq!(
        child_client.api_provider(),
        crate::config::ApiProvider::Custom
    );
    assert_eq!(child_client.base_url(), "http://127.0.0.1:1234/v1");
}

#[test]
fn spawn_child_client_inherits_session_provider_without_pin() {
    // 回归测试：无配置文件的成员和未固定提供者（或固定会话自有提供者）
    // 的成员保留会话客户端。无跨提供者构建、无错误路由、
    // 无来自 #4193 之前的行为变更。
    let runtime = cross_provider_runtime();

    let inherited = child_client_for_member(&runtime, None)
        .expect("profile-less spawn reuses the session client");
    assert_eq!(
        inherited.api_provider(),
        crate::config::ApiProvider::Deepseek
    );
    assert!(
        inherited
            .base_url()
            .contains("session-provider.example.com"),
        "profile-less child stays on the session endpoint, got {}",
        inherited.base_url()
    );

    // 固定与会话相同提供者的成员也保持不变。
    let same = member_pinning_provider("deepseek", "deepseek-v4-flash");
    let same_client = child_client_for_member(&runtime, Some(&same))
        .expect("same-provider pin reuses the session client");
    assert_eq!(
        same_client.api_provider(),
        crate::config::ApiProvider::Deepseek
    );
    assert!(
        same_client
            .base_url()
            .contains("session-provider.example.com")
    );
}

#[test]
fn spawn_child_client_fails_closed_when_pinned_provider_unavailable() {
    // 深度防御（#4093）：如果固定提供者的客户端无法构建
    //（此处：无会话 Config 传入），则失败于派生，
    // 而不是静默地将固定模型 ID 发送到会话提供者的端点。
    let mut runtime = cross_provider_runtime();
    runtime.api_config = None; // simulate a legacy/untethered runtime

    let member = member_pinning_provider("zai", "glm-4.6");
    // DeepSeekClient 不是 Debug，因此使用 match 而非 expect_err。
    let err = match child_client_for_member(&runtime, Some(&member)) {
        Ok(_) => panic!("must fail closed when the pinned client cannot be built"),
        Err(err) => err,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("zai"),
        "error must name the pinned provider so the failure is actionable: {msg}"
    );
}

// ---- #405 会话边界分类 ----
//
// 每个管理器分配一个新的 session_boot_id；代理在派生时标记该 ID。
// 经过*新*管理器的持久化+重新加载后，这些代理携带先前的启动 ID
// 并被分类为 from_prior_session。
// 列表默认仅显示当前会话；include_archived=true 显示
// 带有标志设置的先前会话记录。

fn insert_prior_session_agent(
    manager: &mut SubAgentManager,
    id: &str,
    status: SubAgentStatus,
    boot_id: &str,
) {
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        id.to_string(),
        SubAgentType::General,
        "old prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx,
        manager.workspace.clone(),
        boot_id.to_string(),
    );
    agent.status = status;
    agent.id = id.to_string();
    manager.agents.insert(id.to_string(), agent);
}

#[test]
fn session_boot_ids_are_unique_per_manager() {
    let a = SubAgentManager::new(PathBuf::from("."), 1);
    let b = SubAgentManager::new(PathBuf::from("."), 1);
    assert_ne!(a.session_boot_id(), b.session_boot_id());
}

#[test]
fn list_filtered_drops_prior_session_terminals_by_default() {
    let mut manager = SubAgentManager::new(PathBuf::from("."), 5);
    let current_boot = manager.session_boot_id().to_string();
    insert_prior_session_agent(
        &mut manager,
        "current_running",
        SubAgentStatus::Running,
        &current_boot,
    );
    insert_prior_session_agent(
        &mut manager,
        "prior_completed",
        SubAgentStatus::Completed,
        "boot_old_session",
    );
    insert_prior_session_agent(
        &mut manager,
        "prior_running",
        SubAgentStatus::Running,
        "boot_old_session",
    );

    let listed = manager.list_filtered(false);
    let ids: Vec<&str> = listed.iter().map(|s| s.agent_id.as_str()).collect();
    assert!(ids.contains(&"current_running"), "{ids:?}");
    assert!(
        ids.contains(&"prior_running"),
        "still-running prior-session agents stay visible: {ids:?}"
    );
    assert!(
        !ids.contains(&"prior_completed"),
        "completed prior-session agents are hidden by default: {ids:?}"
    );

    let prior = listed
        .iter()
        .find(|s| s.agent_id == "prior_running")
        .unwrap();
    assert!(prior.from_prior_session);
    let current = listed
        .iter()
        .find(|s| s.agent_id == "current_running")
        .unwrap();
    assert!(!current.from_prior_session);
}

#[test]
fn list_snapshots_refresh_git_branch_from_agent_workspace() {
    let repo = init_subagent_git_repo();
    git(repo.path(), &["checkout", "-b", "feature/agent-old"]);

    let mut manager = SubAgentManager::new(repo.path().to_path_buf(), 5);
    let current_boot = manager.session_boot_id().to_string();
    insert_prior_session_agent(
        &mut manager,
        "current_running",
        SubAgentStatus::Running,
        &current_boot,
    );

    let listed = manager.list_filtered(false);
    let agent = listed
        .iter()
        .find(|agent| agent.agent_id == "current_running")
        .expect("current agent should be listed");
    assert_eq!(agent.git_branch.as_deref(), Some("feature/agent-old"));
    assert_eq!(agent.workspace.as_deref(), Some(repo.path()));

    git(repo.path(), &["checkout", "-b", "feature/agent-new"]);

    let refreshed = manager.list_filtered(false);
    let agent = refreshed
        .iter()
        .find(|agent| agent.agent_id == "current_running")
        .expect("current agent should still be listed");
    assert_eq!(agent.git_branch.as_deref(), Some("feature/agent-new"));
}

#[test]
fn list_filtered_with_include_archived_returns_everything() {
    let mut manager = SubAgentManager::new(PathBuf::from("."), 5);
    let current_boot = manager.session_boot_id().to_string();
    insert_prior_session_agent(
        &mut manager,
        "current_done",
        SubAgentStatus::Completed,
        &current_boot,
    );
    insert_prior_session_agent(
        &mut manager,
        "prior_done",
        SubAgentStatus::Completed,
        "boot_old",
    );
    insert_prior_session_agent(
        &mut manager,
        "prior_failed",
        SubAgentStatus::Failed("boom".to_string()),
        "boot_old",
    );

    let listed = manager.list_filtered(true);
    assert_eq!(listed.len(), 3, "{listed:?}");
    let prior = listed.iter().find(|s| s.agent_id == "prior_done").unwrap();
    assert!(prior.from_prior_session);
    let current = listed
        .iter()
        .find(|s| s.agent_id == "current_done")
        .unwrap();
    assert!(!current.from_prior_session);
}

#[test]
fn agents_with_empty_boot_id_classify_as_prior_session() {
    // #405 之前持久化的记录因 #[serde(default)]
    // 而具有空的 session_boot_id。
    // 管理器将这些视为与不匹配的 ID 相同——即先前的会话。
    let mut manager = SubAgentManager::new(PathBuf::from("."), 5);
    insert_prior_session_agent(&mut manager, "legacy", SubAgentStatus::Completed, "");

    let listed_default = manager.list_filtered(false);
    assert!(
        listed_default.iter().all(|s| s.agent_id != "legacy"),
        "legacy completed agents are hidden by default"
    );

    let listed_archived = manager.list_filtered(true);
    let legacy = listed_archived
        .iter()
        .find(|s| s.agent_id == "legacy")
        .unwrap();
    assert!(legacy.from_prior_session);
}

#[test]
fn persist_round_trip_preserves_session_boot_id() {
    let dir = tempdir().expect("tempdir");
    let state_path = dir.path().join(SUBAGENT_STATE_FILE);

    let original_boot;
    {
        let mut writer =
            SubAgentManager::new(dir.path().to_path_buf(), 2).with_state_path(state_path.clone());
        original_boot = writer.session_boot_id().to_string();
        insert_prior_session_agent(
            &mut writer,
            "agent_persist",
            SubAgentStatus::Completed,
            &original_boot,
        );
        writer
            .persist_state()
            .expect("persist round-trip should write")
            .join()
            .expect("persist thread");
    }

    // 新管理器以*不同*的启动 ID 启动并重新加载持久化状态；
    // 代理现在应被分类为先前的。
    let mut reader =
        SubAgentManager::new(dir.path().to_path_buf(), 2).with_state_path(state_path.clone());
    reader.load_state().expect("reload should succeed");
    assert_ne!(reader.session_boot_id(), original_boot);

    let listed_default = reader.list_filtered(false);
    assert!(
        !listed_default.iter().any(|s| s.agent_id == "agent_persist"),
        "completed prior-session agent hidden after reload: {listed_default:?}"
    );
    let listed_all = reader.list_filtered(true);
    let snap = listed_all
        .iter()
        .find(|s| s.agent_id == "agent_persist")
        .unwrap();
    assert!(snap.from_prior_session);
}

// === Issue #756：父级完成唤醒 ===
//
// 当代理完成时，run_subagent_task 在运行时的 parent_completion_tx 上
// 发送 SubAgentCompletion。对于根派生的代理，引擎轮询循环排空该通道；
// 对于嵌套代理，正在运行的父子代理拥有一个本地接收器，
// 并将完成注入到自己的转录中。
// 这些测试涵盖路由逻辑和无通道安全性。

fn runtime_with_depth(
    spawn_depth: u32,
    parent_completion_tx: Option<mpsc::UnboundedSender<SubAgentCompletion>>,
) -> SubAgentRuntime {
    let mut rt = stub_runtime();
    rt.spawn_depth = spawn_depth;
    rt.parent_completion_tx = parent_completion_tx;
    rt
}

#[test]
fn emit_parent_completion_fires_for_direct_child() {
    let (tx, mut rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    let runtime = runtime_with_depth(1, Some(tx));

    let sent = emit_parent_completion(&runtime, "agent_abc", "summary line\n<sentinel/>");

    assert!(sent, "depth=1 with channel wired should send");
    let received = rx.try_recv().expect("channel should have one message");
    assert_eq!(received.agent_id, "agent_abc");
    assert_eq!(received.payload, "summary line\n<sentinel/>");
    assert!(rx.try_recv().is_err(), "should be exactly one message");
}

#[test]
fn child_runtime_inherits_speech_output_dir() {
    let output_dir = PathBuf::from("configured-speech-output");
    let runtime = stub_runtime().with_speech_output_dir(Some(output_dir.clone()));

    let child = runtime.child_runtime();

    assert_eq!(child.speech_output_dir, Some(output_dir));
    assert_eq!(
        child.agent_tool_surface_options.speech_output_dir,
        Some(PathBuf::from("configured-speech-output"))
    );
}

#[test]
fn emit_parent_completion_fires_for_nested_child() {
    let (tx, mut rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    let runtime = runtime_with_depth(2, Some(tx));

    let sent = emit_parent_completion(&runtime, "agent_grandchild", "nested summary");

    assert!(sent, "depth=2 child should send to its wired parent inbox");
    let received = rx.try_recv().expect("nested completion should be routed");
    assert_eq!(received.agent_id, "agent_grandchild");
    assert_eq!(received.payload, "nested summary");
}

#[test]
fn emit_parent_completion_skips_engine_self() {
    // depth 0 是引擎本身——引擎从不在 depth 0 派生任务，
    // 但要防御意外误用。
    let (tx, mut rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    let runtime = runtime_with_depth(0, Some(tx));

    let sent = emit_parent_completion(&runtime, "agent_root", "ignored");

    assert!(
        !sent,
        "depth=0 must not fire (only depth=1 direct children)"
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn emit_parent_completion_no_channel_is_noop() {
    let runtime = runtime_with_depth(1, None);

    let sent = emit_parent_completion(&runtime, "agent_no_chan", "anything");

    assert!(
        !sent,
        "missing channel should be a silent no-op, not a panic"
    );
}

#[test]
fn emit_parent_completion_dropped_receiver_does_not_panic() {
    let (tx, rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    drop(rx);
    let runtime = runtime_with_depth(1, Some(tx));

    // send 在内部返回错误但我们丢弃它——调用者的
    // run_subagent_task 不关心引擎是否仍在监听
    //（它可能正在关闭）。
    let sent = emit_parent_completion(&runtime, "agent_orphan", "after-rx-drop");

    assert!(
        sent,
        "we still attempt the send; the engine being gone is not our problem"
    );
}

#[test]
fn terminal_results_excluding_returns_only_current_root_undelivered_agents() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = SubAgentManager::new(tmp.path().to_path_buf(), 4);
    let current_boot = manager.current_session_boot_id.clone();
    let (input_tx, _input_rx) = mpsc::unbounded_channel();

    let mut root = SubAgent::new(
        "agent_root_done".to_string(),
        SubAgentType::General,
        "root".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx.clone(),
        tmp.path().to_path_buf(),
        current_boot.clone(),
    );
    root.status = SubAgentStatus::Completed;
    root.result = Some("root result".to_string());

    let mut nested = SubAgent::new(
        "agent_nested_done".to_string(),
        SubAgentType::General,
        "nested".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx.clone(),
        tmp.path().to_path_buf(),
        current_boot,
    );
    nested.status = SubAgentStatus::Completed;

    let mut prior = SubAgent::new(
        "agent_prior_done".to_string(),
        SubAgentType::General,
        "prior".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx,
        tmp.path().to_path_buf(),
        "prior_boot".to_string(),
    );
    prior.status = SubAgentStatus::Completed;

    manager.agents.insert(root.id.clone(), root);
    manager.agents.insert(nested.id.clone(), nested);
    manager.agents.insert(prior.id.clone(), prior);

    manager.register_worker(make_worker_spec(
        "agent_root_done",
        tmp.path().to_path_buf(),
    ));
    let mut nested_spec = make_worker_spec("agent_nested_done", tmp.path().to_path_buf());
    nested_spec.parent_run_id = Some("agent_root_parent".to_string());
    manager.register_worker(nested_spec);
    manager.register_worker(make_worker_spec(
        "agent_prior_done",
        tmp.path().to_path_buf(),
    ));

    let delivered = HashSet::from(["agent_already_delivered".to_string()]);
    let results = manager.terminal_results_excluding(&delivered);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].agent_id, "agent_root_done");

    let delivered = HashSet::from(["agent_root_done".to_string()]);
    assert!(manager.terminal_results_excluding(&delivered).is_empty());
}

#[tokio::test]
async fn run_subagent_task_emits_parent_completion_before_terminal_update() {
    let manager = Arc::new(RwLock::new(SubAgentManager::new(PathBuf::from("."), 2)));
    let (task_input_tx, task_input_rx) = mpsc::unbounded_channel();
    let agent_id = "agent_noop".to_string();
    let mut agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "noop".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        task_input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    agent.status = SubAgentStatus::Running;
    manager.write().await.agents.insert(agent_id.clone(), agent);

    let (completion_tx, mut completion_rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    let mut runtime = runtime_with_depth(1, Some(completion_tx));
    runtime.manager = Arc::clone(&manager);

    let task = SubAgentTask {
        manager_handle: manager.clone(),
        runtime,
        agent_id: agent_id.clone(),
        agent_type: SubAgentType::General,
        prompt: "no-op child run".to_string(),
        assignment: make_assignment(),
        allowed_tools: None,
        fork_context: false,
        started_at: Instant::now(),
        max_steps: 0,
        token_budget: None,
        input_rx: task_input_rx,
        launch_gate: None,
    };

    let manager_lock = manager.write().await;
    let task_handle = tokio::spawn(run_subagent_task(task));

    // 当持有管理器写锁时，仅当完成在终端状态管理器更新之前发送
    // 才可以发出完成事件
    //（由 issue #1961 修复的顺序）。
    let completion = tokio::time::timeout(Duration::from_secs(1), completion_rx.recv())
        .await
        .expect("completion should be emitted while manager write lock is still held");
    let completion = completion.expect("completion channel should remain open");
    assert_eq!(completion.agent_id, agent_id);

    drop(manager_lock);
    task_handle
        .await
        .expect("run_subagent_task should complete after lock release");

    let snapshot = {
        let manager = manager.read().await;
        manager
            .get_result(&agent_id)
            .expect("completed agent should be present")
    };
    assert!(
        matches!(snapshot.status, SubAgentStatus::Failed(_)),
        "0 max_steps cannot produce a final summary, so the child must fail: {:?}",
        snapshot.status
    );
}

#[test]
fn summarize_subagent_result_diagnoses_missing_completed_payload() {
    let snap = make_snapshot(SubAgentStatus::Completed);
    let summary = summarize_subagent_result(&snap);
    assert!(
        summary.contains("no final summary"),
        "Completed without payload must not read as silent success: {summary}"
    );
}

#[test]
fn summarize_subagent_result_budget_exhaustion_is_actionable_not_raw_done() {
    let mut snap = make_snapshot(SubAgentStatus::BudgetExhausted);
    snap.result = Some("partial findings from step 1".to_string());
    let summary = summarize_subagent_result(&snap);
    assert!(summary.contains("partial output preserved"), "{summary}");
    assert!(!summary.eq("Token budget exhausted"), "{summary}");

    let empty = make_snapshot(SubAgentStatus::BudgetExhausted);
    let summary = summarize_subagent_result(&empty);
    assert!(
        summary.contains("retry with a smaller scoped task"),
        "{summary}"
    );
}

#[test]
fn child_runtime_propagates_completion_tx_for_gating() {
    // 通道通过 child_runtime() 克隆，因此后代携带它。
    // 正在运行的子代理替换传递给其嵌套工具注册表的运行时中的通道，
    // 因此此传播不能使其孤立。
    let (tx, _rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    let parent = runtime_with_depth(0, Some(tx));

    let child = parent.child_runtime();

    assert_eq!(child.spawn_depth, 1, "child increments depth");
    assert!(
        child.parent_completion_tx.is_some(),
        "child carries the wakeup channel forward"
    );
}

#[test]
fn nested_tool_runtime_routes_child_completions_to_local_inbox() {
    let (root_tx, mut root_rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    let direct_child_runtime = runtime_with_depth(1, Some(root_tx));
    let fork_context = SubAgentForkContext {
        system: None,
        messages: Vec::new(),
        structured_state_block: None,
    };

    let (tool_runtime, mut local_rx) =
        runtime_for_nested_agent_tools(&direct_child_runtime, "agent_parent", fork_context);
    let nested_child_runtime = tool_runtime.child_runtime();

    let sent = emit_parent_completion(
        &nested_child_runtime,
        "agent_nested",
        "nested child summary\n<codewhale:subagent.done>{}</codewhale:subagent.done>",
    );

    assert!(sent, "nested child should report to the local parent inbox");
    let local = local_rx
        .try_recv()
        .expect("local parent inbox receives nested completion");
    assert_eq!(local.agent_id, "agent_nested");
    assert!(
        root_rx.try_recv().is_err(),
        "root engine must not receive nested child completion directly"
    );
}

#[test]
fn subagent_completion_from_result_surfaces_step_limit_not_silent_success() {
    let snap = make_snapshot(SubAgentStatus::Failed(
        "child reached its step limit (12 steps) without returning a final summary".to_string(),
    ));
    let completion = subagent_completion_from_result(&snap);
    assert!(completion.payload.contains("step limit"), "{completion:?}");
    assert!(!completion.payload.contains("Completed (no output)"));
}

#[test]
fn subagent_completion_from_result_preserves_missing_final_summary_diagnostic() {
    let snap = make_snapshot(SubAgentStatus::Completed);
    let completion = subagent_completion_from_result(&snap);
    assert!(
        completion.payload.contains("no final summary"),
        "{completion:?}"
    );
}

#[test]
fn subagent_budget_exhaustion_completion_carries_budget_exhausted_sentinel() {
    let mut snap = make_snapshot(SubAgentStatus::BudgetExhausted);
    snap.result = Some("partial findings from step 2".to_string());
    let completion = subagent_completion_from_result(&snap);
    assert!(
        completion.payload.contains("partial output preserved"),
        "{completion:?}"
    );
    let inner = completion
        .payload
        .split("<codewhale:subagent.done>")
        .nth(1)
        .and_then(|chunk| chunk.split("</codewhale:subagent.done>").next())
        .expect("sentinel json");
    let parsed: serde_json::Value = serde_json::from_str(inner).expect("sentinel parses");
    assert_eq!(parsed["status"], "budget_exhausted");
    assert_eq!(parsed["summary_location"], "previous_line");
}

#[test]
fn subagent_completion_inlines_evidence_before_sentinel() {
    let mut snap = make_snapshot(SubAgentStatus::Completed);
    snap.result =
        Some("VERDICT: pass\n### EVIDENCE\n- src/lib.rs:1-3 — init ok\n### GAPS\nnone".to_string());
    let completion = subagent_completion_from_result(&snap);
    let evidence_pos = completion
        .payload
        .find("### EVIDENCE")
        .expect("evidence block");
    let sentinel_pos = completion
        .payload
        .find("<codewhale:subagent.done>")
        .expect("sentinel");
    assert!(evidence_pos < sentinel_pos, "evidence before sentinel");
    assert!(completion.payload.contains("src/lib.rs:1-3"));
    assert!(
        completion.payload.find("VERDICT: pass").unwrap_or(0) < evidence_pos,
        "summary before evidence"
    );
}

#[test]
fn subagent_completion_skips_empty_evidence_on_failed_child() {
    let mut snap = make_snapshot(SubAgentStatus::Failed("boom".to_string()));
    snap.result = Some("### EVIDENCE\n- should-not-appear".to_string());
    let completion = subagent_completion_from_result(&snap);
    assert!(!completion.payload.contains("### EVIDENCE"));
}

#[test]
fn child_completion_runtime_message_preserves_agent_and_provenance_guidance() {
    let message = child_completion_runtime_message(&[SubAgentCompletion {
        agent_id: "agent_nested".to_string(),
        payload: "SUMMARY\n### EVIDENCE\n- src/lib.rs:1-3".to_string(),
    }]);
    assert_eq!(message.role, "user");
    let text = match &message.content[0] {
        ContentBlock::Text { text, .. } => text,
        other => panic!("expected text block, got {other:?}"),
    };
    assert!(text.contains("child_subagent_completion"));
    assert!(text.contains("agent_id: agent_nested"));
    assert!(text.contains("cite the child agent_id and the EVIDENCE lines"));
    assert!(text.contains("src/lib.rs:1-3"));
}

#[test]
fn subagent_runtime_default_step_api_timeout_is_legacy_120s() {
    // 遗留的硬编码常量现在是默认字段值，因此现有的调用点
    // 和构造运行时未显式超时配线的测试
    // 保留其旧行为（#1806, #1808）。
    let runtime = stub_runtime();
    assert_eq!(runtime.step_api_timeout, DEFAULT_STEP_API_TIMEOUT);
    assert_eq!(
        DEFAULT_STEP_API_TIMEOUT,
        std::time::Duration::from_secs(crate::config::DEFAULT_SUBAGENT_API_TIMEOUT_SECS)
    );
}

#[test]
fn with_step_api_timeout_overrides_runtime_field() {
    let runtime = stub_runtime().with_step_api_timeout(std::time::Duration::from_secs(900));
    assert_eq!(runtime.step_api_timeout.as_secs(), 900);
}

#[test]
fn tool_timeout_defaults_to_generous_budget_and_survives_spawn() {
    // Track A 将每个工具的超时从旧的 30s（它会杀死长时间但合法的工具运行）
    // 提高到慷慨的默认值，并且该预算必须在子/后台派生克隆中
    // 保留而非恢复。
    let parent = stub_runtime();
    assert!(
        parent.tool_timeout.as_secs() >= 300,
        "per-tool timeout must be a generous (>=300s) budget, not the old 30s"
    );
    let expected = parent.tool_timeout;
    assert_eq!(parent.child_runtime().tool_timeout, expected);
    assert_eq!(parent.background_runtime().tool_timeout, expected);
}

#[test]
fn child_runtime_preserves_step_api_timeout() {
    // 真实的子代理通过 child_runtime() / background_runtime() 派生；
    // 忘记克隆超时会静默丢弃用户的配置覆盖
    // 并复活每个子步骤的 120 秒默认值。
    let parent = stub_runtime().with_step_api_timeout(std::time::Duration::from_secs(900));
    let child = parent.child_runtime();
    let background = parent.background_runtime();

    assert_eq!(
        child.step_api_timeout.as_secs(),
        900,
        "child_runtime must preserve parent's per-step timeout"
    );
    assert_eq!(
        background.step_api_timeout.as_secs(),
        900,
        "background_runtime (detached) must also preserve the parent's timeout"
    );
}

#[test]
fn subagent_completion_payload_carries_existing_sentinel_format() {
    // 载荷格式与 prompts/constitution.md 中已记录的相同：
    // 第 1 行为人类摘要，第 2 行为 <codewhale:subagent.done> 哨兵。
    // 此测试固定该格式，
    // 以便未来的重构不会静默破坏模型的解析约定。
    let mut snap = make_snapshot(SubAgentStatus::Completed);
    snap.result = Some("Found three errors.".to_string());

    let summary = summarize_subagent_result(&snap);
    let sentinel = subagent_done_sentinel("agent_test", &snap, false);
    let payload = format!("{summary}\n{sentinel}");

    let mut lines = payload.lines();
    let first = lines.next().expect("first line is summary");
    let second = lines.next().expect("second line is sentinel");
    assert!(
        !first.starts_with("<codewhale:subagent.done>"),
        "summary should not be the sentinel itself"
    );
    assert!(
        second.starts_with("<codewhale:subagent.done>"),
        "second line is the sentinel"
    );
    assert!(second.ends_with("</codewhale:subagent.done>"));
    assert!(
        second.contains("\"agent_id\":\"agent_test\""),
        "sentinel JSON includes agent_id"
    );
    assert!(
        !second.contains("Found three errors."),
        "sentinel should not duplicate the human summary line"
    );
}

/// #2683 — 验证模型面向的工具目录仅宣传规范的子代理工具
/// 绝不暴露遗留的已废弃名称。
#[test]
fn model_catalog_only_advertises_canonical_subagent_tools() {
    use crate::tools::ToolRegistryBuilder;

    let tmp = tempfile::tempdir().expect("tempdir");
    let runtime = stub_runtime();
    let manager = runtime.manager.clone();
    let ctx = crate::tools::spec::ToolContext::new(tmp.path().to_path_buf());
    let registry = ToolRegistryBuilder::new()
        .with_subagent_tools(manager, runtime)
        .build(ctx);

    let api_names: Vec<String> = registry
        .to_api_tools()
        .into_iter()
        .map(|t| t.name)
        .collect();

    assert_eq!(
        api_names
            .iter()
            .filter(|name| name.as_str() == "agent")
            .count(),
        1,
        "agent should be the only model-facing sub-agent lifecycle tool"
    );
}

// ── #3018：提供者感知的自动路由和模型验证 ─────────────────

#[tokio::test]
async fn faster_route_on_provider_without_known_sibling_stays_on_parent_model() {
    // AC：Ollama 绝不能使用 DeepSeek ID 构建请求；
    // 即使模型明确要求更快的子级，未知家族也保持在父模型上。
    //
    let mut runtime = stub_runtime_for_provider("ollama").with_auto_model(true);
    runtime.model = "qwen3:32b".to_string();

    for prompt in ["hi", "please refactor the whole auth module for security"] {
        let route = resolve_subagent_assignment_route(
            &runtime,
            None,
            prompt,
            &SubAgentType::General,
            ModelRoute::Faster,
            SubAgentThinking::Inherit,
        )
        .await;
        assert_eq!(route.model, "qwen3:32b", "prompt {prompt:?}");
        assert!(
            !route.model.contains("deepseek"),
            "no DeepSeek id may be fabricated: {route:?}"
        );
    }
}

#[test]
fn faster_route_uses_known_deepseek_and_glm_family_siblings() {
    let mut deepseek = stub_runtime();
    deepseek.model = "deepseek-v4-pro".to_string();
    let route = fallback_subagent_assignment_route(
        &deepseek,
        None,
        ModelRoute::Faster,
        SubAgentThinking::Inherit,
        "inspect one file",
    );
    assert_eq!(route.model, "deepseek-v4-flash");

    let mut zai = stub_runtime_for_provider("zai");
    zai.model = "GLM-5.2".to_string();
    let route = fallback_subagent_assignment_route(
        &zai,
        None,
        ModelRoute::Faster,
        SubAgentThinking::Inherit,
        "inspect docs",
    );
    // GLM-5.2 faster/explore 子级路由到 GLM-5-Turbo（同族快速兄弟），
    // 而非 GLM-5.1。
    assert_eq!(route.model, "GLM-5-Turbo");
    assert_ne!(route.model, "GLM-5.1");

    let mut openrouter = stub_runtime_for_provider("openrouter");
    openrouter.model = "z-ai/glm-5.2".to_string();
    let route = fallback_subagent_assignment_route(
        &openrouter,
        None,
        ModelRoute::Faster,
        SubAgentThinking::Inherit,
        "inspect docs",
    );
    assert_eq!(route.model, "z-ai/glm-5-turbo");
    assert_ne!(route.model, "z-ai/glm-5.1");
}

#[test]
fn inherit_route_remaps_stale_deepseek_model_for_sakana_provider() {
    let mut runtime = stub_runtime_for_provider("sakana");
    runtime.model = "deepseek-v4-flash".to_string();

    let route = fallback_subagent_assignment_route(
        &runtime,
        None,
        ModelRoute::Inherit,
        SubAgentThinking::Inherit,
        "summarize the repo layout",
    );
    assert_eq!(route.model, "deepseek-v4-flash");

    let validated = ensure_subagent_model_for_provider(&runtime, &route.model_route, route.model)
        .expect("inherit should remap to operator route");
    assert_eq!(validated, crate::config::DEFAULT_SAKANA_MODEL);
    assert!(
        !validated.contains("deepseek"),
        "Sakana inherit must not keep DeepSeek ids: {validated}"
    );
}

#[test]
fn faster_route_remaps_stale_deepseek_model_for_sakana_provider() {
    let mut runtime = stub_runtime_for_provider("sakana");
    runtime.model = "deepseek-v4-flash".to_string();

    let route = fallback_subagent_assignment_route(
        &runtime,
        None,
        ModelRoute::Faster,
        SubAgentThinking::Inherit,
        "quick scan",
    );
    let validated = ensure_subagent_model_for_provider(&runtime, &route.model_route, route.model)
        .expect("faster should remap to operator route");
    assert_eq!(validated, crate::config::DEFAULT_SAKANA_MODEL);
}

#[test]
fn fixed_route_rejects_deepseek_model_for_sakana_provider() {
    let runtime = stub_runtime_for_provider("sakana");
    let err = ensure_subagent_model_for_provider(
        &runtime,
        &ModelRoute::Fixed("deepseek-v4-flash".to_string()),
        "deepseek-v4-flash".to_string(),
    )
    .expect_err("explicit DeepSeek pin must fail before spawn");
    assert!(
        err.to_string().contains("deepseek-v4-flash"),
        "error should name the model: {err}"
    );
}

#[test]
fn normalize_requested_subagent_model_rejects_cross_namespace_for_sakana() {
    let err = normalize_requested_subagent_model(
        "deepseek-v4-flash",
        "model",
        crate::config::ApiProvider::Sakana,
    )
    .expect_err("Sakana must reject DeepSeek-only model ids at spawn");
    assert!(
        err.to_string().contains("deepseek-v4-flash"),
        "error should name the model: {err}"
    );
}

#[test]
fn gpt55_faster_route_stays_on_gpt55_with_low_reasoning() {
    // AC：GPT-5.5（OpenAI Codex）父级的 faster/explore 子级必须保持在 GPT-5.5——
    // 没有更便宜的同一提供者兄弟，因此我们绝不伪造 DeepSeek/GLM ID——
    // 解析为 Low 推理而非 Off，
    // 因为 Codex 适配器在线路上没有真正的 "off"。
    //
    // Codex 客户端在构造时验证 OAuth 凭证，
    // 因此我们在此测试期间存根 access-token 环境变量
    //（保存/恢复以避免泄漏到并行测试中）。
    let prev_token = std::env::var_os("OPENAI_CODEX_ACCESS_TOKEN");
    // Safety：此测试不与读取 OPENAI_CODEX_ACCESS_TOKEN 的其他测试并发运行，
    // 我们在下方恢复原始值。
    unsafe {
        std::env::set_var("OPENAI_CODEX_ACCESS_TOKEN", "test-token");
    }
    let mut codex = stub_runtime_for_provider("openai-codex");
    unsafe {
        match prev_token {
            Some(prev) => std::env::set_var("OPENAI_CODEX_ACCESS_TOKEN", prev),
            None => std::env::remove_var("OPENAI_CODEX_ACCESS_TOKEN"),
        }
    }
    codex.model = "gpt-5.5".to_string();
    let route = fallback_subagent_assignment_route(
        &codex,
        None,
        ModelRoute::Faster,
        SubAgentThinking::Inherit,
        "inspect one file",
    );
    assert_eq!(route.model, "gpt-5.5");
    assert!(
        !route.model.contains("deepseek"),
        "no DeepSeek id may be fabricated: {route:?}"
    );
    assert!(
        !route.model.contains("glm"),
        "no GLM id may be fabricated: {route:?}"
    );
    assert_eq!(route.reasoning_effort.as_deref(), Some("low"));
    assert_ne!(route.reasoning_effort.as_deref(), Some("off"));
}

#[test]
fn role_model_validation_accepts_provider_native_ids() {
    // AC：[subagents] worker_model = "kimi-k2.5" 在 Moonshot 上不得
    // 以 "Expected a DeepSeek model id" 失败。
    let mut runtime = stub_runtime_for_provider("moonshot");
    runtime
        .role_models
        .insert("worker".to_string(), "kimi-k2.5".to_string());

    let model = configured_model_for_role_or_type(&runtime, Some("worker"), &SubAgentType::General)
        .expect("provider-native id is accepted");
    assert_eq!(model.as_deref(), Some("kimi-k2.5"));
}

#[test]
fn role_model_validation_stays_strict_on_official_deepseek() {
    let mut runtime = stub_runtime();
    runtime
        .role_models
        .insert("worker".to_string(), "kimi-k2.5".to_string());

    let err = configured_model_for_role_or_type(&runtime, Some("worker"), &SubAgentType::General)
        .expect_err("non-DeepSeek id is rejected on the official API");
    let msg = err.to_string();
    assert!(msg.contains("kimi-k2.5"), "names the bad id: {msg}");
    assert!(
        msg.contains("deepseek-v4-pro"),
        "lists accepted ids from model_completion_names_for_provider: {msg}"
    );
}

#[test]
fn operator_model_for_subagent_enumerates_from_catalog_facade() {
    // #4116：operator 路由回退必须从目录支持的 ProviderLake 外观获取其模型，
    // 而非原始遗留表。在严格的官方 DeepSeek API 上，无效 ID 被拒绝，
    // 强制走枚举分支；选择的模型必须是外观的第一个条目
    //（证明消费者已从原始遗留路径迁移），
    // 绝不是一个编造的 ID。
    //
    crate::provider_lake::clear_live_snapshot();
    let mut runtime = stub_runtime(); // official DeepSeek API (strict validation)
    runtime.model = "definitely-not-a-real-model".to_string();

    let provider = runtime.client.api_provider();
    assert_eq!(provider, crate::config::ApiProvider::Deepseek);
    // 合理性检查：严格的提供者确实拒绝无效 ID，
    // 因此 operator_model_for_subagent 必须走枚举分支。
    assert!(crate::config::validate_route(provider, &runtime.model).is_err());

    let facade = crate::provider_lake::all_catalog_models_for_provider(provider);
    assert!(
        !facade.is_empty(),
        "expected the catalog facade to enumerate DeepSeek models"
    );

    let chosen = operator_model_for_subagent(&runtime);
    assert_eq!(
        chosen, facade[0],
        "operator model must come from the catalog-backed facade"
    );
    assert_ne!(
        chosen, "definitely-not-a-real-model",
        "operator model must not echo an invalid id"
    );
    // 无回归守卫：DeepSeek 的目录视图仍然枚举迁移前接受的每个遗留 ID
    //（此提供者的外观 ⊇ 遗留集）。
    let facade_lower: std::collections::BTreeSet<String> =
        facade.iter().map(|m| m.to_ascii_lowercase()).collect();
    for legacy in crate::config::model_completion_names_for_provider(provider) {
        assert!(
            facade_lower.contains(&legacy.to_ascii_lowercase()),
            "catalog facade dropped legacy model {legacy:?} for {provider:?}"
        );
    }
}

#[test]
fn normalize_requested_subagent_model_is_provider_aware() {
    assert_eq!(
        normalize_requested_subagent_model(
            "kimi-k2.5",
            "model",
            crate::config::ApiProvider::Moonshot
        )
        .expect("Moonshot accepts its own ids"),
        "kimi-k2.5"
    );
    assert_eq!(
        normalize_requested_subagent_model(
            "qwen3:32b",
            "model",
            crate::config::ApiProvider::Ollama
        )
        .expect("Ollama tags pass through"),
        "qwen3:32b"
    );
    assert!(
        normalize_requested_subagent_model(
            "kimi-k2.5",
            "model",
            crate::config::ApiProvider::Deepseek
        )
        .is_err(),
        "official DeepSeek API rejects foreign ids"
    );
}

// ── #3030：步骤计数器格式化 ──────────────────────────────────────────

#[test]
fn format_step_counter_hides_unbounded_sentinel() {
    // DEFAULT_MAX_STEPS 是 u32::MAX，表示"无限制"——
    // 将哨兵渲染为分母会产生 "step 16/4294967295"。
    assert_eq!(format_step_counter(16, u32::MAX), "step 16");
}

#[test]
fn format_step_counter_keeps_concrete_budgets() {
    assert_eq!(format_step_counter(3, 25), "step 3/25");
    assert_eq!(format_step_counter(0, 1), "step 0/1");
}

// ── #3095：子代理启动门控 ─────────────────────────────────────────────

#[test]
fn launch_gate_defaults_to_launch_concurrency_capped_by_max_agents() {
    let tmp = tempdir().expect("tempdir");
    let manager = SubAgentManager::new(tmp.path().to_path_buf(), 10);
    // 未设置的启动并发数现在将门控种子设为完整的代理上限。
    assert_eq!(manager.launch_gate.available_permits(), 10);

    let small = SubAgentManager::new(tmp.path().to_path_buf(), 2);
    assert_eq!(small.launch_gate.available_permits(), 2);

    let custom = SubAgentManager::new(tmp.path().to_path_buf(), 10).with_launch_concurrency(0);
    assert_eq!(custom.launch_gate.available_permits(), 1, "clamps up to 1");

    let oversized = SubAgentManager::new(tmp.path().to_path_buf(), 3).with_launch_concurrency(99);
    assert_eq!(
        oversized.launch_gate.available_permits(),
        3,
        "clamps down to max_agents"
    );
}

#[tokio::test]
async fn launch_gate_queues_extra_direct_children() {
    use tokio::sync::Semaphore;
    use tokio_util::sync::CancellationToken;

    let tmp = tempdir().expect("tempdir");
    let manager = Arc::new(RwLock::new(SubAgentManager::new(
        tmp.path().to_path_buf(),
        4,
    )));

    let (client, _calls, _bodies) = delayed_chat_client(Duration::from_millis(150), "done").await;
    let (mailbox, mut mailbox_rx) = Mailbox::new(CancellationToken::new());
    let mut runtime = stub_runtime();
    runtime.client = client;
    runtime.manager = Arc::clone(&manager);
    runtime.context = ToolContext::new(tmp.path());
    runtime.mailbox = Some(mailbox);

    let gate = Arc::new(Semaphore::new(1));
    let held_launch_permit = Arc::clone(&gate)
        .acquire_owned()
        .await
        .expect("test holds the single launch permit");
    let spawn = |agent_id: &str, gate: Option<Arc<Semaphore>>| {
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        let agent = SubAgent::new(
            agent_id.to_string(),
            SubAgentType::General,
            "Answer".to_string(),
            make_assignment(),
            "deepseek-v4-flash".to_string(),
            None,
            Some(vec![]),
            input_tx,
            tmp.path().to_path_buf(),
            "boot_test".to_string(),
        );
        let task = SubAgentTask {
            manager_handle: Arc::clone(&manager),
            runtime: runtime.clone(),
            agent_id: agent_id.to_string(),
            agent_type: SubAgentType::General,
            prompt: "Answer".to_string(),
            assignment: make_assignment(),
            allowed_tools: Some(vec![]),
            fork_context: false,
            started_at: Instant::now(),
            max_steps: 1,
            token_budget: None,
            input_rx,
            launch_gate: gate,
        };
        (agent, task)
    };

    let (agent_b, task_b) = spawn("agent_gate_b", Some(Arc::clone(&gate)));
    {
        let mut mgr = manager.write().await;
        mgr.agents.insert(agent_b.id.clone(), agent_b);
    }

    // 持有许可模拟另一个直接子级占用启动门控，
    // 无需依赖挂钟时间或调度器公平性。
    tokio::spawn(run_subagent_task(task_b));

    let mut messages = Vec::new();
    let queued = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let Some(envelope) = mailbox_rx.recv().await else {
                break;
            };
            let message = envelope.message;
            let queued_b = matches!(
                &message,
                MailboxMessage::Progress { agent_id, status }
                    if agent_id == "agent_gate_b" && status.contains("queued")
            );
            let started_b = matches!(
                &message,
                MailboxMessage::Started { agent_id, .. } if agent_id == "agent_gate_b"
            );
            messages.push(message);
            assert!(
                !started_b,
                "queued child must not start while the launch permit is held: {messages:?}"
            );
            if queued_b {
                break;
            }
        }
    })
    .await;
    assert!(
        queued.is_ok(),
        "second child must publish a visible queued reason: {messages:?}"
    );
    drop(held_launch_permit);

    let collected = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let Some(envelope) = mailbox_rx.recv().await else {
                break;
            };
            let completed_b = matches!(
                &envelope.message,
                MailboxMessage::Completed { agent_id, .. } if agent_id == "agent_gate_b"
            );
            messages.push(envelope.message);
            if completed_b {
                break;
            }
        }
    })
    .await;
    assert!(collected.is_ok(), "queued child should complete");

    let queued_b = messages.iter().position(|m| {
        matches!(
            m,
            MailboxMessage::Progress { agent_id, status }
                if agent_id == "agent_gate_b" && status.contains("queued")
        )
    });
    assert!(
        queued_b.is_some(),
        "second child must publish a visible queued reason: {messages:?}"
    );
    let queued_b = queued_b.expect("queued progress exists");

    let completed_b = messages
        .iter()
        .position(
            |m| matches!(m, MailboxMessage::Completed { agent_id, .. } if agent_id == "agent_gate_b"),
        )
        .expect("queued child completes");
    let started_b = messages
        .iter()
        .position(
            |m| matches!(m, MailboxMessage::Started { agent_id, .. } if agent_id == "agent_gate_b"),
        )
        .expect("second child eventually starts");
    assert!(
        started_b > queued_b && completed_b > started_b,
        "queued child must start only after queuing, then complete: {messages:?}"
    );
}

/// 始终以最终助手文本回复的桩聊天服务器，其 usage 报告给定的 token 数量。
/// 返回客户端和一个调用计数器，以便测试可以断言
/// 在预算上限触发之前运行了多少模型轮次。
/// 镜像 delayed_chat_client 但具有可配置的 usage 且无人工延迟。
///
async fn token_heavy_chat_client(
    prompt_tokens: u64,
    completion_tokens: u64,
    response_text: &str,
) -> (DeepSeekClient, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let response_text = response_text.to_string();
    let app = Router::new().route(
        "/{*path}",
        post({
            let calls = Arc::clone(&calls);
            let response_text = response_text.clone();
            move |Json(_body): Json<Value>| {
                let calls = Arc::clone(&calls);
                let response_text = response_text.clone();
                async move {
                    let attempt = calls.fetch_add(1, Ordering::SeqCst) + 1;
                    Json(json!({
                        "id": format!("chatcmpl-budget-{attempt}"),
                        "model": "deepseek-v4-flash",
                        "choices": [{
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": response_text
                            },
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": prompt_tokens,
                            "completion_tokens": completion_tokens,
                            "total_tokens": prompt_tokens + completion_tokens
                        }
                    }))
                }
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake chat server");
    let addr = listener.local_addr().expect("fake chat server addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let config = crate::config::Config {
        api_key: Some("test-key".to_string()),
        base_url: Some(format!("http://{addr}/v1")),
        ..crate::config::Config::default()
    };
    let client = DeepSeekClient::new(&config).expect("fake chat client");
    (client, calls)
}

/// 每个工作器 token 预算运行时测试的共享脚手架：
/// 使用给定上限针对 token_heavy_chat_client 启动一个通用工作器，
/// 返回管理器、代理 ID、调用计数器和派生任务句柄。
async fn spawn_budget_capped_worker(
    workspace: &Path,
    prompt_tokens: u64,
    completion_tokens: u64,
    token_budget: Option<u64>,
    max_steps: u32,
) -> (
    Arc<RwLock<SubAgentManager>>,
    String,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let manager = Arc::new(RwLock::new(SubAgentManager::new(
        workspace.to_path_buf(),
        2,
    )));
    let agent_id = "agent_budget_worker".to_string();
    let (task_input_tx, task_input_rx) = mpsc::unbounded_channel();
    let agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "Work within budget".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Budget".to_string()),
        Some(vec![]),
        task_input_tx,
        workspace.to_path_buf(),
        "boot_budget".to_string(),
    );
    {
        let mut manager = manager.write().await;
        manager.agents.insert(agent_id.clone(), agent);
        manager.register_worker(make_worker_spec(&agent_id, workspace.to_path_buf()));
    }

    let (client, calls) =
        token_heavy_chat_client(prompt_tokens, completion_tokens, "partial answer").await;
    let mut runtime = stub_runtime();
    runtime.client = client;
    runtime.manager = Arc::clone(&manager);
    runtime.context = ToolContext::new(workspace.to_path_buf());

    let task = SubAgentTask {
        manager_handle: Arc::clone(&manager),
        runtime: runtime.clone(),
        agent_id: agent_id.clone(),
        agent_type: SubAgentType::General,
        prompt: "Work within budget".to_string(),
        assignment: make_assignment(),
        allowed_tools: Some(vec![]),
        fork_context: false,
        started_at: Instant::now(),
        max_steps,
        token_budget,
        input_rx: task_input_rx,
        launch_gate: None,
    };
    let task_handle = tokio::spawn(run_subagent_task(task));
    (manager, agent_id, calls, task_handle)
}

#[tokio::test]
async fn worker_stops_when_per_worker_token_budget_exceeded() {
    let tmp = tempdir().expect("tempdir");
    // 100 tokens/轮（60 输入 + 40 输出）对比 50 token 上限：
    // 工作器必须在第一个模型轮次后以 BudgetExhausted 停止，
    // 而非继续运行到 max_steps。
    let (manager, agent_id, calls, task_handle) =
        spawn_budget_capped_worker(tmp.path(), 60, 40, Some(50), 4).await;

    tokio::time::timeout(Duration::from_secs(5), task_handle)
        .await
        .expect("budget-capped worker must terminate")
        .expect("task should finish");

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "worker must stop after the first over-budget turn, not run to max_steps"
    );

    let result = {
        let manager = manager.read().await;
        manager.get_result(&agent_id).expect("agent registered")
    };
    assert!(
        matches!(result.status, SubAgentStatus::BudgetExhausted),
        "expected BudgetExhausted, got {:?}",
        result.status
    );
}

#[tokio::test]
async fn worker_without_per_worker_token_budget_runs_to_completion() {
    let tmp = tempdir().expect("tempdir");
    // 无每个工作器上限：即使每轮报告 100 tokens，
    // 最终文本响应也正常完成工作器。
    let (manager, agent_id, calls, task_handle) =
        spawn_budget_capped_worker(tmp.path(), 60, 40, None, 4).await;

    tokio::time::timeout(Duration::from_secs(5), task_handle)
        .await
        .expect("uncapped worker must terminate")
        .expect("task should finish");

    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let result = {
        let manager = manager.read().await;
        manager.get_result(&agent_id).expect("agent registered")
    };
    assert!(
        matches!(result.status, SubAgentStatus::Completed),
        "uncapped worker should complete normally, got {:?}",
        result.status
    );
}

#[tokio::test]
async fn per_worker_token_budget_does_not_double_count_scope_accounting() {
    let tmp = tempdir().expect("tempdir");
    // 每个工作器运行时上限停止工作器，但作用域级别记账
    //（#3319 aggregate_budget_spent 汇总 worker_records 的 total_tokens）
    // 必须精确反映实际消耗的 token 一次——
    // 绝不因触发停止的运行时累加器而膨胀。
    let (manager, agent_id, calls, task_handle) =
        spawn_budget_capped_worker(tmp.path(), 60, 40, Some(50), 4).await;

    tokio::time::timeout(Duration::from_secs(5), task_handle)
        .await
        .expect("budget-capped worker must terminate")
        .expect("task should finish");

    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let (result, worker_record) = {
        let manager = manager.read().await;
        (
            manager.get_result(&agent_id).expect("agent registered"),
            manager.get_worker_record(&agent_id).expect("worker record"),
        )
    };
    assert!(
        matches!(result.status, SubAgentStatus::BudgetExhausted),
        "expected BudgetExhausted, got {:?}",
        result.status
    );
    // 一轮 60 输入 + 40 输出 = 100 tokens，精确计数一次。
    assert_eq!(
        worker_record.usage.total_tokens,
        Some(100),
        "scope accounting must equal the single turn's tokens, not double-count: {:?}",
        worker_record.usage
    );
}

/// 在 drop 时清除进程全局速率限制窗口，以便 panic 的测试主体
/// 不会将活动暂停泄漏到并发运行的测试中。
struct ClearRateLimitOnDrop;

impl Drop for ClearRateLimitOnDrop {
    fn drop(&mut self) {
        crate::retry_status::clear_rate_limit();
    }
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn worker_is_not_stranded_by_transient_global_rate_limit_window() {
    // 并行套件不稳定回归：rate_limit_pause_blocks_subagent_spawn
    // 打开一个 30 秒的进程全局速率限制窗口并立即关闭它。
    // 在该窗口内请求到达 send_with_retry 的工作器
    // 曾提交到睡眠整个剩余窗口而不重新检查，
    // 使上面的预算测试中的 5 秒超时失败。
    // 必须重新轮询暂停，以便已清除的窗口
    // 及时释放正在进行的请求。
    let _guard = crate::retry_status::test_guard();
    let _clear = ClearRateLimitOnDrop;
    crate::retry_status::note_rate_limit(Duration::from_secs(30));

    let tmp = tempdir().expect("tempdir");
    let (manager, agent_id, _calls, task_handle) =
        spawn_budget_capped_worker(tmp.path(), 60, 40, Some(50), 4).await;

    // 模拟并发测试完成：窗口在工作器的第一个请求
    // 已经观察到它后不久关闭。
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(250)).await;
        crate::retry_status::clear_rate_limit();
    });

    tokio::time::timeout(Duration::from_secs(5), task_handle)
        .await
        .expect("worker must not be stranded by an already-cleared rate-limit window")
        .expect("task should finish");

    let result = {
        let manager = manager.read().await;
        manager.get_result(&agent_id).expect("agent registered")
    };
    assert!(
        matches!(result.status, SubAgentStatus::BudgetExhausted),
        "expected BudgetExhausted, got {:?}",
        result.status
    );
}

/// #4217：终端工作器记录必须从持久化账本中过期，
/// 以便长期存在的会话不会永远重写多 MB 的 subagents.v1.json。
#[test]
fn cleanup_evicts_stale_terminal_worker_records_and_keeps_live_ones() {
    let tmp = tempdir().expect("tempdir");
    let state_path = tmp.path().join("subagents.v1.json");
    let mut manager =
        SubAgentManager::new(tmp.path().to_path_buf(), 4).with_state_path(state_path.clone());

    manager.register_worker(make_worker_spec("agent_old_done", tmp.path().to_path_buf()));
    manager.register_worker(make_worker_spec(
        "agent_recent_done",
        tmp.path().to_path_buf(),
    ));
    manager.register_worker(make_worker_spec(
        "agent_still_running",
        tmp.path().to_path_buf(),
    ));

    let mut old_done = make_snapshot(SubAgentStatus::Completed);
    old_done.agent_id = "agent_old_done".to_string();
    old_done.name = "agent_old_done".to_string();
    manager.complete_worker_from_result("agent_old_done", &old_done);

    let mut recent_done = make_snapshot(SubAgentStatus::Failed("boom".to_string()));
    recent_done.agent_id = "agent_recent_done".to_string();
    recent_done.name = "agent_recent_done".to_string();
    manager.complete_worker_from_result("agent_recent_done", &recent_done);

    manager.record_worker_event(
        "agent_still_running",
        AgentWorkerStatus::Running,
        Some("working".to_string()),
        Some(1),
        None,
    );

    let now_ms = epoch_millis_now();
    let two_hours_ago = now_ms.saturating_sub(2 * 60 * 60 * 1000);
    {
        let old = manager
            .worker_records
            .get_mut("agent_old_done")
            .expect("old terminal worker");
        old.completed_at_ms = Some(two_hours_ago);
        old.updated_at_ms = two_hours_ago;
    }

    // 一小时保留期匹配 cleanup 调用者使用的 COMPLETED_AGENT_RETENTION。
    let auto_cancelled = manager.cleanup(Duration::from_secs(60 * 60));
    assert_eq!(auto_cancelled, 0);

    assert!(
        manager.get_worker_record("agent_old_done").is_none(),
        "terminal worker older than retention must be evicted"
    );
    assert!(
        manager.get_worker_record("agent_recent_done").is_some(),
        "recent terminal worker must be retained"
    );
    let running = manager
        .get_worker_record("agent_still_running")
        .expect("running worker");
    assert_eq!(running.status, AgentWorkerStatus::Running);

    // 持久化修剪后的账本并确认驱逐在重新加载后仍然存在。
    manager
        .persist_state()
        .expect("persist after cleanup")
        .join()
        .expect("persist thread");
    let mut reloaded =
        SubAgentManager::new(tmp.path().to_path_buf(), 4).with_state_path(state_path);
    reloaded.load_state().expect("load pruned state");
    assert!(
        reloaded.get_worker_record("agent_old_done").is_none(),
        "eviction must survive reload of subagents.v1.json"
    );
    assert!(reloaded.get_worker_record("agent_recent_done").is_some());
    assert!(reloaded.get_worker_record("agent_still_running").is_some());
}

#[test]
fn cleanup_due_gates_write_locked_cleanup_to_a_bounded_cadence() {
    // #3803：新管理器始终到期（从未清理）；
    // 清理后直到间隔期过去才再次到期，因此侧边栏刷新
    //（Op::ListSubAgents）在期间从只读快照渲染，
    // 而不是每次请求都获取写锁。
    let tmp = tempdir().expect("tempdir");
    let mut manager = SubAgentManager::new(tmp.path().to_path_buf(), 4);

    assert!(
        manager.cleanup_due(Duration::from_secs(2)),
        "a never-cleaned manager should be due"
    );

    manager.cleanup(Duration::from_secs(3600));
    assert!(
        !manager.cleanup_due(Duration::from_secs(3600)),
        "immediately after cleanup it should not be due again within the interval"
    );
    assert!(
        manager.cleanup_due(Duration::from_secs(0)),
        "a zero interval is always due"
    );
}

// ── #3882：Fleet 扇出下有界子代理输出 ─────────────────────

/// 共享溢出测试根的序列化-恢复守卫，镜像 tools::truncate::tests 中的模式。
///
fn with_spillover_root<F: FnOnce()>(root: &std::path::Path, f: F) {
    let _guard = crate::tools::truncate::TEST_SPILLOVER_GUARD
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let prior = crate::tools::truncate::set_test_spillover_root(Some(root.to_path_buf()));
    struct Restore(Option<std::path::PathBuf>);
    impl Drop for Restore {
        fn drop(&mut self) {
            crate::tools::truncate::set_test_spillover_root(self.0.take());
        }
    }
    let _restore = Restore(prior);
    f();
}

#[test]
fn bounded_tail_messages_keeps_recent_within_budget_and_counts_omitted() {
    let messages: Vec<Message> = (0..10)
        .map(|i| text_message("user", &format!("{i}:{}", "x".repeat(10_000))))
        .collect();

    let (kept, omitted) = bounded_tail_messages(&messages, 35_000);

    assert!(!kept.is_empty());
    assert_eq!(kept.len() + omitted, messages.len());
    assert!(omitted > 0, "a 100 KB history must not fit a 35 KB budget");
    // 尾部是按顺序排列的最新切片。
    let last_kept = message_text(kept.last().expect("tail non-empty"));
    assert!(
        last_kept.starts_with("9:"),
        "kept tail must end at the newest message"
    );
    let total: usize = kept.iter().map(approximate_message_bytes).sum();
    assert!(
        total <= 35_000 + 11_000,
        "kept tail exceeds budget by more than one message: {total}"
    );
}

#[test]
fn bounded_tail_messages_always_keeps_the_final_message() {
    let messages = vec![
        text_message("user", &"a".repeat(50_000)),
        text_message("assistant", &"b".repeat(50_000)),
    ];

    let (kept, omitted) = bounded_tail_messages(&messages, 10);

    assert_eq!(
        kept.len(),
        1,
        "the newest message survives even over budget"
    );
    assert_eq!(omitted, 1);
    assert!(message_text(&kept[0]).starts_with('b'));
}

#[test]
fn checkpoints_are_byte_bounded_under_fanout_scale_output() {
    // 模拟 #3882 报告形状：工具结果是多 MB 构建日志的工作器。
    // 没有界限时，每个每步检查点克隆携带整个历史；
    // 持久化的舰队文件和每个快照进一步放大了它。
    //
    let huge = "error: expected `;`\n".repeat(120_000); // ~2.3 MB per message
    let messages: Vec<Message> = (0..6).map(|_| text_message("user", &huge)).collect();

    let checkpoint = make_checkpoint("fleet-worker-1", 6, messages.clone());

    assert_eq!(checkpoint.message_count, messages.len());
    assert!(checkpoint.omitted_messages > 0);
    assert!(
        !checkpoint.messages.is_empty(),
        "checkpoint must stay continuable"
    );
    let serialized = serde_json::to_string(&checkpoint).expect("serialize checkpoint");
    assert!(
        serialized.len() <= SUBAGENT_CHECKPOINT_MESSAGE_BUDGET_BYTES + huge.len() + 64 * 1024,
        "checkpoint JSON must be bounded, got {} bytes",
        serialized.len()
    );
    // 原始历史约为 14 MB；检查点不得携带它。
    assert!(
        serialized.len() < 4 * 1024 * 1024,
        "checkpoint JSON should be far below the raw transcript size, got {} bytes",
        serialized.len()
    );
}

#[test]
fn checkpoint_without_omitted_field_still_deserializes() {
    // v0.8.67 之前持久化的记录不携带 omitted_messages 键。
    let legacy = r#"{
        "checkpoint_id": "a:step:1:ts:1",
        "agent_id": "a",
        "continuation_handle": "agent:a:checkpoint:a:step:1:ts:1",
        "reason": "interrupted",
        "continuable": true,
        "steps_taken": 1,
        "message_count": 1,
        "created_at_ms": 1
    }"#;
    let checkpoint: SubAgentCheckpoint =
        serde_json::from_str(legacy).expect("legacy checkpoint should load");
    assert_eq!(checkpoint.omitted_messages, 0);
}

#[test]
fn subagent_tool_results_spill_to_disk_and_stay_bounded_inline() {
    let tmp = tempdir().expect("tempdir");
    with_spillover_root(tmp.path(), || {
        let raw = "cargo build noise line\n".repeat(220_000); // ~5 MB
        let raw_len = raw.len();

        let (inline, spilled) =
            bound_subagent_tool_result("fleet-worker-1", "call-42", raw.clone());

        let path = spilled.expect("multi-MB output must spill");
        // 模型可见内容被限制为头部+尾部。
        assert!(inline.len() <= crate::tools::truncate::SPILLOVER_HEAD_BYTES + 1024);
        assert!(inline.contains("Sub-agent tool output truncated"));
        assert!(inline.contains(&path.display().to_string()));
        assert!(inline.contains("read_file"));
        // 完整输出可从磁盘恢复。
        let on_disk = std::fs::read_to_string(&path).expect("spill file readable");
        assert_eq!(on_disk.len(), raw_len);

        // 小型输出原样通过，无溢出文件。
        let (small, spilled) =
            bound_subagent_tool_result("fleet-worker-1", "call-43", "ok".to_string());
        assert_eq!(small, "ok");
        assert!(spilled.is_none());

        // 过大的错误输出也受限制：子代理错误通常是完整的构建日志，
        // 不像根循环的简短错误。
        let (bounded_err, spilled) =
            bound_subagent_tool_result("fleet-worker-1", "call-44", format!("Error: {raw}"));
        assert!(spilled.is_some());
        assert!(bounded_err.len() <= crate::tools::truncate::SPILLOVER_HEAD_BYTES + 1024);
        assert!(bounded_err.starts_with("Error:"));
    });
}

#[test]
fn fanout_of_workers_with_huge_outputs_keeps_resident_state_bounded() {
    // #3882 的验收形态：多个工作器，每个发出多 MB 工具输出。
    // 模型可见内容和每个工作器检查点保持有界，
    // 同时每个完整输出可从磁盘恢复。
    let tmp = tempdir().expect("tempdir");
    with_spillover_root(tmp.path(), || {
        let huge = "warning: unused import `std::mem`\n".repeat(70_000); // ~2.4 MB
        let mut resident_bytes = 0usize;

        for worker in 0..4 {
            let agent_id = format!("fleet-worker-{worker}");
            let mut messages = Vec::new();
            for call in 0..3 {
                let (inline, spilled) =
                    bound_subagent_tool_result(&agent_id, &format!("call-{call}"), huge.clone());
                let path = spilled.expect("should spill");
                assert_eq!(
                    std::fs::read_to_string(&path).expect("readable").len(),
                    huge.len()
                );
                resident_bytes += inline.len();
                messages.push(text_message("user", &inline));
            }
            let checkpoint = make_checkpoint(&agent_id, 3, messages);
            let serialized = serde_json::to_string(&checkpoint).expect("serialize");
            assert!(
                serialized.len() <= SUBAGENT_CHECKPOINT_MESSAGE_BUDGET_BYTES + 128 * 1024,
                "worker {worker} checkpoint too large: {} bytes",
                serialized.len()
            );
            resident_bytes += serialized.len();
        }

        // 4 个工作器 × 3 次调用 × ~2.4 MB ≈ 29 MB 原始数据。
        // 有界的常驻状态必须保持在总计 2 MB 以下。
        assert!(
            resident_bytes < 2 * 1024 * 1024,
            "resident bytes not bounded: {resident_bytes}"
        );
    });
}

#[test]
fn write_json_atomic_survives_concurrent_writers() {
    use std::sync::Arc;
    // 多个线程并发持久化同一个 state.json（真实的
    // persist_state_best_effort 模式）绝不能发布撕裂的文件。
    let dir = tempdir().expect("tempdir");
    // 规范化基础路径以匹配 write_json_atomic 规范化工作区的方式
    //（在 macOS 上，tempdir 位于 /var -> /private/var 符号链接下）；
    // 否则工作区相对路径检查会拒绝它。
    let base = dir.path().canonicalize().expect("canonicalize tempdir");
    let workspace = Arc::new(base.clone());
    let path = Arc::new(base.join(".codewhale").join("subagents").join("state.json"));
    let mut handles = Vec::new();
    for i in 0..16 {
        let ws = Arc::clone(&workspace);
        let p = Arc::clone(&path);
        handles.push(std::thread::spawn(move || {
            let payload = serde_json::json!({ "writer": i, "blob": "x".repeat(8192) });
            let _ = write_json_atomic(&ws, &p, &payload);
        }));
    }
    for h in handles {
        h.join().expect("writer thread");
    }
    // 发布的文件必须是完整、有效的 JSON——而非写入一半的混合内容。
    let contents = std::fs::read_to_string(&*path).expect("read state.json");
    let parsed: serde_json::Value =
        serde_json::from_str(&contents).expect("state.json must be complete/valid JSON");
    assert!(parsed.get("writer").is_some());
    // 不留任何零散临时文件。
    let leftover: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .expect("read subagents dir")
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
        .collect();
    assert!(leftover.is_empty(), "temp files leaked: {leftover:?}");
}

// === agent(action="wait") + peek 节流 (#4097) ===

fn insert_running_agent(inner: &mut SubAgentManager, name: &str) -> String {
    let current_boot = inner.session_boot_id().to_string();
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        name.to_string(),
        SubAgentType::Explore,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx,
        PathBuf::from("."),
        current_boot,
    );
    agent.task_handle = Some(tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }));
    let agent_id = agent.id.clone();
    inner.agents.insert(agent_id.clone(), agent);
    agent_id
}

#[tokio::test]
async fn agent_wait_returns_immediately_with_no_children() {
    let manager = Arc::new(RwLock::new(SubAgentManager::new(PathBuf::from("."), 1)));
    let context = ToolContext::new(".");
    let result = wait_for_subagents_from_input(&json!({"action": "wait"}), manager, &context)
        .await
        .expect("wait with no children should succeed");
    let payload: serde_json::Value =
        serde_json::from_str(&result.content).expect("wait payload should be json");
    assert_eq!(payload["running"], json!(0));
    assert!(
        payload["settled"]
            .as_array()
            .expect("settled array")
            .is_empty()
    );
}

#[tokio::test]
async fn agent_wait_wakes_when_child_settles() {
    let mut inner = SubAgentManager::new(PathBuf::from("."), 1);
    let agent_id = insert_running_agent(&mut inner, "test_agent_wait_settles");
    let manager = Arc::new(RwLock::new(inner));

    let flip = manager.clone();
    let flip_id = agent_id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut manager = flip.write().await;
        if let Some(agent) = manager.agents.get_mut(&flip_id) {
            agent.status = SubAgentStatus::Completed;
        }
    });

    let context = ToolContext::new(".");
    let started = Instant::now();
    let result = wait_for_subagents_from_input(
        &json!({"action": "wait", "timeout_secs": 30}),
        manager,
        &context,
    )
    .await
    .expect("wait should succeed");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "wait must wake on settle, not run out the 30s timeout"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&result.content).expect("wait payload should be json");
    let settled = payload["settled"].as_array().expect("settled array");
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0]["agent_id"], json!(agent_id));
    assert_eq!(settled[0]["status"], json!("completed"));
    assert_eq!(payload["timed_out"], json!(false));
}

#[tokio::test]
async fn agent_wait_times_out_and_reports_running_child() {
    let mut inner = SubAgentManager::new(PathBuf::from("."), 1);
    let _agent_id = insert_running_agent(&mut inner, "test_agent_wait_timeout");
    let manager = Arc::new(RwLock::new(inner));

    let context = ToolContext::new(".");
    let result = wait_for_subagents_from_input(
        &json!({"action": "wait", "timeout_secs": 1}),
        manager,
        &context,
    )
    .await
    .expect("wait timeout should return a snapshot, not an error");
    let payload: serde_json::Value =
        serde_json::from_str(&result.content).expect("wait payload should be json");
    assert_eq!(payload["timed_out"], json!(true));
    assert_eq!(payload["running"], json!(1));
    assert!(
        payload["settled"]
            .as_array()
            .expect("settled array")
            .is_empty()
    );
}

#[tokio::test]
async fn agent_wait_rejects_unknown_agent_ref() {
    let manager = Arc::new(RwLock::new(SubAgentManager::new(PathBuf::from("."), 1)));
    let context = ToolContext::new(".");
    let err = wait_for_subagents_from_input(
        &json!({"action": "wait", "agent_id": "agent_missing"}),
        manager,
        &context,
    )
    .await
    .expect_err("unknown agent ref must fail fast instead of blocking");
    assert!(matches!(err, ToolError::InvalidInput { .. }));
}

#[tokio::test]
async fn agent_peek_unchanged_within_window_returns_compact_nudge() {
    let mut inner = SubAgentManager::new(PathBuf::from("."), 1);
    let agent_id = insert_running_agent(&mut inner, "test_agent_peek_throttle");
    let manager = Arc::new(RwLock::new(inner));
    let memo: Arc<std::sync::Mutex<HashMap<String, PeekMemo>>> =
        Arc::new(std::sync::Mutex::new(HashMap::new()));
    let context = ToolContext::new(".");
    let input = json!({"action": "peek", "agent_id": agent_id});

    let first = inspect_agent_from_input(&input, manager.clone(), &context, true, Some(&memo))
        .await
        .expect("first peek should succeed");
    let first_payload: serde_json::Value =
        serde_json::from_str(&first.content).expect("first peek payload should be json");
    assert!(
        first_payload.get("unchanged").is_none(),
        "first peek must return the full projection"
    );

    let second = inspect_agent_from_input(&input, manager, &context, true, Some(&memo))
        .await
        .expect("second peek should succeed");
    let second_payload: serde_json::Value =
        serde_json::from_str(&second.content).expect("second peek payload should be json");
    assert_eq!(second_payload["unchanged"], json!(true));
    assert!(
        second_payload["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("wait"),
        "nudge should point at agent(action=wait)"
    );
}

#[test]
fn agent_action_parses_wait_aliases() {
    for alias in ["wait", "join", "await", "block"] {
        assert_eq!(
            parse_agent_tool_action(&json!({"action": alias})).expect("alias should parse"),
            AgentToolAction::Wait,
        );
    }
}

// ===========================================================================
// #4042 — 子代理工具限制继承（第一阶段，从 PR #4096 由 @JayBeest 收割）。
//
//
// 这些测试验证父会话的 --disallowed-tools 通过 SubAgentRuntime → SubAgentToolRegistry
// 流入派生的子代理。
// 拒绝列表由引擎 stamped 到 worker_profile.denied_tools 上，
// 并通过 child_runtime()/background_runtime() 克隆，
// 因此从子运行时构建的注册表在 is_tool_allowed()、tools_for_model()
// 和 execute() 中强制执行它。
//
// 拒绝始终优先于允许。通配符（prefix*）和不区分大小写的匹配镜像会话端的
// command_denies_tool()。
// ===========================================================================

/// 构建一个桩运行时，在 WorkerRuntimeProfile 上设置父级的 disallowed_tools。
/// 注册表在构造时从配置文件读取拒绝列表，
/// child_runtime() 克隆配置文件使列表跨代传播。
///
fn stub_runtime_with_disallowed(disallowed: Vec<String>) -> SubAgentRuntime {
    let mut rt = stub_runtime();
    rt.worker_profile.denied_tools = disallowed;
    rt
}

/// 构建一个配线了 disallowed_tools 的 SubAgentToolRegistry。
/// 将运行时传递给 SubAgentToolRegistry::new()，以便构造函数获取
/// worker_profile.denied_tools。allowed_tools 直接转发。
fn new_registry_with_disallowed(
    runtime: SubAgentRuntime,
    allowed_tools: Option<Vec<String>>,
) -> SubAgentToolRegistry {
    SubAgentToolRegistry::new(
        runtime,
        SubAgentType::General,
        allowed_tools,
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    )
}

#[test]
fn test_disallowed_tools_inheritance_denies_tool() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime =
        stub_runtime_with_disallowed(vec!["exec_shell".to_string(), "write_file".to_string()]);
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    let registry = new_registry_with_disallowed(runtime, None);

    assert!(
        !registry.is_tool_allowed("exec_shell"),
        "exec_shell should be denied"
    );
    assert!(
        !registry.is_tool_allowed("write_file"),
        "write_file should be denied"
    );
    assert!(
        registry.is_tool_allowed("read_file"),
        "read_file should still be allowed"
    );
    assert!(
        registry.is_tool_allowed("grep_files"),
        "unrelated tools should be allowed"
    );

    let tools = registry.tools_for_model(&SubAgentType::General);
    let names: HashSet<_> = tools.into_iter().map(|t| t.name).collect();
    assert!(!names.contains("exec_shell"), "catalog excludes exec_shell");
    assert!(!names.contains("write_file"), "catalog excludes write_file");
    assert!(names.contains("read_file"), "catalog includes read_file");
}

#[test]
fn test_disallowed_tools_deny_wins_over_allow() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime_with_disallowed(vec!["exec_shell".to_string()]);
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    // exec_shell 同时位于允许列表和拒绝列表中——拒绝必须获胜。
    let registry = new_registry_with_disallowed(
        runtime,
        Some(vec!["exec_shell".to_string(), "read_file".to_string()]),
    );

    assert!(
        !registry.is_tool_allowed("exec_shell"),
        "deny must win over allow"
    );
    assert!(
        registry.is_tool_allowed("read_file"),
        "read_file is allowed and not denied"
    );

    let tools = registry.tools_for_model(&SubAgentType::General);
    let names: HashSet<_> = tools.into_iter().map(|t| t.name).collect();
    assert!(
        !names.contains("exec_shell"),
        "catalog must exclude denied tool even when allowlisted"
    );
    assert!(names.contains("read_file"), "catalog includes allowed tool");
}

#[test]
fn test_disallowed_tools_wildcard_matching() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime_with_disallowed(vec!["mcp_*".to_string()]);
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    let registry = new_registry_with_disallowed(runtime, None);

    assert!(
        !registry.is_tool_allowed("mcp_github_list_prs"),
        "mcp_* wildcard should deny all MCP tools"
    );
    assert!(
        !registry.is_tool_allowed("mcp_database_query"),
        "mcp_* wildcard denies any server prefix"
    );
    assert!(
        registry.is_tool_allowed("read_file"),
        "non-MCP tools are unaffected by mcp_* deny"
    );
}

#[test]
fn test_disallowed_tools_case_insensitive_match() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime_with_disallowed(vec!["Exec_Shell".to_string()]);
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    let registry = new_registry_with_disallowed(runtime, None);

    assert!(
        !registry.is_tool_allowed("exec_shell"),
        "case-insensitive: Exec_Shell denies exec_shell"
    );
    assert!(
        !registry.is_tool_allowed("EXEC_SHELL"),
        "case-insensitive: Exec_Shell denies EXEC_SHELL"
    );
    assert!(
        registry.is_tool_allowed("read_file"),
        "unrelated tool unaffected"
    );
}

#[test]
fn test_disallowed_tools_specific_server_wildcard() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime_with_disallowed(vec!["mcp_dangerous_*".to_string()]);
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    let registry = new_registry_with_disallowed(runtime, None);

    assert!(
        !registry.is_tool_allowed("mcp_dangerous_read"),
        "specific server wildcard denies its tools"
    );
    assert!(
        registry.is_tool_allowed("mcp_safe_query"),
        "different server prefix is not denied"
    );
}

#[test]
fn test_disallowed_tools_tools_for_model_excludes_denied() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime_with_disallowed(vec![
        "exec_shell".to_string(),
        "write_file".to_string(),
        "apply_patch".to_string(),
    ]);
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    let registry = new_registry_with_disallowed(runtime, None);

    let tools = registry.tools_for_model(&SubAgentType::General);
    let names: HashSet<_> = tools.into_iter().map(|t| t.name).collect();

    assert!(!names.contains("exec_shell"), "catalog excludes exec_shell");
    assert!(!names.contains("write_file"), "catalog excludes write_file");
    assert!(
        !names.contains("apply_patch"),
        "catalog excludes apply_patch"
    );
    assert!(names.contains("read_file"), "catalog includes read_file");
    assert!(names.contains("grep_files"), "catalog includes grep_files");
}

#[tokio::test]
async fn test_disallowed_tools_execute_rejects_denied_tool() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime_with_disallowed(vec!["exec_shell".to_string()]);
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    runtime.allow_shell = true; // remove posture as a confound
    let registry = new_registry_with_disallowed(runtime, None);

    let result = registry
        .execute("agent_test", "exec_shell", json!({"command": "echo hi"}))
        .await;
    assert!(
        result.is_err(),
        "execute must reject a tool denied by disallowed_tools"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("not allowed") || err.contains("denied"),
        "error should mention denial: {err}"
    );
}

// === 拒绝列表通过运行时克隆传播 ===

#[test]
fn test_disallowed_tools_propagates_through_child_runtime() {
    let runtime = stub_runtime_with_disallowed(vec!["exec_shell".to_string()]);
    let child = runtime.child_runtime();
    assert_eq!(
        child.worker_profile.denied_tools,
        vec!["exec_shell".to_string()],
        "child_runtime() must preserve parent's denied_tools"
    );
}

#[test]
fn test_disallowed_tools_propagates_through_background_runtime() {
    let runtime = stub_runtime_with_disallowed(vec!["write_file".to_string()]);
    let bg = runtime.background_runtime();
    assert_eq!(
        bg.worker_profile.denied_tools,
        vec!["write_file".to_string()],
        "background_runtime() must preserve parent's denied_tools"
    );
}

#[test]
fn test_disallowed_tools_across_two_generations() {
    let tmp = tempdir().expect("tempdir");
    let mut parent = stub_runtime_with_disallowed(vec!["exec_shell".to_string()]);
    parent.context = ToolContext::new(tmp.path().to_path_buf());
    let parent_registry = new_registry_with_disallowed(parent.clone(), None);
    assert!(!parent_registry.is_tool_allowed("exec_shell"));

    // 子级 A 继承自父级。
    let child_a = parent.child_runtime();
    assert_eq!(
        child_a.worker_profile.denied_tools,
        vec!["exec_shell".to_string()]
    );

    // 子级 B 继承自子级 A——相同拒绝列表。
    let mut child_b = child_a.child_runtime();
    child_b.context = ToolContext::new(tmp.path().to_path_buf());
    let b_registry = new_registry_with_disallowed(child_b, None);
    assert!(
        !b_registry.is_tool_allowed("exec_shell"),
        "third-generation sub-agent still inherits deny list"
    );
    assert!(b_registry.is_tool_allowed("read_file"));
}

// === 派生路径选择退出模拟 ===

#[test]
fn test_disallowed_tools_opt_out_clears_inherited_denies() {
    // 模拟派生路径合并：父运行时具有拒绝项，
    // 子级设置 inherit_disallowed_tools = false——继承的拒绝项被清除。
    let tmp = tempdir().expect("tempdir");
    let runtime =
        stub_runtime_with_disallowed(vec!["exec_shell".to_string(), "write_file".to_string()]);
    let mut child_runtime = runtime.child_runtime();
    child_runtime.context = ToolContext::new(tmp.path().to_path_buf());
    assert!(
        !child_runtime.worker_profile.denied_tools.is_empty(),
        "child starts with parent's denies"
    );

    // 模拟派生合并：inherit_disallowed_tools = false，无调用者拒绝。
    child_runtime.worker_profile.denied_tools.clear();

    let registry = new_registry_with_disallowed(child_runtime, None);
    assert!(
        registry.is_tool_allowed("exec_shell"),
        "exec_shell allowed after opt-out cleared parent denies"
    );
    assert!(
        registry.is_tool_allowed("write_file"),
        "write_file allowed after opt-out cleared parent denies"
    );
    assert!(registry.is_tool_allowed("read_file"));
}

#[test]
fn test_disallowed_tools_opt_out_keeps_explicit_caller_deny() {
    // 选择退出清除继承的拒绝项，但显式的调用者 disallowed_tools
    // 仍然应用（联合合并——调用者拒绝始终应用）。
    let tmp = tempdir().expect("tempdir");
    let runtime =
        stub_runtime_with_disallowed(vec!["exec_shell".to_string(), "write_file".to_string()]);
    let mut child_runtime = runtime.child_runtime();
    child_runtime.context = ToolContext::new(tmp.path().to_path_buf());

    // 模拟派生合并：inherit_disallowed_tools = false，然后调用者添加
    // ["write_file"]。
    child_runtime.worker_profile.denied_tools.clear();
    child_runtime
        .worker_profile
        .denied_tools
        .push("write_file".to_string());

    let registry = new_registry_with_disallowed(child_runtime, None);
    // 父级拒绝了 exec_shell，但选择退出清除了它 → 允许。
    assert!(
        registry.is_tool_allowed("exec_shell"),
        "exec_shell allowed (parent deny cleared by opt-out)"
    );
    // 调用者显式拒绝了 write_file → 仍被拒绝。
    assert!(
        !registry.is_tool_allowed("write_file"),
        "write_file denied by caller's explicit list"
    );
    assert!(registry.is_tool_allowed("read_file"));
}

// === parse_spawn_request disallowed_tools ===

#[test]
fn test_parse_spawn_request_reads_disallowed_tools() {
    let input = json!({
        "prompt": "do something",
        "disallowed_tools": ["exec_shell", "write_file"]
    });
    let req = parse_spawn_request(&input).expect("parse");
    assert_eq!(
        req.disallowed_tools,
        Some(vec!["exec_shell".to_string(), "write_file".to_string()])
    );
}

#[test]
fn test_parse_spawn_request_disallowed_tools_dedupes_and_trims() {
    let input = json!({
        "prompt": "do something",
        "disallowed_tools": [" exec_shell ", "exec_shell", "", "  ", "write_file"]
    });
    let req = parse_spawn_request(&input).expect("parse");
    assert_eq!(
        req.disallowed_tools,
        Some(vec!["exec_shell".to_string(), "write_file".to_string()]),
        "blanks and duplicates are dropped"
    );
}

#[test]
fn test_parse_spawn_request_disallowed_tools_defaults_to_none() {
    let input = json!({"prompt": "do something"});
    let req = parse_spawn_request(&input).expect("parse");
    assert!(
        req.disallowed_tools.is_none(),
        "disallowed_tools should be None when not provided"
    );
}

#[test]
fn test_parse_spawn_request_inherit_disallowed_tools_defaults_true() {
    let input = json!({"prompt": "do something"});
    let req = parse_spawn_request(&input).expect("parse");
    assert!(
        req.inherit_disallowed_tools,
        "inherit_disallowed_tools should default to true"
    );
}

#[test]
fn test_parse_spawn_request_inherit_disallowed_tools_explicit_false() {
    let input = json!({
        "prompt": "do something",
        "inherit_disallowed_tools": false
    });
    let req = parse_spawn_request(&input).expect("parse");
    assert!(
        !req.inherit_disallowed_tools,
        "inherit_disallowed_tools should parse an explicit false"
    );
}
