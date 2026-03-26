//! 运行时 dashboard。
//!
//! 当前 dashboard 只做一件事：
//! - 把进程内运行状态和实时事件，通过一个轻量 Web 页面暴露出来。
//!
//! 这里刻意不引入完整前后端框架，也不做复杂控制面板；
//! 当前目标只有“能稳定看见实时状态”。

mod api;
mod http;

pub use api::Handle;

pub const DEFAULT_ADDR: &str = http::DEFAULT_ADDR;

pub async fn start() -> anyhow::Result<Handle> {
    let handle = Handle::new();
    http::spawn(handle.clone()).await?;
    Ok(handle)
}
