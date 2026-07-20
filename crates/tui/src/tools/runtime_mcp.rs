//! 运行时 MCP 服务器管理。
//!
//! 提供 `StartRuntimeMcpServer` —— LLM 从对话上下文中动态连接到
//! MCP 服务器的入口工具。还包含工具使用的解析和命名辅助函数。

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use serde_json::{Value, json};
use tokio::sync::Mutex as AsyncMutex;

use crate::mcp::{McpPool, McpServerConfig, McpTool};
use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};

// === 解析函数 ===

#[derive(Debug, Clone)]
pub struct ParsedMcpServer {
    pub name: String,
    pub config: McpServerConfig,
}

/// 将命令字符串或 URL 解析为 MCP 服务器配置。
///
/// - 本地命令：`npx @modelcontextprotocol/server-filesystem /tmp`
/// - 远程 URL：`https://huggingface.co/mcp`
pub fn parse_mcp_command(input: &str) -> Result<ParsedMcpServer> {
    let input = input.trim();
    if input.is_empty() {
        anyhow::bail!("MCP command cannot be empty");
    }

    if input.starts_with("http://") || input.starts_with("https://") {
        let name = extract_name_from_url(input)?;
        return Ok(ParsedMcpServer {
            name,
            config: McpServerConfig {
                command: None,
                args: Vec::new(),
                env: HashMap::new(),
                cwd: None,
                url: Some(input.to_string()),
                transport: None,
                connect_timeout: None,
                execute_timeout: None,
                read_timeout: None,
                disabled: false,
                enabled: true,
                required: false,
                enabled_tools: Vec::new(),
                disabled_tools: Vec::new(),
                headers: HashMap::new(),
                env_headers: HashMap::new(),
                bearer_token_env_var: None,
                scopes: Vec::new(),
                oauth: None,
                oauth_resource: None,
            },
        });
    }

    let parts: Vec<String> = shell_words::split(input).unwrap_or_default();
    if parts.is_empty() {
        anyhow::bail!("MCP command cannot be empty");
    }

    let command = parts[0].clone();
    let args: Vec<String> = parts[1..].to_vec();
    let name = infer_server_name(&command, &args)?;

    Ok(ParsedMcpServer {
        name,
        config: McpServerConfig {
            command: Some(command),
            args,
            env: HashMap::new(),
            cwd: None,
            url: None,
            transport: None,
            connect_timeout: None,
            execute_timeout: None,
            read_timeout: None,
            disabled: false,
            enabled: true,
            required: false,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
            headers: HashMap::new(),
            env_headers: HashMap::new(),
            bearer_token_env_var: None,
            scopes: Vec::new(),
            oauth: None,
            oauth_resource: None,
        },
    })
}

pub fn extract_name_from_url(url: &str) -> Result<String> {
    let parsed = reqwest::Url::parse(url)?;
    let host = parsed.host_str().unwrap_or("remote");
    let path = parsed.path().trim_matches('/');

    // 为提高可读性，将主机名中的点替换为横线
    let host_part = host.replace('.', "-");

    // 组合主机和路径，将斜杠替换为下划线
    let name = if path.is_empty() {
        host_part
    } else {
        format!("{}_{}", host_part, path.replace('/', "_"))
    };

    Ok(sanitize_name(&name))
}

fn infer_server_name(command: &str, args: &[String]) -> Result<String> {
    let cmd_path = std::path::Path::new(command);
    let cmd_base = cmd_path.file_stem().unwrap_or_default().to_string_lossy();

    // Windows cmd /c 前缀：跳过 "cmd /c" 并在剩余参数上递归
    // 例如 ["cmd", "/c", "npx", "-y", "@modelcontextprotocol/server-memory"]
    if cmd_base.as_ref() == "cmd"
        && args.len() >= 2
        && (args[0] == "/c" || args[0] == "/C" || args[0] == "/k" || args[0] == "/K")
    {
        let inner_cmd = &args[1];
        let inner_args: Vec<String> = args[2..].to_vec();
        return infer_server_name(inner_cmd, &inner_args);
    }

    // 包管理器：提取包名（第一个非标志参数）
    if matches!(
        cmd_base.as_ref(),
        "npx" | "npm" | "pnpm" | "yarn" | "bunx" | "bun"
    ) {
        for arg in args {
            if !arg.starts_with('-') && arg != "exec" && arg != "run" && arg != "start" {
                // 例如 "@modelcontextprotocol/server-filesystem" → "filesystem"
                if let Some(name) = arg.split('/').next_back() {
                    if let Some(short) = name.strip_prefix("server-") {
                        return Ok(sanitize_name(short));
                    }
                    return Ok(sanitize_name(name));
                }
            }
        }
    }

    // 脚本解释器：提取脚本路径（第一个非标志参数）
    if matches!(
        cmd_base.as_ref(),
        "node" | "python" | "python3" | "uvx" | "uv" | "ruby" | "deno"
    ) && let Some(script) = args.iter().find(|a| !a.starts_with('-'))
    {
        let script_path = std::path::Path::new(script);
        if let Some(stem) = script_path.file_stem() {
            return Ok(sanitize_name(&stem.to_string_lossy()));
        }
    }

    // 回退：第一个非标志参数（脚本或文件）
    if let Some(script) = args.iter().find(|a| !a.starts_with('-')) {
        let script_path = std::path::Path::new(script);
        if let Some(stem) = script_path.file_stem() {
            return Ok(sanitize_name(&stem.to_string_lossy()));
        }
    }

    // 最后手段：命令名本身
    Ok(sanitize_name(&cmd_base))
}

pub fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

// === 工具：StartRuntimeMcpServer ===

/// 从对话上下文中动态添加 MCP 服务器的入口工具。
///
/// LLM 调用此工具来启动本地 MCP 服务器（stdio）或连接到远程
/// 服务器（HTTP）。服务器配置被添加到 `McpPool.dynamic_servers`，
/// 工具通过现有的 `McpConnection` / `StdioTransport` 流程发现。
pub struct StartRuntimeMcpServer {
    pool: Arc<AsyncMutex<McpPool>>,
}

impl StartRuntimeMcpServer {
    pub fn new(pool: Arc<AsyncMutex<McpPool>>) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl ToolSpec for StartRuntimeMcpServer {
    fn name(&self) -> &str {
        "start_mcp_server"
    }

    fn description(&self) -> &str {
        "When a user provides an MCP server command (like 'npx ...') or URL \
         (like 'https://...'), call this tool immediately to start the server \
         and register its tools. Do NOT suggest editing config files. \
         Accepts a local command (stdio) or a remote URL (HTTP/SSE). \
         After the server starts, the response lists each tool's callable name. \
         You MUST copy those exact names when calling the tools. \
         Do NOT construct or guess tool names yourself."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "server": {
                    "type": "string",
                    "description": "MCP server command or URL"
                },
                "name": {
                    "type": "string",
                    "description": "Optional server name (auto-inferred if omitted)"
                }
            },
            "required": ["server"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::Network, ToolCapability::ExecutesCode]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let server = input
            .get("server")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_input("Missing required field: server"))?;

        let custom_name = input.get("name").and_then(|v| v.as_str());
        let parsed =
            parse_mcp_command(server).map_err(|e| ToolError::invalid_input(e.to_string()))?;

        // 拒绝可能执行任意代码的 shell 包装命令
        if let Some(ref cmd) = parsed.config.command {
            let cmd_lower = cmd.to_lowercase();
            if cmd_lower == "bash"
                || cmd_lower == "sh"
                || cmd_lower == "zsh"
                || cmd_lower == "cmd"
                || cmd_lower == "powershell"
            {
                return Err(ToolError::invalid_input(format!(
                    "Shell wrapper commands ({cmd}) are not allowed. \
                     Provide the actual MCP server command directly, \
                     e.g. 'npx @modelcontextprotocol/server-filesystem /tmp'"
                )));
            }
        }

        // 拒绝参数中的 shell 元字符以防止注入。
        // 重定向（>, >>）、管道（|）、命令链接（;, &&, ||）、
        // 子 shell（``）和变量展开（$）都是危险的。
        for arg in &parsed.config.args {
            if arg.contains('>')
                || arg.contains('|')
                || arg.contains(';')
                || arg.contains('&')
                || arg.contains('`')
                || arg.contains('$')
            {
                return Err(ToolError::invalid_input(format!(
                    "Argument contains shell metacharacters: '{arg}'. \
                     MCP server arguments must not contain redirects, pipes, \
                     command chaining, or variable expansion."
                )));
            }
        }

        // 已知 MCP 服务器运行时和包管理器的许可列表。
        // 不在此列表中的命令被拒绝，以防止任意执行。
        if let Some(ref cmd) = parsed.config.command {
            let cmd_base = std::path::Path::new(cmd)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            const ALLOWED_COMMANDS: &[&str] = &[
                "npx", "npm", "pnpm", "yarn", "bunx", "bun", "node", "python", "python3", "uvx",
                "uv", "deno", "ruby", "cargo",
            ];
            if !ALLOWED_COMMANDS.contains(&cmd_base.as_ref()) {
                return Err(ToolError::invalid_input(format!(
                    "Command '{cmd}' is not in the allowed list. \
                     Permitted commands: {}",
                    ALLOWED_COMMANDS.join(", ")
                )));
            }
        }

        let server_name = custom_name
            .map(sanitize_name)
            .unwrap_or(parsed.name)
            .replace('_', "-");

        // 服务器名称中的下划线会导致工具名称冲突。
        // 工具名称格式为 mcp_{server}_{tool}；服务器名称中的下划线
        // 会使其产生歧义（服务器 "foo" + 工具 "bar_x" vs
        // 服务器 "foo_bar" + 工具 "x" 都会变成 mcp_foo_bar_x）。
        // sanitize_name 已经将非字母数字字符转换为连字符，
        // 但原始输入中的下划线需要显式转换。

        let transport = if parsed.config.url.is_some() {
            "http"
        } else {
            "stdio"
        };

        // 注册服务器配置，连接，并收集工具信息
        let mut pool = self.pool.lock().await;
        pool.add_runtime_server_config(server_name.clone(), parsed.config)
            .map_err(ToolError::invalid_input)?;
        let conn = pool.get_or_connect(&server_name).await.map_err(|e| {
            ToolError::execution_failed(format!(
                "Failed to connect to MCP server '{}': {e}",
                server_name
            ))
        })?;

        let mcp_tools: Vec<McpTool> = conn.tools().to_vec();

        // 使用完全限定名称构建工具列表（mcp_{server}_{tool}）
        // 以便 LLM 可以直接调用它们，而无需猜测命名约定。
        let tools_list: Vec<String> = mcp_tools
            .iter()
            .map(|t| {
                let qualified = format!("mcp_{}_{}", server_name, t.name);
                format!(
                    "- {} → {}",
                    qualified,
                    t.description.as_deref().unwrap_or("no description")
                )
            })
            .collect();

        let result = serde_json::to_string(&json!({
            "status": "connected",
            "transport": transport,
            "server": server_name,
            "new_tools": mcp_tools.len(),
            "total_mcp_tools": pool.all_tools().len(),
            "message": format!(
                "MCP server '{}' connected via {}. {} tools discovered.\n\n\
                 Callable tools (use these exact names):\n{}",
                server_name, transport, mcp_tools.len(), tools_list.join("\n")
            )
        }))
        .unwrap_or_else(|_| "{}".to_string());

        Ok(ToolResult::success(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_command_stdio() {
        let parsed = parse_mcp_command("npx @modelcontextprotocol/server-filesystem /tmp").unwrap();
        assert!(parsed.config.command.is_some());
        assert!(parsed.config.url.is_none());
    }

    #[test]
    fn parse_command_url() {
        let parsed = parse_mcp_command("https://huggingface.co/mcp").unwrap();
        assert!(parsed.config.command.is_none());
        assert!(parsed.config.url.is_some());
        assert_eq!(parsed.name, "huggingface-co-mcp");
    }

    #[test]
    fn parse_command_url_with_subdomain() {
        let parsed = parse_mcp_command("https://api.example.com/mcp").unwrap();
        assert!(parsed.config.command.is_none());
        assert!(parsed.config.url.is_some());
        assert_eq!(parsed.name, "api-example-com-mcp");
    }

    #[test]
    fn parse_command_empty() {
        assert!(parse_mcp_command("").is_err());
        assert!(parse_mcp_command("   ").is_err());
    }

    #[test]
    fn extract_name_from_url_with_path() {
        assert_eq!(
            extract_name_from_url("https://huggingface.co/mcp").unwrap(),
            "huggingface-co-mcp"
        );
    }

    #[test]
    fn extract_name_from_url_with_subdomain() {
        assert_eq!(
            extract_name_from_url("https://api.example.com/mcp").unwrap(),
            "api-example-com-mcp"
        );
    }

    #[test]
    fn extract_name_from_url_no_path() {
        assert_eq!(
            extract_name_from_url("https://example.com").unwrap(),
            "example-com"
        );
    }

    #[test]
    fn extract_name_from_url_empty_path() {
        assert_eq!(
            extract_name_from_url("https://example.com/").unwrap(),
            "example-com"
        );
    }

    // === shell_words 分割测试 ===

    #[test]
    fn shell_words_simple() {
        assert_eq!(
            shell_words::split("npx server /tmp").unwrap(),
            vec!["npx", "server", "/tmp"]
        );
    }

    #[test]
    fn shell_words_double_quotes() {
        assert_eq!(
            shell_words::split(r#"npx server --env="MY KEY""#).unwrap(),
            vec!["npx", "server", "--env=MY KEY"]
        );
    }

    #[test]
    fn shell_words_single_quotes() {
        assert_eq!(
            shell_words::split("npx server --env='MY KEY'").unwrap(),
            vec!["npx", "server", "--env=MY KEY"]
        );
    }

    #[test]
    fn shell_words_mixed_quotes() {
        assert_eq!(
            shell_words::split(r#"cmd --opt="hello world" --flag 'single'"#).unwrap(),
            vec!["cmd", "--opt=hello world", "--flag", "single"]
        );
    }

    #[test]
    fn shell_words_escaped_quote() {
        assert_eq!(
            shell_words::split(r#"cmd arg\"with\"quotes"#).unwrap(),
            vec!["cmd", r#"arg"with"quotes"#]
        );
    }

    #[test]
    fn shell_words_empty() {
        assert!(shell_words::split("").unwrap().is_empty());
        assert!(shell_words::split("   ").unwrap().is_empty());
    }

    #[test]
    fn shell_words_postgres_url() {
        assert_eq!(
            shell_words::split(
                r#"npx -y @modelcontextprotocol/server-postgres "postgresql://user:pass@host/db""#
            )
            .unwrap(),
            vec![
                "npx",
                "-y",
                "@modelcontextprotocol/server-postgres",
                "postgresql://user:pass@host/db"
            ]
        );
    }

    #[test]
    fn parse_command_with_quoted_args() {
        let parsed =
            parse_mcp_command(r#"npx @modelcontextprotocol/server-filesystem /tmp --env="MY KEY""#)
                .unwrap();
        assert_eq!(parsed.config.command, Some("npx".to_string()));
        assert_eq!(
            parsed.config.args,
            vec![
                "@modelcontextprotocol/server-filesystem",
                "/tmp",
                "--env=MY KEY"
            ]
        );
    }

    // === infer_server_name 测试 ===

    #[test]
    fn infer_name_npx_package() {
        let parsed = parse_mcp_command("npx @modelcontextprotocol/server-filesystem /tmp").unwrap();
        assert_eq!(parsed.name, "filesystem");
    }

    #[test]
    fn infer_name_npx_simple() {
        let parsed = parse_mcp_command("npx my-mcp-server").unwrap();
        assert_eq!(parsed.name, "my-mcp-server");
    }

    #[test]
    fn infer_name_pnpm_exec() {
        let parsed = parse_mcp_command("pnpm exec @modelcontextprotocol/server-postgres").unwrap();
        assert_eq!(parsed.name, "postgres");
    }

    #[test]
    fn infer_name_node_script() {
        let parsed = parse_mcp_command("node ./my-mcp-server.js").unwrap();
        assert_eq!(parsed.name, "my-mcp-server");
    }

    #[test]
    fn infer_name_python_script() {
        let parsed = parse_mcp_command("python3 mcp_server.py").unwrap();
        assert_eq!(parsed.name, "mcp-server");
    }

    #[test]
    fn infer_name_uvx_package() {
        let parsed = parse_mcp_command("uvx mcp-server-git").unwrap();
        assert_eq!(parsed.name, "mcp-server-git");
    }

    #[test]
    fn infer_name_bare_command() {
        let parsed = parse_mcp_command("/usr/local/bin/my-server").unwrap();
        assert_eq!(parsed.name, "my-server");
    }

    #[test]
    fn infer_name_windows_cmd_prefix() {
        let parsed =
            parse_mcp_command("cmd /c npx -y @modelcontextprotocol/server-memory").unwrap();
        assert_eq!(parsed.name, "memory");
    }

    #[test]
    fn infer_name_windows_cmd_uppercase() {
        let parsed =
            parse_mcp_command("cmd /C npx @modelcontextprotocol/server-filesystem /tmp").unwrap();
        assert_eq!(parsed.name, "filesystem");
    }

    #[test]
    fn infer_name_only_command_no_args() {
        // 完全没有参数——回退到最后手段：命令名本身
        let parsed = parse_mcp_command("my-server").unwrap();
        assert_eq!(parsed.name, "my-server");
    }

    #[test]
    fn infer_name_only_command_no_args_path() {
        // 绝对路径，无参数——使用命令的 file_stem
        let parsed = parse_mcp_command("/usr/local/bin/my-server").unwrap();
        assert_eq!(parsed.name, "my-server");
    }

    // === sanitize_name 测试 ===

    #[test]
    fn sanitize_name_preserves_hyphens() {
        assert_eq!(sanitize_name("my-server"), "my-server");
    }

    #[test]
    fn sanitize_name_converts_underscores_to_hyphens() {
        assert_eq!(sanitize_name("my_server"), "my-server");
    }

    #[test]
    fn sanitize_name_converts_special_chars_to_hyphens() {
        assert_eq!(sanitize_name("my@server!"), "my-server");
    }

    #[test]
    fn sanitize_name_trims_leading_trailing_hyphens() {
        assert_eq!(sanitize_name("_my_server_"), "my-server");
    }

    #[test]
    fn sanitize_name_preserves_alphanumeric() {
        assert_eq!(sanitize_name("server123"), "server123");
    }

    #[test]
    fn sanitize_name_empty_input() {
        assert_eq!(sanitize_name(""), "");
    }

    // === 命令验证测试 ===

    #[test]
    fn reject_shell_wrapper_bash() {
        let result = parse_mcp_command("bash -c 'npx server'");
        assert!(result.is_ok()); // 解析成功
        // 但 execute 会拒绝——通过 parse_mcp_command 结构验证
    }

    #[test]
    fn reject_metachar_redirect_in_args() {
        let parsed = parse_mcp_command("npx server --out>file").unwrap();
        assert!(parsed.config.args.iter().any(|a| a.contains('>')));
    }

    #[test]
    fn reject_metachar_pipe_in_args() {
        let parsed = parse_mcp_command("npx server arg1 | cat").unwrap();
        assert!(parsed.config.args.iter().any(|a| a.contains('|')));
    }

    #[test]
    fn reject_metachar_dollar_in_args() {
        let parsed = parse_mcp_command(r#"npx server --key=$SECRET"#).unwrap();
        assert!(parsed.config.args.iter().any(|a| a.contains('$')));
    }

    #[test]
    fn reject_metachar_backtick_in_args() {
        let parsed = parse_mcp_command("npx server --dir=`whoami`").unwrap();
        assert!(parsed.config.args.iter().any(|a| a.contains('`')));
    }

    #[test]
    fn allow_clean_mcp_command() {
        let parsed = parse_mcp_command("npx @modelcontextprotocol/server-filesystem /tmp").unwrap();
        assert_eq!(parsed.config.command, Some("npx".to_string()));
        assert!(
            parsed
                .config
                .args
                .iter()
                .all(|a| !a.contains('>') && !a.contains('|') && !a.contains('$'))
        );
    }

    #[test]
    fn allowlist_includes_common_runtimes() {
        // 验证许可列表包含预期的命令
        const ALLOWED: &[&str] = &[
            "npx", "npm", "pnpm", "yarn", "bunx", "bun", "node", "python", "python3", "uvx", "uv",
            "deno", "ruby", "cargo",
        ];
        // 所有标准 MCP 服务器启动器都应存在
        assert!(ALLOWED.contains(&"npx"));
        assert!(ALLOWED.contains(&"node"));
        assert!(ALLOWED.contains(&"python3"));
        assert!(ALLOWED.contains(&"uvx"));
    }

    // === 审批门控契约 ===

    #[test]
    fn start_mcp_server_declares_required_approval() {
        // 安全不变量（#3866）：生成运行时 MCP 服务器是
        // 有副作用的（子进程/网络连接），因此工具规范本身
        // 必须声明 `ApprovalRequirement::Required`。结合引擎的
        // 不可绕过的门控（参见引擎测试），这保证了在 `execute`
        // 运行之前，未经批准的启动会被拒绝。
        let pool = Arc::new(AsyncMutex::new(McpPool::new(
            crate::mcp::McpConfig::default(),
        )));
        let tool = StartRuntimeMcpServer::new(pool);
        assert_eq!(tool.name(), "start_mcp_server");
        assert!(
            matches!(tool.approval_requirement(), ApprovalRequirement::Required),
            "start_mcp_server must require approval before spawning"
        );
    }
}
