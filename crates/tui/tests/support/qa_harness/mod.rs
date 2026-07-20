//! TUI 集成测试的最小 PTY/帧捕获测试框架。
//!
//! 在真实伪终端中启动 `codewhale-tui` 二进制文件，发送脚本化的按键/粘贴操作，
//! 并将 ANSI 输出流解析为终端帧，使测试能够断言可见文本和文件系统状态。
//!
//! 测试通过以下方式启用：
//! ```ignore
//! #[path = "support/qa_harness/mod.rs"]
//! mod qa_harness;
//! use qa_harness::harness::Harness;
//! use qa_harness::keys;
//! ```
//!
//! 设计说明位于本模块旁边的 `README.md` 中。

#![allow(dead_code)]

pub mod frame;
pub mod harness;
pub mod keys;
pub mod pty;

pub use frame::Frame;
pub use keys::paste;
pub use pty::PtySession;
