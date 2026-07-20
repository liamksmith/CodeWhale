//! 测试支持：可脚本化的内存中 [`WorkflowDriver`]。
//!
//! [`FakeDriver`] 记录它收到的每个 [`TaskRequest`] 和 [`ProgressEvent`]，
//! 根据子串匹配的回复规则（带有可选的延迟用于排序测试）回答 spawn 请求，
//! 并统计 `cancel_all` 调用次数。它的存在使得此 crate ——
//! 以及实现真实驱动的 tui 连接代码 —— 可以在不产生任何真实子代理的情况下
//! 进行测试。

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::oneshot;

use crate::driver::{
    BudgetSnapshot, ProgressEvent, SpawnedTask, TaskCompletion, TaskRequest, WorkflowDriver,
};
use crate::error::DriverError;

/// 假驱动如何回答匹配的 spawn 请求。
#[derive(Debug, Clone)]
pub enum FakeReply {
    /// 使用此完整结果文本解析。
    Complete(String),
    /// 解析为失败的子代理。
    Fail(String),
    /// 解析为已取消。
    Cancelled,
    /// 解析为预算耗尽（中途）。
    BudgetExhausted(String),
    /// 拒绝准入：`spawn_task` 返回 [`DriverError::Rejected`]。
    Reject(String),
    /// 接受任务但永不完成（用于取消测试）。
    /// 持有完成发送者以使通道保持打开状态。
    Never,
}

#[derive(Debug)]
struct ReplyRule {
    needle: String,
    delay: Option<Duration>,
    reply: FakeReply,
}

#[derive(Debug, Default)]
struct Inner {
    rules: Vec<ReplyRule>,
    requests: Vec<TaskRequest>,
    events: Vec<ProgressEvent>,
    budget: BudgetSnapshot,
    spend_per_task: u64,
    next_id: u64,
    held: Vec<oneshot::Sender<TaskCompletion>>,
}

/// 具有脚本化回复的内存中 [`WorkflowDriver`]。
///
/// 未匹配的 spawn 会立即以 `done:<description>` 完成。
/// 规则通过对请求描述的子串匹配来确定，首个匹配获胜。
#[derive(Debug, Default)]
pub struct FakeDriver {
    inner: Mutex<Inner>,
    cancel_calls: AtomicUsize,
}

impl FakeDriver {
    /// 一个没有规则、没有预算上限且具有回显回复的假驱动。
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加一个回复规则：描述包含 `needle` 的请求会
    /// 立即获得 `reply`。
    pub fn on(&self, needle: &str, reply: FakeReply) {
        self.on_with_delay_opt(needle, reply, None);
    }

    /// 类似 [`FakeDriver::on`]，但在延迟 `delay` 后发送完成
    ///（spawn 本身仍立即返回）。
    pub fn on_with_delay(&self, needle: &str, reply: FakeReply, delay: Duration) {
        self.on_with_delay_opt(needle, reply, Some(delay));
    }

    fn on_with_delay_opt(&self, needle: &str, reply: FakeReply, delay: Option<Duration>) {
        self.lock().rules.push(ReplyRule {
            needle: needle.to_string(),
            delay,
            reply,
        });
    }

    /// 配置预算池：上限加上每次 spawn 时的固定消耗
    ///（模拟驱动侧的设计 §5.3 预留）。
    pub fn set_budget(&self, total: Option<u64>, spend_per_task: u64) {
        let mut inner = self.lock();
        inner.budget = BudgetSnapshot { total, spent: 0 };
        inner.spend_per_task = spend_per_task;
    }

    /// 迄今为止收到的每个请求，按 spawn 顺序排列。
    pub fn requests(&self) -> Vec<TaskRequest> {
        self.lock().requests.clone()
    }

    /// 每个请求的描述，按 spawn 顺序排列。
    pub fn request_descriptions(&self) -> Vec<String> {
        self.lock()
            .requests
            .iter()
            .map(|request| request.description.clone())
            .collect()
    }

    /// 已接受的 spawn 调用次数。
    pub fn spawn_count(&self) -> usize {
        self.lock().requests.len()
    }

    /// 迄今为止收到的每个进度事件，按触发顺序排列。
    pub fn events(&self) -> Vec<ProgressEvent> {
        self.lock().events.clone()
    }

    /// `cancel_all` 被调用的次数。
    pub fn cancel_all_calls(&self) -> usize {
        self.cancel_calls.load(Ordering::SeqCst)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("FakeDriver 互斥锁被污染")
    }
}

#[async_trait]
impl WorkflowDriver for FakeDriver {
    async fn spawn_task(&self, request: TaskRequest) -> Result<SpawnedTask, DriverError> {
        let (task_id, reply, delay) = {
            let mut inner = self.lock();
            let matched = inner
                .rules
                .iter()
                .find(|rule| request.description.contains(&rule.needle))
                .map(|rule| (rule.reply.clone(), rule.delay));
            let (reply, delay) = matched.unwrap_or_else(|| {
                (
                    FakeReply::Complete(format!("done:{}", request.description)),
                    None,
                )
            });
            if let FakeReply::Reject(message) = reply {
                return Err(DriverError::Rejected(message));
            }
            inner.requests.push(request);
            inner.budget.spent += inner.spend_per_task;
            inner.next_id += 1;
            (format!("agent_{:04}", inner.next_id), reply, delay)
        };

        let (tx, rx) = oneshot::channel();
        match reply {
            FakeReply::Never => self.lock().held.push(tx),
            reply => {
                let completion = match reply {
                    FakeReply::Complete(text) => TaskCompletion::Completed { text },
                    FakeReply::Fail(message) => TaskCompletion::Failed { message },
                    FakeReply::Cancelled => TaskCompletion::Cancelled,
                    FakeReply::BudgetExhausted(message) => {
                        TaskCompletion::BudgetExhausted { message }
                    }
                    FakeReply::Reject(_) | FakeReply::Never => unreachable!("以上已处理"),
                };
                match delay {
                    None => {
                        let _ = tx.send(completion);
                    }
                    Some(delay) => {
                        tokio::spawn(async move {
                            tokio::time::sleep(delay).await;
                            let _ = tx.send(completion);
                        });
                    }
                }
            }
        }
        Ok(SpawnedTask {
            task_id,
            completion: rx,
        })
    }

    fn cancel_all(&self) {
        self.cancel_calls.fetch_add(1, Ordering::SeqCst);
    }

    fn budget(&self) -> BudgetSnapshot {
        self.lock().budget
    }

    fn progress(&self, event: ProgressEvent) {
        self.lock().events.push(event);
    }
}
