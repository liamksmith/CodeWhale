# LSP：PHP 支持与自定义语言服务器扩展

> v0.8.65+ | `codex/lsp-php-custom-servers`

## 概述

此功能将 **PHP** 添加到内置的 LSP 语言注册表中，并引入了一个
`[lsp.custom]` 配置节，以便用户可以按文件扩展名注册任意的 LSP 服务器——
覆盖内置 `Language` 枚举中不存在的语言（Ruby、C#、Swift、Lua 等）。

## 变更

### 1. PHP 内置支持

- `Language::Php` 变体已添加到 `crates/tui/src/lsp/registry.rs` 的枚举中
- `.php` 文件会被检测并默认路由到 `intelephense --stdio`
- 用户可以通过 `[lsp.servers].php` 进行覆盖

### 2. 自定义 LSP 服务器扩展

新的结构体 `CustomLspDef`（定义在 `crates/tui/src/lsp/mod.rs` 中，并在
`crates/config/src/lib.rs` 中镜像）：

```rust
pub struct CustomLspDef {
    pub language_id: String,  // textDocument/didOpen 的 LSP languageId
    pub command: String,      // 要启动的可执行文件
    pub args: Vec<String>,    // 参数（默认为空）
}
```

新增配置字段 `LspConfig.custom: HashMap<String, CustomLspDef>`——以文件扩展名（不含点号）
为键，例如 `"rb"`、`"cs"`、`"swift"`。

在 `LspManager::diagnostics_for` 中，当内置注册表返回
`Language::Other` 时，管理器在放弃之前会检查用户的自定义表。
自定义服务器拥有自己的惰性启动传输映射和每个扩展名一次性的缺失二进制文件警告（不会刷屏日志）。

### 3. 传输层泛化

`StdioLspTransport::spawn` 现在接受 `&str language_id` 而不是
`Language`，因此内置和自定义服务器共享相同的传输实现。旧的 `Language` 导入已从 `client.rs` 中移除。

### 4. 轮询管线提取

`poll_diagnostics` 是一个新的私有方法，内置和自定义诊断路径共享使用——
消除了重复的调用/等待/过滤/排序/截断逻辑。

## 配置

### 内置 PHP（如果 `intelephense` 在 PATH 中则默认启用）

```toml
# 无需配置——PHP .php 文件会自动检测。
# 如果需要，可以覆盖服务器：
[lsp.servers]
php = ["phpactor", "language-server"]
```

### 自定义语言服务器

```toml
[lsp.custom.rb]
command = "ruby-lsp"
args = ["--stdio"]
language_id = "ruby"

[lsp.custom.cs]
command = "csharp-ls"
language_id = "csharp"

[lsp.custom.swift]
command = "sourcekit-lsp"
language_id = "swift"
```

键是文件扩展名（不含前导点号）。`args` 字段默认为空。
`language_id` 必须与 LSP 服务器在 `textDocument/didOpen` 中期望的值匹配。

## 架构

```
edit_file / write_file / apply_patch 成功
        │
        ▼
  LspManager.diagnostics_for(file)
        │
        ├── custom_for_extension(file) ── 找到？ ──► transport_for_custom(ext, def)
        │                                                  │
        ├── detect_language(file) ── Other？ ──► return None（跳过）
        │
        └── transport_for(lang)
                │
                ▼
          poll_diagnostics(file, text, transport)
                │
                ▼
          DiagnosticBlock → 注入到会话消息流中
```

## 验证

```
cargo test -p codewhale-tui --bin codewhale-tui lsp::
# 32 个测试通过（新增 3 个：detects_php_extension、language_ids_for_php、
# server_for_php_is_intelephense）
cargo clippy -p codewhale-tui --bin codewhale-tui
# lsp 模块：零新增警告
```

## 涉及的文件

| 文件 | 变更 |
|------|--------|
| `crates/tui/src/lsp/registry.rs` | +Php 变体、检测、服务器映射、测试 |
| `crates/tui/src/lsp/mod.rs` | +CustomLspDef、LspConfig.custom、LspManager 自定义回退 |
| `crates/tui/src/lsp/client.rs` | spawn 接受 &str language_id |
| `crates/tui/src/config.rs` | LspConfigToml.custom + into_runtime |
| `crates/config/src/lib.rs` | LspConfigToml.custom + CustomLspDef |
| `config.example.toml` | PHP 文档 + 自定义扩展示例 |
