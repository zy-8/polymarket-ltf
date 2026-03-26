from __future__ import annotations

import argparse
from dataclasses import dataclass
import subprocess
import sys
from pathlib import Path

import numpy as np
import pandas as pd
import talib.abstract as ta

from strategies.CryptoReversalHyperopt import CryptoReversalHyperopt


@dataclass(frozen=True)
class BackgroundProfile:
    """背景周期过滤参数。

    这些数值直接对齐 Rust `crypto_reversal/service.rs` 里的配置。
    """

    fast_timeframe: str
    slow_timeframe: str
    fast_lookback: int
    slow_lookback: int
    block_fast_pct: float
    block_slow_pct: float
    reduce_fast_pct: float
    reduce_slow_pct: float
    reduce_factor: float


def build_cli_parser() -> argparse.ArgumentParser:
    """构造命令行参数。

    这个脚本的职责非常单一：

    - 可选地下载 Binance futures K 线
    - 计算 `crypto_reversal` 信号
    - 统计“当前柱子出信号后，下一根柱子方向是否命中”
    """

    parser = argparse.ArgumentParser(
        description="评估 crypto_reversal 信号对下一根柱子方向的命中率"
    )
    parser.add_argument(
        "--pair",
        default="BTC/USDT:USDT",
        help="交易对，默认 BTC/USDT:USDT",
    )
    parser.add_argument(
        "--timeframe",
        default="5m",
        help="K 线周期，默认 5m",
    )
    parser.add_argument(
        "--timerange",
        default="20240101-20241231",
        help="下载数据的时间范围，默认 20240101-20241231",
    )
    parser.add_argument(
        "--skip-download",
        action="store_true",
        help="跳过数据下载，直接使用本地已有数据",
    )
    return parser


def freqtrade_root_dir() -> Path:
    """返回 `backtest/freqtrade/` 根目录。"""

    return Path(__file__).resolve().parent


def run_command(command: list[str], cwd: Path) -> None:
    """执行外部命令。

    这里主要用于调用 `freqtrade download-data`。
    如果命令失败，直接抛异常，方便用户看到原始报错。
    """

    full_command = [sys.executable, "-m", *command]
    print("running:", " ".join(full_command))
    subprocess.run(full_command, cwd=str(cwd), check=True)


def data_file_path(data_dir: Path, pair: str, timeframe: str) -> Path:
    """根据交易对和周期拼出 Freqtrade 数据文件路径。

    例如：

    - `BTC/USDT:USDT`
    - `5m`

    会映射到：

    - `futures/BTC_USDT_USDT-5m-futures.feather`
    """

    normalized_pair = pair.replace("/", "_").replace(":", "_")
    return data_dir / "futures" / f"{normalized_pair}-{timeframe}-futures.feather"


def ensure_data_downloaded(
    project_root: Path,
    config_path: Path,
    user_dir: Path,
    data_dir: Path,
    pair: str,
    timeframe: str,
    timerange: str,
) -> None:
    """用 Freqtrade 下载主周期 OHLCV 数据。"""

    run_command(
        [
            "freqtrade",
            "download-data",
            "--config",
            str(config_path),
            "--userdir",
            str(user_dir),
            "--datadir",
            str(data_dir),
            "--pairs",
            pair,
            "--timeframes",
            timeframe,
            "--timerange",
            timerange,
        ],
        cwd=project_root,
    )


def load_ohlcv_frame(path: Path) -> pd.DataFrame:
    """读取本地 feather 数据，并按时间排序。"""

    frame = pd.read_feather(path).copy()
    frame["date"] = pd.to_datetime(frame["date"], utc=True)
    return frame.sort_values("date").reset_index(drop=True)


def default_params() -> dict[str, float | int]:
    """读取策略类里的当前默认参数。

    这里统一从 `CryptoReversalHyperopt` 里拿默认值，
    这样信号评估脚本和 Freqtrade 策略定义不会各维护一份参数。
    """

    return {
        "long_rsi_max": int(CryptoReversalHyperopt.long_rsi_max.value),
        "short_rsi_min": int(CryptoReversalHyperopt.short_rsi_min.value),
        "min_width_pct": float(CryptoReversalHyperopt.min_width_pct.value),
        "band_pad_pct": float(CryptoReversalHyperopt.band_pad_pct.value),
        "rsi_period": int(CryptoReversalHyperopt.rsi_period.value),
        "bb_period": int(CryptoReversalHyperopt.bb_period.value),
        "bb_stddev": float(CryptoReversalHyperopt.bb_stddev.value),
        "macd_fast": int(CryptoReversalHyperopt.macd_fast),
        "macd_slow": int(CryptoReversalHyperopt.macd_slow),
        "macd_signal": int(CryptoReversalHyperopt.macd_signal),
        "add_score": float(CryptoReversalHyperopt.add_score),
        "max_score": float(CryptoReversalHyperopt.max_score),
    }


def background_profile(timeframe: str) -> BackgroundProfile | None:
    """返回给定主周期对应的背景过滤规则。"""

    if timeframe == "5m":
        return BackgroundProfile(
            fast_timeframe="15m",
            slow_timeframe="1h",
            fast_lookback=8,
            slow_lookback=6,
            block_fast_pct=0.006,
            block_slow_pct=0.010,
            reduce_fast_pct=0.003,
            reduce_slow_pct=0.005,
            reduce_factor=0.5,
        )
    if timeframe == "15m":
        return BackgroundProfile(
            fast_timeframe="1h",
            slow_timeframe="4h",
            fast_lookback=8,
            slow_lookback=6,
            block_fast_pct=0.008,
            block_slow_pct=0.012,
            reduce_fast_pct=0.004,
            reduce_slow_pct=0.006,
            reduce_factor=0.75,
        )
    return None


def compute_signal_frame(frame: pd.DataFrame) -> pd.DataFrame:
    """给原始 K 线加上信号评估所需的全部列。

    这个函数只做研究，不做交易回测。

    最终关心的是：

    - 当前柱子是否触发 `UP`
    - 当前柱子是否触发 `DOWN`
    - 下一根柱子最终是涨还是跌
    """

    params = default_params()
    result = frame.copy()

    # 1. 计算基础技术指标。
    result["rsi"] = ta.RSI(result, timeperiod=params["rsi_period"])
    bb_upper, bb_middle, bb_lower = ta.BBANDS(
        result["close"],
        timeperiod=params["bb_period"],
        nbdevup=params["bb_stddev"],
        nbdevdn=params["bb_stddev"],
        matype=0,
    )
    result["bb_upper"] = bb_upper
    result["bb_middle"] = bb_middle
    result["bb_lower"] = bb_lower
    _macd_line, _macd_signal, macd_hist = ta.MACD(
        result["close"],
        fastperiod=params["macd_fast"],
        slowperiod=params["macd_slow"],
        signalperiod=params["macd_signal"],
    )
    result["macdhist"] = macd_hist

    # 2. 计算布林带宽度和 MACD 确认列。
    basis = result["bb_middle"].replace(0, np.nan)
    result["bb_width_pct"] = ((result["bb_upper"] - result["bb_lower"]) / basis) * 100.0
    result["band_pad"] = result["bb_middle"] * (params["band_pad_pct"] / 100.0)
    result["macd_confirm_long"] = result["macdhist"] >= result["macdhist"].shift(1)
    result["macd_confirm_short"] = result["macdhist"] <= result["macdhist"].shift(1)

    # 3. 计算 score 和 size_factor。
    basis_safe = result["bb_middle"].clip(lower=1e-9)
    long_rsi_denom = max(float(params["long_rsi_max"]), 1e-9)
    short_rsi_denom = max(100.0 - float(params["short_rsi_min"]), 1e-9)

    result["score_long"] = (
        ((params["long_rsi_max"] - result["rsi"]).clip(lower=0.0) / long_rsi_denom)
        + (((result["bb_lower"] - result["close"]).clip(lower=0.0) / basis_safe) * 100.0)
        + (result["bb_width_pct"] / 10.0)
        + np.where(result["macd_confirm_long"], 0.15, 0.0)
    )
    result["score_short"] = (
        ((result["rsi"] - params["short_rsi_min"]).clip(lower=0.0) / short_rsi_denom)
        + (((result["close"] - result["bb_upper"]).clip(lower=0.0) / basis_safe) * 100.0)
        + (result["bb_width_pct"] / 10.0)
        + np.where(result["macd_confirm_short"], 0.15, 0.0)
    )

    result["size_factor_long"] = np.select(
        [
            result["score_long"] >= params["max_score"],
            result["score_long"] >= params["add_score"],
        ],
        [2.0, 1.5],
        default=1.0,
    )
    result["size_factor_short"] = np.select(
        [
            result["score_short"] >= params["max_score"],
            result["score_short"] >= params["add_score"],
        ],
        [2.0, 1.5],
        default=1.0,
    )

    # 4. 给每根柱子打上 `UP / DOWN` 信号标签。
    result["enter_long"] = (
        (result["volume"] > 0)
        & (result["bb_width_pct"] >= params["min_width_pct"])
        & (result["close"] <= (result["bb_lower"] + result["band_pad"]))
        & (result["rsi"] < params["long_rsi_max"])
    )
    result["enter_short"] = (
        (result["volume"] > 0)
        & (result["bb_width_pct"] >= params["min_width_pct"])
        & (result["close"] >= (result["bb_upper"] - result["band_pad"]))
        & (result["rsi"] > params["short_rsi_min"])
    )

    # 5. 生成“下一根柱子方向”。
    # 如果当前第 t 根出信号，就只检查第 t+1 根是涨还是跌。
    result["next_open"] = result["open"].shift(-1)
    result["next_close"] = result["close"].shift(-1)
    result["next_is_up"] = result["next_close"] > result["next_open"]
    result["next_is_down"] = result["next_close"] < result["next_open"]

    return result


def percent_change(first: float, last: float) -> float:
    """计算区间涨跌幅。"""

    return 0.0 if first == 0 else (last - first) / first


def pandas_rule(timeframe: str) -> str:
    """把 Freqtrade 周期字符串映射成 pandas 重采样规则。"""

    mapping = {
        "5m": "5min",
        "15m": "15min",
        "1h": "1h",
        "4h": "4h",
    }
    try:
        return mapping[timeframe]
    except KeyError as exc:
        raise ValueError(f"暂不支持的周期: {timeframe}") from exc


def resample_ohlcv(frame: pd.DataFrame, timeframe: str) -> pd.DataFrame:
    """把主周期 OHLCV 重采样成背景周期。

    这里直接从本地已有 K 线重采样，避免为了背景过滤再额外联网拉数据。
    """

    indexed = frame.copy().set_index("date")
    resampled = indexed.resample(pandas_rule(timeframe), label="right", closed="right").agg(
        {
            "open": "first",
            "high": "max",
            "low": "min",
            "close": "last",
            "volume": "sum",
        }
    )
    resampled = resampled.dropna(subset=["open", "high", "low", "close"]).reset_index()
    return resampled


def load_main_timeframe_frame(data_dir: Path, pair: str, timeframe: str) -> pd.DataFrame:
    """读取主周期数据。

    优先读取与目标周期完全匹配的本地文件。
    如果目标是 `15m` 且本地没有 `15m` 文件，则退化为：

    - 读取现有 `5m` 文件
    - 在本地重采样成 `15m`

    这样在你已经下载过 `5m` 的情况下，不必额外联网再拉一次 `15m`。
    """

    direct_path = data_file_path(data_dir, pair, timeframe)
    if direct_path.exists():
        return load_ohlcv_frame(direct_path)

    if timeframe == "15m":
        fallback_path = data_file_path(data_dir, pair, "5m")
        if fallback_path.exists():
            fallback_frame = load_ohlcv_frame(fallback_path)
            return resample_ohlcv(fallback_frame, "15m")

    raise FileNotFoundError(
        f"未找到主周期数据文件: {direct_path}。"
        "如果本地还没有数据，请不要传 --skip-download。"
    )


def evaluate_background_action(
    side: str,
    change_fast: float,
    change_slow: float,
    profile: BackgroundProfile,
) -> str:
    """按 Rust 规则返回背景过滤动作。"""

    if side == "up":
        if change_fast <= -profile.block_fast_pct and change_slow <= -profile.block_slow_pct:
            return "block"
        if change_fast <= -profile.reduce_fast_pct or change_slow <= -profile.reduce_slow_pct:
            return "reduce"
        return "allow"

    if change_fast >= profile.block_fast_pct and change_slow >= profile.block_slow_pct:
        return "block"
    if change_fast >= profile.reduce_fast_pct or change_slow >= profile.reduce_slow_pct:
        return "reduce"
    return "allow"


def action_for_timestamp(
    timestamp: pd.Timestamp,
    side: str,
    fast_frame: pd.DataFrame,
    slow_frame: pd.DataFrame,
    profile: BackgroundProfile,
) -> str:
    """计算某个信号时间点的背景过滤动作。

    只使用“当前信号时刻之前已经收盘”的背景周期 K 线。
    如果样本不足，则返回 `missing`。
    """

    fast_closed = fast_frame[fast_frame["date"] <= timestamp].tail(profile.fast_lookback)
    slow_closed = slow_frame[slow_frame["date"] <= timestamp].tail(profile.slow_lookback)

    if len(fast_closed) < profile.fast_lookback or len(slow_closed) < profile.slow_lookback:
        return "missing"

    change_fast = percent_change(float(fast_closed["close"].iloc[0]), float(fast_closed["close"].iloc[-1]))
    change_slow = percent_change(float(slow_closed["close"].iloc[0]), float(slow_closed["close"].iloc[-1]))
    return evaluate_background_action(side, change_fast, change_slow, profile)


def compute_background_actions(
    signal_dates: pd.Series,
    side: str,
    fast_frame: pd.DataFrame,
    slow_frame: pd.DataFrame,
    profile: BackgroundProfile,
) -> list[str]:
    """批量计算背景过滤动作。

    这里不用逐条切 dataframe，而是：

    - 先把背景周期的 `date/close` 转成数组
    - 再用 `searchsorted` 快速找到“当前信号时刻之前最后一根已收盘背景 K 线”

    这样在全年 5m 数据上会快很多。
    """

    fast_dates = fast_frame["date"].to_numpy(dtype="datetime64[ns]")
    slow_dates = slow_frame["date"].to_numpy(dtype="datetime64[ns]")
    fast_closes = fast_frame["close"].to_numpy(dtype=float)
    slow_closes = slow_frame["close"].to_numpy(dtype=float)

    actions: list[str] = []
    for timestamp in signal_dates.to_numpy(dtype="datetime64[ns]"):
        fast_end = np.searchsorted(fast_dates, timestamp, side="right")
        slow_end = np.searchsorted(slow_dates, timestamp, side="right")

        if fast_end < profile.fast_lookback or slow_end < profile.slow_lookback:
            actions.append("missing")
            continue

        fast_start = fast_end - profile.fast_lookback
        slow_start = slow_end - profile.slow_lookback

        change_fast = percent_change(fast_closes[fast_start], fast_closes[fast_end - 1])
        change_slow = percent_change(slow_closes[slow_start], slow_closes[slow_end - 1])
        actions.append(evaluate_background_action(side, change_fast, change_slow, profile))

    return actions


def apply_background_filter(
    signal_frame: pd.DataFrame,
    timeframe: str,
) -> pd.DataFrame:
    """把 Rust `service.rs` 的背景过滤补到主信号结果里。"""

    profile = background_profile(timeframe)
    if profile is None:
        result = signal_frame.copy()
        result["background_action_long"] = "allow"
        result["background_action_short"] = "allow"
        return result

    fast_frame = resample_ohlcv(signal_frame[["date", "open", "high", "low", "close", "volume"]], profile.fast_timeframe)
    slow_frame = resample_ohlcv(signal_frame[["date", "open", "high", "low", "close", "volume"]], profile.slow_timeframe)

    result = signal_frame.copy()
    result["background_action_long"] = compute_background_actions(
        result["date"], "up", fast_frame, slow_frame, profile
    )
    result["background_action_short"] = compute_background_actions(
        result["date"], "down", fast_frame, slow_frame, profile
    )

    # 背景过滤会影响最终可用信号：
    # - `block`：该信号在原始策略里会被直接拦掉
    # - `reduce`：信号仍保留，只是 size_factor 会被打折
    result["effective_long"] = result["enter_long"] & (result["background_action_long"] != "block")
    result["effective_short"] = result["enter_short"] & (result["background_action_short"] != "block")

    result["effective_size_factor_long"] = np.where(
        result["background_action_long"] == "reduce",
        result["size_factor_long"] * profile.reduce_factor,
        result["size_factor_long"],
    )
    result["effective_size_factor_short"] = np.where(
        result["background_action_short"] == "reduce",
        result["size_factor_short"] * profile.reduce_factor,
        result["size_factor_short"],
    )

    return result


def hit_ratio(hits: int, total: int) -> float:
    """把命中数转换成百分比。"""

    return 0.0 if total == 0 else hits / total * 100.0


def print_summary(frame: pd.DataFrame, pair: str, timeframe: str) -> None:
    """打印结果摘要。"""

    valid = frame.dropna(subset=["next_open", "next_close"]).copy()
    up_signals = valid[valid["effective_long"]].copy()
    down_signals = valid[valid["effective_short"]].copy()

    up_hits = int(up_signals["next_is_up"].sum())
    down_hits = int(down_signals["next_is_down"].sum())
    total_signals = len(up_signals) + len(down_signals)
    total_hits = up_hits + down_hits

    print()
    print("信号方向评估")
    print(f"交易对: {pair}")
    print(f"周期: {timeframe}")
    print(f"样本区间: {valid['date'].iloc[0]} -> {valid['date'].iloc[-1]}")
    print()
    print("总体")
    print(f"总信号数: {total_signals}")
    print(f"总命中数: {total_hits}")
    print(f"总体命中率: {hit_ratio(total_hits, total_signals):.2f}%")
    print()
    print("背景过滤")
    print(
        "UP allow/reduce/block/missing: "
        f"{int((valid['background_action_long'] == 'allow').sum())} / "
        f"{int((valid['background_action_long'] == 'reduce').sum())} / "
        f"{int((valid['background_action_long'] == 'block').sum())} / "
        f"{int((valid['background_action_long'] == 'missing').sum())}"
    )
    print(
        "DOWN allow/reduce/block/missing: "
        f"{int((valid['background_action_short'] == 'allow').sum())} / "
        f"{int((valid['background_action_short'] == 'reduce').sum())} / "
        f"{int((valid['background_action_short'] == 'block').sum())} / "
        f"{int((valid['background_action_short'] == 'missing').sum())}"
    )
    print()
    print("UP 信号")
    print(f"信号数: {len(up_signals)}")
    print(f"下一根上涨命中数: {up_hits}")
    print(f"命中率: {hit_ratio(up_hits, len(up_signals)):.2f}%")
    print()
    print("DOWN 信号")
    print(f"信号数: {len(down_signals)}")
    print(f"下一根下跌命中数: {down_hits}")
    print(f"命中率: {hit_ratio(down_hits, len(down_signals)):.2f}%")
    print()

    up_high = up_signals[up_signals["effective_size_factor_long"] >= 2.0]
    down_high = down_signals[down_signals["effective_size_factor_short"] >= 2.0]

    print("UP 高分组(size=2.0)")
    print(f"信号数: {len(up_high)}")
    print(f"命中率: {hit_ratio(int(up_high['next_is_up'].sum()), len(up_high)):.2f}%")
    if len(up_high) > 0:
        print(f"平均 score: {up_high['score_long'].mean():.4f}")
    print()

    print("DOWN 高分组(size=2.0)")
    print(f"信号数: {len(down_high)}")
    print(f"命中率: {hit_ratio(int(down_high['next_is_down'].sum()), len(down_high)):.2f}%")
    if len(down_high) > 0:
        print(f"平均 score: {down_high['score_short'].mean():.4f}")
    print()


def main() -> int:
    """脚本入口。

    执行顺序固定为：

    1. 可选下载数据
    2. 读取本地 K 线
    3. 计算信号
    4. 输出下一根柱子方向命中率
    """

    args = build_cli_parser().parse_args()

    freqtrade_root = freqtrade_root_dir()
    project_root = freqtrade_root.parent.parent
    config_path = freqtrade_root / "config.json"
    user_dir = freqtrade_root / "user_data"
    data_dir = user_dir / "data"

    if not args.skip_download:
        ensure_data_downloaded(
            project_root=project_root,
            config_path=config_path,
            user_dir=user_dir,
            data_dir=data_dir,
            pair=args.pair,
            timeframe=args.timeframe,
            timerange=args.timerange,
        )

    frame = load_main_timeframe_frame(data_dir, args.pair, args.timeframe)
    signal_frame = compute_signal_frame(frame)
    signal_frame = apply_background_filter(signal_frame, args.timeframe)
    print_summary(signal_frame, args.pair, args.timeframe)
    return 0


if __name__ == "__main__":
    sys.exit(main())
