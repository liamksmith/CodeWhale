# 旧版 `.deepseek/` 兼容路径 — 审计与迁移状态 (#3068)

CodeWhale 是从 DeepSeek-TUI 重命名而来的。为避免破坏现有安装，运行时从新的 `~/.codewhale/` 位置读取状态，但**回退**到旧版 `~/.deepseek/` 位置，并且始终**写入**到 `~/.codewhale/`。本文档审计每个旧版引用并记录保留/弃用/移除的决策，使迁移过程可审计。

## 规范解析器（新代码请使用此方式）

状态目录解析统一在 `crates/config/src/lib.rs` 中：

| 符号 | 行号 | 用途 |
|---|---|---|
| `CODEWHALE_APP_DIR = ".codewhale"` | 3428 | 规范应用目录 |
| `LEGACY_APP_DIR = ".deepseek"` | 3431 | 旧版应用目录（仅限回退） |
| `codewhale_home()` | 3437 | `~/.codewhale` |
| `legacy_deepseek_home()` | 3451 | `~/.deepseek`（旧版） |
| `resolve_state_dir(subdir)` | 3469 | **读取**路径：`~/.codewhale/<subdir>`，当仅存在旧版目录时回退到 `~/.deepseek/<subdir>` |
| `ensure_state_dir(subdir)` | 3484 | **写入**路径：始终在 `~/.codewhale/<subdir>` 下创建 |

迁移约定：读取带回退，写入到新位置。这为仍保留 `~/.deepseek/` 的用户保留了 v0.8.44 的迁移，同时将所有新写入引导到 `~/.codewhale/`。

## 逐路径决策

**对以下所有旧版引用的决策：保留为回退。** 移除 `.deepseek` 回退将导致就地升级且从未重新运行 onboarding 的用户无法使用。仅在有一个在首次运行时主动迁移 `~/.deepseek/` → `~/.codewhale/` 的发布版本以及一段弃用窗口之后，才应重新审视。

| 引用 | 是否通过 `resolve_state_dir` 路由？ | 决策 |
|---|---|---|
| `config::resolve_state_dir` / `ensure_state_dir` | 不适用（解析器自身） | 保留 — 规范 |
| `crates/tui/src/skills/mod.rs`（`~/.deepseek/skills`） | 否 — 硬编码 | 保留为回退；在后续重构中通过解析器路由 |
| `crates/tui/src/prompts.rs`（`LEGACY_HANDOFF_RELATIVE_PATH = ".deepseek/handoff.md"`） | 否 — 显式旧版常量 | 保留 — 显式旧版交接回退 |
| `crates/tui/src/workspace_trust.rs` | 否 — 硬编码 | 保留为回退；后续处理 |
| `crates/tui/src/session_manager.rs` | 否 — 硬编码 | 保留为回退；后续处理 |
| `crates/tui/src/skill_state.rs` | 否 — 硬编码 | 保留为回退；后续处理 |
| `crates/tui/src/tools/skill.rs` | 否 — 硬编码 | 保留为回退；后续处理 |
| `crates/tui/src/snapshot/mod.rs` | 否 — 硬编码 | 保留为回退；后续处理 |
| `crates/tui/src/workspace_discovery.rs` | 否 — 硬编码 | 保留为回退；后续处理 |

## 后续工作（独立的、非文档变更 — 不在 #3068 范围内）

该 issue 提到的可选整合 — 将上述硬编码的位置通过 `resolve_state_dir`/`ensure_state_dir` 路由，而不是手动拼接 `.deepseek`/`.codewhale` — 是一个小型重构，应该作为独立的 PR 提交，并为每个迁移的位置附带测试来断言读取回退 + 写入到新位置。它被有意地排除在本次审计之外，以便文档可以安全地独立提交。
