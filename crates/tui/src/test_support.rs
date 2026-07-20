//! 共享的仅测试辅助函数。

use std::ffi::{OsStr, OsString};
use std::sync::{Mutex, MutexGuard, OnceLock};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 获取进程级的环境变量互斥锁。
///
/// 如果之前的测试在持有锁时发生 panic，则恢复守卫而不是让级联失败波及不相关的测试。
pub(crate) fn lock_test_env() -> MutexGuard<'static, ()> {
    match env_lock().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 在丢弃时恢复一个环境变量。
///
/// 修改进程级环境变量的调用者必须持有 [`lock_test_env`] 直到此守卫被丢弃。
pub(crate) struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    pub(crate) fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY：调用者持有进程级测试环境互斥锁。
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }

    pub(crate) fn remove(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: callers hold the process-wide test env mutex.
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }

    pub(crate) fn previous(&self) -> Option<OsString> {
        self.previous.clone()
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: callers hold the process-wide test env mutex until after this
        // guard is dropped.
        unsafe {
            if let Some(value) = self.previous.take() {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

/// Find the byte position of the first divergence between two strings,
/// returning a windowed view (`±32 bytes` around the divergence) so failures
/// in cache-prefix-stability tests show *which* bytes drifted, not just that
/// they did. Returns `None` when the strings are byte-identical.
pub(crate) fn first_divergence(a: &str, b: &str) -> Option<(usize, String, String)> {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    let max = a_bytes.len().min(b_bytes.len());
    for i in 0..max {
        if a_bytes[i] != b_bytes[i] {
            let lo = i.saturating_sub(32);
            let a_hi = (i + 32).min(a_bytes.len());
            let b_hi = (i + 32).min(b_bytes.len());
            let a_ctx = String::from_utf8_lossy(&a_bytes[lo..a_hi]).into_owned();
            let b_ctx = String::from_utf8_lossy(&b_bytes[lo..b_hi]).into_owned();
            return Some((i, a_ctx, b_ctx));
        }
    }
    if a_bytes.len() != b_bytes.len() {
        return Some((
            max,
            format!("(len={})", a_bytes.len()),
            format!("(len={})", b_bytes.len()),
        ));
    }
    None
}

/// Assert two strings are byte-identical, panicking with a windowed diff
/// around the first divergence when they aren't. Used by the prefix-cache
/// stability harness (#263, #280) to pin construction surfaces that land in
/// DeepSeek's KV cache prefix.
#[track_caller]
pub(crate) fn assert_byte_identical(label: &str, a: &str, b: &str) {
    if let Some((pos, a_ctx, b_ctx)) = first_divergence(a, b) {
        panic!(
            "{label}: prompt construction is non-deterministic — first diff at byte {pos}\n\
             ── side A (±32B) ──\n{a_ctx:?}\n── side B (±32B) ──\n{b_ctx:?}",
        );
    }
}
