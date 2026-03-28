# Freqtrade

`backtest/freqtrade/` 用来跑 `crypto_reversal` 的回测数据。

这里的 Freqtrade 只承担：

- 现货 OHLCV 数据下载
- backtesting
- hyperopt
- 信号触发统计

这里不承担：

- `freqtrade trade`
- 实盘交易
- 模拟交易

信号计算参考 Rust：

- `src/strategy/crypto_reversal/model.rs`

过滤条件参考 Rust：

- `src/strategy/crypto_reversal/service.rs`

## 目录

```text
backtest/freqtrade/
├── README.md
├── config.json
├── strategies/
│   └── crypto_reversal.py
└── user_data/
```

## 文件

- `config.json`
  面向数据下载、回测和 hyperopt 的研究配置
- `strategies/crypto_reversal.py`
  `crypto_reversal` 的正式研究策略实现

## 默认口径

- 交易模式：`spot`
- 交易对：`BTC/USDT`
- 周期：`5m`
- 研究策略名：`CryptoReversal`
- 最大同时持仓：`1`（仅用于回测约束）
- 单笔 stake：`100 USDT`（Freqtrade 的固定字段名，这里仅表示回测资金配置）
- 时间范围示例：最近一年 `20250328-20260328`

## 命令

这里不再维护 `backtest/scripts/` 包装脚本。  
请直接执行下面这些研究命令，也不要把这个目录当成交易入口。

下载数据：

```bash
cd backtest
freqtrade download-data \
  --config freqtrade/config.json \
  --userdir freqtrade/user_data \
  --datadir freqtrade/user_data/data \
  --timeframes 5m \
  --timerange 20250328-20260328
```

典型的下载 + 回测流程：

```bash
cd backtest
freqtrade download-data \
  --config freqtrade/config.json \
  --userdir freqtrade/user_data \
  --datadir freqtrade/user_data/data \
  --timeframes 5m \
  --timerange 20250328-20260328

freqtrade backtesting \
  --config freqtrade/config.json \
  --userdir freqtrade/user_data \
  --datadir freqtrade/user_data/data \
  --strategy-path freqtrade/strategies \
  --strategy CryptoReversal \
  --timerange 20250328-20260328
```

信号统计：

```bash
cd backtest
python3 freqtrade/signal_eval.py
```

如果本地已经有数据，不想重复下载：

```bash
cd backtest
python3 freqtrade/signal_eval.py --skip-download
```

这个脚本会直接输出：

- 周期过滤后的触发次数
- 胜次数
- 负次数
- 胜率

回测：

```bash
cd backtest
freqtrade backtesting \
  --config freqtrade/config.json \
  --userdir freqtrade/user_data \
  --datadir freqtrade/user_data/data \
  --strategy-path freqtrade/strategies \
  --strategy CryptoReversal \
  --timerange 20250328-20260328
```

参数搜索：

```bash
cd backtest
freqtrade hyperopt \
  --config freqtrade/config.json \
  --userdir freqtrade/user_data \
  --datadir freqtrade/user_data/data \
  --strategy-path freqtrade/strategies \
  --strategy CryptoReversal \
  --spaces buy \
  --timerange 20250328-20260328
```
