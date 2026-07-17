# RFC：v0.9.0 文件分解

## 问题

六个文件超过 5,000 行。最严重的违规者将 provider 特定逻辑、测试代码和 UI 渲染累积在单个翻译单元中。这使得添加 provider 需要触碰 15+ 个文件，并使代码审查变得脆弱。

### 当前状态（行数）

| 文件 | 行数 | 内容 |
|------|-------|----------|
| `crates/tui/src/config.rs` | 10,046 | Provider 解析、环境变量处理、模型别名、能力矩阵、2,000+ 行测试 |
| `crates/tui/src/tui/ui.rs` | 9,400 | TUI 渲染循环、输入处理、命令分发、/logout 清理 |
| `crates/tui/src/tui/ui/tests.rs` | 8,360 | ui.rs 的测试 |
| `crates/tui/src/main.rs` | 7,998 | CLI 参数解析、模式选择、启动 |
| `crates/tui/src/tui/app.rs` | 7,256 | 应用程序状态结构和生命周期 |
| `crates/tui/src/runtime_threads.rs` | 5,454 | 异步运行时编排 |

## 提案的分解

### 1. `config.rs` → provider 模块树

将 `crates/tui/src/config.rs` 拆分为：

```
crates/tui/src/config/
├── mod.rs              # 重新导出、Config 结构体、加载/保存
├── provider.rs         # ApiProvider 枚举、parse/as_str/display_name/all
├── capability.rs       # ProviderCapability、provider_capability()
├── model_resolution.rs # wire_model_for_provider、normalize_model_name_for_provider
├── env.rs              # EnvGuard、环境变量优先级、每个 provider 的环境变量处理
├── constants.rs        # 所有 DEFAULT_*_MODEL 和 DEFAULT_*_BASE_URL 常量
└── tests/              # 测试模块
    ├── mod.rs
    ├── provider.rs
    ├── capability.rs
    ├── model_resolution.rs
    └── env.rs
```

**原因：** 当前，每个新 provider 需要编辑分散在一个 10K 行文件中的约 20 个 match 分支。通过将常量放在自己的模块中并隔离解析逻辑，添加 provider 变为：添加常量、添加枚举变体、为每个函数添加一个 match 分支。差异检查脚本可以独立验证每个子模块。

### 2. `ui.rs` → 视图模块

将 `crates/tui/src/tui/ui.rs` 拆分为：

```
crates/tui/src/tui/
├── ui.rs               # 核心渲染循环、帧分发（保持在 2,000 行以下）
├── input.rs            # 键盘/鼠标输入处理
├── command_dispatch.rs # /command 路由、/logout、/config
└── status_bar.rs       # 状态栏渲染
```

**原因：** /logout 清理逻辑、命令分发和渲染循环是独立的关注点。`ui.rs` 当前有一个 6,200 行的 `execute_command_input` 函数体，混合了输入解析、命令路由和状态变更。

### 3. `main.rs` → CLI 模块

将 `crates/tui/src/main.rs` 拆分为：

```
crates/tui/src/cli/
├── mod.rs              # Cli 结构体、参数解析
├── args.rs             # 参数定义
└── startup.rs          # 模式选择、配置加载、恢复逻辑
```

**原因：** `main.rs` 有 8K 行，表明 CLI 定义已经超出了一个文件的范围。将参数定义与启动逻辑分离使入口点具有可读性。

### 4. Provider 添加应是数据驱动的

当前的 provider 模式需要触碰：
- `config.rs`：20+ match 分支
- `cli/src/lib.rs`：4+ match 分支
- `agent/src/lib.rs`：静态注册表
- `tui/provider_picker.rs`：选择器列表
- `docs/PROVIDERS.md`：注册表
- `config.example.toml`：示例部分
- `README.md`：环境变量表
- `scripts/check-provider-registry.py`：差异检查

数据驱动的方法将每个 provider 定义为一个结构体，包含其常量、环境变量、能力元数据和显示名称 — 然后从数据中派生 match 分支。这是一个更大的重构，但可以将 provider 添加减少为单文件更改。

## 优先级

1. **config.rs 分解** — 影响最大，大多数 provider 变更发生在这里
2. **ui.rs 分解** — 第二大影响，/logout 和命令分发是独立的
3. **数据驱动的 provider** — v0.9.0 的愿景，需要 trait 设计

## 迁移策略

每个分解应是一个独立的 PR，应该：
1. 创建新的模块树
2. 使用 `git mv` 移动代码（保留历史）
3. 在旧文件位置添加 `pub use` 重新导出（零 API 变更）
4. 运行完整的测试套件
5. 在消费者更新后，在后续 PR 中移除重新导出

在分解 PR 中不做功能变更。保持它们枯燥。
