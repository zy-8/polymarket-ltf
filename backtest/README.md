# backtest

`backtest/` 是 `polymarket-ltf` 的 Python crypto 回测与研究项目。

它消费 Rust 已落盘的 `data/snapshots/...csv`，围绕 Polymarket crypto `up/down` 双腿市场做离线回放、批量评估、参数扫描和报表输出。

当前 CLI 已经支持两类注册点：

- `--data-format`
  选择数据读取器，当前默认是 `snapshot_csv`
- `--strategy`
  选择策略实现，当前默认是 `mean_reversion_zscore`

## 项目结构

```text
backtest/
├── pyproject.toml
├── README.md
├── src/
│   ├── backtest.py
│   ├── scan.py
│   ├── report.py
│   ├── data/
│   ├── domain/
│   ├── engine/
│   ├── reports/
│   └── strategies/
└── tests/
    └── fixtures/
```

目录职责：

- `src/data/`
  数据读取器与格式注册，当前 `snapshot_csv` 读取器直接使用 `pandas`
- `src/domain/`
  Polymarket crypto 回测领域模型、信号协议、持仓与结果对象
- `src/engine/`
  撮合、手续费、仓位再平衡和权益曲线
- `src/reports/`
  结构化结果、汇总、CSV/JSON 输出
- `src/strategies/`
  回测策略
- `src/backtest.py`
  单次回测入口
- `src/scan.py`
  `pandas` 参数扫描与汇总入口
- `src/report.py`
  `report.json` 再渲染入口
- `tests/`
  单元测试与最小示例数据

## 当前回测模型

当前引擎按 Polymarket `up/down` 双腿模型回测：

- 买 `up`：`up_ask_price`
- 卖 `up`：`up_bid_price`
- 买 `down`：`down_ask_price`
- 卖 `down`：`down_bid_price`
- `up` 估值：`up_mid_price`
- `down` 估值：`down_mid_price`
- 成交默认按 taker 口径记账
- `--fee-bps` 表示 Polymarket fee rate，不再按 `gross * bps` 简化
- 买单手续费先按 Polymarket crypto fee 公式算成 `USDC`，再折成 shares 扣减持仓
- 卖单手续费按同一公式直接从 `USDC` 收益中扣减

当前基线策略是 `z_score` 均值回归：

- `z_score <= -entry_z` 时做多 `up`
- `z_score >= entry_z` 时买入 `down`
- `abs(z_score) <= exit_z` 时平仓

## 使用方式

推荐在 `backtest/` 目录下执行，并显式设置 `PYTHONPATH=src`。

当前入口拆成三个命令：

- `backtest`
  跑回测并可写结构化结果
- `scan`
  跑参数扫描
- `report`
  基于已有 `report.json` 生成 HTML / quantstats 报表

当前默认入口参数：

- `--data-format snapshot_csv`
- `--strategy mean_reversion_zscore`

直接按模块运行：

```bash
cd backtest
PYTHONPATH=src python3 -m backtest
PYTHONPATH=src python3 -m backtest --data-format snapshot_csv --strategy mean_reversion_zscore
PYTHONPATH=src python3 -m backtest --interval 5m
PYTHONPATH=src python3 -m backtest --interval 15m
PYTHONPATH=src python3 -m backtest --symbol btc --interval 5m
PYTHONPATH=src python3 -m backtest --symbol btc --interval 5m --market-slug <market_slug>
PYTHONPATH=src python3 -m backtest --csv ../data/snapshots/btc/5m/<market_slug>.csv
```

也可以先安装为可编辑项目：

```bash
cd backtest
python3 -m pip install -e .
backtest --interval 5m
scan --interval 5m --top-k 10
report --report-json ../data/backtests/demo/report.json --html-report
```

## pandas

这个项目把 `pandas` 作为标准分析依赖使用，主要用于：

- snapshot CSV 读取
- 参数扫描结果汇总
- 多文件回测结果 DataFrame 化
- HTML 报表中的扫描表展示
- 后续更大样本的分组研究

## 结构化输出

传入 `--output-dir` 后，会写出：

```text
<output-dir>/
├── report.json
├── run_summaries.csv
├── group_summaries.csv
├── trades/
└── equity/
```

其中：

- `report.json`
  完整结果，包含策略参数、分组汇总、运行明细、成交明细和权益曲线
- `run_summaries.csv`
  单文件级别结果
- `group_summaries.csv`
  分组级别结果
- `trades/`
  每个 run 的成交明细
- `equity/`
  每个 run 的权益曲线

示例：

```bash
cd backtest
PYTHONPATH=src python3 -m backtest --interval 5m --output-dir ../data/backtests/demo
```

## 参数扫描

```bash
cd backtest
PYTHONPATH=src python3 -m scan \
  --interval 5m \
  --scan-entry-z-values 1.0,1.5,2.0 \
  --scan-exit-z-values 0.3,0.5,0.8 \
  --scan-size-values 0.5,1 \
  --scan-max-run-values 0,2 \
  --top-k 10 \
  --output-dir ../data/backtests/scan_5m
```

扫描会用 `pandas` 生成 `scan_results.csv` 和 `scan_results.json`。

安装方式：

```bash
cd backtest
python3 -m pip install -e .
```

## 数据与策略扩展

当前已经预留了两个扩展点：

- `src/data/registry.py`
  注册数据格式 loader
- `src/strategies/registry.py`
  注册策略与参数扫描变体

以后新增一种数据格式时，新增 loader 并注册到 `DATA_FORMATS`。
以后新增一种策略时，新增策略文件并注册到 `STRATEGIES`。

## 报表

HTML 报表：

```bash
cd backtest
PYTHONPATH=src python3 -m backtest --interval 5m --output-dir ../data/backtests/demo --html-report
```

quantstats 报表：

```bash
cd backtest
python3 -m pip install -e .[reports]
PYTHONPATH=src python3 -m backtest --interval 5m --output-dir ../data/backtests/demo --quantstats-report
PYTHONPATH=src python3 -m report --report-json ../data/backtests/demo/report.json --html-report --quantstats-report
```

## 验证

```bash
cd backtest
PYTHONPATH=src python3 -m unittest discover -s tests
PYTHONPATH=src python3 -m backtest --interval 5m
PYTHONPATH=src python3 -m backtest --interval 15m
PYTHONPATH=src python3 -m scan --csv tests/fixtures/sample.csv --top-k 1
PYTHONPATH=src python3 -m backtest --interval 5m --output-dir ../data/backtests/smoke
```
