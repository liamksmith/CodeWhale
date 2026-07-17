# 终端视觉回归矩阵

Issue：#3487

本矩阵跟踪应在无需 provider 或网络访问的情况下保持可读的确定性终端 UI 夹具。它有意聚焦于客观故障：不可读的对比度、破损的边框、被截断的关键标签、缺失的窗格、替换字符以及长行截断。

## 门禁命令

```sh
cargo test -p codewhale-tui --test palette_audit --locked
cargo test -p codewhale-tui --bin codewhale-tui --locked visual_matrix -- --nocapture
cargo test -p codewhale-tui --bin codewhale-tui --locked selected_provider_row_uses_strong_highlight -- --nocapture
cargo test -p codewhale-tui --bin codewhale-tui --locked config_view_selected_row_uses_muted_selection_highlight -- --nocapture
```

## 矩阵

| 界面 | 宽度 | 夹具 | 护栏 |
|---------|--------|---------|------------|
| 调色板对比度 | 暗色、亮色 | `crates/tui/tests/palette_audit.rs::contrast_guardrails_for_key_ui_pairs` | body、muted、warning、error、选中行、提升行和亮色调色板文本对达到 4.5:1 对比度 |
| `/model` provider 选择器 | 较窄、中等 | `provider_picker::tests::small_list_render_keeps_selected_provider_visible_after_down_navigation`、`selected_provider_row_uses_strong_highlight` | 选中的 provider 在滚动后保持可见，选中背景连续且避免亮色强调背景 |
| `/sessions` 选择器 | 72x20、120x28 | `session_picker::tests::session_picker_visual_matrix_covers_narrow_and_medium_rendering`、`session_picker_selected_row_renders_readable_selection_contrast` | 两个窗格均正常渲染，边框保持完整，长 CJK 标题使用省略号截断，无替换字符，选中行保持可见并保持可读对比度 |
| 设置/配置模态框 | 60x18、100x24 | `views::tests::localized_config_view_renders_at_narrow_width`、`config_view_selected_row_uses_muted_selection_highlight`、`config_view_keeps_scope_column_aligned_for_long_keys` | 本地化标题在窄宽度下保持完整，选中行使用柔和高亮，长标签和 CJK 作用域列保持对齐 |
| 侧边栏 hotbar/任务行 | 侧边栏单元宽度 | `sidebar::tests::hotbar_panel_lines_keep_two_fixed_rows_and_hover_status`、`hotbar_panel_slots_handle_empty_partial_and_unknown_config` | 固定行不调整大小，空/未知槽位渲染显式状态 |
| 转录/实时覆盖层 | 40x10、48x10、60x16 | `live_transcript::tests::backtrack_preview_opens_near_latest_user_not_transcript_start`、`cache_reuses_unchanged_cells_across_renders` | 覆盖层无需 provider 访问即可渲染，最近的轮次保持可见，未变更的单元格复用换行缓存 |

## 推迟的行

- 完整的子代理/Fleet 进度覆盖层截图仍在 #3480 下，因为当前测试断言模型行和实时扇出成员身份，但尚未渲染完整的窄终端 shell。
- 审批模态框破坏性审查语义在 #3466 下跟踪；视觉检查应在权限文案最终确定后添加。
- Hotbar 端到端来源覆盖在 #3401 下跟踪，待 MCP、技能和插件来源适配器落地后。
