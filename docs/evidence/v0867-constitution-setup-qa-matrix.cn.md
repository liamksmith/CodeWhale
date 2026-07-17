# v0.8.67 Constitution 设置 QA 矩阵

本矩阵是 v0.8.67 constitution 优先设置通道的发布证据清单。它将 `/setup`、`/constitution`、doctor、上下文报告和文档绑定到一个共享的设置状态词汇表，而不是孤立地检查每个界面。

当前构建的自动化文本/渲染证据记录在
`docs/evidence/v0867-constitution-setup-current-build-evidence.md` 中。
引导式 constitution 输出示例记录在
`docs/evidence/v0867-guided-constitution-examples.md` 中。

## 门禁命令

在声明设置通道就绪之前运行：

```sh
cargo fmt --all -- --check
git diff --check
jq empty crates/tui/locales/en.json crates/tui/locales/es-419.json crates/tui/locales/ja.json crates/tui/locales/pt-BR.json crates/tui/locales/vi.json crates/tui/locales/zh-Hans.json
cargo test -p codewhale-tui --bin codewhale-tui --locked setup -- --nocapture
cargo test -p codewhale-tui --bin codewhale-tui --locked constitution -- --nocapture
cargo test -p codewhale-tui --bin codewhale-tui --locked context_report -- --nocapture
cargo test -p codewhale-tui --bin codewhale-tui --locked doctor_setup -- --nocapture
cargo test -p codewhale-tui --bin codewhale-tui --locked tui::onboarding -- --nocapture
RUSTFLAGS="-D warnings" cargo test -p codewhale-tui --bin codewhale-tui --locked --no-run
cargo test -p codewhale-config --lib
```

## 自动化无头探针

`scripts/v0867-setup-qa.sh` 针对隔离的临时主目录运行以下非交互式合约，遇到任何回归时以非零退出（需要 `jq`）：

```sh
scripts/v0867-setup-qa.sh                       # 如果需要则构建 release
CODEWHALE_BIN=target/release/codewhale-tui scripts/v0867-setup-qa.sh
```

它验证：`doctor --json .setup` 块的形态和 `next_actions.constitution`、已配置的密钥绝不会出现在 `doctor --json` 中、仓库的 `.codewhale/constitution.json` 在 `--context-json` 中可见、以及旧版 `WHALE.md` 正文绝不会被加载。它打印出无法覆盖的剩余人工视觉检查。这将手动验证缩减为文本快照清单中列举的视觉项。

## 隔离的本地设置

使用临时主目录，以便矩阵不会读取或变更真实安装：

```sh
tmp="$(mktemp -d)"
export CODEWHALE_HOME="$tmp/codewhale-home"
export HOME="$tmp/home"
export USERPROFILE="$tmp/home"
export DEEPSEEK_CONFIG_PATH="$CODEWHALE_HOME/config.toml"
mkdir -p "$CODEWHALE_HOME" "$HOME"
```

有用的非交互式探针：

```sh
cargo run -p codewhale-tui --locked -- doctor --json | jq '.setup'
cargo run -p codewhale-tui --locked -- doctor --context-json | jq '.entries[] | select(.source_kind | test("constitution|project_context_warning"))'
```

## 矩阵

| 场景 | 预期行为 | 证据 |
| --- | --- | --- |
| 干净主目录，内置/默认 constitution | 首次运行可通过选择语言、将 provider 就绪状态记录为就绪或需要操作、审查运行时姿态、选择内置/默认来打开设置报告完成。 | `/setup` Constitution 步骤按 `U`；`crates/tui/src/tui/setup/mod.rs::bundled_constitution_commit_marks_checkpoint_complete`；`doctor --json .setup.constitution.choice == "bundled"` |
| 干净主目录，引导式用户全局 constitution | 引导式自定义保存写入 `$CODEWHALE_HOME/constitution.json`，在 `setup_state.json` 中记录 source/validity/hash/version/authoring，并在批准第二次 `G` 之前预览渲染块。 | `crates/tui/src/tui/setup/mod.rs::guided_constitution_requires_preview_before_save`；`guided_constitution_answers_shape_preview_and_saved_payload`；`deterministic_ratification_records_guided_authoring`；`persist_user_constitution_choice_writes_constitution_and_state`；`/constitution preview` |
| 模型辅助起草提议门控 | 仅当第一个 provider/模型路由就绪（密钥/本地运行时存在）时，`A`"让模型帮你起草"操作才出现并响应；没有就绪路由时，`A` 不可见。 | `crates/tui/src/tui/setup/mod.rs::model_draft_button_visible_only_when_first_route_ready`；`model_draft_button_hidden_when_no_provider_key_or_local_runtime` |
| 模型辅助起草摄入 | 模型响应被视为不可信数据：提取第一个 JSON 对象，模式解析（丢弃未知键），净化控制字符/chameleon 字节/标签伪造，边界限制文本长度。无效、为空或失败的草稿降级到确定性路径并显示原因。 | `crates/config/src/tests.rs::untrusted_draft_rejects_invalid_json`；`untrusted_draft_drops_unknown_keys`；`untrusted_draft_sanitizes_control_characters_and_tag_forgery`；`untrusted_draft_bounds_text_fields`；`untrusted_draft_fallback_constructs_deterministic_payload_with_reason` |
| 模型起草提议失败 | 网络、超时、空响应或工具错误报告原因，保留引导路径不变，且不保存。 | `crates/tui/src/tui/setup/mod.rs::model_draft_failure_reports_reason_and_leaves_deterministic_path` |
| 引导答案调整 | 更改任何引导答案（`1-6`）会丢弃已安装的模型草稿并强制在保存前重新预览。 | `crates/tui/src/tui/setup/mod.rs::tuning_guided_answer_discards_model_draft_and_forces_preview` |
| 跳过/暂缓 constitution | 跳过记录为 deferred；暂缓状态抑制注入，`/setup` 和 `doctor` 报告 `next_actions.constitution`。 | `crates/tui/src/tui/setup/mod.rs::skip_and_retry_emit_setup_state_commits`；`doctor --json .setup.constitution.choice == "deferred"` |
| 已有 constitution 存在 | 已有 constitution 文件直接完成检查点，预览现有块，不覆盖。 | `crates/tui/src/tui/setup/mod.rs::keep_existing_constitution_completes_checkpoint_without_overwriting` |
| 仓库 constitution 发现 | `.codewhale/constitution.json` 出现在 `/constitution` 和 `doctor --context-json` 中，不注入为全局块。 | `/constitution` 概览中的仓库行；`constitution_manager_shows_repo_constitution_source`；`doctor --context-json .entries` |
| `/constitution` 管理器 | 概览列出内置、用户全局、仓库本地、AGENTS、memory/handoff 和预览操作。 | `crates/tui/src/tui/constitution/manager.rs::overview_lists_all_sources_and_actions` |
| `/constitution preview` | 预览使用与设置保存和 prompt 组装相同的确定性渲染器。 | `constitution_preview_uses_deterministic_renderer` |
| `/constitution edit`（内置） | 内置 constitution 的编辑和修复路径受保护且可审计。 | `bundled_constitution_edit_and_repair_paths_are_protected_and_auditable` |
| `/constitution explain` + `/constitution posture` | 帮助路径在不加载旧版文件的情况下描述 constitution 系统。 | 宪法帮助文本 |
| Prompt 注入 | `<codewhale_user_constitution>` 块仅对有效的用户全局 constitution 出现；内置、暂缓、无效、为空、不可读或专家覆盖状态抑制注入。 | `crates/tui/src/tui/app.rs::prompt_assembly_injects_constitution_block_only_for_valid_user_global` |
| 仓库 constitution 注入 | 仓库 constitution 保持通过 repo-law 工具门控路径本地化，不作为全局 prompt 前缀注入。 | `crates/tui/src/repo_law.rs::protected_invariants_compiled_into_tool_holds` |
| 专家覆盖 | 专家覆盖配置抑制 constitution 注入，但不清除已保存的 constitution。 | `crates/config/src/tests.rs::expert_override_suppresses_injection_without_clearing_constitution` |
| 验证报告 | 最终报告不含密钥、命名 constitution 选择、列出状态为就绪/跳过/需要操作/暂缓/已完成的步骤，并指向 `next_actions`。 | `crates/tui/src/tui/setup/mod.rs::verification_report_records_ready_after_bundled_checkpoint`；`step_result_carries_no_secret_by_construction` |
| `codewhale doctor --json` | `.setup` 块形状包含 `constitution`、`runtime_posture_source`、`steps` 和 `next_actions`；已配置的密钥绝不会出现。 | `crates/tui/src/tui/setup/mod.rs::doctor_setup_block_shape` |
| 运行时姿态 | 设置记录运行时姿态来源（constitution/默认/配置/无），不静默变更运行时策略。 | `crates/tui/src/tui/setup/mod.rs::runtime_posture_source_is_constitution_or_default_not_silent_policy` |
| Provider/模型审查（就绪路由） | 就绪的 provider/模型路由可直接继续到 constitution 步骤；审查卡显示不含密钥的摘要。 | `crates/tui/src/tui/setup/mod.rs::provider_model_review_records_ready_route_and_continues` |
| Provider/模型缺少密钥 | 设置将 provider/模型记录为 `needs_action` 并继续；最终报告指向 `/provider` 或 `/model`。 | `crates/tui/src/tui/setup/mod.rs::provider_model_review_records_missing_auth_as_needs_action`；`doctor --json .setup.next_actions.provider_model` |
| Provider 健康检查失败 | 健康探针失败的路由将 provider/模型记录为 `needs_action`，并附带不含密钥的 `health=needs action` 摘要；constitution 检查点完成不受阻，报告指向修复。 | `crates/tui/src/tui/setup/mod.rs` 健康派生（`SetupRuntimeFacts`、`provider_result`）；`provider_model_review_records_missing_auth_as_needs_action`；`first_run_ready()` 接受 needs-action |
| 迁移的旧版 `.deepseek` 配置 | 旧版 `~/.deepseek` 配置在设置写入期间保留注释和禁用的密钥；继承的设置状态从现有安装派生，不退化已配置的界面；设置仅阶段化用户全局路径。 | `crates/config/src/tests.rs::config_store_rendered_body_preserves_comments_at_legacy_deepseek_path`；`crates/config/src/setup_state.rs::derive_inherited` 测试；隔离环境如上设置 `DEEPSEEK_CONFIG_PATH` |
| 自定义 provider/模型路由 | `/model` 可以记录 provider 限定的自定义路由，而不会将其与仅活动 provider 混淆。 | `cargo test -p codewhale-tui --bin codewhale-tui --locked model_picker -- --nocapture` |
| MCP/工具已配置或跳过 | 可选工具/MCP 就绪状态绝不会阻止 constitution 检查点完成，并以共享的设置步骤状态表示。 | `/setup` Tools/MCP 行；设置过滤门禁 |
| Hotbar 默认或自定义 | Hotbar 设置保持独立于 constitution 设置；设置/hotbar 测试覆盖默认和已保存的绑定。 | `docs/evidence/hotbar-qa-matrix.md`；`cargo test -p codewhale-tui --bin codewhale-tui --locked hotbar -- --nocapture` |
| 远程/运行时跳过 | 远程运行时保持可选；跳过/暂缓状态通过 `SetupState` 记录，而不阻塞首次运行。 | `/setup` Remote Runtime 行；`skip_and_retry_emit_setup_state_commits` |
| WHALE.md 迁移 | 旧版 `WHALE.md` 被忽略，报告为需要迁移，其正文不会加载到 prompt 或上下文报告中。 | `context_report_marks_whale_md_ignored_without_loading_body`；`constitution_manager_marks_whale_md_ignored` |
| 最终设置报告不含密钥 | 报告命名 constitution 选择、provider 就绪状态、运行时姿态、跳过/暂缓/需要操作步骤，且不含原始密钥。 | `doctor --json .setup`；`verification_report_records_ready_after_bundled_checkpoint`；`step_result_carries_no_secret_by_construction` |

## 文本快照清单

在剪切发布候选时，在发布说明或 PR 证据中捕获以下片段：

1. 欢迎屏幕以"code"的双重含义（"Code means two things here"）打开，引导设置弧线（选择模型、让它起草它将遵循的 constitution、阅读并批准），并声明"Nothing becomes law until you confirm."（在你确认之前，任何内容都不会成为法则）。
2. `/setup` Provider 和 Model 卡片显示 provider、模型、认证状态和健康状态，不含密钥。
3. `/setup` Runtime Posture 卡片说明 constitution 引导不会静默更改运行时策略。
4. `/setup` Constitution 步骤显示内置/默认和引导式自定义操作，以及 — 一旦 provider 路由就绪 — 命名第一个已配置模型的 `A` 模型起草邀请。
5. `/constitution` 概览显示内置、用户全局、仓库本地、AGENTS、memory/handoff、预览和维护操作。
6. `/setup report` 或 `codewhale doctor --json | jq '.setup'` 显示 `constitution`、`runtime_posture_source`、`steps` 和 `next_actions`。
7. `doctor --context-json` 显示仓库 constitution 或 WHALE.md 迁移诊断，不含旧版文件正文。
