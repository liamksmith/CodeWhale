//! 从 PTY 输出流构建的终端帧快照。
//!
//! 包装 `vt100::Parser`，以便测试可以增量地提供字节并查询当前屏幕内容（可见文本、单独的行、是否包含指定内容）。

use std::time::Instant;

pub struct Frame {
    parser: vt100::Parser,
    captured_at: Option<Instant>,
}

impl Frame {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new(rows, cols, 0),
            captured_at: None,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.parser.process(bytes);
        self.captured_at = Some(Instant::now());
    }

    pub fn rows(&self) -> u16 {
        self.parser.screen().size().0
    }

    pub fn cols(&self) -> u16 {
        self.parser.screen().size().1
    }

    /// 完整的可见屏幕，作为单个字符串，行之间以 `\n` 分隔。
    /// 每行尾部空白被保留，以便列位置断言保持有意义。
    pub fn text(&self) -> String {
        self.parser.screen().contents()
    }

    /// 屏幕的单行，从顶部开始 0 索引，在右侧边缘修整。对越界行返回空字符串。
    pub fn row(&self, y: u16) -> String {
        if y >= self.rows() {
            return String::new();
        }
        let cols = self.cols();
        let mut out = String::with_capacity(cols as usize);
        for x in 0..cols {
            if let Some(cell) = self.parser.screen().cell(y, x) {
                out.push_str(cell.contents());
            }
        }
        out
    }

    pub fn contains(&self, needle: &str) -> bool {
        self.text().contains(needle)
    }

    /// 屏幕是否有任何行包含非空白内容。用于检测完全分离/空白的视口。
    pub fn any_visible_text(&self) -> bool {
        self.text().chars().any(|c| !c.is_whitespace())
    }

    /// 光标位置为 (row, col)。用于断言编辑器拥有光标（#1073）或光标不在第 0 行的中间帧。
    pub fn cursor(&self) -> (u16, u16) {
        self.parser.screen().cursor_position()
    }

    /// 将屏幕渲染为字符串，用于断言失败时的诊断转储。
    pub fn debug_dump(&self) -> String {
        let (rows, cols) = (self.rows(), self.cols());
        let mut out = String::new();
        out.push_str(&format!(
            "== frame {rows}x{cols} cursor={:?} ==\n",
            self.cursor()
        ));
        for y in 0..rows {
            out.push_str(&format!("{y:>3} | {}\n", self.row(y).trim_end()));
        }
        out
    }
}
