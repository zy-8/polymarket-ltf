# Freqtrade Research Adapter

`backtest/freqtrade/` 现在主要用于做一件事：

- 评估 `crypto_reversal` 信号，对“下一根柱子涨跌方向”的命中率

这里不是正式的 Polymarket 回测系统，也不是 Rust runtime 的执行模拟。

## 目录结构

```text
backtest/freqtrade/
├── README.md
├── config.json
├── signal_eval.py
├── strategies/
│   └── CryptoReversalHyperopt.py
└── user_data/
    ├── data/
    ├── backtest_results/
    └── hyperopt_results/
```

## 现在各文件是干什么的

- `signal_eval.py`
  这是当前主入口。
  它读取 Binance futures K 线，计算 `UP / DOWN` 信号，并纳入背景周期过滤，然后统计：
  当前柱子出信号后，下一根柱子方向是否命中。

- `strategies/CryptoReversalHyperopt.py`
  这是保留给 Freqtrade strategy / hyperopt 使用的策略定义文件。
  它不再承担你平时直接运行的主入口职责。

- `config.json`
  Freqtrade 下载数据时使用的最小配置。

- `user_data/`
  Freqtrade 的本地工作目录：
  数据下载到这里，其他实验产物也放这里。

## 当前研究口径

当前脚本研究的是：

- 当前这根 5m 柱子收盘时，如果出现 `UP` 信号
- 那么下一根 5m 柱子最终是否上涨

以及：

- 当前这根 5m 柱子收盘时，如果出现 `DOWN` 信号
- 那么下一根 5m 柱子最终是否下跌

所以这里看的不是：

- 开仓价
- 平仓价
- 盈亏
- 回撤

而是：

- `UP` 命中率
- `DOWN` 命中率
- 总体命中率
- 高分组命中率

## 背景过滤

当前 `signal_eval.py` 已经补入 Rust `crypto_reversal/service.rs` 的背景过滤逻辑。

对 `5m` 主周期：

- 主信号使用 `5m`
- 背景过滤使用 `15m + 1h`

对 `15m` 主周期：

- 主信号使用 `15m`
- 背景过滤使用 `1h + 4h`

背景过滤结果分为：

- `allow`
- `reduce`
- `block`

其中：

- `block` 的信号会被排除，不计入最终有效信号统计
- `reduce` 的信号会保留
- 当前脚本还会把 `allow / reduce / block / missing` 的数量打印出来

## 研究边界

这里复刻的是 `src/strategy/crypto_reversal/model.rs` 的 K 线信号层，包括：

- RSI
- Bollinger Bands
- MACD histogram
- reversal entry 条件
- score / size_factor 的近似表达

这里不复刻：

- Polymarket next market 选择
- registry 可交易性检查
- Polymarket 下单与执行约束
- Rust runtime 的账户状态协同

## 安装

你可以直接用项目现有 `.venv`，只要里面已经安装了 `freqtrade`。

如果还没装：

```bash
source .venv/bin/activate
python3 -m pip install -U pip
python3 -m pip install freqtrade
```

## 最常用命令

如果本地已经有数据，直接跑：

```bash
source .venv/bin/activate
python backtest/freqtrade/signal_eval.py --skip-download
```

如果本地没有数据，就让脚本自动下载再评估：

```bash
source .venv/bin/activate
python backtest/freqtrade/signal_eval.py
```

如果只想换交易对或时间范围：

```bash
python backtest/freqtrade/signal_eval.py \
  --pair BTC/USDT:USDT \
  --timeframe 5m \
  --timerange 20240101-20241231
```

## 输出结果

`signal_eval.py` 会直接在终端打印：

- 总信号数
- 总体命中率
- 背景过滤下 `allow / reduce / block / missing` 的数量
- `UP` 信号数和命中率
- `DOWN` 信号数和命中率
- 高分组 `size=2.0` 的命中率

## 如果以后还要用 Freqtrade strategy

如果你后面还想继续做 Freqtrade 的策略实验或 hyperopt，可以继续用：

- `strategies/CryptoReversalHyperopt.py`

但对你当前这个项目目标来说，平时优先使用：

- `signal_eval.py`
