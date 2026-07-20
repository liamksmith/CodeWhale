//! 按键和粘贴的字节序列构建器。
//!
//! 这些函数生成真实终端会传递给子进程 PTY 从设备的原始字节。
//! 它们匹配 crossterm 的输入解码表（键盘增强关闭、鼠标捕获关闭、括号粘贴开启）。

/// 普通按键辅助函数。
pub mod key {
    pub fn ch(c: char) -> Vec<u8> {
        let mut buf = [0u8; 4];
        c.encode_utf8(&mut buf).as_bytes().to_vec()
    }

    pub fn enter() -> Vec<u8> {
        b"\r".to_vec()
    }

    pub fn text(s: &str) -> Vec<u8> {
        s.as_bytes().to_vec()
    }
}

/// 括号粘贴辅助函数。
///
/// 将有效负载包装在 `ESC [ 2 0 0 ~` … `ESC [ 2 0 1 ~` 中，使接收者看到
/// `crossterm::Event::Paste(text)` 而非逐键流。
pub mod paste {
    pub fn bracketed(text: &str) -> Vec<u8> {
        let mut out = b"\x1b[200~".to_vec();
        out.extend_from_slice(text.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    }

    /// 与 [`bracketed`] 相同但不包装 — 模拟禁用了括号粘贴的终端（例如某些 Windows PowerShell 环境）。
    /// 子进程将字节视为普通按键；嵌入的 `\n` 变为回车键，这正好复现了 #1073。
    pub fn unbracketed(text: &str) -> Vec<u8> {
        text.replace('\n', "\r").as_bytes().to_vec()
    }
}
