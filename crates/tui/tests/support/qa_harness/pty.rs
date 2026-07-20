//! 伪终端会话，包装 `portable-pty`。
//!
//! 在真实 PTY 中生成二进制文件，在后台线程中将子进程的 stdout
//! 泵入内存缓冲区，并公开测试框架组合使用的 write/wait/kill 原语。
//!
//! 读取线程是必要的，因为 `portable-pty` 的读取器是阻塞的，
//! 而测试线程必须保持自由以发送输入和轮询屏幕变化。

use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub struct PtySession {
    /// 持有（不读取）PTY master，使其在子进程生命周期内保持打开。
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    buffer: Arc<Mutex<Vec<u8>>>,
    reader_handle: Option<JoinHandle<()>>,
}

pub struct PtySessionBuilder<'a> {
    program: &'a Path,
    args: Vec<String>,
    cwd: Option<&'a Path>,
    env: Vec<(String, String)>,
    rows: u16,
    cols: u16,
    clear_env: bool,
}

impl<'a> PtySessionBuilder<'a> {
    pub fn new(program: &'a Path) -> Self {
        Self {
            program,
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            rows: 40,
            cols: 120,
            clear_env: false,
        }
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn cwd(mut self, p: &'a Path) -> Self {
        self.cwd = Some(p);
        self
    }

    pub fn env(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.env.push((k.into(), v.into()));
        self
    }

    /// 在应用显式的 `env(..)` 覆写之前清除继承的环境。
    /// 用于不得看到开发者真实 `~/.deepseek/`、`$HOME` 或 API 密钥的密封场景。
    pub fn clear_env(mut self, yes: bool) -> Self {
        self.clear_env = yes;
        self
    }

    pub fn size(mut self, rows: u16, cols: u16) -> Self {
        self.rows = rows;
        self.cols = cols;
        self
    }

    pub fn spawn(self) -> Result<PtySession> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: self.rows,
                cols: self.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("openpty")?;

        let mut cmd = CommandBuilder::new(self.program);
        for a in &self.args {
            cmd.arg(a);
        }
        if let Some(cwd) = self.cwd {
            cmd.cwd(cwd);
        }
        if self.clear_env {
            cmd.env_clear();
            if let Some(path) = std::env::var_os("PATH") {
                cmd.env("PATH", path);
            }
        }
        // TERM 必须设置为类似 xterm 的值，以便 crossterm 启用
        // TUI 所需的功能（256 色、bracketed paste 等）。
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        for (k, v) in &self.env {
            cmd.env(k, v);
        }

        let child = pair.slave.spawn_command(cmd).context("生成子进程")?;
        // 丢弃 slave 端，以便子进程退出时 EOF 正确传播。
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().context("克隆读取器")?;
        let writer = pair.master.take_writer().context("获取写入器")?;

        let buffer: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let buf_thread = Arc::clone(&buffer);
        let reader_handle = thread::Builder::new()
            .name("qa-pty-reader".into())
            .spawn(move || {
                let mut chunk = [0u8; 8192];
                loop {
                    match reader.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            if let Ok(mut b) = buf_thread.lock() {
                                b.extend_from_slice(&chunk[..n]);
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
            .context("读取器线程")?;

        Ok(PtySession {
            master: pair.master,
            child,
            writer,
            buffer,
            reader_handle: Some(reader_handle),
        })
    }
}

impl PtySession {
    pub fn builder(program: &Path) -> PtySessionBuilder<'_> {
        PtySessionBuilder::new(program)
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes).context("pty 写入")?;
        self.writer.flush().context("pty 刷新")?;
        Ok(())
    }

    /// 清空读取器线程已推入缓冲区的所有字节。返回
    /// 此调用读取的字节。非阻塞——即使缓冲区为空也立即返回。
    pub fn drain(&mut self) -> Vec<u8> {
        let mut b = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *b)
    }

    /// 阻塞直到子进程退出或超过截止时间。如果已回收则返回退出
    /// 状态，超时返回 `None`。
    pub fn wait_until(&mut self, deadline: Instant) -> Option<i32> {
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return Some(status.exit_code() as i32),
                Ok(None) => {}
                Err(_) => return None,
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// 发送 SIGTERM 等效信号并短暂等待。如果在 `grace` 内回收了
    /// 子进程则返回退出状态，否则返回 `None`。
    pub fn shutdown(mut self, grace: Duration) -> Option<i32> {
        self.kill_and_join_reader(grace)
    }

    fn kill_and_join_reader(&mut self, grace: Duration) -> Option<i32> {
        let _ = self.child.kill();
        let exit = self.wait_until(Instant::now() + grace);
        if exit.is_some()
            && let Some(handle) = self.reader_handle.take()
        {
            // 不要永远阻塞读取器线程——它在 EOF 时退出。
            let _ = handle.join();
        }
        exit
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.kill_and_join_reader(Duration::from_secs(2));
    }
}
