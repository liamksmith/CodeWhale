//! `js_execution` 工具——通过本地 Node.js 运行时执行模型提供的 JavaScript，
//! 以 JSON 形式返回 stdout / stderr / 退出码。
//!
//! 与 `code_execution`（Python）形状一致，使模型看到"在此处运行代码片段
//! 并告诉我它输出了什么"的统一接口。拆分为专用模块（而不是内联在
//! `core::engine::tool_catalog` 中 `execute_code_execution_tool` 旁边）
//! 可以将依赖探测和临时文件生成逻辑隔离出来以便测试固定。
//!
//! 注册受 [`crate::dependencies::resolve_node`] 门控：当 Node 缺失时，
//! 该工具根本不会被暴露（给模型），因此模型永远不会看到它实际无法使用的运行时。
//! 目录端调度见 `core::engine::tool_catalog::ensure_advanced_tooling`。

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use crate::dependencies::ExternalTool;
use serde_json::{Value, json};

use crate::models::Tool;
use crate::tools::spec::{ToolError, ToolResult, required_str};

/// 暴露给模型的工具名称。与 `code_execution` 并排放置在延迟工具调度器中。
pub const JS_EXECUTION_TOOL_NAME: &str = "js_execution";
/// 工具类型标签——使用与 `code_execution_*` 相同的系列，
/// Anthropic 消息 API 期望这样做，以便在线格式在两个解释器之间保持稳定。
const JS_EXECUTION_TOOL_TYPE: &str = "code_execution_20250825";
const NODE_USE_ENV_PROXY: &str = "NODE_USE_ENV_PROXY";
const NODE_PROXY_PAIRS: &[(&str, &str)] =
    &[("HTTP_PROXY", "http_proxy"), ("HTTPS_PROXY", "https_proxy")];

fn first_non_empty_env_from(
    keys: &[&str],
    env: &impl Fn(&str) -> Option<OsString>,
) -> Option<OsString> {
    keys.iter()
        .filter_map(|key| env(key))
        .find(|value| !value.is_empty())
}

fn node_proxy_env_overrides_from(
    env: impl Fn(&str) -> Option<OsString>,
) -> Vec<(&'static str, OsString)> {
    let all_proxy = first_non_empty_env_from(&["ALL_PROXY", "all_proxy"], &env);
    let proxy_configured = all_proxy.is_some()
        || NODE_PROXY_PAIRS
            .iter()
            .any(|(upper, lower)| first_non_empty_env_from(&[upper, lower], &env).is_some());

    let mut overrides = Vec::new();
    if proxy_configured && first_non_empty_env_from(&[NODE_USE_ENV_PROXY], &env).is_none() {
        overrides.push((NODE_USE_ENV_PROXY, OsString::from("1")));
    }

    for (upper, lower) in NODE_PROXY_PAIRS {
        if first_non_empty_env_from(&[upper], &env).is_none()
            && let Some(value) =
                first_non_empty_env_from(&[lower], &env).or_else(|| all_proxy.clone())
        {
            overrides.push((*upper, value));
        }
    }

    if first_non_empty_env_from(&["NO_PROXY"], &env).is_none()
        && let Some(value) = first_non_empty_env_from(&["no_proxy"], &env)
    {
        overrides.push(("NO_PROXY", value));
    }

    overrides
}

fn node_proxy_env_overrides() -> Vec<(&'static str, OsString)> {
    node_proxy_env_overrides_from(|key| std::env::var_os(key))
}

fn apply_node_execution_env(cmd: &mut tokio::process::Command) {
    crate::child_env::apply_to_tokio_command(cmd, node_proxy_env_overrides());
}

/// 构建当主机上存在 Node.js 时目录应广告的 `Tool` 定义。
/// 保持为构造函数（而不是 `static`），以便输入模式可以保持声明式，
/// 而无需 `lazy_static!` 风格的间接引用。
#[must_use]
pub fn js_execution_tool_definition() -> Tool {
    Tool {
        tool_type: Some(JS_EXECUTION_TOOL_TYPE.to_string()),
        name: JS_EXECUTION_TOOL_NAME.to_string(),
        description:
            "Execute JavaScript code in a local sandboxed Node.js runtime and return stdout/stderr/return_code as JSON."
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "code": { "type": "string", "description": "JavaScript source code to execute." }
            },
            "required": ["code"]
        }),
        allowed_callers: Some(vec!["direct".to_string()]),
        defer_loading: Some(false),
        input_examples: None,
        strict: None,
        cache_control: None,
    }
}

/// 运行模型提供的 JavaScript 并返回捕获的 stdout / stderr / return_code
/// 负载。精确镜像 `execute_code_execution_tool`——相同的临时文件模式、
/// 相同的 120 秒超时、相同的错误形状——以便这两个表面从模型的角度看
/// 可以互换。
///
/// 临时文件仅在此执行期间存在；`Drop` 会移除它。我们使用 `.js` 扩展名，
/// 以便解释器中的任何源映射 / shebang / 编码探测逻辑正常工作。
pub async fn execute_js_execution_tool(
    input: &Value,
    workspace: &Path,
) -> Result<ToolResult, ToolError> {
    let code = required_str(input, "code")?;

    // 通过 ExternalTool 解析 Node 运行时。如果它现在不可用，
    // tokio_command() 返回 None，我们快速失败并给出清晰的消息。

    let temp_dir = tempfile::tempdir()
        .map_err(|e| ToolError::execution_failed(format!("tempdir failed: {e}")))?;
    let script_path = temp_dir.path().join("js_execution.js");
    tokio::fs::write(&script_path, code)
        .await
        .map_err(|e| ToolError::execution_failed(format!("tempfile write failed: {e}")))?;

    let mut cmd = crate::dependencies::Node::tokio_command().ok_or_else(|| {
        ToolError::execution_failed("js_execution: Node.js runtime became unavailable".to_string())
    })?;
    // 最近的 Node 版本使用此启动环境变量使 fetch/http(s) 遵循
    // 标准代理变量；较旧的运行时忽略它并保持先前行为。
    apply_node_execution_env(&mut cmd);
    cmd.arg(&script_path).current_dir(workspace);

    // #3273：Node 内置的 `fetch`（undici）忽略 HTTP(S)_PROXY 环境变量，
    // 除非设置了 `NODE_USE_ENV_PROXY`（Node >= 24）。此子进程已继承
    // CodeWhale 的代理环境，因此启用该标志让 `js_execution` 的 `fetch()`
    // 通过与应用其余部分相同的代理/VPN 访问网络，并遵循 `NO_PROXY`。
    // 仅在用户未选择值时默认启用，以便显式退出（`NODE_USE_ENV_PROXY=0`）
    // 仍然有效。Node < 24 上无操作，因为会忽略未知变量。
    if std::env::var_os("NODE_USE_ENV_PROXY").is_none() {
        cmd.env("NODE_USE_ENV_PROXY", "1");
    }

    let output = tokio::time::timeout(Duration::from_secs(120), cmd.output())
        .await
        .map_err(|_| ToolError::Timeout { seconds: 120 })
        .and_then(|res| res.map_err(|e| ToolError::execution_failed(e.to_string())))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let return_code = output.status.code().unwrap_or(-1);
    let success = output.status.success();
    let payload = json!({
        "type": "code_execution_result",
        "stdout": stdout,
        "stderr": stderr,
        "return_code": return_code,
        "content": [],
    });

    Ok(ToolResult {
        content: serde_json::to_string(&payload).unwrap_or_else(|_| payload.to_string()),
        success,
        metadata: Some(payload),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EnvVarGuard, lock_test_env};
    use std::ffi::OsString;
    use tempfile::tempdir;

    /// 跳过辅助函数——在没有 Node 的主机上 `js_execution` 是空操作。
    /// 在这种情况下，该工具根本不会被广告，因此正常路径测试不会失败；
    /// 它们只是不执行生成路径。
    fn node_present() -> bool {
        crate::dependencies::resolve_node().is_some()
    }

    fn proxy_env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |key| {
            pairs
                .iter()
                .find_map(|(name, value)| (*name == key).then(|| OsString::from(value)))
        }
    }

    #[test]
    fn tool_definition_advertises_js_execution_name_and_required_code_field() {
        let tool = js_execution_tool_definition();
        assert_eq!(tool.name, JS_EXECUTION_TOOL_NAME);
        assert_eq!(tool.tool_type.as_deref(), Some(JS_EXECUTION_TOOL_TYPE));
        let required = tool
            .input_schema
            .get("required")
            .and_then(|v| v.as_array())
            .expect("schema must declare a `required` array");
        assert!(
            required.iter().any(|v| v.as_str() == Some("code")),
            "input_schema 必须要求 `code`",
        );
    }

    #[test]
    fn node_proxy_overrides_enable_env_proxy_when_proxy_env_is_present() {
        let overrides =
            node_proxy_env_overrides_from(proxy_env(&[("HTTPS_PROXY", "http://127.0.0.1:20499")]));

        assert_eq!(
            overrides,
            vec![(NODE_USE_ENV_PROXY, OsString::from("1"))],
            "大写代理变量被子进程继承；只需 Node 的环境代理标志"
        );
    }

    #[test]
    fn node_proxy_overrides_mirror_lowercase_proxy_vars() {
        let overrides = node_proxy_env_overrides_from(proxy_env(&[
            ("https_proxy", "http://127.0.0.1:20499"),
            ("no_proxy", "localhost"),
        ]));

        assert_eq!(
            overrides,
            vec![
                (NODE_USE_ENV_PROXY, OsString::from("1")),
                ("HTTPS_PROXY", OsString::from("http://127.0.0.1:20499")),
                ("NO_PROXY", OsString::from("localhost")),
            ]
        );
    }

    #[tokio::test]
    async fn execute_js_runs_node_and_returns_stdout_payload() {
        if !node_present() {
            // 目录构建在没有 Node 的主机上完全跳过此工具——
            // 在测试中匹配该行为，而不是让没有安装 Node 的用户
            // 遇到测试套件失败。
            return;
        }
        let tmp = tempdir().expect("tempdir");
        let result = execute_js_execution_tool(
            &json!({ "code": "process.stdout.write('hello from node')" }),
            tmp.path(),
        )
        .await
        .expect("execute");
        assert!(result.success, "成功运行的 node 必须报告 success");
        assert!(
            result.content.contains("hello from node"),
            "stdout 负载必须包含打印的文本；得到 {}",
            result.content
        );
    }

    #[tokio::test]
    async fn execute_js_surfaces_runtime_error_with_nonzero_exit() {
        if !node_present() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        let result = execute_js_execution_tool(
            &json!({ "code": "throw new Error('intentional fail')" }),
            tmp.path(),
        )
        .await
        .expect("execute should not Err — runtime errors land in stderr/exit code");
        assert!(
            !result.success,
            "非零退出必须在结果负载中报告 success=false"
        );
        assert!(
            result.content.contains("intentional fail"),
            "stderr 负载必须包含错误消息；得到 {}",
            result.content
        );
    }

    // env 锁必须在 await 期间保持，以免其他修改 env 的测试在子节点进程
    // 读取进程环境时竞争。
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn execute_js_does_not_inherit_parent_secret_env() {
        if !node_present() {
            return;
        }
        let _env_lock = lock_test_env();
        let _secret = EnvVarGuard::set("CODEWHALE_JS_SECRET_LEAK_TEST", "secret-value");
        let tmp = tempdir().expect("tempdir");
        let result = execute_js_execution_tool(
            &json!({
                "code": "process.stdout.write(process.env.CODEWHALE_JS_SECRET_LEAK_TEST || 'missing')"
            }),
            tmp.path(),
        )
        .await
        .expect("execute");
        assert!(
            result.success,
            "node 运行应成功：{}",
            result.content
        );
        assert!(
            result.content.contains("missing"),
            "清理后的子进程环境不得暴露父进程密钥；得到 {}",
            result.content
        );
        assert!(
            !result.content.contains("secret-value"),
            "密钥值不得出现在 js_execution 输出中"
        );
    }

    #[tokio::test]
    async fn execute_js_enables_env_proxy_so_fetch_honors_proxy_vars() {
        if !node_present() {
            return;
        }
        // 该工具遵循调用者的显式选择；仅断言当周围环境未设置时的
        // 默认启用行为。
        if std::env::var_os("NODE_USE_ENV_PROXY").is_some() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        let result = execute_js_execution_tool(
            &json!({ "code": "process.stdout.write(String(process.env.NODE_USE_ENV_PROXY))" }),
            tmp.path(),
        )
        .await
        .expect("execute");
        assert!(
            result.content.contains("\"stdout\":\"1\""),
            "#3273: js_execution 必须默认 NODE_USE_ENV_PROXY=1，以便 Node 的 fetch \
             通过 HTTP(S)_PROXY 路由；得到 {}",
            result.content
        );
    }

    #[tokio::test]
    async fn execute_js_rejects_input_without_code_field() {
        let tmp = tempdir().expect("tempdir");
        let err = execute_js_execution_tool(&json!({}), tmp.path())
            .await
            .expect_err("missing `code` must reject before any node spawn");
        let msg = err.to_string();
        assert!(
            msg.contains("code"),
            "错误必须指明缺失的 `code` 字段；得到 {msg}"
        );
    }
}
