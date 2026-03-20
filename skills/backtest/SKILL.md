---
name: backtest
description: 当处理本仓库的 LTF Python 回测工作流时使用，包括 backtest/、snapshot CSV 分析、5m/15m 批量回测、Polymarket up/down 双腿模型、多源研究输入，以及中文 CLI 报告输出与回测文档维护。
---

# Backtest

当任务集中在本仓库的离线回测流程时，使用这个 skill。

## 工作前先看

- `docs/project.md`
- `docs/research.md`
- `docs/development.md`
- `backtest/README.md`

## 适用场景

- 修改 `backtest/` 下的代码
- 调整回测策略、CLI 行为或中文报告输出
- 分析 `data/snapshots/` 下的 snapshot CSV
- 设计结构化回测结果、报表和研究输出
- 分别回测 `5m` / `15m`
- 修改 Polymarket `up/down` 双腿仿真逻辑
- 重写 README / AGENTS / 回测文档中与研究层相关的说明

## 仓库内布局

- 代码目录：`backtest/`
- 回测入口：`backtest/src/cli.py`
- 测试目录：`backtest/tests/`
- 数据输入：`data/snapshots/<symbol>/<interval>/<market_slug>.csv`
- 真实数据放在 `data/` 下，不要放进 `backtest/`

## 工程约束

- 先复用现有 `data/`、`domain/`、`engine/`、`reports/`、`strategies/`、`cli.py` 边界，不要随意加中间层。
- 回测主循环和结果聚合逻辑优先保持简单、直达、低分支。
- 不要为了一个策略临时需求把结果结构、字段语义或目录结构做成补丁堆叠。

## 当前回测模型

- 引擎按 Polymarket `up/down` 双腿模型回测，不做裸空 `up`。
- 买 `up` 用 `up_ask_price`，卖 `up` 用 `up_bid_price`。
- 买 `down` 用 `down_ask_price`，卖 `down` 用 `down_bid_price`。
- 持仓估值使用 `up_mid_price` 和 `down_mid_price`。
- 示例策略基于 `z_score` 做均值回归：
  - `z_score` 低时买 `up`
  - `z_score` 高时买 `down`
  - 回到中性区间时平仓

## 项目上下文

- 这是 LTF 高频价差研究的离线验证层
- 它消费 Rust 已落盘的 snapshot 数据
- 当前重点是“验证研究假设”，不是模拟完整实盘执行系统
- 未来会逐步承接更多多交易所研究输入和更标准化的结果输出

## 新策略进入实现前必须检查

- 研究假设是否清楚
- fair value 定义是否清楚
- 数据和 snapshot 字段是否足够
- 是否显式考虑手续费、滑点、样本量和分组验证
- 是否显式考虑延迟预算和失效窗口
- 输出是否能追溯到数据、参数和策略版本
- 是否定义了不交易条件和风险约束

## CLI 行为

- `cd backtest && PYTHONPATH=src python3 -m cli`
  默认跑全部 snapshot CSV，并按 interval 分组汇总
- `cd backtest && PYTHONPATH=src python3 -m cli --interval 5m`
  跑整个 `5m` 目录
- `cd backtest && PYTHONPATH=src python3 -m cli --interval 15m`
  跑整个 `15m` 目录
- `cd backtest && PYTHONPATH=src python3 -m cli --symbol btc --interval 5m`
  跑某个 symbol + interval 切片
- `cd backtest && PYTHONPATH=src python3 -m cli --symbol btc --interval 5m --market-slug ...`
  跑单个 market 文件
- `cd backtest && PYTHONPATH=src python3 -m cli --csv ...`
  跑显式指定的 CSV 文件

## 输出约定

- 终端输出默认保持中文，除非用户明确要求英文。
- 保持现有输出结构：
  - `回测明细：`
  - `分组汇总：`
  - `总体汇总：`
- 如果新增指标，优先补有实际价值的字段，例如现金、权益、胜率、每文件平均成交数、回撤。
- 如果新增结果文件或日志输出，应遵守 `docs/research.md` 中的数据与结果标准。

## 验证方式

- 路径解析、策略逻辑、汇总格式都要有单元测试。
- 改动后优先运行：

```bash
cd backtest
PYTHONPATH=src python3 -m unittest discover -s tests
PYTHONPATH=src python3 -m cli --interval 5m
PYTHONPATH=src python3 -m cli --interval 15m
```

## 工作方式

1. 保持 `data/`、`domain/`、`engine/`、`reports/`、`strategies/`、`cli.py` 按职责分层。
2. 生产数据不要塞进测试 fixture。
3. 改策略模型时，同时验证单文件与批量回测输出。
4. 如果 CLI 行为变化，记得同步更新仓库文档。
5. 如果新增策略或结果结构，记得同步更新 `docs/research.md`。
6. 能改现有引擎和结果模型就先改，不要堆一层“strategy_v2”式旁路实现。
