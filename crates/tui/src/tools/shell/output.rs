use std::sync::{Arc, Mutex};

pub(super) fn take_delta_from_buffer(
    buffer: &Arc<Mutex<Vec<u8>>>,
    cursor: &mut usize,
) -> (Vec<u8>, usize) {
    let guard = buffer.lock().unwrap_or_else(|e| e.into_inner());
    let total = guard.len();
    let start = (*cursor).min(total);
    // 只克隆未读取的部分（增量），而不是整个累积缓冲区。
    // 长时间运行的进程可能产生数兆字节的输出；每次轮询时克隆完整缓冲区会使 ShellManager 互斥锁持有 O(total_bytes) 时间。
    let delta = guard[start..].to_vec();
    *cursor = total;
    (delta, total)
}

/// 仅读取字节缓冲区的尾部并返回 (total_len, tail_string)。
///
/// 当只需要尾部摘录时（例如作业面板显示），避免克隆完整缓冲区。
/// `max_tail_chars` 以 Unicode 标量值为单位；我们从末尾最多读取 `max_tail_chars * 4` 字节以考虑多字节 UTF-8 序列。
pub(super) fn tail_from_buffer(
    buffer: &Arc<Mutex<Vec<u8>>>,
    max_tail_chars: usize,
) -> (usize, String) {
    let guard = buffer.lock().unwrap_or_else(|e| e.into_inner());
    let total = guard.len();
    // 高估字节数（UTF-8 最坏情况每个字符 4 字节）。
    let mut tail_start = total.saturating_sub(max_tail_chars.saturating_mul(4));
    // 向前跳到下一个有效的 UTF-8 码点边界，以免将起始为继续字节（0x80-0xBF）的切片
    // 传递给 from_utf8_lossy，后者会输出前导的 U+FFFD 替换字符。
    while tail_start < total && (guard[tail_start] & 0xC0) == 0x80 {
        tail_start += 1;
    }
    let tail_str = String::from_utf8_lossy(&guard[tail_start..]).into_owned();
    (total, tail_text(&tail_str, max_tail_chars))
}

fn tail_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let tail = text
        .chars()
        .rev()
        .take(max_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("...{tail}")
}
