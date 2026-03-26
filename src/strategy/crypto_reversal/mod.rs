//! `crypto_reversal` 策略模块。
//!
//! 这个模块当前承载四类职责：
//! - `model`：纯策略模型与纯计算逻辑；
//! - `service`：把策略模型与 next market 组合成候选结果。
//! - `execute`：最小提交入口与执行去重。
//! - `runtime`：把策略模块与外部数据源、账户状态和 dashboard 接起来。
//!
//! 这里刻意不放：
//! - CLI；
//! - 审计 / replay / report。
//!
//! 这样可以让策略层先作为稳定、可复用的库能力存在，
//! 后续再由 example、runtime 或执行层按需接入。

pub mod constants;
pub mod execute;
pub mod model;
pub mod runtime;
pub mod service;
