# polymarket-ltf

`polymarket-ltf` 是一个面向 **LTF（Low Time Frame）** 的量化研究基础设施项目。  
当前项目以 **Polymarket crypto 市场** 为核心研究对象，并引入 CEX 与 oracle 价格作为参考系，用于研究短周期偏离、验证策略、评估执行可行性，并逐步演进到更完整的量化与高频套利框架。

## 定位

这个仓库当前重点解决四件事：

- 实时接入 Polymarket、CEX、oracle 等研究数据
- 将多源数据聚合成稳定的 snapshot 研究输入
- 对 snapshot 做离线回测、批量评估与策略验证
- 沉淀研究、结果、日志和协作文档标准

它适合被理解为：

- Polymarket crypto 市场研究底座
- LTF 价差与微观结构研究仓库
- 多源数据驱动的策略工程起点

它当前**不是**：

- 完整实盘执行系统
- 生产级订单管理与风控平台
- 已上线的自动交易引擎

## 当前范围

当前仓库已经具备：

- Polymarket 活跃市场发现与订单簿订阅
- Polymarket 账户 `open orders / positions` 本地同步与监控示例
- Binance `bookTicker` 参考中间价
- Chainlink RTDS 锚定价格
- 秒级 snapshot 生成与 CSV 落盘
- 基于 snapshot 的 Polymarket `up/down` 双腿回测
- `5m` / `15m` 分组的中文回测汇总

当前仓库还没有完整覆盖：

- 执行层
- 风控层
- 订单生命周期管理
- 统一研究报表中心

## 标准研究链路

```text
数据获取
  -> 数据标准化
  -> 特征构造与 snapshot
  -> 回测与研究评估
  -> 策略定义与实现
  -> 执行候选 / 执行
  -> 结果输出、日志归档与复盘
```

对这个项目来说，策略开发不应从“直接写信号”开始，而应从数据定义、fair value 定义、回测口径和结果可追溯性开始。

## 仓库结构

```text
.
├── benches/       # Rust 热路径性能基准
├── docs/          # 主文档目录，以 3 份核心文档为准
├── src/           # Rust 实时数据链路
├── examples/      # 最小可运行示例
├── backtest/      # Python crypto 回测与研究项目
├── data/          # snapshot 输入与研究输出
├── skills/        # 项目内 skill
├── README.md
└── AGENTS.md
```

## 文档地图

当前 `docs/` 由 3 份主文档构成，分别覆盖项目总览、研究方法和开发规范。

- [docs/project.md](docs/project.md)
  项目定义、系统设计、架构摘要、研究边界和路线图摘要
- [docs/research.md](docs/research.md)
  研究工作流、量化与高频套利关键要素、核心问题、数据标准与策略准入清单
- [docs/development.md](docs/development.md)
  工程规范、低延迟约束、测试规则、数据结构设计和文档同步标准
- [AGENTS.md](AGENTS.md)
  仓库级协作入口
- [backtest/README.md](backtest/README.md)
  Python 回测层说明

## 快速开始

Rust:

```bash
cargo check
cargo run
cargo bench --bench ws_hot_paths
cargo run --example snapshot_write
```

账户相关 example 需要先配置根目录 `.env`，可以从 `.env.example` 复制：

```bash
cp .env.example .env
```

其中最小必填配置只有：

- `PRIVATE_KEY`
  Polymarket CLOB 鉴权私钥

可选覆盖项：

- `SYMBOLS`
  默认监控或下单标的，逗号分隔，例如 `btc,eth`
- `INTERVALS`
  默认运行周期，逗号分隔，例如 `5m,15m`
- `SQLITE_PATH`
  事件 SQLite 路径
- `ALLOW_ORDER_USDC`
  正常放行候选使用的固定下单美元金额
- `REDUCE_ORDER_USDC`
  降档参与候选使用的固定下单美元金额
- `CRYPTO_REVERSAL_ORDER_PRICE`
  `0` 或不填表示继续按实时 Polymarket 报价下单；大于 `0` 时 `crypto_reversal` 触发后固定按该价格挂单
- `POLYMARKET_LTF_LOG_DIR`
  日志目录；默认使用当前运行目录下的 `logs/`

常用 example：

```bash
cargo run --example book_monitor
cargo run --example user_monitor
cargo run --example clob_ok -- btc buy 5 0.55 0.55 gtc 2
```

说明：

- `cargo run`
  启动 `crypto_reversal` runtime；策略参数和下单参数固定写在策略代码里
  `5m` 固定在每个 300 秒窗口的第 `290` 秒后开始扫描，`15m` 固定在每个 900 秒窗口的第 `890` 秒后开始扫描
- `user_monitor`
  启动账户 `open orders / positions` bootstrap 和 WS 增量监控
- `clob_ok`
  仅用于鉴权、下单与账户状态联调示例，不代表完整执行系统

Python:

```bash
cd backtest
PYTHONPATH=src python3 -m backtest
PYTHONPATH=src python3 -m scan --csv tests/fixtures/sample.csv --top-k 1
PYTHONPATH=src python3 -m unittest discover -s tests
```

## 建议阅读顺序

1. [docs/project.md](docs/project.md)
2. [docs/research.md](docs/research.md)
3. [docs/development.md](docs/development.md)
4. [AGENTS.md](AGENTS.md)
5. [backtest/README.md](backtest/README.md)
