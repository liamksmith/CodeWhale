//! CodeWhale 的动态 Workflow 运行时。
//!
//! 此 crate 是 Workflow 的命令式半部分：一个沙箱化的 QuickJS（rquickjs）运行时，执行模型编写的 JS 程序，
//! 通过 `task()` 分派舰队路由的子代理，通过 `parallel()`/`pipeline()` 进行扇出，
//! 通过 `log()`/`phase()` 报告进度，并通过 `budget` 全局对象自我调节 token 池。
//! 静态声明式 IR（记录/回放、模型策略）保留在 `codewhale-workflow` 中；
//! 此 crate 仅通过 [`WorkflowDriver`] 接口与外部通信，因此无需启动真实子代理即可完全可测试（参见 [`testing::FakeDriver`]）。
//!
//! # 脚本表面
//!
//! 每个脚本都在一个异步函数内运行，具有以下全局变量：
//!
//! * `args` — 调用输入，原样传递。
//! * `await task(opts)` — 分派一个子代理；解析为完整的结果文本，或者当设置了 `opts.responseSchema` 时，
//!   解析为经过解析和 schema 验证后的对象。在拒绝、失败、取消、预算耗尽或已达到 [`WORKFLOW_LIFETIME_CAP`] 次生成尝试时抛出。
//! * `parallel(thunks)` — all-settled 扇出；失败的槽位变为 `null`；
//!   最多 [`PARALLEL_MAX_ITEMS`] 项。
//! * `pipeline(items, ...stages)` — 逐项阶段链，阶段之间无屏障；
//!   阶段错误会将该项丢弃为 `null`；相同项数上限。
//! * `log(msg)` / `phase(title)` — 转发给驱动器的进度事件。
//! * `budget.total` / `budget.spent()` / `budget.remaining()` — 实时驱动器快照
//!   （当未配置上限时，`total` 为 `null`，`remaining()` 为 `Infinity`）。
//!
//! `Date.now()`、`new Date()`、`Date.parse/UTC` 和 `Math.random()` 会抛出异常：
//! 运行必须是确定性的，以便可以回放记录的跟踪。
//!
//! # 所有权边界
//!
//! Token 记账和预留（设计 §5.3）属于驱动器；VM 仅读取快照，并在池已耗尽时快速失败生成操作。
//! 对于 `profile` 的舰队名册解析也在驱动器侧完成；此 crate 仅对 profile 字符串进行规范化
//! 和 token 验证，不处理其他事项。

mod driver;
mod error;
mod schema;
pub mod testing;
mod vm;

pub use driver::{
    BudgetSnapshot, ProgressEvent, SpawnedTask, TaskCompletion, TaskRequest, WorkflowDriver,
    normalize_profile,
};
pub use error::{DriverError, WorkflowJsError};
pub use vm::{VmLimits, WorkflowRunCancel, WorkflowVm};

/// 每次运行的 `task()` 最大生成尝试次数（设计 §4.3）。在咨询驱动器之前在 VM 中计数，
/// 因此即使驱动器持续接收新任务，失控的 `loop-until-dry` 也会终止。
///
/// 生产规模：每个 Workflow 运行最多 1_000 个 agent。
pub const WORKFLOW_LIFETIME_CAP: u64 = 1000;

/// 单个 Workflow 运行内最大并发执行的 agent 数量。
///
/// 扇出可能通过 `parallel()` / `pipeline()` *声明*更多工作，但宿主一次最多允许这么多活跃的 `task()` 子任务；
/// 额外的生成操作将等待槽位。
pub const WORKFLOW_MAX_CONCURRENT: usize = 16;

/// 每次 `parallel()` 或 `pipeline()` 调用的最大项数（设计 §4.2）。
/// 保持为每次运行的 agent 上限，以便单次扇出声明的任务量不会超过生命周期上限能完成的总量。
pub const PARALLEL_MAX_ITEMS: usize = 1000;
