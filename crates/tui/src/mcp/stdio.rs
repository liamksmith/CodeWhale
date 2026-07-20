use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex as TokioMutex;

use super::{McpServerConfig, McpTransport};
use crate::child_env;

pub(super) struct StdioTransport {
    pub(super) child: Child,
    pub(super) stdin: ChildStdin,
    pub(super) reader: tokio::io::BufReader<ChildStdout>,
    /// 从生成的 MCP 服务器收集的 stderr 行尾部。后台任务
    /// 将子进程的 stderr 排入此缓冲区，以便运行时崩溃
    /// 能留下一些上下文，而不是被 `Stdio::null` 吞没。
    pub(super) stderr_tail: Arc<StderrTail>,
}

/// `StdioTransport::shutdown` 等待子进程通过 SIGTERM 退出
/// 的超时时间，超时后 `kill_on_drop` 会发送 SIGKILL。
/// 设置较短，这样挂起的 MCP 服务器不会阻塞 TUI 退出；
/// 行为良好的服务器几乎总是在几百毫秒内退出。
pub(super) const STDIO_SHUTDOWN_GRACE: Duration = Duration::from_millis(2_000);

/// 为崩溃诊断保留的 MCP 服务器 stderr 行数。
/// 有限制，防止健谈的服务器无限制增长；足够大以
/// 捕获典型的 Node/Python 启动或 panic 输出。
const STDERR_TAIL_CAPACITY: usize = 64;

/// 来自生成的 MCP 服务器的最新 stderr 行的有界环形缓冲区。
/// 由 `StdioTransport` 使用，在传输读取端失败时显示
/// 服务器端上下文（服务器崩溃、提前退出等）。
#[derive(Default)]
pub(super) struct StderrTail {
    lines: TokioMutex<VecDeque<String>>,
}

impl StderrTail {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            lines: TokioMutex::new(VecDeque::with_capacity(STDERR_TAIL_CAPACITY)),
        })
    }

    pub(super) async fn push(&self, line: String) {
        let mut buf = self.lines.lock().await;
        if buf.len() >= STDERR_TAIL_CAPACITY {
            buf.pop_front();
        }
        buf.push_back(line);
    }

    async fn snapshot(&self) -> Vec<String> {
        self.lines.lock().await.iter().cloned().collect()
    }
}

impl StdioTransport {
    pub(super) fn spawn(
        server_name: &str,
        command: &str,
        config: &McpServerConfig,
    ) -> Result<Self> {
        let mut cmd = tokio::process::Command::new(command);
        crate::utils::suppress_tokio_console_window(&mut cmd);
        cmd.args(&config.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }

        // 展开 `${NAME}` 占位符，使密钥环境变量值可以从
        // 进程环境获取，而不是以明文形式存储在 MCP 配置中。
        // 子进程环境已通过允许列表清理，因此如果不这样做，
        // 这些变量将不会被子进程继承。
        let expanded_env = super::expand_env_placeholders_map(&config.env, "env")
            .with_context(|| format!("MCP server '{server_name}' env expansion failed"))?;

        // MCP stdio 服务器是用户配置的集成。使用
        // 更广泛的 MCP 允许列表，使常见的 Node/Python/代理/CA 证书包
        // 引导变量（NVM_DIR, NODE_OPTIONS, NPM_CONFIG_*,
        // HTTP(S)_PROXY, …）能传递到子进程。参见 `sanitized_mcp_env`
        // 和 #1244 了解上下文。
        child_env::apply_to_tokio_command_mcp(&mut cmd, child_env::string_map_env(&expanded_env));

        let mut child = cmd.spawn().with_context(|| {
            let env_keys: Vec<&str> = expanded_env.keys().map(String::as_str).collect();
            format!(
                "MCP stdio spawn failed (transport=stdio server={server_name} cmd={command:?} args={:?} env_keys={env_keys:?})",
                config.args,
            )
        })?;

        let stdin = child.stdin.take().context("Failed to get MCP stdin")?;
        let stdout = child.stdout.take().context("Failed to get MCP stdout")?;
        let stderr = child.stderr.take().context("Failed to get MCP stderr")?;

        // 将 stderr 排入有界环形缓冲区，以便运行时崩溃留下
        // 诊断线索，而不是消失在 `Stdio::null` 中。
        // 当子进程关闭其 stderr 时，该任务自然退出
        //（kill_on_drop / exit / 显式关闭）。
        let stderr_tail = StderrTail::new();
        {
            let tail = Arc::clone(&stderr_tail);
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tail.push(line).await;
                }
            });
        }

        Ok(Self {
            child,
            stdin,
            reader: tokio::io::BufReader::new(stdout),
            stderr_tail,
        })
    }
}

/// 格式化捕获的 stderr 尾部以包含在错误消息中。空尾部
/// 返回 `None`，以便调用者回退到其原始消息。
async fn format_stderr_context(tail: &StderrTail) -> Option<String> {
    let lines = tail.snapshot().await;
    if lines.is_empty() {
        return None;
    }
    Some(format!(
        "MCP server stderr (last {} line{}):\n{}",
        lines.len(),
        if lines.len() == 1 { "" } else { "s" },
        lines.join("\n"),
    ))
}

/// 尽力发送 SIGTERM。在 Unix 上使用 `libc::kill`；在 Windows 上没有
/// 等效操作，因此让 `kill_on_drop`（TerminateProcess）通过后续的
/// Drop 处理。返回是否实际发送了信号。
fn send_sigterm(child: &Child) -> bool {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            // 安全保证：pid 刚刚从 `child.id()` 获取。`libc::kill`
            // 配合 `SIGTERM` 是异步信号安全的，永远不会访问无效
            // 内存。最坏情况（pid 回绕/进程已消失）返回
            // ESRCH，我们有意忽略它。
            unsafe {
                let _ = libc::kill(pid as i32, libc::SIGTERM);
            }
            return true;
        }
        false
    }
    #[cfg(not(unix))]
    {
        let _ = child;
        false
    }
}

#[async_trait::async_trait]
impl McpTransport for StdioTransport {
    async fn send(&mut self, mut msg: Vec<u8>) -> Result<()> {
        msg.push(b'\n');
        self.stdin.write_all(&msg).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn recv(&mut self) -> Result<Vec<u8>> {
        let mut line_bytes: Vec<u8> = Vec::new();
        loop {
            // 有界读取：服务器发出无换行符的多 GB "行"时
            // 不能 OOM 我们（read_line 是无界的）。
            let bytes = match read_line_capped(
                &mut self.reader,
                &mut line_bytes,
                super::MAX_MCP_RESPONSE_BYTES,
            )
            .await
            {
                Ok(b) => b,
                Err(err) => {
                    if let Some(stderr) = format_stderr_context(&self.stderr_tail).await {
                        anyhow::bail!("Stdio transport read error: {err}\n{stderr}");
                    }
                    return Err(err.into());
                }
            };
            if bytes == 0 {
                if let Some(stderr) = format_stderr_context(&self.stderr_tail).await {
                    anyhow::bail!("Stdio transport closed\n{stderr}");
                }
                anyhow::bail!("Stdio transport closed");
            }

            let line = String::from_utf8_lossy(&line_bytes);
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            return Ok(trimmed.as_bytes().to_vec());
        }
    }

    /// 发送 SIGTERM 并等待最多 `STDIO_SHUTDOWN_GRACE` 时间以便优雅退出，
    /// 然后让 Drop / `kill_on_drop` 作为兜底发送 SIGKILL。
    async fn shutdown(&mut self) {
        send_sigterm(&self.child);
        // 给子进程一个干净退出的窗口。丢弃结果——
        // 要么它退出（成功），要么超时触发（Drop 会发送 SIGKILL）。
        let _ = tokio::time::timeout(STDIO_SHUTDOWN_GRACE, self.child.wait()).await;
    }
}

/// Drop 回退（#420）：如果从未显式调用 `shutdown`，仍然在
/// tokio 的 `kill_on_drop` 发送 SIGKILL 之前发送 SIGTERM。两个信号
/// 连续到达，因此行为良好的服务器至少先看到 SIGTERM；
/// 行为不端的服务器无论如何都会被 SIGKILL。
impl Drop for StdioTransport {
    fn drop(&mut self) {
        send_sigterm(&self.child);
    }
}

/// 将一行以换行符结尾的数据读入 `out`（先清空），如果超过
/// `max` 字节而没有换行符则中止。限制了原本无界的 `read_line`，
/// 防止行为不端的 MCP 服务器 OOM 客户端。返回累积的字节数；
/// 0 表示 EOF。
async fn read_line_capped<R>(
    reader: &mut R,
    out: &mut Vec<u8>,
    max: usize,
) -> std::io::Result<usize>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    use tokio::io::AsyncBufReadExt;
    out.clear();
    loop {
        let (chunk, consumed, done) = {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                (Vec::new(), 0usize, true)
            } else if let Some(pos) = available.iter().position(|&b| b == b'\n') {
                (available[..=pos].to_vec(), pos + 1, true)
            } else {
                (available.to_vec(), available.len(), false)
            }
        };
        if consumed > 0 {
            reader.consume(consumed);
        }
        out.extend_from_slice(&chunk);
        if done {
            break;
        }
        if out.len() > max {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("MCP stdio line exceeded {max} bytes without a newline"),
            ));
        }
    }
    Ok(out.len())
}

#[cfg(test)]
mod read_cap_tests {
    use super::read_line_capped;

    #[tokio::test]
    async fn reads_a_line_and_reports_eof() {
        let data = b"hello\nworld\n".to_vec();
        let mut reader = tokio::io::BufReader::new(std::io::Cursor::new(data));
        let mut out = Vec::new();
        assert_eq!(
            read_line_capped(&mut reader, &mut out, 1024).await.unwrap(),
            6
        );
        assert_eq!(out, b"hello\n");
        assert_eq!(
            read_line_capped(&mut reader, &mut out, 1024).await.unwrap(),
            6
        );
        assert_eq!(out, b"world\n");
        // EOF。
        assert_eq!(
            read_line_capped(&mut reader, &mut out, 1024).await.unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn aborts_on_newline_free_line_over_cap() {
        let data = vec![b'x'; 4096]; // no newline
        let mut reader = tokio::io::BufReader::new(std::io::Cursor::new(data));
        let mut out = Vec::new();
        let err = read_line_capped(&mut reader, &mut out, 1024)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }
}
