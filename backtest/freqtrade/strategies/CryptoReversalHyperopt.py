from __future__ import annotations

from functools import reduce

import numpy as np
import talib.abstract as ta
from freqtrade.strategy import DecimalParameter, IStrategy, IntParameter
from pandas import DataFrame


class CryptoReversalHyperopt(IStrategy):
    """`crypto_reversal` 的 Freqtrade 信号研究适配策略。

    这个策略只复刻 Rust `crypto_reversal/model.rs` 里的纯 K 线信号层，
    用来做参数搜索、信号统计和标准化代理回测。

    它不尝试复刻：

    - Polymarket next market 选择
    - registry 可交易性检查
    - 真实下单、账户状态与执行约束
    """

    INTERFACE_VERSION = 3

    # 为了与 Rust `crypto_reversal` 的 `Up / Down` 双向信号保持一致，
    # 这里启用做空支持：`Up -> enter_long`，`Down -> enter_short`。
    can_short = True
    timeframe = "5m"

    # 对齐 Rust `min_bars()` 的最小热身口径：
    # max(rsi_period + 1, bb_period, max(macd_fast, macd_slow) + macd_signal, warmup_bars + 2)
    startup_candle_count = 128

    minimal_roi = {"0": 10.0}
    stoploss = -0.99
    trailing_stop = False
    process_only_new_candles = True
    use_exit_signal = True
    exit_profit_only = False
    ignore_roi_if_entry_signal = False

    # 第一阶段优先搜索入场阈值，不先把指标窗口一起放开。
    long_rsi_max = IntParameter(30, 45, default=40, space="buy")
    short_rsi_min = IntParameter(55, 70, default=60, space="buy")
    min_width_pct = DecimalParameter(0.05, 0.60, decimals=2, default=0.20, space="buy")
    band_pad_pct = DecimalParameter(0.0, 0.30, decimals=2, default=0.00, space="buy")

    # 第二阶段再逐步放开指标窗口，避免搜索空间一开始就失控。
    rsi_period = IntParameter(7, 21, default=14, space="buy")
    bb_period = IntParameter(20, 40, default=30, space="buy")
    bb_stddev = DecimalParameter(1.5, 2.5, decimals=1, default=2.0, space="buy")

    # 先固定 MACD 参数，降低过拟合和搜索维度。
    macd_fast = 12
    macd_slow = 26
    macd_signal = 9

    # 对齐 Rust 里的 score 分档阈值。
    add_score = 0.32
    max_score = 0.50

    def populate_indicators(self, dataframe: DataFrame, metadata: dict) -> DataFrame:
        """计算与 Rust 信号模型对应的指标列和派生分数列。"""

        rsi_period = int(self.rsi_period.value)
        bb_period = int(self.bb_period.value)
        bb_stddev = float(self.bb_stddev.value)
        long_rsi_max = float(self.long_rsi_max.value)
        short_rsi_min = float(self.short_rsi_min.value)
        band_pad_pct = float(self.band_pad_pct.value)

        dataframe["rsi"] = ta.RSI(dataframe, timeperiod=rsi_period)

        # `talib.abstract` 在不同输入形状下，返回值可能是 tuple / list，
        # 这里显式解包，避免按字典键访问导致兼容性问题。
        bb_upper, bb_middle, bb_lower = ta.BBANDS(
            dataframe["close"],
            timeperiod=bb_period,
            nbdevup=bb_stddev,
            nbdevdn=bb_stddev,
            matype=0,
        )
        dataframe["bb_upper"] = bb_upper
        dataframe["bb_middle"] = bb_middle
        dataframe["bb_lower"] = bb_lower

        _macd_line, _macd_signal, macd_hist = ta.MACD(
            dataframe["close"],
            fastperiod=self.macd_fast,
            slowperiod=self.macd_slow,
            signalperiod=self.macd_signal,
        )
        dataframe["macdhist"] = macd_hist

        # 布林带宽度按百分比表达，和 Rust `bb_width_pct` 保持同一语义。
        basis = dataframe["bb_middle"].replace(0, np.nan)
        dataframe["bb_width_pct"] = (
            (dataframe["bb_upper"] - dataframe["bb_lower"]) / basis
        ) * 100.0
        dataframe["band_pad"] = dataframe["bb_middle"] * (band_pad_pct / 100.0)

        # Rust 里 MACD 不是硬过滤，而是 score 的加分项。
        dataframe["macd_confirm_long"] = dataframe["macdhist"] >= dataframe["macdhist"].shift(1)
        dataframe["macd_confirm_short"] = dataframe["macdhist"] <= dataframe["macdhist"].shift(1)

        basis_safe = dataframe["bb_middle"].clip(lower=1e-9)
        long_rsi_denom = max(long_rsi_max, 1e-9)
        short_rsi_denom = max(100.0 - short_rsi_min, 1e-9)

        # `score_long / score_short` 对应 Rust `score(...)` 的研究近似实现。
        dataframe["score_long"] = (
            ((long_rsi_max - dataframe["rsi"]).clip(lower=0.0) / long_rsi_denom)
            + (((dataframe["bb_lower"] - dataframe["close"]).clip(lower=0.0) / basis_safe) * 100.0)
            + (dataframe["bb_width_pct"] / 10.0)
            + np.where(dataframe["macd_confirm_long"], 0.15, 0.0)
        )

        dataframe["score_short"] = (
            ((dataframe["rsi"] - short_rsi_min).clip(lower=0.0) / short_rsi_denom)
            + (((dataframe["close"] - dataframe["bb_upper"]).clip(lower=0.0) / basis_safe) * 100.0)
            + (dataframe["bb_width_pct"] / 10.0)
            + np.where(dataframe["macd_confirm_short"], 0.15, 0.0)
        )

        # `size_factor_*` 只保留作研究标签，不直接声明为真实仓位控制。
        dataframe["size_factor_long"] = np.select(
            [
                dataframe["score_long"] >= self.max_score,
                dataframe["score_long"] >= self.add_score,
            ],
            [2.0, 1.5],
            default=1.0,
        )
        dataframe["size_factor_short"] = np.select(
            [
                dataframe["score_short"] >= self.max_score,
                dataframe["score_short"] >= self.add_score,
            ],
            [2.0, 1.5],
            default=1.0,
        )

        return dataframe

    def populate_entry_trend(self, dataframe: DataFrame, metadata: dict) -> DataFrame:
        """生成入场信号。

        `enter_long` 对应 Rust 里的 `Up` 反转信号，
        `enter_short` 对应 Rust 里的 `Down` 反转信号。
        """

        long_rsi_max = float(self.long_rsi_max.value)
        short_rsi_min = float(self.short_rsi_min.value)
        min_width_pct = float(self.min_width_pct.value)

        # 做多反转：
        # 价格压到下轨附近，且 RSI 足够低，并要求当前波动不至于过窄。
        long_conditions = [
            dataframe["volume"] > 0,
            dataframe["bb_width_pct"] >= min_width_pct,
            dataframe["close"] <= (dataframe["bb_lower"] + dataframe["band_pad"]),
            dataframe["rsi"] < long_rsi_max,
        ]
        # 做空反转：
        # 价格抬到上轨附近，且 RSI 足够高，并要求当前波动不至于过窄。
        short_conditions = [
            dataframe["volume"] > 0,
            dataframe["bb_width_pct"] >= min_width_pct,
            dataframe["close"] >= (dataframe["bb_upper"] - dataframe["band_pad"]),
            dataframe["rsi"] > short_rsi_min,
        ]

        long_mask = reduce(lambda left, right: left & right, long_conditions)
        short_mask = reduce(lambda left, right: left & right, short_conditions)

        dataframe.loc[long_mask, "enter_long"] = 1
        dataframe.loc[short_mask, "enter_short"] = 1
        # 把 score 和 size_factor 写进 tag，便于导出成交后做分桶分析。
        dataframe.loc[long_mask, "enter_tag"] = (
            "reversal_up_score="
            + dataframe.loc[long_mask, "score_long"].round(4).astype(str)
            + "_size="
            + dataframe.loc[long_mask, "size_factor_long"].astype(str)
        )
        dataframe.loc[short_mask, "enter_tag"] = (
            "reversal_down_score="
            + dataframe.loc[short_mask, "score_short"].round(4).astype(str)
            + "_size="
            + dataframe.loc[short_mask, "size_factor_short"].astype(str)
        )

        return dataframe

    def populate_exit_trend(self, dataframe: DataFrame, metadata: dict) -> DataFrame:
        """生成研究代理退出信号。

        当前研究口径：

        - 在某根 K 线结束时出现入场信号并建仓；
        - 持有整整一根后，在下一根 K 线结束时平仓。

        对 Freqtrade dataframe 来说，这里用“前一根出现入场信号，则当前根触发退出”
        来表达固定持有 1 根柱子的退出规则。
        """

        dataframe.loc[
            (dataframe["volume"] > 0) & (dataframe["enter_long"].shift(1) == 1),
            "exit_long",
        ] = 1
        dataframe.loc[
            (dataframe["volume"] > 0) & (dataframe["enter_long"].shift(1) == 1),
            "exit_tag",
        ] = "hold_1_bar"
        dataframe.loc[
            (dataframe["volume"] > 0) & (dataframe["enter_short"].shift(1) == 1),
            "exit_short",
        ] = 1
        dataframe.loc[
            (dataframe["volume"] > 0) & (dataframe["enter_short"].shift(1) == 1),
            "exit_tag",
        ] = "hold_1_bar"

        return dataframe
