# Repository Guidelines

## 项目概览
本仓库是一个基于 Rust 的异步实时数据项目，当前围绕 Polymarket、Binance 和 Chainlink 价格流展开。工程使用 Cargo 管理，运行时基于 `tokio`，网络通信主要通过 websocket 完成，数据采样与 CSV 快照写入由本地缓存驱动。

协作者在修改代码前，优先理解数据流、模块边界和错误传播方式，不要把实验性逻辑直接混入核心模块。

## 项目结构与模块划分
当前核心代码位于 `src/`：

- `src/main.rs`：程序入口，适合放置本地调试或最小可运行流程。
- `src/lib.rs`：库导出入口，统一暴露公共模块。
- `src/errors.rs`：项目级错误定义与错误类型汇总。
- `src/binance/websocket.rs`：Binance websocket 接入逻辑。
- `src/types/crypto.rs`：跨 Binance / Polymarket / RTDS 共用的 `Symbol`、`Interval` 类型。
- `src/snapshot.rs`：snapshot 计算与 CSV 写入。
- `src/logging.rs`：统一日志初始化。
- `src/polymarket/market_registry.rs`：Polymarket 活跃市场发现、注册表与订阅调度。
- `src/polymarket/orderbook_stream.rs`：Polymarket 订单簿 websocket 处理。
- `src/polymarket/rtds_stream.rs`：Polymarket RTDS Chainlink 价格订阅与缓存。
- `src/polymarket/relayer.rs`：中继或同步相关逻辑。
- `src/polymarket/types/`：Polymarket 领域模型与数据类型定义。
- `examples/`：可运行示例，优先用来验证真实链路。

新增功能时，遵循以下原则：

- 交易所或协议相关逻辑放到独立模块目录下，不要堆到 `main.rs`。
- 类型定义尽量放在功能模块附近，例如订单簿类型放入 `types/`。
- 跨模块复用的枚举或标识类型优先放到 `src/types/`，不要在功能模块里重复定义。
- 公共能力通过 `lib.rs` 暴露，避免在多个文件中重复实现。

## 构建、测试与开发命令
常用命令如下：

- `cargo check`：快速检查语法、类型和依赖，适合开发中频繁执行。
- `cargo build`：完整编译项目。
- `cargo run`：运行当前默认二进制入口。
- `cargo test`：运行全部单元测试与集成测试。
- `cargo fmt`：使用 `rustfmt` 格式化代码。
- `cargo clippy --all-targets --all-features -D warnings`：执行静态检查，并将警告视为失败。

提交前至少执行：

```bash
cargo fmt
cargo clippy --all-targets --all-features -D warnings
cargo test
```

如果只是验证接口改动是否能通过编译，优先跑 `cargo check`，避免无谓等待。

## 编码风格与命名规范
本项目遵循 Rust 社区默认风格，并以工具自动化结果为准。

- 缩进使用 4 个空格，不使用制表符。
- 模块名、文件名、函数名使用 `snake_case`。
- 结构体、枚举、 trait 使用 `PascalCase`。
- 常量使用 `SCREAMING_SNAKE_CASE`。
- 一个文件只负责一类明确职责，不要把 websocket、类型定义、业务转换全部混在一起。
- 如果只是为了写 CSV，不要再额外引入“writer 类型”；优先保持 API 直接。

实现细则：

- 优先使用 `Result` 进行错误传播，避免随意 `unwrap()` 或 `expect()`。
- 异步逻辑统一基于 `tokio`；新增并发流程时，先考虑取消、安全退出和错误回传。
- 与外部接口交互的数据结构应显式 `serde` 序列化/反序列化，不要依赖隐式格式假设。
- 处理价格、数量等金融字段时，优先沿用 `rust_decimal`，避免无必要的浮点误差。
- 字段命名优先反映真实语义，例如 `binance_mid_price`、`chainlink_run`、`up_bid_price`，不要使用模糊名称如 `price`、`run`、`delta`。

## 测试规范
当前仓库尚未看到独立的 `tests/` 目录，因此新增测试时按以下约定执行：

- 模块内部行为验证使用内联单元测试：`#[cfg(test)] mod tests`。
- 跨模块或端到端行为验证放到 `tests/` 目录中。
- 测试名称应描述行为，而不是描述实现，例如 `parses_orderbook_snapshot`、`reconnects_after_disconnect`。
- 修改 websocket、订单簿、类型转换或错误处理等核心行为时，应补充或更新至少一个能覆盖该变更风险的测试。
- 优先测试有业务价值的行为、边界条件和状态变化，不要为枚举到字符串映射、常量返回或一行包装函数单独补“最小测试”。
- 如果一个测试只是在重复实现细节、几乎不会独立失效，或只能验证私有辅助函数的直观输出，应删除或并入更高层行为测试。
- 对 snapshot 这类纯计算逻辑，优先测试字段公式和列顺序，不要测试 CSV 字符串拼接的每个逗号细节。

如果某段逻辑因依赖真实网络而难以测试，优先抽象输入数据，再对解析、转换和状态更新部分做可重复测试。

## 提交与 Pull Request 规范
当前工作区不是一个可读取 Git 历史的仓库快照，因此无法从本地历史中总结既有提交风格。默认采用简洁、祈使句、单一目的的提交信息，例如：

- `Add Polymarket orderbook parser`
- `Refactor Binance websocket reconnect flow`
- `Handle empty position payload`

PR 建议包含以下内容：

- 变更目的与背景。
- 影响的模块或数据流。
- 验证方式，例如执行了哪些 Cargo 命令。
- 如涉及运行时行为变化，附上关键日志、示例输出或复现步骤。
- 如涉及配置项变更，明确新增环境变量及默认行为。

避免把重构、格式化、功能新增和行为修复混在同一个 PR 中。

## 配置与安全注意事项
- 不要在代码中硬编码 API Key、私钥、用户凭证或私有地址。
- 新增配置时优先使用环境变量，并在 PR 描述中写清变量名、用途和是否必填。
- 外部服务地址、订阅参数、市场标识等可变项，尽量通过配置注入，而不是写死在业务逻辑里。
- 输出日志时避免泄露敏感标识或完整请求载荷。
- 新增示例时，优先复用 `logging::init()`，不要各自再造日志初始化。

## 协作建议
- 先小步修改，再扩展范围；不要在一次提交中同时重写多个核心模块。
- 对外部 SDK 的使用保持收敛，尽量通过本地适配层隔离第三方变化。
- 文档要与当前代码保持一致；模块、示例或命令变动后同步更新 `README.md` 和本文件。
- 如果新增目录或模块，请同步更新本文件，确保后续协作者能快速理解仓库结构。
