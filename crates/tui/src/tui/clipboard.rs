//! TUI 中粘贴支持的剪贴板处理。
//!
//! 支持文本和图像粘贴操作。剪贴板上的图像编码为 PNG
//! 并持久化到 `~/.codewhale/clipboard-images/`，以便模型可以通过
//! 现有的 `@`-提及/文件工具访问它们（DeepSeek V4 目前不接受
//! 其 Chat Completions 端点上的内联图像输入，因此我们将字节具体化
//! 到磁盘，而不是在请求中 base64 嵌入它们）。

#[cfg(not(test))]
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
#[cfg(any(
    all(test, unix),
    all(not(test), target_os = "macos"),
    all(not(test), target_os = "windows"),
    all(not(test), target_os = "linux", not(target_env = "ohos"))
))]
use std::process::{Command, Stdio};
#[cfg(any(
    test,
    target_os = "macos",
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos"))
))]
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
#[cfg(any(
    test,
    target_os = "macos",
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos"))
))]
use arboard::{Clipboard, ImageData};
use base64::Engine as _;
#[cfg(any(
    test,
    target_os = "macos",
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos"))
))]
use image::{ImageBuffer, Rgba};

const OSC52_MAX_BYTES: usize = 100 * 1024;

// === 类型 ===

/// 为粘贴的剪贴板图像捕获的元数据。由编辑器用于渲染
/// 类似 `Pasted 1024x768 image (235KB) → <path>` 的状态提示。
#[derive(Clone)]
pub struct PastedImage {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub byte_len: usize,
}

impl PastedImage {
    /// 简短的人类可读摘要，例如 `1024x768 PNG`。
    pub fn short_label(&self) -> String {
        format!("{}x{} PNG", self.width, self.height)
    }

    /// 近似文件大小后缀，例如 `235KB`。
    pub fn size_label(&self) -> String {
        let kb = (self.byte_len as f64 / 1024.0).round() as u64;
        format!("{kb}KB")
    }
}

/// TUI 支持的剪贴板负载。
#[cfg_attr(
    all(any(target_env = "ohos", target_os = "android"), not(test)),
    allow(dead_code)
)]
pub enum ClipboardContent {
    Text(String),
    Image(PastedImage),
}

/// 剪贴板读取/写入辅助程序。
pub struct ClipboardHandler {
    #[cfg(any(
        test,
        target_os = "macos",
        target_os = "windows",
        all(target_os = "linux", not(target_env = "ohos"))
    ))]
    clipboard: Option<Clipboard>,
    #[cfg(any(
        test,
        target_os = "macos",
        target_os = "windows",
        all(target_os = "linux", not(target_env = "ohos"))
    ))]
    clipboard_init_attempted: bool,
    #[cfg(test)]
    written_text: Vec<String>,
}

impl ClipboardHandler {
    /// 创建新的剪贴板处理程序而不连接。
    ///
    /// 实际的剪贴板连接延迟到首次使用（`ensure_clipboard`），
    /// 因此在没有 X11/Wayland 服务器的主机上（无头、WSL2）
    /// 启动时从不阻塞 TUI 事件循环。
    pub fn new() -> Self {
        Self {
            #[cfg(any(
                test,
                target_os = "macos",
                target_os = "windows",
                all(target_os = "linux", not(target_env = "ohos"))
            ))]
            clipboard: None,
            #[cfg(any(
                test,
                target_os = "macos",
                target_os = "windows",
                all(target_os = "linux", not(target_env = "ohos"))
            ))]
            clipboard_init_attempted: false,
            #[cfg(test)]
            written_text: Vec::new(),
        }
    }

    /// 尝试连接到系统剪贴板，带有短超时。
    ///
    /// 在 Linux 上，`arboard::Clipboard::new()` 打开阻塞的 X11 连接。
    /// 当没有 X 服务器运行时（无头、没有 WSLg 的 WSL2），连接调用
    /// 可能会无限挂起。我们在临时线程上生成连接尝试并给它
    /// 500 毫秒；如果它没有及时返回，处理程序保持在回退/空操作模式，
    /// `read`/`write_text` 回退到它们的 OSC 52 和 pbcopy/powershell 回退。
    #[cfg(any(
        test,
        target_os = "macos",
        target_os = "windows",
        all(target_os = "linux", not(target_env = "ohos"))
    ))]
    fn ensure_clipboard(&mut self) {
        if self.clipboard_init_attempted {
            return;
        }
        self.clipboard_init_attempted = true;

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(Clipboard::new().ok());
        });
        self.clipboard = rx
            .recv_timeout(std::time::Duration::from_millis(500))
            .ok()
            .flatten();
    }

    /// 读取剪贴板并返回解析的内容。
    ///
    /// `workspace` 在 `~/.codewhale/` 无法解析时用作回退位置
    ///（例如在 CI 沙箱中使用剥离的 HOME 运行）。
    pub fn read(&mut self, workspace: &Path) -> Option<ClipboardContent> {
        #[cfg(all(target_os = "linux", not(target_env = "ohos"), not(test)))]
        if let Ok(text) = read_text_with_wlpaste() {
            return Some(ClipboardContent::Text(text));
        }

        #[cfg(any(
            test,
            target_os = "macos",
            target_os = "windows",
            all(target_os = "linux", not(target_env = "ohos"))
        ))]
        {
            self.ensure_clipboard();
            let clipboard = self.clipboard.as_mut()?;
            if let Ok(text) = clipboard.get_text() {
                return Some(ClipboardContent::Text(text));
            }

            if let Ok(image) = clipboard.get_image()
                && let Ok(pasted) = save_image_as_png(workspace, &image)
            {
                return Some(ClipboardContent::Image(pasted));
            }
        }

        let _ = workspace;
        None
    }

    /// 将文本写入剪贴板（如果不可用则为空操作）。
    pub fn write_text(&mut self, text: &str) -> Result<()> {
        #[cfg(test)]
        {
            self.written_text.push(text.to_string());
            Ok(())
        }

        #[cfg(not(test))]
        {
            #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
            if write_text_with_wlcopy(text).is_ok() {
                return Ok(());
            }

            #[cfg(any(
                target_os = "macos",
                target_os = "windows",
                all(target_os = "linux", not(target_env = "ohos"))
            ))]
            {
                self.ensure_clipboard();
                if let Some(clipboard) = self.clipboard.as_mut()
                    && clipboard.set_text(text.to_string()).is_ok()
                {
                    return Ok(());
                }
            }

            #[cfg(target_os = "macos")]
            if write_text_with_pbcopy(text).is_ok() {
                return Ok(());
            }

            #[cfg(target_os = "windows")]
            if write_text_with_set_clipboard(text).is_ok() {
                return Ok(());
            }

            write_text_with_osc52(text)
                .map_err(|err| anyhow::anyhow!("剪贴板不可用: {err}"))
        }
    }

    #[cfg(test)]
    pub fn last_written_text(&self) -> Option<&str> {
        self.written_text.last().map(String::as_str)
    }
}

#[cfg(all(target_os = "macos", not(test)))]
fn write_text_with_pbcopy(text: &str) -> Result<()> {
    write_text_with_stdin_command("pbcopy", &[], text, "pbcopy")
}

#[cfg(all(target_os = "windows", not(test)))]
fn write_text_with_set_clipboard(text: &str) -> Result<()> {
    write_text_with_stdin_command(
        "powershell.exe",
        &["-NoProfile", "-Command", "Set-Clipboard -Value $input"],
        text,
        "Set-Clipboard",
    )
}

#[cfg(all(any(target_os = "macos", target_os = "windows"), not(test)))]
fn write_text_with_stdin_command(
    program: &str,
    args: &[&str],
    text: &str,
    label: &str,
) -> Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow::anyhow!("运行 {label} 失败: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| anyhow::anyhow!("写入 {label} 失败: {e}"))?;
    }
    let _ = std::thread::Builder::new()
        .name("clipboard-wait".to_string())
        .spawn(move || {
            let _ = child.wait();
        });
    Ok(())
}

#[cfg(all(target_os = "linux", not(target_env = "ohos"), not(test)))]
fn write_text_with_wlcopy(text: &str) -> Result<()> {
    write_text_with_wlcopy_using_argv("wl-copy", text)
}

#[cfg(all(target_os = "linux", not(target_env = "ohos"), not(test)))]
fn read_text_with_wlpaste() -> Result<String> {
    read_text_with_wlpaste_using_argv("wl-paste")
}

#[cfg(any(all(test, unix), all(target_os = "linux", not(target_env = "ohos"))))]
fn read_text_with_wlpaste_using_argv(program: &str) -> Result<String> {
    let output = Command::new(program)
        .arg("--no-newline")
        .arg("--type")
        .arg("text/plain")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| anyhow::anyhow!("运行 {program} 失败: {e}"))?;
    if !output.status.success() {
        bail!("{program} 退出状态 {}", output.status);
    }
    String::from_utf8(output.stdout).context("wl-paste 返回了非 UTF-8 文本")
}

#[cfg(all(target_os = "linux", not(target_env = "ohos"), not(test)))]
fn write_text_with_wlcopy_using_argv(program: &str, text: &str) -> Result<()> {
    let mut child = Command::new(program)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow::anyhow!("运行 {program} 失败: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| anyhow::anyhow!("写入 {program} 失败: {e}"))?;
    }
    // stdin 在此处被丢弃，关闭管道以便 wl-copy 刷新。
    let status = child
        .wait()
        .map_err(|e| anyhow::anyhow!("等待 {program} 失败: {e}"))?;
    if !status.success() {
        bail!("{program} 退出状态 {status}");
    }
    Ok(())
}

#[cfg(not(test))]
fn write_text_with_osc52(text: &str) -> Result<()> {
    let mut stdout = io::stdout();
    if !stdout.is_terminal() {
        bail!("OSC 52 剪贴板回退需要终端");
    }

    let in_tmux = std::env::var_os("TMUX").is_some();
    let sequence = osc52_sequence(text, in_tmux)?;
    stdout
        .write_all(sequence.as_bytes())
        .context("写入 OSC 52 剪贴板序列")?;
    stdout.flush().context("刷新 OSC 52 剪贴板序列")
}

fn osc52_sequence(text: &str, in_tmux: bool) -> Result<String> {
    if text.len() > OSC52_MAX_BYTES {
        bail!("选择对于 OSC 52 剪贴板回退过大");
    }

    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let sequence = format!("\x1b]52;c;{encoded}\x07");
    if in_tmux {
        return Ok(format!("\x1bPtmux;\x1b{sequence}\x1b\\"));
    }
    Ok(sequence)
}

/// 解析粘贴图像应放置的目录。优先选择
/// `~/.codewhale/clipboard-images/`，以便路径在各工作树中稳定
/// 并与用户面向文档中描述的位置匹配；如果 home 目录不可用，
/// 回退到 `<workspace>/clipboard-images/`。
pub(crate) fn clipboard_images_dir(workspace: &Path) -> PathBuf {
    let home = dirs::home_dir();
    clipboard_images_dir_for_home(workspace, home.as_deref())
}

fn clipboard_images_dir_for_home(workspace: &Path, home: Option<&Path>) -> PathBuf {
    if let Some(home) = home {
        return home.join(".codewhale").join("clipboard-images");
    }
    workspace.join("clipboard-images")
}

/// 将 arboard 的 RGBA `ImageData` 编码为 PNG 并持久化。返回
/// 结果路径以及用于渲染粘贴提示的元数据。
#[cfg(any(
    test,
    target_os = "macos",
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos"))
))]
fn save_image_as_png(workspace: &Path, image: &ImageData) -> Result<PastedImage> {
    save_image_as_png_in(&clipboard_images_dir(workspace), image)
}

/// 较低级别的变体，写入显式目录。暴露以便
/// 单元测试不必在用户真实的 home 目录中乱写。
#[cfg(any(
    test,
    target_os = "macos",
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos"))
))]
fn save_image_as_png_in(dir: &Path, image: &ImageData) -> Result<PastedImage> {
    std::fs::create_dir_all(dir).context("创建 clipboard-images 目录")?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = dir.join(format!("clipboard-{timestamp}.png"));

    let width = u32::try_from(image.width).context("剪贴板图像宽度过大")?;
    let height = u32::try_from(image.height).context("剪贴板图像高度过大")?;

    // arboard 给我们 RGBA8 行优先。复制到 ImageBuffer 中，
    // 以便通过 `image` crate 的 PNG 编码器运行。
    // 我们填充/截断任何不匹配的尾随字节——仅为防御，
    // arboard 已在每个支持的后端验证了缓冲区长度。
    let expected = (width as usize) * (height as usize) * 4;
    let mut rgba = image.bytes.as_ref().to_vec();
    if rgba.len() < expected {
        rgba.resize(expected, 0);
    } else if rgba.len() > expected {
        rgba.truncate(expected);
    }

    let buffer: ImageBuffer<Rgba<u8>, _> = ImageBuffer::from_raw(width, height, rgba)
        .context("剪贴板图像尺寸与缓冲区长度不匹配")?;
    buffer
        .save_with_format(&path, image::ImageFormat::Png)
        .context("写入剪贴板 PNG")?;

    let byte_len = std::fs::metadata(&path)
        .map(|m| m.len() as usize)
        .unwrap_or(0);
    Ok(PastedImage {
        path,
        width,
        height,
        byte_len,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn solid_rgba(width: u16, height: u16, rgba: [u8; 4]) -> ImageData<'static> {
        let mut bytes = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for _ in 0..(width as usize * height as usize) {
            bytes.extend_from_slice(&rgba);
        }
        ImageData {
            width: width as usize,
            height: height as usize,
            bytes: Cow::Owned(bytes),
        }
    }

    #[test]
    fn save_image_as_png_writes_valid_png() {
        let dir = tempfile::tempdir().unwrap();
        let img = solid_rgba(8, 4, [255, 0, 0, 255]);
        let pasted = save_image_as_png_in(dir.path(), &img).expect("编码 png");

        assert_eq!(pasted.width, 8);
        assert_eq!(pasted.height, 4);
        assert!(pasted.byte_len > 0);
        assert_eq!(
            pasted.path.extension().and_then(|s| s.to_str()),
            Some("png")
        );

        // 任何 PNG 文件的前八个字节是魔数签名；如果
        // 我们曾经回归到 PPM 或其他格式，这将捕获它。
        let header = std::fs::read(&pasted.path).unwrap();
        assert_eq!(&header[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn clipboard_images_dir_uses_codewhale_home_directory() {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();

        assert_eq!(
            clipboard_images_dir_for_home(workspace.path(), Some(home.path())),
            home.path().join(".codewhale").join("clipboard-images")
        );
    }

    #[test]
    fn clipboard_images_dir_falls_back_to_workspace_without_home() {
        let workspace = tempfile::tempdir().unwrap();

        assert_eq!(
            clipboard_images_dir_for_home(workspace.path(), None),
            workspace.path().join("clipboard-images")
        );
    }

    #[test]
    fn pasted_image_labels_format_correctly() {
        let p = PastedImage {
            path: PathBuf::from("/tmp/x.png"),
            width: 1024,
            height: 768,
            byte_len: 235 * 1024,
        };
        assert_eq!(p.short_label(), "1024x768 PNG");
        assert_eq!(p.size_label(), "235KB");
    }

    #[test]
    fn osc52_sequence_encodes_text_clipboard_write() {
        let sequence = osc52_sequence("hello", false).expect("sequence");
        assert_eq!(sequence, "\x1b]52;c;aGVsbG8=\x07");
    }

    #[test]
    fn osc52_sequence_wraps_for_tmux_passthrough() {
        let sequence = osc52_sequence("copy", true).expect("sequence");
        assert_eq!(sequence, "\x1bPtmux;\x1b\x1b]52;c;Y29weQ==\x07\x1b\\");
    }

    #[test]
    fn osc52_sequence_rejects_oversized_selection() {
        let text = "x".repeat(OSC52_MAX_BYTES + 1);
        let err = osc52_sequence(&text, false).expect_err("过大应失败");
        assert!(
            err.to_string().contains("too large"),
            "意外错误: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn wl_paste_helper_reads_text_from_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("wl-paste");
        std::fs::write(
            &script,
            r#"#!/bin/sh
seen_no_newline=0
seen_text_plain=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --no-newline) seen_no_newline=1 ;;
    --type)
      shift
      [ "${1:-}" = "text/plain" ] && seen_text_plain=1
      ;;
  esac
  shift
done
[ "$seen_text_plain" -eq 1 ] || exit 40
if [ "$seen_no_newline" -eq 1 ]; then
  printf 'from-wayland'
else
  printf 'from-wayland\n'
fi
"#,
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        let text = read_text_with_wlpaste_using_argv(script.to_str().unwrap())
            .expect("通过 wl-paste helper 读取文本");

        assert_eq!(text, "from-wayland");
    }
}
