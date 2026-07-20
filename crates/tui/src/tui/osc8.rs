//! OSC 8 超链接的发出和剥离。
//!
//! 现代终端（iTerm2、Terminal.app 13+、Ghostty、Kitty、WezTerm、
//! Alacritty、较新的 gnome-terminal/konsole）在子字符串被包装为
//! 以下格式时使其可点击：
//!
//! ```text
//! \x1b]8;;TARGET\x1b\\LABEL\x1b]8;;\x1b\\
//! ```
//!
//! 不理解此序列的终端只渲染可见的 `LABEL` 并忽略转义。因此发出
//! OSC 8 对支持终端来说是严格的 UX 升级，对其他终端是无操作。
//!
//! # 架构（#3029）
//!
//! Markdown 渲染器通过 [`wrap_link`] 将链接载荷*内联*嵌入 `Span::content`
//! 内部。ratatui 的缓冲区管道丢弃前导的 `ESC` 字节，但将载荷的
//! 其余内容每字节一单元格地绘制，这会破坏列对齐。因此每个渲染接缝
//! 在 `Paragraph::render` 之后调用 [`extract_buffer_link_regions`]：
//! 它恢复每个链接的目标 + 标签显示列，将载荷单元格清空（没有单元格
//! 会包含 `\x1b` 或 `]8;;`），并将 [`LinkRegion`] 发布到线程本地。
//! 然后 `ColorCompatBackend::draw` 消费这些区域并通过后端的 `Write`
//! 实现*带外*发出 OSC 8 转义——与单元格流交错发送，永远不会在缓冲区
//! 单元格内部。内联路径是链接信息的来源；带外路径才是到达终端的。
//!
//! 剪贴板/选择提取路径仍然通过 [`strip_into`] / [`strip_ansi_into`]
//! 剥离任何残留的转义码，作为深度防御。

use std::sync::atomic::{AtomicBool, Ordering};

const OSC8_PREFIX: &str = "\x1b]8;;";
const OSC8_TERMINATOR: &str = "\x1b\\";
const OSC8_CLOSE: &str = "\x1b]8;;\x1b\\";

/// 在一个终端行上共享一个超链接目标的连续单元格序列。
#[derive(Debug, Clone)]
pub struct LinkRegion {
    pub row: u16,
    pub col_start: u16,
    pub col_end: u16,
    pub target: String,
}

/// 将 `target` 的 OSC 8 超链接打开序列写入 `w`。
pub fn write_osc8_open(w: &mut impl std::io::Write, target: &str) -> std::io::Result<()> {
    w.write_all(OSC8_PREFIX.as_bytes())?;
    w.write_all(target.as_bytes())?;
    w.write_all(OSC8_TERMINATOR.as_bytes())
}

/// 将 OSC 8 超链接关闭序列写入 `w`。
pub fn write_osc8_close(w: &mut impl std::io::Write) -> std::io::Result<()> {
    w.write_all(OSC8_CLOSE.as_bytes())
}

/// 进程级启用标志。在应用初始化时从 `[tui] osc8_links` 设置一次
///（当存在时）；否则默认在 macOS/Linux 上启用，在 Windows 旧版
/// 控制台上禁用（参见 `ui.rs` 中的 `osc8_default_on`）。由渲染器
/// 读取以控制带外 OSC 8 的发出。
static ENABLED: AtomicBool = AtomicBool::new(true);

/// 设置进程级 OSC 8 启用标志。旨在启动时调用一次；
/// 后续调用立即可见效。
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// OSC 8 超链接发出当前是否已启用。
#[must_use]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

// --- 线程本地链接区域累加器（#3029）---

use std::cell::RefCell;

thread_local! {
    /// 在当前渲染帧期间收集的链接区域。
    /// 由渲染闭包在扫描 ratatui 缓冲区后填充；
    /// 由 `ColorCompatBackend::draw()` 消费并清除。
    pub static FRAME_LINKS: RefCell<Vec<LinkRegion>> = const { RefCell::new(Vec::new()) };
}

/// 用 `links` 替换线程本地帧链接缓冲区。
pub fn set_frame_links(links: Vec<LinkRegion>) {
    FRAME_LINKS.with(|cell| {
        *cell.borrow_mut() = links;
    });
}

/// 将 `links` 追加到线程本地帧链接缓冲区。当多个部件将包含链接的
/// 内容渲染到同一个帧时使用（例如，主抄本和活跃抄本覆盖层）：
/// 每个接缝追加而不是替换，以便所有区域都能到达 `ColorCompatBackend::draw`。
pub fn append_frame_links(links: Vec<LinkRegion>) {
    FRAME_LINKS.with(|cell| cell.borrow_mut().extend(links));
}

/// 取出线程本地帧链接，留下空 vec。
pub fn take_frame_links() -> Vec<LinkRegion> {
    FRAME_LINKS.with(|cell| std::mem::take(&mut *cell.borrow_mut()))
}

// --- 内联载荷提取（#3029）---
//
// Markdown 渲染器通过 [`wrap_link`] 将 OSC 8 超链接*内联*嵌入 `Span`
// 内容中。ratatui 的缓冲区管道丢弃前导的 `ESC` 字节，但将载荷的
// 每个其他字节绘制到自己的单元格中，这会漂移列并破坏可见字形流。
// 我们不是将结构化的链接元数据传递整个渲染管道，而是在每次
// `Paragraph::render` 之后扫描渲染的 `Buffer`，并且：
//
//   1. 恢复每个链接的目标及其标签的显示列范围，和
//   2. 清空载荷单元格（`]8;;`、目标、终止符），只留下干净的标签。
//
// 恢复的 [`LinkRegion`] 被传递给 [`set_frame_links`] /
// [`append_frame_links`]；`ColorCompatBackend::draw` 消费它们并
// 通过后端的 `Write` 实现*带外*发出 OSC 8 转义，因此没有载荷字节
// 会到达缓冲区单元格。这通过构造满足了 #3029 验收标准
//（"没有 Buffer 单元格包含 `\x1b` 或 `]8;;`"）。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

/// OSC 8 打开前缀 `ESC ] 8 ; ;` 在 ratatui 剥离前导 ESC 后的四个单元格：`]`、`8`、`;`、`;`。
const OPEN_CELLS: [char; 4] = [']', '8', ';', ';'];

/// 扫描 `buf` 的 `area` 以查找内联 OSC 8 链接载荷，清空其载荷
/// 单元格，并为每个恢复的链接返回一个 [`LinkRegion`]（位于标签的
/// 显示列上，使用绝对缓冲区坐标）。
///
/// 缓冲区中的完整载荷（ESC 已被 ratatui 剥离）看起来像
/// `]8;;TARGET\LABEL]8;;\`——四个打开单元格、目标字节、`\`
/// 终止符、可见标签，然后四个单元格的关闭 `]8;;\`。如果
/// 关闭部分丢失（例如载荷被换行截断），整个运行被视为损坏：
/// 单元格被清空但不发出区域，因为半链接比没有链接更糟糕。
///
/// `row`/`col_start`/`col_end` 是绝对缓冲区坐标（包含
/// `area.x`/`area.y`），与 `ColorCompatBackend::draw` 测试的内容匹配。
#[must_use]
pub fn extract_buffer_link_regions(buf: &mut Buffer, area: Rect) -> Vec<LinkRegion> {
    let mut regions = Vec::new();
    let x_start = area.x;
    let x_end = area.x.saturating_add(area.width);
    let y_start = area.y;
    let y_end = area.y.saturating_add(area.height);

    for y in y_start..y_end {
        let mut x = x_start;
        while x < x_end {
            // 在当前列查找打开前缀 `]8;;`。
            if matches_open(buf, x, y, x_end) {
                let payload_start = x;
                // 跳过 4 个打开单元格，然后消费目标直到 `\`。
                let mut scan = x + OPEN_CELLS.len() as u16;
                let mut target = String::new();
                let mut found_target_term = false;
                while scan < x_end {
                    let ch = cell_char(buf, scan, y);
                    scan += 1;
                    if ch == '\\' {
                        found_target_term = true;
                        break;
                    }
                    target.push(ch);
                }
                if !found_target_term {
                    // 未终止的载荷：清空我们可以确定是载荷的内容
                    //（打开前缀）并放弃此运行——其余内容可能是
                    // 我们不能破坏的合法内容。
                    blank_cells(buf, payload_start..payload_start + 4, y);
                    x = scan;
                    continue;
                }
                let label_start = scan;
                // 消费标签单元格直到关闭前缀 `]8;;\`。`scan`
                // 一次前进一个单元格；当下四个单元格拼出 `]8;;`
                // 且第五个是 `\` 时，标签就在它们之前结束。
                let mut found_close = false;
                while scan + 4 < x_end {
                    if matches_open(buf, scan, y, x_end) && cell_char(buf, scan + 4, y) == '\\' {
                        found_close = true;
                        break;
                    }
                    scan += 1;
                }
                // `scan` 现在要么在关闭前缀处（已找到），要么已过
                // 行尾（未找到）；两种情况下标签占据
                // `label_start..scan`（不包含结束）。
                if !found_close {
                    // 行内无关闭：清空打开+目标+终止符和部分标签，不产生区域。
                    blank_cells(buf, payload_start..scan, y);
                    x = scan;
                    continue;
                }
                let close_start = scan;
                let close_end = scan + (OPEN_CELLS.len() as u16) + 1; // `]8;;` + `\`
                // 在标签的列上记录区域。LinkRegion 使用
                // 包含结束坐标，匹配 ColorCompatBackend 的
                // `x >= col_start && x <= col_end` 测试。跳过空标签。
                if scan > label_start {
                    regions.push(LinkRegion {
                        row: y,
                        col_start: label_start,
                        col_end: scan - 1,
                        target,
                    });
                }
                // 清空标签*周围*的载荷单元格，从不清空标签本身的单元格：
                // 打开前缀 + 目标 + 第一个 `\`，然后关闭 `]8;;\`。
                // `label_start..scan` 中的标签单元格保持完整，
                // 这样可见字形流不变。
                blank_cells(buf, payload_start..label_start, y);
                blank_cells(buf, close_start..close_end, y);
                x = close_end;
                continue;
            }
            x += 1;
        }
    }
    regions
}

/// 从 `(x, y)` 开始的四个单元格是否拼出 OSC 8 打开前缀 `]8;;`（限制到 `x_end`）。
fn matches_open(buf: &Buffer, x: u16, y: u16, x_end: u16) -> bool {
    if x.saturating_add(OPEN_CELLS.len() as u16) > x_end {
        return false;
    }
    OPEN_CELLS
        .iter()
        .enumerate()
        .all(|(i, want)| cell_char(buf, x + i as u16, y) == *want)
}

/// `(x, y)` 处符号的第一个字符（载荷字节是 ASCII，因此单元格符号是单个字符）。
/// 对空单元格返回 `'\0'`，以便它们永远不会错误匹配载荷字符。
fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
    let sym = buf[(x, y)].symbol();
    sym.chars().next().unwrap_or('\0')
}

/// 将行 `y` 上 `cols`（相对于绝对 `x`）中的单元格重置为空白空格，清除任何载荷字节。
fn blank_cells(buf: &mut Buffer, cols: std::ops::Range<u16>, y: u16) {
    for x in cols {
        if let Some(cell) = buf.cell_mut(ratatui::layout::Position { x, y }) {
            cell.set_symbol(" ");
        }
    }
}

/// 包装 `label`，使其在支持 OSC 8 的终端中链接到 `target`。返回的
/// 字符串包含完整的 `\x1b]8;;TARGET\x1b\LABEL\x1b]8;;\x1b\` 载荷。
///
/// **不**检查 [`enabled()`]；想要运行时门控的调用方应在调用之前
/// 对其分支。这使辅助函数保持对测试友好。
#[must_use]
pub fn wrap_link(target: &str, label: &str) -> String {
    let mut out = String::with_capacity(target.len() + label.len() + 12);
    out.push_str(OSC8_PREFIX);
    out.push_str(target);
    out.push_str(OSC8_TERMINATOR);
    out.push_str(label);
    out.push_str(OSC8_PREFIX);
    out.push_str(OSC8_TERMINATOR);
    out
}

/// 从 `s` 中剥离每个 ANSI 转义序列到 `out`，只保留可见字符。
/// ratatui 的缓冲区丢弃前导的 `ESC` 字节，但愉快地将转义的其他
/// 每个字节（`[`、`0`、`;`、`m`、OSC 载荷等）绘制到缓冲区单元格中，
/// 漂移列。包含 ANSI 的工具 stdout（例如强制着色的 `gh`/`git`、
/// 通过 PTY 运行的任何内容）必须在进入抄本前进行清理。
///
/// 处理 CSI（`ESC [ … final`）、OSC（`ESC ] … BEL` 或 `ESC \`）、DCS、SOS、
/// PM、APC 和独立的两字节 ESC 序列。OSC 8 超链接包装
///（`ESC ] 8 ; … BEL` / `ESC \`）与其他序列一起被剥离。
pub fn strip_ansi_into(s: &str, out: &mut String) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() {
            let next = bytes[i + 1];
            match next {
                // CSI: ESC [ ... <final byte 0x40..=0x7E>
                b'[' => {
                    let mut j = i + 2;
                    while j < bytes.len() {
                        let b = bytes[j];
                        if (0x40..=0x7e).contains(&b) {
                            j += 1;
                            break;
                        }
                        j += 1;
                    }
                    i = j;
                    continue;
                }
                // OSC / DCS / SOS / PM / APC: ESC ] | P | X | ^ | _ ... ST(ESC \) or BEL
                b']' | b'P' | b'X' | b'^' | b'_' => {
                    let mut j = i + 2;
                    while j < bytes.len() {
                        if bytes[j] == 0x07 {
                            j += 1;
                            break;
                        }
                        if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                            j += 2;
                            break;
                        }
                        j += 1;
                    }
                    i = j;
                    continue;
                }
                // 独立的两字节 ESC 序列（RIS、字符集选择等）
                _ => {
                    i += 2;
                    continue;
                }
            }
        }
        // 剥离 ratatui 否则会丢弃的孤立控制字节（在抄本输出中
        // 没有意义），但保留 \n、\r、\t 作为合法的格式化字符。
        let b = bytes[i];
        if b < 0x80 {
            if b < 0x20 && b != b'\n' && b != b'\r' && b != b'\t' {
                i += 1;
                continue;
            }
            out.push(b as char);
            i += 1;
        } else {
            // UTF-8 多字节序列：完整复制整个码点。
            // 将 `b` 作为 char 推入会将其误解码为 Latin-1 并破坏
            // 非 ASCII 文本（中文、重音拉丁文、表情符号等）。
            let len = utf8_seq_len(b);
            let end = (i + len).min(bytes.len());
            if let Ok(chunk) = std::str::from_utf8(&bytes[i..end]) {
                out.push_str(chunk);
            }
            i = end;
        }
    }
}

/// 以 `lead` 开头的 UTF-8 序列的字节长度。对于续字节/无效前导字节
/// 回退到 `1`，以便调用方总能向前推进。
fn utf8_seq_len(lead: u8) -> usize {
    if lead < 0xc0 {
        1
    } else if lead < 0xe0 {
        2
    } else if lead < 0xf0 {
        3
    } else {
        4
    }
}

/// 从 `s` 中剥离 OSC 8 转义序列到 `out`，保留可见的标签文本。
/// 其他转义（颜色、样式）原样通过。实现处理标准的 `ESC \` 和
/// 一些发射器使用的孤立的 `BEL` 终止符。
pub fn strip_into(s: &str, out: &mut String) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // 查找 OSC 8 前缀 `ESC ] 8 ;`
        if i + 4 <= bytes.len()
            && bytes[i] == 0x1b
            && bytes[i + 1] == b']'
            && bytes[i + 2] == b'8'
            && bytes[i + 3] == b';'
        {
            // 跳过直到字符串终止符（ESC \）或 BEL。
            let mut j = i + 4;
            while j < bytes.len() {
                if bytes[j] == 0x07 {
                    j += 1;
                    break;
                }
                if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                    j += 2;
                    break;
                }
                j += 1;
            }
            i = j;
            continue;
        }
        let b = bytes[i];
        if b < 0x80 {
            out.push(b as char);
            i += 1;
        } else {
            let len = utf8_seq_len(b);
            let end = (i + len).min(bytes.len());
            if let Ok(chunk) = std::str::from_utf8(&bytes[i..end]) {
                out.push_str(chunk);
            }
            i = end;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 序列化读取或写入 `ENABLED` 标志的测试，使它们在 cargo
    /// 默认的并行测试运行程序下不会相互竞争。
    static FLAG_GUARD: Mutex<()> = Mutex::new(());

    fn strip(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        strip_into(s, &mut out);
        out
    }

    #[test]
    fn wrap_link_shape_is_osc_8_compliant() {
        let wrapped = wrap_link("https://example.com", "click me");
        assert_eq!(
            wrapped,
            "\x1b]8;;https://example.com\x1b\\click me\x1b]8;;\x1b\\"
        );
    }

    #[test]
    fn strip_removes_wrapper_keeps_label() {
        let wrapped = wrap_link("https://example.com", "click me");
        assert_eq!(strip(&wrapped), "click me");
    }

    #[test]
    fn strip_handles_bel_terminator() {
        let wrapped = "\x1b]8;;https://example.com\x07click me\x1b]8;;\x07";
        assert_eq!(strip(wrapped), "click me");
    }

    #[test]
    fn strip_passes_through_text_with_no_escapes() {
        let plain = "no escapes here";
        assert_eq!(strip(plain), plain);
    }

    #[test]
    fn strip_preserves_non_osc_8_escapes() {
        // 颜色转义保持不动；只有 OSC 8 包装被移除。
        let mixed = format!(
            "\x1b[31mred\x1b[0m {wrapped}",
            wrapped = wrap_link("https://example.com", "click")
        );
        assert_eq!(strip(&mixed), "\x1b[31mred\x1b[0m click");
    }

    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        strip_ansi_into(s, &mut out);
        out
    }

    #[test]
    fn strip_ansi_removes_csi_sgr_and_keeps_text() {
        let coloured = "526   \x1b[1;32mOPEN\x1b[0m  bug fix";
        assert_eq!(strip_ansi(coloured), "526   OPEN  bug fix");
    }

    #[test]
    fn strip_ansi_removes_osc_8_wrapper() {
        let wrapped = wrap_link("https://example.com", "click");
        assert_eq!(strip_ansi(&wrapped), "click");
    }

    #[test]
    fn strip_ansi_preserves_newlines_tabs_and_cr() {
        let s = "a\nb\tc\rd";
        assert_eq!(strip_ansi(s), "a\nb\tc\rd");
    }

    #[test]
    fn strip_ansi_drops_lone_control_bytes() {
        // 孤立的 BEL 或其他非 \n/\r/\t 的 C0 控制字节被丢弃，
        // 这样它们不会作为可见单元格绘制。
        let s = "a\x07b\x01c";
        assert_eq!(strip_ansi(s), "abc");
    }

    #[test]
    fn strip_ansi_preserves_utf8_multibyte_chars() {
        // CJK、重音拉丁文和表情符号必须在剥离后存活，而不会被
        // 重新解码为 Latin-1（这会把 你 炸成 ä½ ）。
        let s = "Phase 1: 第一步 README é 🚀";
        assert_eq!(strip_ansi(s), "Phase 1: 第一步 README é 🚀");

        let coloured = "\x1b[1;32m第一步\x1b[0m done";
        assert_eq!(strip_ansi(coloured), "第一步 done");
    }

    #[test]
    fn strip_preserves_utf8_multibyte_chars() {
        let wrapped = wrap_link("https://example.com", "点击我");
        assert_eq!(strip(&wrapped), "点击我");
    }

    #[test]
    fn enabled_is_true_by_default_when_untouched() {
        // 持有标志守卫，以便我们观察初始状态，而不是从
        // `set_enabled_round_trips` 半路飞过的值。标志*默认*
        // 在静态初始化时为 true，并且此模块中的测试是唯一的写入者。
        let _g = FLAG_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        assert!(enabled());
    }

    #[test]
    fn set_enabled_round_trips() {
        let _g = FLAG_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prior = enabled();
        set_enabled(false);
        assert!(!enabled());
        set_enabled(true);
        assert!(enabled());
        set_enabled(prior);
    }

    // ── #3029: extract_buffer_link_regions ───────────────────────────────

    /// 将 `lines`（其 spans 可能包含内联 `wrap_link` 载荷）渲染到
    /// `area` 的新 Buffer 中并返回它，镜像真实抄本路径布局文本的方式。
    fn render_lines(
        lines: Vec<ratatui::text::Line<'static>>,
        area: ratatui::layout::Rect,
    ) -> Buffer {
        use ratatui::widgets::{Paragraph, Widget};
        let mut buf = Buffer::empty(area);
        Paragraph::new(lines).render(area, &mut buf);
        buf
    }

    fn row_text(buf: &Buffer, y: u16, x_start: u16, x_end: u16) -> String {
        (x_start..x_end)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect()
    }

    #[test]
    fn extract_finds_label_span_target_and_blanks_payload() {
        // wrap_link("https://x.test", "click") 在 ratatui 剥离 ESC 后占用：
        // ]8;;<target>\<label>]8;;\（终止符之间的标签 "click"）。
        let target = "https://x.test";
        let label = "click";
        let wrapped = wrap_link(target, label);
        let area = ratatui::layout::Rect::new(0, 0, 40, 1);
        let mut buf = render_lines(
            vec![ratatui::text::Line::from(vec![ratatui::text::Span::raw(
                wrapped,
            )])],
            area,
        );

        let regions = extract_buffer_link_regions(&mut buf, area);
        assert_eq!(regions.len(), 1, "exactly one link region");
        let r = &regions[0];
        assert_eq!(r.row, 0);
        assert_eq!(r.target, target);
        // 标签列源自载荷布局：open(4) + target + \(1)，
        // 然后标签单元格。计算而非硬编码以在测试数据更改时保持正确。
        let expected_start = 4 + target.len() as u16 + 1;
        let expected_end = expected_start + label.len() as u16 - 1;
        assert_eq!(r.col_start, expected_start);
        assert_eq!(r.col_end, expected_end);
        // 标签单元格保持完整。
        assert_eq!(
            row_text(&buf, 0, expected_start, expected_start + label.len() as u16),
            label
        );
        // 任何地方都没有残留的载荷字节：打开、目标和两个终止符
        // 都被清空。整行除标签范围外都是空格。
        let full = row_text(&buf, 0, 0, expected_end + 6);
        assert!(
            !full.contains(']') && !full.contains('\\') && !full.contains('h'),
            "payload bytes blanked, got: {full:?}"
        );
    }

    #[test]
    fn extract_handles_two_links_same_row() {
        let w1 = wrap_link("https://a.test", "AAA");
        let w2 = wrap_link("https://b.test", "BB");
        let combined = format!("{w1} {w2}");
        let area = ratatui::layout::Rect::new(0, 0, 60, 1);
        let mut buf = render_lines(
            vec![ratatui::text::Line::from(vec![ratatui::text::Span::raw(
                combined,
            )])],
            area,
        );

        let regions = extract_buffer_link_regions(&mut buf, area);
        assert_eq!(regions.len(), 2, "two disjoint links");
        assert_eq!(regions[0].target, "https://a.test");
        assert_eq!(regions[1].target, "https://b.test");
        // 标签存活且不重叠。
        let a_span = regions[0].col_start..=regions[0].col_end;
        let b_span = regions[1].col_start..=regions[1].col_end;
        assert!(a_span.end() < b_span.start(), "regions must not overlap");
        // 行上任何位置都没有残留的载荷字节。
        let full = row_text(&buf, 0, 0, 60);
        assert!(!full.contains(']'), "no open/close brackets remain");
        assert!(!full.contains('\\'), "no terminator backslash remains");
    }

    #[test]
    fn extract_uses_absolute_coordinates_with_area_offset() {
        // 后端测试绝对 (x,y)；区域必须包含 area.x/area.y。
        let wrapped = wrap_link("u", "L");
        let area = ratatui::layout::Rect::new(5, 3, 30, 2);
        let mut buf = render_lines(
            vec![ratatui::text::Line::from(vec![ratatui::text::Span::raw(
                wrapped,
            )])],
            area,
        );

        let regions = extract_buffer_link_regions(&mut buf, area);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].row, 3, "row includes area.y");
        assert!(regions[0].col_start >= 5, "col includes area.x");
        assert_eq!(regions[0].target, "u");
    }

    #[test]
    fn extract_preserves_plain_text_and_emits_no_regions() {
        let area = ratatui::layout::Rect::new(0, 0, 20, 1);
        let mut buf = render_lines(
            vec![ratatui::text::Line::from(vec![ratatui::text::Span::raw(
                "just plain text",
            )])],
            area,
        );
        let before = row_text(&buf, 0, 0, 15);
        let regions = extract_buffer_link_regions(&mut buf, area);
        let after = row_text(&buf, 0, 0, 15);
        assert!(regions.is_empty());
        assert_eq!(before, after, "plain text untouched");
    }

    #[test]
    fn extract_blanks_unterminated_payload_and_emits_no_region() {
        // 关闭部分被截断（例如因换行）的载荷不得产生半链接；
        // 其载荷单元格仍然被清空。
        // 构建一个包含 `]8;;ab\cd` 且没有尾部关闭的缓冲区。
        let area = ratatui::layout::Rect::new(0, 0, 12, 1);
        let mut buf = render_lines(
            vec![ratatui::text::Line::from(vec![ratatui::text::Span::raw(
                // wrap_link 减去尾部关闭：open+target+term+label。
                // 我们不能轻松地通过 wrap_link 产生"无关闭"，因此直接
                // 构建内联字节（ESC 将被 ratatui 剥离）。
                "\x1b]8;;t\x1b\\lab",
            )])],
            area,
        );
        let regions = extract_buffer_link_regions(&mut buf, area);
        assert!(regions.is_empty(), "no close -> no region");
        let text = row_text(&buf, 0, 0, 12);
        assert!(!text.contains(']'), "open payload blanked");
    }
}
