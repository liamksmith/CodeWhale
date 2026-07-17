# Hotbar QA 矩阵

本矩阵是 #3401 的 v0.8.66 发布门禁。它将已发布的 Hotbar MVP 绑定到可重复检查，而非将孤立的单元测试视为充分的覆盖范围。

## 支持级别

| 来源 | 支持级别 | 发布声明 |
| --- | --- | --- |
| 内置应用操作 | 支持 | 默认槽位通过 `AppAction` 或应用内状态变更来分发现有的应用路径。 |
| 斜杠命令 | 支持 | 无参数和可选参数命令通过 `commands::execute` 分发；必需参数命令预填充到输入框。 |
| MCP 工具/资源/提示 | 暂缓 | 只有在参数和审批门禁接线完成前，通过命令面板/MCP 管理器可见。 |
| 技能 | 暂缓 | 通过命令面板和斜杠技能激活可见；直接的 Hotbar 绑定推迟到激活回执接线完成。 |
| 插件 | 暂缓 | 通过 `/plugins` 可见；直接的 Hotbar 绑定推迟到插件审批门禁接线完成。 |

## 配置状态

| 场景 | 预期行为 | 证据 |
| --- | --- | --- |
| 无 hotbar 配置 | 默认槽位解析为内置的八槽栏。 | `crates/config/src/tests.rs::hotbar_defaults_when_config_is_absent` |
| 空 hotbar 配置 | `hotbar = []` 禁用所有默认槽位。 | `crates/config/src/tests.rs::hotbar_empty_array_disables_default_slots`；`crates/tui/src/config_persistence.rs::persist_hotbar_bindings_writes_empty_array_to_disable_defaults` |
| 部分配置 | 缺失的槽位渲染为空，不填充默认值。 | `crates/tui/src/tui/sidebar.rs::hotbar_panel_slots_handle_empty_partial_and_unknown_config` |
| 未知操作 | 未知的已配置操作保持可见为未知状态，而不是被静默丢弃。 | `crates/config/src/tests.rs::hotbar_validation_warns_without_dropping_unknown_actions`；`crates/tui/src/tui/sidebar.rs::hotbar_panel_slots_handle_empty_partial_and_unknown_config` |
| 自定义标签 | 已配置的标签随绑定一起渲染和持久化。 | `crates/tui/src/tui/hotbar/actions.rs::recommended_hotbar_bindings_serialize_action_ids_and_labels`；`crates/tui/src/tui/ui/tests.rs::hotbar_setup_save_persists_bindings_to_config_path` |
| 工作区覆盖 | 项目配置不会替换用户拥有的 Hotbar 绑定。 | `crates/config/src/tests.rs::project_merge_does_not_replace_user_hotbar_bindings` |
| 旧版/用户配置路径 | 全新安装写入主配置路径；现有注释在替换后保留。 | `crates/tui/src/config_persistence.rs::persist_hotbar_bindings_writes_primary_config_path_for_fresh_installs`；`crates/tui/src/config_persistence.rs::persist_hotbar_bindings_preserves_comments_and_replaces_existing_tables` |
| 持久化失败 | 实时配置和配置文件保持不变，错误被上报。 | `crates/tui/src/tui/ui/tests.rs::hotbar_setup_save_error_leaves_live_config_and_file_unchanged` |

## UI 状态

| 场景 | 预期行为 | 证据 |
| --- | --- | --- |
| 普通 TUI/输入框 | `Alt-1` 到 `Alt-8` 分发已配置槽位；裸数字保持为文本输入。 | `crates/tui/src/tui/ui/tests.rs::hotbar_alt_digit_fires_from_composer_and_sidebar_states`；`crates/tui/src/tui/ui/tests.rs::hotbar_bare_digit_inserts_text_even_when_composer_empty` |
| 隐藏/侧边栏焦点状态 | Hotbar 分发在隐藏、自动、固定和聚焦的侧边栏状态中均可用。 | `crates/tui/src/tui/ui/tests.rs::hotbar_alt_digit_fires_from_composer_and_sidebar_states` |
| 窄侧边栏 | Hotbar 面板保持固定的两行布局和有界的悬停/状态文本。 | `crates/tui/src/tui/sidebar.rs::hotbar_panel_lines_keep_two_fixed_rows_and_hover_status`；`docs/evidence/terminal-visual-regression-matrix.md` |
| 模态/覆盖层打开 | 模态、审批、选择器、决策卡和引导状态会阻止 Hotbar 数字所有权。 | `crates/tui/src/tui/ui/tests.rs::hotbar_digits_are_blocked_while_modal_or_onboarding_is_active`；`crates/tui/src/tui/ui/tests.rs::hotbar_alt_digit_is_blocked_while_inline_selectors_are_open`；`crates/tui/src/tui/ui/tests.rs::hotbar_alt_digit_is_blocked_while_decision_card_is_active` |
| 设置向导打开/保存 | 设置列出支持的来源类别、更新草稿绑定、保存并持久化。 | `crates/tui/src/tui/hotbar/setup.rs::wizard_sources_follow_registered_action_categories`；`crates/tui/src/tui/hotbar/setup.rs::wizard_save_emits_bindings_but_escape_only_closes`；`crates/tui/src/tui/ui/tests.rs::hotbar_setup_save_persists_bindings_to_config_path` |
| 重启/重新分发 | 持久化的绑定解析回配置并通过相同的分发路径解析。 | `crates/config/src/tests.rs::hotbar_tables_parse_and_round_trip`；`crates/tui/src/tui/ui/tests.rs::hotbar_dispatches_bound_slot_and_ignores_empty_slot` |

## 分发结果

| 结果 | 预期行为 | 证据 |
| --- | --- | --- |
| 在应用内处理 | 本地 UI/状态操作变更应用状态并在需要时标记重绘。 | `crates/tui/src/tui/hotbar/actions.rs::sidebar_toggle_reports_visibility_and_dispatches`；`crates/tui/src/tui/hotbar/actions.rs::trust_toggle_reports_trust_state_and_dispatches` |
| `AppAction` 返回 | 必须由事件循环处理的操作返回现有的 `AppAction`。 | `crates/tui/src/tui/hotbar/actions.rs::compact_action_emits_existing_app_action`；`crates/tui/src/tui/ui/tests.rs::hotbar_dispatches_bound_slot_and_ignores_empty_slot` |
| 输入框预填充 | 必需参数斜杠命令预填充到输入框而不是以空参数触发。 | `crates/tui/src/tui/hotbar/actions.rs::slash_hotbar_action_prefills_required_argument_command` |
| 禁用原因 | 禁用的操作从推荐中排除，如果手动绑定则报告原因。 | `crates/tui/src/tui/hotbar/actions.rs::hotbar_recommendations_exclude_disabled_actions`；`crates/tui/src/tui/ui/tests.rs::hotbar_bound_disabled_action_reports_reason_without_dispatching` |
| 未知操作 | 未知的已配置操作可见但不分发。 | `crates/tui/src/tui/sidebar.rs::hotbar_panel_slots_handle_empty_partial_and_unknown_config`；`crates/tui/src/tui/ui.rs::dispatch_hotbar_slot` |
| 审批门控/暂缓来源 | 来源显式标记为暂缓，在门禁存在之前不得注册可绑定操作。 | `crates/tui/src/tui/hotbar/actions.rs::source_descriptors_cover_dispatch_boundaries`；`crates/tui/src/tui/hotbar/actions.rs::deferred_sources_cannot_register_dispatchable_actions` |

## 发布冒烟清单

在声明 Hotbar MVP 就绪之前运行：

1. `cargo test -p codewhale-config hotbar -- --nocapture`
2. `cargo test -p codewhale-tui --bin codewhale-tui --locked hotbar::actions -- --nocapture`
3. `cargo test -p codewhale-tui --bin codewhale-tui --locked hotbar_setup -- --nocapture`
4. `cargo test -p codewhale-tui --bin codewhale-tui --locked hotbar_panel -- --nocapture`
5. `cargo test -p codewhale-tui --bin codewhale-tui --locked hotbar_alt_digit -- --nocapture`
6. `cargo test -p codewhale-tui --bin codewhale-tui --locked hotbar_dispatch -- --nocapture`

如果有发布候选二进制文件，进行手动验证：

1. 在没有 `[hotbar]` 配置的情况下启动，验证默认的八个槽位在侧边栏中渲染，并显示可见的 `Alt1` 到 `Alt8` 快捷键标签。
2. 打开 `/hotbar`，绑定一个斜杠命令，保存，重启，验证绑定已持久化。
3. 在输入框/侧边栏状态下按 `Alt-1` 到 `Alt-8`，验证只有 `Alt` 组合键触发分发。
4. 打开命令面板、斜杠菜单、设置向导、决策卡和审批模态框；验证在这些界面拥有输入所有权时 Hotbar 数字被阻止。
5. 确认 MCP、技能和插件条目通过其现有的命令面板或斜杠命令路径仍可发现，且不作为直接的 Hotbar 可绑定操作提供。
