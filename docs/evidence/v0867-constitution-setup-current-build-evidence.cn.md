# v0.8.67 Constitution 设置当前构建证据

本笔记记录了 v0.8.67 constitution 优先设置通道的当前构建文本/渲染证据。它补充了
`docs/evidence/v0867-constitution-setup-qa-matrix.md`；它不是发布标签、制品或发布记录。

具有代表性的引导式输出示例（包括面向 GLM-5.2 的配置文件）记录在
`docs/evidence/v0867-guided-constitution-examples.md` 中。截至本快照，向导还提供模型辅助起草：在 Constitution
步骤按 `A` 会请求第一个已配置的模型根据引导式答案起草，经过 `UserConstitution::from_untrusted_json`
（解析/净化/边界限制）门禁和相同的批准预览 + 显式 `G` 保存。

- 日期：2026-07-02T03:54:02Z
- 分支：`claude/v0.8.67-constitution-setup-174rj9`
- HEAD：`fa7c4b055`
- 在 `Cargo.toml` 中观察到的工作区版本：`0.8.66`

## 覆盖的界面

这些检查覆盖了 #3412 发布文档请求的文本快照方面：

- `/setup` constitution 步骤在阻塞终端尺寸 `80x24`、`100x30`、`120x32` 和 `160x40` 下。
- `/setup` provider/模型就绪状态、运行时姿态、constitution 选择、引导式预览/保存、更新检查点、跳过/重试、验证报告和 zh-Hans 检查点文案。
- `/constitution` 管理器、预览、编辑/修复/内置/仓库/解释/姿态帮助路径，包括 zh-Hans 管理器/预览文案。
- 用户全局 `<codewhale_user_constitution>` 块的 prompt 注入，以及内置/暂缓或专家覆盖选择时的抑制。
- `codewhale doctor --json` 设置状态派生和持久化状态回读。
- 上下文报告 constitution/WHALE.md 迁移诊断，不加载旧版 `WHALE.md` 正文。
- 已发布设置区域设置文件的 Locale JSON 有效性。

## 已运行的命令

```sh
cargo test -p codewhale-tui --bin codewhale-tui --locked setup_wizard_is_usable_and_opaque_at_blocker_sizes -- --nocapture
```

结果：1 通过，0 失败。

```sh
cargo test -p codewhale-tui --bin codewhale-tui --locked constitution -- --nocapture
```

结果：42 通过，0 失败。

```sh
cargo test -p codewhale-tui --bin codewhale-tui --locked setup -- --nocapture
```

结果：127 通过，0 失败（包括模型起草请求/摄入、批准、调整时丢弃和来源归属测试）。

```sh
cargo test -p codewhale-tui --bin codewhale-tui --locked context_report -- --nocapture
```

结果：10 通过，0 失败。

```sh
cargo test -p codewhale-tui --bin codewhale-tui --locked doctor_setup -- --nocapture
```

结果：5 通过，0 失败。

```sh
cargo test -p codewhale-config --lib
```

结果：342 通过，0 失败（包括 `untrusted_draft_*` 摄入门禁测试和 `constitution_authoring` 往返/旧版加载测试）。

```sh
cargo test -p codewhale-tui --bin codewhale-tui --locked verification_report -- --nocapture
```

结果：2 通过，0 失败。

```sh
jq empty crates/tui/locales/en.json crates/tui/locales/es-419.json crates/tui/locales/ja.json crates/tui/locales/pt-BR.json crates/tui/locales/vi.json crates/tui/locales/zh-Hans.json crates/tui/locales/zh-Hant.json
```

结果：通过。

## 本地化覆盖和回退（zh-Hant）

v0.8.67 的设置/constitution 界面对 `en` 和 `zh-Hans` 完全本地化（每种 545+ 消息键）。`zh-Hant` 提供部分目录（约 162 个键），尚未包含设置/constitution 字符串。

针对 zh-Hant 未携带字符串的已记录回退行为：运行时消息加载器使用 `i18n!("locales", fallback = ["en"])` 初始化（`crates/tui/src/main.rs`），因此未翻译的 zh-Hant 键以 **英文** 渲染。`crates/tui/src/localization.rs` 中的 `LocaleSpec { fallback: "zh-Hans" }` 条目仅为描述性元数据，未接入消息解析；每个语言环境的 zh-Hant → zh-Hans 链将是一个行为变更，有意在 v0.8.67 中排除。这满足了 #3412/#3794 对 zh-Hant 的"已记录回退"验收替代方案；完整的 zh-Hant 覆盖（或 zh-Hans 回退链）仍作为后续本地化工作保持开放。

## 发布编排快照（2026-07-01）

- 分支：`claude/v0.8.67-constitution-setup-174rj9`
- HEAD：`19971477a`
- 在 `Cargo.toml` 中观察到的工作区版本：`0.8.66`

自上次快照以来落地：`008987464`（constitution 优先设置文案润色 — 欢迎双重含义弧线、仪式性起草邀请、权限和限制加连续性而非记忆的批准框架、起草提示引导）、`b58dcf99c`（#3884：子代理故障记录携带完整分类错误链）、`b4ca8b539`（#3883：持久审查基于操作种类的底限键；常规 YOLO 后台工作不再提示；破坏性/发布保留保持）、`19971477a`（QA 矩阵行：失败的健康检查和旧版 `.deepseek` 配置迁移）。

此 HEAD 的门禁结果（精确观察到的计数）：

```
cargo fmt --all -- --check                     PASS
git diff --check                               PASS
jq empty crates/tui/locales/*.json             PASS（7 个文件，包括 zh-Hant）
cargo test -p codewhale-config --lib           342 通过；0 失败
cargo test ... --locked setup                  127 通过；0 失败
cargo test ... --locked constitution            42 通过；0 失败
cargo test ... --locked context_report          10 通过；0 失败
cargo test ... --locked doctor_setup             5 通过；0 失败
cargo test ... --locked tui::onboarding          9 通过；0 失败
RUSTFLAGS="-D warnings" cargo test ... --no-run  clean（1 分 07 秒）
cargo build --release -p codewhale-cli -p codewhale-tui  clean（1 分 02 秒）
```

针对两个修复提交额外运行的套件：yolo_mode 10、auto_review 31、approval 163、engine 272、widgets 186、command_safety 58、client 240、retry 41、subagent 256 — 全部通过。

## 本地化覆盖和回退（其他语言环境）

与 zh-Hant 类似，`es-419`、`ja`、`pt-BR` 和 `vi` 目录尚未携带 v0.8.67 的设置/constitution 键（其目录早于此通道）；这些字符串通过相同的 `fallback = ["en"]` 加载器以 **英文** 渲染。这是 v0.8.67 的已接受状态；将设置/constitution 覆盖扩展到其余完整语言环境是后续本地化工作（在 #3792/#3793 本地化残余下跟踪）。

## 增强通道快照（2026-07-01，通宵）

- 分支：`claude/v0.8.67-constitution-setup-174rj9`
- HEAD：`d73875b7a`（领先 origin 19 个提交）

在发布编排快照之后落地：`d46047a74`（保留现有 constitution 检查点完成，#3794）、`ed6b21be1`（平静/紧凑的基于风险审批提示 + `agent` 工具分类）、`41d26c774` / `c57062077` / `7f82737b7`（#3757 启动和 @mention 性能 + 启动里程碑追踪）、`a429f82c2`（路由标识固定修复了四个机器依赖的测试失败，在交接 HEAD 处验证为预先存在的）、`d73875b7a`（在起草→预览→批准门禁后的模型起草 fleet 配置文件 — constitution 管道的第二个消费者）以及网站第一阶段（`28efab313`、`5a38e2059`、`d3d1d5e25`：实时星标徽章/版本导航、en+zh 的 constitution 论文英雄区和基于真实会话轨迹的动画终端播放器）。

此 HEAD 的最终测试集（精确观察到的计数）：

```
cargo test -p codewhale-tui --bin codewhale-tui --locked   5676 通过；0 失败；2 忽略
cargo test -p codewhale-config --lib                        342 通过；0 失败
cargo build --release -p codewhale-cli -p codewhale-tui     clean（47.94 秒）
cargo fmt --all -- --check                                  PASS
cd web && npm run build                                     预渲染所有语言环境路由
```

## 通宵审查 + 加固快照（2026-07-02）

- 分支：`claude/v0.8.67-constitution-setup-174rj9`
- HEAD：`44eb7e935`（领先 origin 约 35 个提交；分支差异仅涉及 `crates/`、`web/`、`docs/`、`scripts/`、`Cargo.*`）

对通宵差异运行了五镜头对抗性 supercode 审查（34 个代理；正确性 / constitution 安全不变量 / 性能 / UX / 测试充分性，每个发现由独立质疑者驳斥或确认）。确认的发现已修复：

- **安全底限销毁器缺口**（`88c545f2b`）：#3883 底限收窄已停止在 YOLO 后台中保留 `dd`-to-device / `mkfs` / `shred` / `wipefs` / 强制递归删除绝对系统路径；`segment_is_device_or_filesystem_destroyer` 恢复这些保留。
- **仓库法则作为机制**（`88c545f2b`）：`.codewhale/constitution.json` 的 `protected_invariants` 现在可携带路径 glob + `action`（ask|block），编译为工具门禁中的写入保留（`crates/tui/src/repo_law.rs`），仅收紧，不可被模式绕过，附带命名不变量的回执。
- **启动看门人竞争条件**（`007ce2680`）：后台会话清理不再与会话恢复竞争（跳过/排除恢复的 id）。
- **事件循环冻结类已关闭**：fleet-profile（`138dbad1b`）和 constitution（`58aefa392`）模型起草器已从内联 await 转移到后台 cell + poll 模式；UI 在起草期间保持交互。
- **Fleet 起草完成 + UX 对等**（`007ce2680`）：provider 就绪门控、en+zh 本地化、Enter 批准、重复 id 防护。
- **UX 文案**（`126633e78`、`e09fd46c2`）：状态分类器不再将否定成功失败涂成绿色；shell 退出码可读；锁术语替换为可操作的文案；GitHub issue 编号从 `--help` 和配置 UI 中移除；会话/模型选择器具有可操作的空状态。
- **测试充分性差距**（`432fa0fbe`）和无头 QA 探针（`scripts/v0867-setup-qa.sh`、`93b5c1e59`）已添加。
- **#3830 缺少认证交接**（`e2b32ec4c`）：因缺少密钥而失败的路由切换会打开 `/provider` 定位到该 provider 的密钥输入。

此 HEAD 的最终门禁测试集：

```
cargo fmt --all -- --check                              PASS
git diff --check                                        PASS
jq empty crates/tui/locales/*.json                      PASS（7 个文件）
cargo test -p codewhale-tui --bin codewhale-tui --locked  5686 通过；0 失败；2 忽略
cargo test -p codewhale-config --lib                    342 通过；0 失败
RUSTFLAGS="-D warnings" cargo test ... --locked --no-run  clean
cargo build --release -p codewhale-cli -p codewhale-tui   clean（47 秒）
scripts/v0867-setup-qa.sh                               9 通过；0 失败
```

## PR 推送 + 社区收集快照（2026-07-02）

- 分支已推送到 origin 作为 PR #3861（等待审查），领先 `main` 118 个提交；`Cargo.toml` 仍为 `0.8.66`。
- 对分支的五镜头对抗性审查发现并关闭了真实的仓库法则执行绕过（内部 `./`/`..` 路径规避、apply_patch 头格式、`fim_edit` 未门控）和安全底限销毁器规避（env/wrapper 前缀、引号、管道）— 全部在推送前通过测试修复。
- 起草器事件循环冻结类完全关闭（fleet + constitution）。
- 两个安全的运行时性能优化（空闲离线队列克隆已跳过；工具输出哈希一次）。结构性渲染循环项记录在本地路线图中，供人工监督的 TUI QA 使用，而非盲目变更。
- **社区收集（v0.8.67）：** 四个经过审查的 PR 已收集并署名（贡献者为规范作者 + `Co-authored-by`）：#3763（@idling11，网站 i18n 矩阵）、#3760（@idling11，Homebrew 发布文档）、#3872 和 #3871（@cyq1017，死代码移除）。重复项 #3873/#3879 在 PR 上注明。MCP-spawn/provider/persistence/constitution-context PR 保留 NEEDS-REVIEW。
- 推送时 CI：fmt / clippy（工作区，CI 标志）/ provider-registry / co-author（`--check-authors` 规范）/ version-drift / GitGuardian / CodeQL / ubuntu+macOS 测试全部绿色；在快照时仅有最慢的任务在完成；零失败。

## 剩余手动证据

在宣布发布就绪之前，保留 QA 矩阵中的最终手动验证：打开当前 TUI 构建，目视确认通过 `/setup`、`/constitution`、`/setup report`、`doctor --json` 和 `doctor --context-json` 的相同流程。本文件记录自动化当前构建覆盖，而非人工视觉验收。
