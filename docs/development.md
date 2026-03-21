# Development Guide

## 1. 工程目标

这份文档描述 `polymarket-ltf` 的工程规范。  
它关注的不是“策略好不好”，而是：

- 代码应如何组织
- 数据契约应如何维护
- 测试应如何选择
- 文档应如何同步
- 研究代码与实时代码如何分层

## 2. 工程原则

### 2.1 真实边界清晰

- 研究层和执行层分开
- 实时层和回测层分开
- 原始数据、snapshot、结果和日志分开

### 2.2 模块职责单一

- 数据源逻辑不要混到一个入口文件里
- 类型定义不要到处重复
- 策略逻辑不要和路径解析、输出打印混在一起

### 2.3 低延迟工程意识

- 热路径代码要主动考虑分配次数、复制次数、锁竞争和序列化成本
- 性能敏感路径优先使用稳定、直接、可预测的数据结构
- 低延迟不等于到处写“技巧代码”，而是让热点路径足够短、足够清晰、足够少分支

### 2.4 数据契约优先

- snapshot 字段语义稳定比短期开发方便更重要
- 结果和日志必须可追溯

### 2.5 复用优先，避免补丁式扩张

- 能在现有模块内自然演化，就不要新开近似重复模块
- 能抽公共逻辑，就不要复制一份“只改两个字段”的版本
- 不要每加一个需求就包一层薄封装，让系统逐渐变成补丁堆叠
- 如果新增抽象无法明显降低重复、降低耦合或提升可测试性，就先不要加
- 删除不必要的类型转换、中间 struct 和镜像数据模型
- 能直接使用 SDK/上游返回类型时，不要为了“看起来整齐”再包一层一次性转换对象
- 需要长期维护的本地状态，如果 SDK 类型已经能表达核心语义，优先直接存 SDK 类型；不要再定义字段几乎一致的镜像 record
- 如果 REST 快照模型和 WS 增量模型语义不同，优先定义一个本地 canonical 状态类型，让两者都更新这一个类型
- 配置 struct 只承载配置；不要把运行时状态、bootstrap 数据或临时拼装结果塞进配置对象
- 如果只有一条真实使用路径，就保留一个直接入口；不要为了“以后可能用到”保留未被使用的备用构造函数或分支入口

### 2.6 文档与代码同步

- 目录变化、命令变化、字段变化、结果格式变化都必须同步更新文档

## 3. 仓库结构与职责

### 3.1 Rust 实时层

- `src/main.rs`
  默认入口，适合健康检查和最小可运行流程
- `src/lib.rs`
  公共模块导出入口
- `src/errors.rs`
  项目级错误类型
- `src/config.rs`
  环境变量与本地 `.env` 加载入口
- `src/logging.rs`
  日志初始化
- `src/binance/websocket.rs`
  当前 CEX 参考价格接入
- `src/polymarket/market_registry.rs`
  市场发现与订阅调度
- `src/polymarket/orderbook_stream.rs`
  Polymarket 订单簿接入与缓存
- `src/polymarket/rtds_stream.rs`
  Chainlink RTDS 价格接入
- `src/polymarket/user_stream.rs`
  用户 open orders / positions 启动同步与 WS 增量维护
- `src/polymarket/relayer.rs`
  Polymarket Relayer 交易辅助逻辑
- `src/snapshot.rs`
  snapshot 特征计算与 CSV 写入
- `src/strategy/`
  Rust 侧策略目录，适合放运行时策略和执行候选逻辑
- `src/types/crypto.rs`
  `Symbol` 与 `Interval`
- `src/polymarket/types/open_orders.rs`
  本地 open orders 状态模型
- `src/polymarket/types/positions.rs`
  本地 positions、成交手续费推导与 bootstrap fee fallback
- `benches/`
  Rust 热路径性能基准与回归测量

### 3.2 Python 研究层

- `backtest/src/data/`
  文件格式与输入输出适配，含数据格式注册
- `backtest/src/domain/`
  领域模型与策略协议
- `backtest/src/engine/`
  回测核心逻辑
- `backtest/src/reports/`
  结果输出与报表结构
- `backtest/src/strategies/`
  策略信号实现与策略注册
- `backtest/src/backtest.py`
  单次回测入口
- `backtest/src/scan.py`
  参数扫描与 `pandas` 汇总入口
- `backtest/src/report.py`
  结构化结果再渲染入口

### 3.3 文档与工作流层

- `README.md`
  对外入口
- `AGENTS.md`
  协作入口
- `docs/`
  正式项目文档
- `.env.example`
  本地账户监控与下单示例的环境变量模板
- `skills/`
  项目内 skill

## 4. 变更规则

### 4.1 新增数据源

- 优先新增独立模块
- 先定义该数据源在研究中的角色：
  原始盘口、fair value、oracle，还是执行反馈
- 先定义标准化口径，再决定是否进入 snapshot
- 不要为了兼容新数据源破坏现有字段语义

如果一个新数据源与现有数据源只是协议不同、角色相同，优先考虑：

- 复用现有字段定义
- 复用现有 adapter 模式
- 复用现有状态更新路径

而不是直接新造一套并行逻辑

### 4.2 修改 snapshot

修改 `src/snapshot.rs` 时必须评估：

- 是否改变字段语义
- 是否改变列顺序
- 是否改变计算口径或窗口
- 是否影响 Python 读取器与回测逻辑

如果答案是“会”，必须同步更新：

- [docs/research.md](./research.md)
- [backtest/README.md](../backtest/README.md)
- 根目录入口文档

### 4.3 修改回测层

修改 `backtest/` 时必须评估：

- 是否改变了成交模型
- 是否改变了策略准入条件
- 是否改变了输出字段
- 是否改变了 CLI 行为

如果输出结构变化，应同步更新文档与测试。

如果修改了 `--data-format`、`--strategy`、参数扫描口径或 loader / strategy 注册表，也必须同步更新 [backtest/README.md](../backtest/README.md)。

### 4.4 策略代码放置规则

- 纯研究、纯回测策略：
  放 `backtest/src/strategies/`
- Rust 运行时策略、执行候选或实时决策逻辑：
  放 `src/strategy/`

不要把两类策略目录混用。  
如果一个策略还没有脱离研究阶段，不要提前塞进 Rust 运行时目录。

### 4.5 修改热点路径

涉及下面这些位置时，应额外做性能审视：

- websocket 消息解析
- 本地状态更新
- snapshot 构造
- 高频循环内的格式转换
- 回测主循环

至少要检查：

- 是否引入了不必要的分配
- 是否引入了不必要的 clone
- 是否引入了更粗的锁粒度
- 是否把简单直达的数据路径改成了多层包装
- 是否需要补充或更新对应 benchmark
- 如果热点路径依赖特定消息形状、fallback 策略或线上观测结论，应在代码旁补充高价值注释，说明为什么这样设计

### 4.5.1 本地订单与持仓监控约束

- `user_stream` 或其他本地持仓监控入口，启动时必须支持 bootstrap 远端仓位快照；否则账户已有仓位时，第一笔卖出成交会直接把本地状态打坏
- `user_stream` 默认维护全账户 `open orders / positions / trades`；不要为了滚动 market 逻辑把账户状态订阅绑死到单个 market 集合
- 本地持仓只由“当前账户自己的成交”驱动；轮询或补拉 `trades` 时必须按用户地址过滤，不能直接吃整条 market 的公共成交流
- 本地 `positions` 的基线来自远端 `positions` bootstrap；运行时仓位增量只由 `TradeMessage` 驱动。taker 直接用顶层 `trade.side`，maker 必须通过 `maker_order.order_id -> 本地 order_context.side` 还原方向
- 实时成交手续费优先使用成交消息自带的 `fee_rate_bps` 推导；不要再把 crypto 用户监控写成固定 `FeeRule::crypto()` 硬编码落账
- `positions.market_fees` 只作为 bootstrap 持仓后续成交、或其他缺少 `fee_rate_bps` 的数据源兜底；不要把它重新扩张成实时链路的主手续费来源
- 本地 `open_orders` 由 `/data/orders` bootstrap、`OrderMessage` 创建/校准，以及本账户 maker `TradeMessage` 的成交增量共同维护；taker trade 不应更新 `open_orders`
- `open_orders` 语义必须保持为“当前活跃挂单视图”；终态订单不能继续留在 `open_orders`
- `OrderMessage` 只允许维护挂单和订单上下文，不允许直接更新 `positions`
- 如果用户成交监控依赖特定消息字段缺失或裁剪行为，应在代码旁补充关键注释说明设计原因；当前 Polymarket SDK 未暴露 `maker_orders.side`，因此 maker 方向必须依赖本地 `order_context`
- crypto 监控链路优先保持单一 fee 公式与单一 exponent；如果不是明确存在多费率场景，不要提前引入更复杂的 market 级 fee 策略抽象
- bootstrap 过程优先直接消费远端 `positions` 返回值，不要先复制成只用一次的中间 struct 再写回本地状态

### 4.6 配置与环境变量

- Rust 侧环境变量读取优先统一收敛到 `src/config.rs`
- `.env` 只作为本地开发辅助，不要把业务配置读取散落到各个 example 或模块里
- 业务代码应尽量只依赖统一配置接口，而不是直接到处写 `std::env::var(...)`
- 根目录 `.env.example` 只维护当前真实使用的本地开发变量说明，新增或删除环境变量时必须同步更新
- 当前默认配置口径：
  `PRIVATE_KEY` 用于 Polymarket 账户鉴权，`SYMBOLS` 用于本地下单 / 监控示例的默认标的列表

### 4.7 Example 与入口约束

- `src/main.rs` 保持最小健康检查入口，不要把多数据源或账户状态逻辑继续堆进默认入口
- `examples/book_monitor.rs` 用于 market registry + orderbook 观测，适合验证公开市场数据链路
- `examples/user_monitor.rs` 用于只读账户状态观测，适合验证 `user_stream` 的 bootstrap 与增量维护
- `examples/clob_ok.rs` 只应被描述为鉴权、下单和本地账户状态联调示例，不应描述为完整执行系统
- example 的 CLI 行为、默认参数或环境变量入口变化时，必须同步更新 `README.md` 和相关正式文档

### 4.8 Polymarket `PriceChange` 热路径设计依据

2026-03-20 对根目录真实 WebSocket 样本做了一次结构检查，结论如下：

- `book` 消息 `298` 条
- `price_change` 消息 `9968` 条
- 其他消息 `157` 条
- 所有 `price_change` batch 都是 `2` 条 entry
- 所有 batch 都是 `up/down` 镜像对
- 所有镜像对归一化后都落到同一个 canonical level
- 样本内没有发现 `size` 冲突

基于这个真实分布，这条热路径的设计重点不是“理论上能兼容多少 batch 形状”，而是“默认成本该为谁服务”。

当前设计的理由如下：

- 主路径应该服务真实流量。
  对这份样本来说，`PriceChange` 的常见形状不是“任意长度的通用 batch”，而是“2-entry 的 up/down 镜像对”。因此主路径应该直接把这种消息压成一次 canonical update，而不是让它先经过通用 dedupe 结构。
- 冷路径要保持正确，但不应该伪装成热路径。
  上游 WS 契约不由本仓库控制，所以仍要保留对非标准 batch 的处理能力。但这类消息当前没有成为主流流量，因此更合适的设计是保持语义正确和实现直接，而不是让主路径为冷路径长期支付 `HashMap`、scratch 状态和额外分支的成本。
- 协议漂移必须可观测。
  当消息未命中 `2-entry` 镜像形状时，系统应显式发出 `warn!`，把“上游行为开始变化”变成日志信号，而不是静默吞掉。这样后续是否恢复更通用的去重逻辑，可以基于观测而不是猜测。
- 临时状态只有在 steady-state 受益时才值得保留。
  如果主路径已经不依赖 `PriceChangeScratch` 或批内 `HashMap` 去重，继续保留这些状态只会增加理解成本、维护成本和分支面，不会带来真实吞吐收益。
- 快照语义应该贴着数据流直写。
  `BookUpdate` 是整本替换语义，中间再组一层 `Vec<Level>` 没有新的信息价值。更合理的设计是直接把输入流映射到本地 `OrderBooks`，减少对象数量和中间搬运。

用 benchmark 验证后的结论也支持这个方向：

- `2-entry` 镜像 fast path 比顺序 fallback 更快，说明“为真实消息形状单独建主路径”是值得的
- `with_lock` 和 `no_lock` 的差距不大，说明当前主要成本不在 `RwLock`
- synthetic `apply` 相对上一版通用 `HashMap dedupe fallback` 有明显改善，说明之前的主要额外成本确实来自那套通用批内去重

当前观测下，这条链路更适合遵循下面的原则：

- 让最常见的消息形状拥有最短、最稳定的路径
- 让少见情况保持正确，但不要主导默认成本
- 让上游协议变化优先暴露为可观测信号
- 只有当真实流量证明需要时，才把更通用的处理逻辑重新放回热路径

当前实测基准如下：

- `ws_price_change_pair_apply/fast_path`: `101.66 ns` 到 `106.12 ns`
- `ws_price_change_pair_apply/sequential_fallback`: `113.61 ns` 到 `125.71 ns`
- `ws_price_change_apply_no_lock`
  - `1` 档每边: `115.71 ns` 到 `122.20 ns`
  - `8` 档每边: `1.1612 us` 到 `1.2621 us`
  - `32` 档每边: `5.3442 us` 到 `5.4869 us`
- `ws_price_change_apply_with_lock`
  - `1` 档每边: `116.60 ns` 到 `127.74 ns`
  - `8` 档每边: `1.1571 us` 到 `1.2903 us`
  - `32` 档每边: `5.4493 us` 到 `5.7649 us`

相对上一版 `HashMap dedupe fallback`，当前 synthetic `apply` benchmark 在 `1 / 8 / 32` 档每边上大致提升了 `51%` 到 `61%`。这也说明当前热点主要不是 `RwLock`，而是 batch 归一化和状态更新本身。

## 5. 构建与验证

常用 Rust 命令：

- `cargo check`
- `cargo build`
- `cargo run`
- `cargo test`
- `cargo bench --bench ws_hot_paths`
- `cargo fmt`
- `cargo clippy --all-targets --all-features -D warnings`

常用 Python 命令：

```bash
cd backtest
PYTHONPATH=src python3 -m backtest
PYTHONPATH=src python3 -m backtest --interval 5m
PYTHONPATH=src python3 -m backtest --interval 15m
PYTHONPATH=src python3 -m scan --csv tests/fixtures/sample.csv --top-k 1
PYTHONPATH=src python3 -m unittest discover -s tests
```

提交前至少应执行：

```bash
cargo fmt
cargo test
```

修改范围较大时，再执行：

```bash
cargo clippy --all-targets --all-features -D warnings
```

## 6. 编码风格与命名

### 6.1 Rust

- 使用 4 个空格缩进
- 模块、文件、函数使用 `snake_case`
- 结构体、枚举、trait 使用 `PascalCase`
- 常量使用 `SCREAMING_SNAKE_CASE`
- 优先使用 `Result` 做错误传播
- 避免随意 `unwrap()` / `expect()`
- 异步流程统一基于 `tokio`
- 与外部接口交互的数据结构显式 `serde` 化
- 金融字段优先使用 `rust_decimal`
- 热路径上优先减少不必要的分配、复制和锁竞争
- 先复用已有结构体和状态对象，再考虑新增抽象
- 对性能敏感路径，优先让数据流直达、边界清楚、对象数量可控
- 注释优先解释协议假设、性能取舍、fallback 边界和设计原因
- 不要给直白的赋值、转发、循环和字段访问逐行写解释性注释

### 6.2 Python

- 回测代码统一放在 `backtest/`
- `data/`、`domain/`、`engine/`、`reports/`、`strategies/` 按职责分层
- 顶层入口保持克制，只保留 `backtest.py`、`scan.py`、`report.py`
- 新数据格式统一走 `data/registry.py`
- 新策略统一走 `strategies/registry.py`
- 终端输出默认中文
- 真实数据不要放进 `tests/fixtures/`
- 结果、报表和日志尽量放到 `data/` 或 `logs/`
- 回测主循环优先保持简单、线性、低分支，不要把热点路径包装得过深

### 6.3 数据结构设计要求

- 优先用明确语义的结构体、类和字段表达状态
- 不要在核心路径依赖“万能字典”或松散 payload
- 对读多写少的共享状态，优先考虑读取成本和锁竞争
- 对高频更新路径，优先考虑是否能复用对象和缓冲区
- 如果一个抽象层主要作用只是“再包一层”，通常应删掉或并回原模块

### 6.4 命名要求

字段命名必须尽量贴近真实含义，例如：

- `binance_mid_price`
- `chainlink_run`
- `up_bid_price`
- `down_ask_price`

避免使用：

- `price`
- `run`
- `delta`
- `data`

除非作用域已经非常明确。

## 7. 测试规范

### 7.1 测试目标

测试优先验证高风险行为，而不是低价值实现细节。

推荐测试方向：

- 数据流恢复与解析逻辑
- market registry 的窗口更新与切换
- snapshot 公式和字段语义
- Python 回测的路径解析、策略信号和汇总格式

不推荐测试方向：

- 一行包装函数
- 纯粹的枚举转字符串
- 重复实现私有辅助函数的逻辑

### 7.2 Rust

- 模块内部行为优先使用内联单元测试
- 跨模块行为再考虑放到 `tests/`

### 7.3 Python

- 路径解析、策略逻辑、汇总输出都应有单元测试
- 改策略模型时，同时验证单文件和批量回测

## 8. 文档规范

文档职责如下：

- [README.md](../README.md)
  对外介绍
- [AGENTS.md](../AGENTS.md)
  协作入口
- [project.md](./project.md)
  项目定义、系统设计、架构摘要和路线图摘要
- [research.md](./research.md)
  研究方法、数据标准和策略准入
- [development.md](./development.md)
  工程规范、低延迟约束和测试标准

以下变化发生时，必须更新文档：

- 模块或目录结构变化
- snapshot 字段变化
- 回测输出变化
- CLI 行为变化
- skill 结构或用途变化

## 9. 提交与 PR 规范

建议使用简洁、单一目的的提交信息，例如：

- `Refactor snapshot feature calculation`
- `Add CEX reference adapter`
- `Update backtest reporting format`

PR 至少应说明：

- 变更目的
- 影响的模块或数据流
- 验证方式
- 如果涉及输出变化，附关键示例

## 10. 一条默认准则

如果你不确定某段修改属于哪里，优先用下面这条判断：

**让数据源更清楚，让字段语义更稳定，让回测结果更可追溯，让文档与代码保持一致。**

再加一条默认工程判断：

**优先修改现有正确边界内的代码，优先复用已有结构，不要把系统做成补丁堆叠。**
