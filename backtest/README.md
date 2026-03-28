# backtest

`backtest/` 的正式研究入口是：

- [`freqtrade/`](./freqtrade/README.md)

它用于承载 `crypto_reversal` 的 Binance K 线回测，包括：

- 研究策略实现
- 历史数据下载
- backtesting
- hyperopt
- 信号统计

这里的 Freqtrade 用途限定为研究，不作为交易执行入口。

## 当前边界

这里使用 `backtest/freqtrade/` 作为 `crypto_reversal` 的研究入口。

当前仓库里，`backtest/` 应理解为：

- `crypto_reversal` 的 Freqtrade 研究工作区

当前仓库里，`backtest/` 不承担：

- Polymarket snapshot 自定义回测引擎
- 独立于 Freqtrade 的第二套 K 线回测入口
- Freqtrade 实盘或模拟交易入口

## 目录结构

```text
backtest/
├── README.md
├── pyproject.toml
└── freqtrade/
    ├── README.md
    ├── config.json
    └── strategies/
```

目录职责：

- `freqtrade/config.json`
  Freqtrade 数据下载、回测和 hyperopt 的研究配置
- `freqtrade/strategies/crypto_reversal.py`
  `crypto_reversal` 的正式研究策略实现

## 使用方式

优先直接看并使用：

- [`freqtrade/README.md`](./freqtrade/README.md)

典型流程是：

1. 下载 Binance spot OHLCV
2. 运行 `signal_eval.py` 统计触发次数、胜负次数和胜率
3. 运行 `CryptoReversal` backtesting
4. 运行 hyperopt
5. 导出并分析结果

`backtest/` 当前不再维护 `scripts/` 包装脚本。  
研究命令统一直接写在 [`freqtrade/README.md`](./freqtrade/README.md)。

## 依赖

这个目录本身不提供可安装的自定义回测包。  
请直接使用 Freqtrade 运行环境。
