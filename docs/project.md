# Project

## 1. 项目定位

`polymarket-ltf` 是一个面向 **LTF（Low Time Frame）** 的量化研究基础设施项目。  
它当前以 **Polymarket crypto 市场** 为核心研究对象，结合 CEX 与 oracle 数据，研究短周期偏离、策略可行性、执行约束以及更长期的高频套利框架。

本文件是项目总览文档，用来收敛仓库级叙事与设计边界。

项目的目标不是“快速做一个交易机器人”，而是建立一套可持续迭代的研究底座：

- 持续接入多源数据
- 构建稳定的研究输入
- 验证策略假设
- 形成可追溯的结果与日志
- 为未来执行层预留清晰边界

## 2. 研究命题

本项目服务于一个明确的研究问题：

**在低时间周期下，围绕 Polymarket crypto 市场，结合外部交易所与参考价格，识别和验证具有执行意义的短期偏离。**

项目长期关注的是：

- fair value 如何定义
- Polymarket 与外部市场之间的偏离是否具备统计意义
- 偏离在费用、滑点和延迟之后是否仍然成立
- 偏离能否转化为可执行的策略规则

## 3. 当前范围与边界

当前仓库已明确覆盖：

- Polymarket 活跃市场发现
- Polymarket 订单簿订阅与本地盘口缓存
- Polymarket 用户 `open orders / positions` 启动同步与增量维护
- Binance `bookTicker` 参考价
- Chainlink RTDS 锚定价
- 秒级 snapshot 生成与落盘
- Python 离线回测与分组评估

当前支持的研究维度包括：

- 标的：`btc`、`eth`、`sol`、`xrp`
- 周期：`5m`、`15m`

当前仓库应被视为：

- 研究基础设施
- 策略工程底座
- 多源数据驱动的离线验证平台

当前仓库不应被视为：

- 完整实盘执行系统
- 完整风控与订单管理系统
- 生产级自动交易平台

## 4. 标准研究闭环

```text
数据获取
  -> 数据标准化
  -> 特征构造与 snapshot
  -> 回测与研究评估
  -> 策略定义与实现
  -> 执行候选 / 执行
  -> 结果输出、日志归档与复盘
```

当前仓库已经较完整支持：

- 数据获取
- 多源聚合
- snapshot 构造
- 回测与研究评估

后续继续加强：

- 策略版本化
- 结果结构化落盘
- 执行层
- 风控层
- 统一研究报表与日志中心

## 5. 为什么是 Polymarket + 外部参考市场

Polymarket 的研究价值来自几个维度：

- 它是二元市场，不等同于传统现货或永续市场
- 盘口结构与外部 fair value 之间可能存在短周期偏离
- `up/down` 双腿结构天然适合做概率、盘口和偏离研究
- 通过接入 CEX 和 oracle，可以构建更稳定的参考系

因此，这个项目不是单纯的“Polymarket 数据抓取工具”，而是：

**围绕 Polymarket 的多源量化研究仓库。**

## 6. 系统设计概览

### 6.1 设计目标

当前架构围绕五个目标设计：

- 稳定接入 Polymarket、CEX、oracle 等多源数据
- 在短时间窗口内维护一致的研究状态
- 生成稳定的 snapshot 研究输入
- 将实时链路与离线研究严格分层
- 为未来执行层、结果层和日志层预留清晰边界

### 6.2 设计原则

- 实时层与研究层分离
- 数据源解耦
- 研究输入稳定
- 先研究，后执行
- 低延迟优先，但不牺牲结构清晰度
- 优先演化现有模块，不做补丁式扩张

### 6.3 分层结构

```text
L0 外部市场与参考源
  ├── Polymarket orderbook / Gamma
  ├── CEX market data
  └── Oracle / reference data

L1 实时接入层（Rust）
  ├── src/binance/websocket.rs
  ├── src/config.rs
  ├── src/polymarket/market_registry.rs
  ├── src/polymarket/orderbook_stream.rs
  ├── src/polymarket/rtds_stream.rs
  └── src/polymarket/user_stream.rs

L2 状态与特征层（Rust）
  ├── src/snapshot.rs
  ├── src/strategy/
  ├── src/types/crypto.rs
  ├── src/polymarket/types/open_orders.rs
  ├── src/polymarket/types/positions.rs
  └── src/polymarket/*

L3 数据边界
  └── data/snapshots/<symbol>/<interval>/<market_slug>.csv

L4 离线研究层（Python）
  ├── backtest/src/data/
  ├── backtest/src/domain/
  ├── backtest/src/engine/
  ├── backtest/src/reports/
  ├── backtest/src/strategies/
  ├── backtest/src/backtest.py
  ├── backtest/src/scan.py
  └── backtest/src/report.py

L5 未来扩展层
  ├── backtest reports
  ├── execution candidate layer
  └── logs and metadata
```

### 6.4 当前数据流

```text
Gamma -> Market Registry -> subscription set
                             │
                             ▼
Polymarket orderbook ----┐
                         ├──> Snapshot Engine -> CSV
Binance bookTicker ------┤
                         │
Chainlink RTDS ----------┘
```

```text
.env / env vars -> src/config.rs -> authenticated CLOB client
                                      │
                                      ▼
                         /data/orders + /data/positions bootstrap
                                      │
                                      ▼
                        user_stream WS incrementals -> local open_orders / positions
```

```text
data/snapshots/...csv
        │
        ▼
backtest/src/data/
        │
        ▼
backtest/src/engine/
        │
        ▼
backtest/src/strategies/
        │
        ▼
CLI 汇总 / 结果文件 / 未来研究报表
```

### 6.5 核心模块

- `src/polymarket/market_registry.rs`
  市场发现、注册表和订阅切换
- `src/polymarket/orderbook_stream.rs`
  Polymarket 订单簿接入与本地盘口缓存
- `src/binance/websocket.rs`
  当前 CEX 参考价格接入
- `src/polymarket/rtds_stream.rs`
  Chainlink RTDS 价格接入
- `src/config.rs`
  统一环境变量与本地 `.env` 加载入口
- `src/polymarket/user_stream.rs`
  账户级 `open orders / positions` bootstrap 与 WS 增量维护
- `src/polymarket/types/open_orders.rs`
  本地活跃挂单 canonical 状态
- `src/polymarket/types/positions.rs`
  本地持仓、成交费用推导与 fee fallback 规则

当前用户成交监控口径：

- 持仓增量只由当前账户自己的成交回报驱动
- maker 方向通过本地 `order_context` 还原，不依赖公共 trade side 猜测
- 实时手续费优先使用成交消息里的 `fee_rate_bps` 推导
- bootstrap 或缺少 `fee_rate_bps` 的数据源才回退到本地 market fee 规则
- `src/snapshot.rs`
  snapshot 特征计算与 CSV 写入
- `src/polymarket/relayer.rs`
  潜在执行层辅助能力，不属于当前默认研究主链路
- `backtest/`
  离线回测、批量评估与研究输出

## 7. 仓库角色分工

```text
src/
  Rust 实时数据接入、状态维护、snapshot 生成

src/strategy/
  Rust 侧策略目录，预留给运行时策略或执行候选逻辑

backtest/
  离线回测、批量评估、研究输出

backtest/src/strategies/
  Python 回测策略目录

data/
  snapshot 输入与后续研究结果

docs/
  项目总览、研究手册、开发规范

skills/
  项目内工作流和协作提示
```

## 8. 当前核心产出

### 8.1 研究输入

```text
data/snapshots/<symbol>/<interval>/<market_slug>.csv
```

这是当前实时层与研究层之间的正式接口。

### 8.2 研究逻辑

- 多源价格与盘口聚合
- spread 与统计特征计算
- `up/down` 双腿回测模型
- 分组与批量评估

### 8.3 研究规范

- 数据标准
- 结果与日志标准
- 策略准入清单
- 开发规范

## 9. 路线图摘要

### 当前阶段

当前仓库可定义为：

**Stage 1: Realtime Research Infrastructure**

已经具备：

- Polymarket 活跃市场发现
- Polymarket 订单簿订阅与本地盘口缓存
- Polymarket 账户状态监控示例与本地 `open orders / positions` 维护
- Binance `bookTicker` 参考中间价
- Chainlink RTDS 参考价格
- 秒级 snapshot 生成与落盘
- Python `up/down` 双腿离线回测
- 分组中文汇总输出
- 一套成形的研究与开发规范

尚未完整具备：

- 实盘执行
- 风控限额
- 完整订单生命周期管理
- 参数扫描平台
- 统一研究报表中心
- 多交易所标准化数据层

### 后续阶段

- Stage 1.5：提升研究质量
  手续费、滑点、胜率、best / worst market、结构化结果落盘
- Stage 2：策略研究平台化
  多策略、多参数、多特征组合与更多交易所接入
- Stage 3：执行候选层
  paper trading、执行信号、订单生命周期和执行日志
- Stage 4：风控与组合约束
  风险暴露、库存约束、停止条件和资金分配
- Stage 5：生产级执行栈
  真实下单接口、状态一致性、监控与运行手册

### 近期优先级

最近 1 到 3 个迭代最值得优先做的是：

1. 结构化研究输出
2. 强化回测口径
3. 固定数据标准
4. 继续补测试

## 10. 阅读顺序

推荐按下面顺序阅读：

1. [README.md](../README.md)
2. [research.md](./research.md)
3. [development.md](./development.md)
4. [AGENTS.md](../AGENTS.md)

如果你只关心回测层，再补充看：

- [backtest/README.md](../backtest/README.md)
