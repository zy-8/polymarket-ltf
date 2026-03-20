---
name: dev
description: 当处理本仓库的 LTF Rust 实时链路开发任务时使用，包括 src/、examples/、Cargo.toml、Polymarket/Binance/Chainlink 及后续多交易所数据流、snapshot 特征生成、结构调整、异步流程和错误传播。
---

# Dev

当任务集中在本仓库的 Rust 实时数据链路时，使用这个 skill。

这个 skill 面向的是 `polymarket-ltf` 的研究底座层，而不是前端、产品页面或通用脚手架开发。

## 工作前先看

- `docs/project.md`
- `docs/research.md`
- `docs/development.md`

## 适用场景

- 修改 `src/`、`examples/` 或 `Cargo.toml`
- 调整 websocket 接入、market registry 调度、RTDS 处理或 snapshot 写入
- 新增其他 CEX / oracle / 外部参考数据接入
- 重构模块边界、日志、错误传播或异步任务流程
- 评估 Rust 侧行为回归，或判断新代码应该放在哪个模块
- 重写 README / AGENTS 中和 Rust 实时链路有关的描述

## 仓库内约束

- 修改前先理解数据流，不要把实验性逻辑直接塞进核心模块。
- 交易所或协议相关逻辑放到独立模块，不要堆到 `main.rs`。
- 共享标识类型优先复用 `src/types/crypto.rs` 中的 `Symbol` 和 `Interval`。
- 优先用显式 `Result` 传播错误，尽量避免 `unwrap()` 和 `expect()`。
- 价格、数量等金融字段沿用仓库现有约定，优先使用 `rust_decimal`。
- 优先在现有正确边界内演化，不要每次需求都新增一层补丁式代码。
- 热路径要主动考虑分配、复制、锁竞争和数据结构布局。

## 重点模块

- `src/binance/websocket.rs`：Binance `bookTicker` 流与缓存
- `src/polymarket/market_registry.rs`：活跃市场发现与订阅调度
- `src/polymarket/orderbook_stream.rs`：Polymarket 订单簿 websocket
- `src/polymarket/rtds_stream.rs`：Chainlink RTDS 价格流
- `src/snapshot.rs`：snapshot 计算与 CSV 追加写入
- `src/errors.rs`：项目级错误类型

## 项目上下文

- 本仓库面向 LTF（Low Time Frame）抢价差与高频研究
- 当前阶段重点是多源数据链路、特征构造、离线研究
- 不要把尚未实现的“实盘执行能力”写成既成事实

## 处理多源数据时必须回答

- 这个新数据源在研究里扮演什么角色：
  原始盘口、fair value、oracle，还是执行反馈
- 它的 symbol、时间戳、价格字段如何标准化
- 它会不会影响现有 snapshot 字段语义
- 它应该直接进入 snapshot，还是先保持在独立适配层
- 它需要补哪些日志、异常处理和测试

## 修改实现时必须检查

- 能不能直接在现有模块内扩展，而不是新建近似重复模块
- 能不能复用现有类型、状态结构和错误模型
- 是否把简单直达的数据流改成了多层包装
- 是否在热点路径引入了额外分配、clone 或锁竞争

## 工作方式

1. 先读现有结构和数据流，再决定怎么改。
2. 尽量保持修改范围小且职责集中。
3. 涉及 websocket、snapshot、解析、错误处理等行为变化时，补至少一个有业务价值的测试。
4. 结构或流程变化后，同步更新仓库文档。
5. 如果新增数据源或改 snapshot 口径，记得同步更新 `docs/research.md`。
6. 如果只是为了“方便接一下”而想新开模块，先停下来检查是不是在制造补丁层。

## 验证方式

- 先用 `cargo check` 做快速反馈。
- 改完代码后跑 `cargo fmt`。
- 行为变化时跑有针对性的 `cargo test`，必要时跑全量 `cargo test`。
- 修改范围较大时跑 `cargo clippy --all-targets --all-features -D warnings`。

## 常用命令

```bash
cargo check
cargo fmt
cargo test
cargo clippy --all-targets --all-features -D warnings
```
