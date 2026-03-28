#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

import pandas as pd


SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from strategies.crypto_reversal import CryptoReversal


DEFAULT_TIMERANGE = "1774677600-"

BACKGROUND_LOOKBACK_15M = 8
BACKGROUND_LOOKBACK_1H = 6
BACKGROUND_BLOCK_15M_PCT = 0.006
BACKGROUND_BLOCK_1H_PCT = 0.010
BACKGROUND_REDUCE_15M_PCT = 0.003
BACKGROUND_REDUCE_1H_PCT = 0.005
BACKGROUND_REDUCE_FACTOR = 0.5
BACKGROUND_BLOCK_1H_PCT_15M = 0.008
BACKGROUND_BLOCK_4H_PCT_15M = 0.012
BACKGROUND_REDUCE_1H_PCT_15M = 0.004
BACKGROUND_REDUCE_4H_PCT_15M = 0.006
BACKGROUND_REDUCE_FACTOR_15M = 0.75


@dataclass(frozen=True)
class BackgroundProfile:
    fast_timeframe: str
    slow_timeframe: str
    fast_lookback: int
    slow_lookback: int
    block_fast_pct: float
    block_slow_pct: float
    reduce_fast_pct: float
    reduce_slow_pct: float
    reduce_factor: float


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="拉取现货 OHLCV 并统计 CryptoReversal 信号表现。"
    )
    parser.add_argument(
        "--config",
        type=Path,
        default=SCRIPT_DIR / "config.json",
        help="Freqtrade 配置文件路径。",
    )
    parser.add_argument(
        "--userdir",
        type=Path,
        default=SCRIPT_DIR / "user_data",
        help="Freqtrade user_data 路径。",
    )
    parser.add_argument(
        "--datadir",
        type=Path,
        default=SCRIPT_DIR / "user_data" / "data",
        help="Freqtrade OHLCV 数据目录。",
    )
    parser.add_argument(
        "--pair",
        help="要统计的交易对。默认取 config.json 里的第一个 pair_whitelist。",
    )
    parser.add_argument(
        "--timeframe",
        help="OHLCV 周期。默认取 config.json 里的 timeframe。",
    )
    parser.add_argument(
        "--exchange",
        help="交易所名。默认取 config.json 里的 exchange.name。",
    )
    parser.add_argument(
        "--skip-download",
        action="store_true",
        help="跳过 freqtrade download-data，只统计本地已有 OHLCV。",
    )
    parser.add_argument(
        "--export-csv",
        type=Path,
        help="可选。导出逐笔信号明细 CSV 的路径。",
    )
    return parser.parse_args()


def load_config(config_path: Path) -> dict:
    return json.loads(config_path.read_text())


def parse_timerange_value(raw: str) -> pd.Timestamp | None:
    raw = raw.strip()
    if not raw:
        return None
    if raw.isdigit():
        if len(raw) == 8:
            return pd.Timestamp.strptime(raw, "%Y%m%d").tz_localize("UTC")
        if len(raw) >= 13:
            return pd.to_datetime(int(raw), unit="ms", utc=True)
        return pd.to_datetime(int(raw), unit="s", utc=True)
    return pd.to_datetime(raw, utc=True)


def parse_timerange(timerange: str | None) -> tuple[pd.Timestamp | None, pd.Timestamp | None]:
    if not timerange:
        return None, None
    if "-" not in timerange:
        raise ValueError(f"Unsupported timerange: {timerange}")
    start_raw, end_raw = timerange.split("-", 1)
    return parse_timerange_value(start_raw), parse_timerange_value(end_raw)


def normalize_pair_filename(pair: str) -> str:
    return pair.replace("/", "_").replace(":", "_")


def resolve_market_settings(args: argparse.Namespace, config: dict) -> tuple[str, str, str]:
    pair = args.pair or config["exchange"]["pair_whitelist"][0]
    timeframe = args.timeframe or config["timeframe"]
    exchange = args.exchange or config["exchange"]["name"]
    return pair, timeframe, exchange


def download_spot_data(
    config_path: Path,
    userdir: Path,
    datadir: Path,
    exchange: str,
    pair: str,
    timeframes: list[str],
    timerange: str | None,
) -> None:
    command = [
        "freqtrade",
        "download-data",
        "--config",
        str(config_path),
        "--userdir",
        str(userdir),
        "--datadir",
        str(datadir),
        "--exchange",
        exchange,
        "--trading-mode",
        "spot",
        "--timeframes",
        *timeframes,
        "-p",
        pair,
    ]
    if timerange:
        command.extend(["--timerange", timerange])

    subprocess.run(command, check=True)


def resolve_ohlcv_file(datadir: Path, pair: str, timeframe: str) -> Path:
    base = f"{normalize_pair_filename(pair)}-{timeframe}"
    candidates = [
        datadir / f"{base}.feather",
        datadir / f"{base}.parquet",
        datadir / f"{base}.json",
        datadir / f"{base}.json.gz",
    ]

    for candidate in candidates:
        if candidate.exists():
            return candidate

    raise FileNotFoundError(f"OHLCV file not found for {pair} {timeframe} under {datadir}")


def timeframe_to_minutes(timeframe: str) -> int:
    unit = timeframe[-1]
    value = int(timeframe[:-1])
    if unit == "m":
        return value
    if unit == "h":
        return value * 60
    raise ValueError(f"Unsupported timeframe: {timeframe}")


def timeframe_to_timedelta(timeframe: str) -> pd.Timedelta:
    return pd.Timedelta(minutes=timeframe_to_minutes(timeframe))


def timeframe_to_pandas_freq(timeframe: str) -> str:
    unit = timeframe[-1]
    value = int(timeframe[:-1])
    if unit == "m":
        return f"{value}min"
    if unit == "h":
        return f"{value}h"
    raise ValueError(f"Unsupported timeframe: {timeframe}")


def background_profile(timeframe: str) -> BackgroundProfile | None:
    if timeframe == "5m":
        return BackgroundProfile(
            fast_timeframe="15m",
            slow_timeframe="1h",
            fast_lookback=BACKGROUND_LOOKBACK_15M,
            slow_lookback=BACKGROUND_LOOKBACK_1H,
            block_fast_pct=BACKGROUND_BLOCK_15M_PCT,
            block_slow_pct=BACKGROUND_BLOCK_1H_PCT,
            reduce_fast_pct=BACKGROUND_REDUCE_15M_PCT,
            reduce_slow_pct=BACKGROUND_REDUCE_1H_PCT,
            reduce_factor=BACKGROUND_REDUCE_FACTOR,
        )
    if timeframe == "15m":
        return BackgroundProfile(
            fast_timeframe="1h",
            slow_timeframe="4h",
            fast_lookback=BACKGROUND_LOOKBACK_15M,
            slow_lookback=BACKGROUND_LOOKBACK_1H,
            block_fast_pct=BACKGROUND_BLOCK_1H_PCT_15M,
            block_slow_pct=BACKGROUND_BLOCK_4H_PCT_15M,
            reduce_fast_pct=BACKGROUND_REDUCE_1H_PCT_15M,
            reduce_slow_pct=BACKGROUND_REDUCE_4H_PCT_15M,
            reduce_factor=BACKGROUND_REDUCE_FACTOR_15M,
        )
    return None


def required_timeframes(main_timeframe: str) -> list[str]:
    profile = background_profile(main_timeframe)
    if profile is None:
        return [main_timeframe]

    ordered = [main_timeframe, profile.fast_timeframe, profile.slow_timeframe]
    deduped: list[str] = []
    for timeframe in ordered:
        if timeframe not in deduped:
            deduped.append(timeframe)
    return deduped


def load_ohlcv(path: Path) -> pd.DataFrame:
    if path.suffix == ".feather":
        dataframe = pd.read_feather(path)
    elif path.suffix == ".parquet":
        dataframe = pd.read_parquet(path)
    elif path.suffix == ".json":
        dataframe = pd.read_json(path)
    elif path.name.endswith(".json.gz"):
        dataframe = pd.read_json(path, compression="gzip")
    else:
        raise ValueError(f"Unsupported OHLCV format: {path}")

    dataframe["date"] = pd.to_datetime(dataframe["date"], utc=True)
    return dataframe.sort_values("date").reset_index(drop=True)


def filter_dataframe_by_timerange(
    dataframe: pd.DataFrame,
    start: pd.Timestamp | None,
    end: pd.Timestamp | None,
) -> pd.DataFrame:
    filtered = dataframe.copy()
    if start is not None:
        filtered = filtered[filtered["date"] >= start]
    if end is not None:
        filtered = filtered[filtered["date"] <= end]
    return filtered.reset_index(drop=True)


def add_close_time(dataframe: pd.DataFrame, timeframe: str) -> pd.DataFrame:
    enriched = dataframe.copy()
    enriched["close_time"] = enriched["date"] + timeframe_to_timedelta(timeframe)
    enriched.attrs["timeframe"] = timeframe
    return enriched


def resample_ohlcv(dataframe: pd.DataFrame, source_timeframe: str, target_timeframe: str) -> pd.DataFrame:
    source_minutes = timeframe_to_minutes(source_timeframe)
    target_minutes = timeframe_to_minutes(target_timeframe)
    if target_minutes % source_minutes != 0:
        raise ValueError(f"Cannot resample {source_timeframe} to {target_timeframe}")

    bars_per_target = target_minutes // source_minutes
    target_freq = timeframe_to_pandas_freq(target_timeframe)
    indexed = dataframe.set_index("date")
    resampled = indexed.resample(target_freq, label="left", closed="left").agg(
        {
            "open": "first",
            "high": "max",
            "low": "min",
            "close": "last",
            "volume": "sum",
        }
    )
    counts = indexed["close"].resample(target_freq, label="left", closed="left").count()
    resampled["bar_count"] = counts
    resampled = resampled[resampled["bar_count"] == bars_per_target].drop(columns=["bar_count"])
    resampled = resampled.dropna(subset=["open", "high", "low", "close"]).reset_index()
    return resampled


def load_timeframe_data(datadir: Path, pair: str, timeframe: str, base_data: pd.DataFrame | None = None) -> pd.DataFrame:
    try:
        dataframe = load_ohlcv(resolve_ohlcv_file(datadir, pair, timeframe))
    except FileNotFoundError:
        if base_data is None:
            raise
        base_minutes = timeframe_to_minutes(base_data.attrs["timeframe"])
        target_minutes = timeframe_to_minutes(timeframe)
        if target_minutes <= base_minutes:
            raise
        dataframe = resample_ohlcv(base_data, base_data.attrs["timeframe"], timeframe)

    return add_close_time(dataframe, timeframe)


def percent_change(first: float, last: float) -> float:
    if first == 0.0:
        return 0.0
    return (last - first) / first


def evaluate_background_action(
    signal_close_time: pd.Timestamp,
    profile: BackgroundProfile | None,
    background_fast: pd.DataFrame | None,
    background_slow: pd.DataFrame | None,
) -> str | None:
    if profile is None:
        return "allow"
    if background_fast is None or background_slow is None:
        return None

    fast = background_fast[background_fast["close_time"] <= signal_close_time].tail(profile.fast_lookback)
    slow = background_slow[background_slow["close_time"] <= signal_close_time].tail(profile.slow_lookback)

    if len(fast) < profile.fast_lookback or len(slow) < profile.slow_lookback:
        return None

    change_fast = percent_change(float(fast.iloc[0]["close"]), float(fast.iloc[-1]["close"]))
    change_slow = percent_change(float(slow.iloc[0]["close"]), float(slow.iloc[-1]["close"]))

    if change_fast <= -profile.block_fast_pct and change_slow <= -profile.block_slow_pct:
        return "block"
    if change_fast <= -profile.reduce_fast_pct or change_slow <= -profile.reduce_slow_pct:
        return "reduce"
    return "allow"


def evaluate_signals(
    dataframe: pd.DataFrame,
    timeframe: str,
    background_fast: pd.DataFrame | None = None,
    background_slow: pd.DataFrame | None = None,
    eval_start: pd.Timestamp | None = None,
    eval_end: pd.Timestamp | None = None,
) -> tuple[pd.DataFrame, dict]:
    strategy = CryptoReversal({})
    enriched = strategy.populate_indicators(dataframe.copy(), {})
    enriched = strategy.populate_entry_trend(enriched, {})

    signal_rows: list[dict] = []
    startup_candle_count = int(strategy.startup_candle_count)
    signal_series = enriched.get("enter_long", pd.Series(dtype=float)).fillna(0)
    raw_signal_count = int((signal_series == 1).sum())
    warmup_signal_count = int((signal_series.iloc[:startup_candle_count] == 1).sum())
    dropped_tail_signals = 0
    blocked_signal_count = 0
    background_missing_count = 0
    trigger_count = 0
    profile = background_profile(timeframe)
    main_close_delta = timeframe_to_timedelta(timeframe)

    for index in range(len(enriched)):
        row = enriched.iloc[index]
        if row.get("enter_long") != 1:
            continue
        if eval_start is not None and row["date"] < eval_start:
            continue
        if eval_end is not None and row["date"] > eval_end:
            continue

        signal_close_time = row["date"] + main_close_delta
        action = evaluate_background_action(
            signal_close_time=signal_close_time,
            profile=profile,
            background_fast=background_fast,
            background_slow=background_slow,
        )
        if action is None:
            background_missing_count += 1
            continue
        if action == "block":
            blocked_signal_count += 1
            continue

        trigger_count += 1
        if index + 1 >= len(enriched):
            dropped_tail_signals += 1
            continue

        exit_row = enriched.iloc[index + 1]
        entry_price = float(row["close"])
        exit_price = float(exit_row["close"])
        pnl_pct = ((exit_price / entry_price) - 1.0) * 100.0

        if pnl_pct > 0:
            outcome = "win"
        elif pnl_pct < 0:
            outcome = "loss"
        else:
            outcome = "flat"

        signal_rows.append(
            {
                "signal_date": row["date"],
                "exit_date": exit_row["date"],
                "entry_price": entry_price,
                "exit_price": exit_price,
                "pnl_pct": pnl_pct,
                "outcome": outcome,
                "rsi": float(row["rsi"]),
                "bb_lower": float(row["bb_lower"]),
                "bb_upper": float(row["bb_upper"]),
                "bb_width_pct": float(row["bb_width_pct"]),
                "score_long": float(row["score_long"]),
                "size_factor_long": float(row["size_factor_long"]),
                "background_action": action,
                "enter_tag": row.get("enter_tag"),
            }
        )

    signals = pd.DataFrame(signal_rows)

    wins = int((signals["outcome"] == "win").sum()) if not signals.empty else 0
    losses = int((signals["outcome"] == "loss").sum()) if not signals.empty else 0
    flats = int((signals["outcome"] == "flat").sum()) if not signals.empty else 0
    completed_signals = len(signals)
    win_rate = (wins / completed_signals) * 100.0 if completed_signals else 0.0
    avg_pnl_pct = float(signals["pnl_pct"].mean()) if not signals.empty else 0.0

    summary = {
        "startup_candle_count": startup_candle_count,
        "raw_signal_count": raw_signal_count,
        "warmup_signal_count": warmup_signal_count,
        "total_signal_count": trigger_count,
        "blocked_signal_count": blocked_signal_count,
        "background_missing_count": background_missing_count,
        "completed_signals": completed_signals,
        "dropped_tail_signals": dropped_tail_signals,
        "wins": wins,
        "losses": losses,
        "flats": flats,
        "win_rate_pct": win_rate,
        "avg_pnl_pct": avg_pnl_pct,
    }
    return signals, summary


def print_row(label: str, value: object) -> None:
    print(f"{label:<16}: {value}")


def print_summary(
    pair: str,
    timeframe: str,
    data_path: Path,
    dataframe: pd.DataFrame,
    summary: dict,
) -> None:
    print("=== CryptoReversal 现货信号统计 ===")
    print_row("交易对", pair)
    print_row("周期", timeframe)
    print_row("数据文件", data_path)
    if not dataframe.empty:
        print_row("统计开始", dataframe.iloc[0]["date"])
        print_row("统计结束", dataframe.iloc[-1]["date"])
    print_row("K线数量", len(dataframe))
    print_row("预热K线", summary["startup_candle_count"])
    print_row("总触发次数", summary["total_signal_count"])
    print_row("胜次数", summary["wins"])
    print_row("负次数", summary["losses"])
    print_row("胜率", f"{summary['win_rate_pct']:.2f}%")


def main() -> int:
    args = parse_args()
    config = load_config(args.config)
    pair, timeframe, exchange = resolve_market_settings(args, config)
    timeframes = required_timeframes(timeframe)
    eval_start, eval_end = parse_timerange(DEFAULT_TIMERANGE)

    if not args.skip_download:
        download_spot_data(
            config_path=args.config,
            userdir=args.userdir,
            datadir=args.datadir,
            exchange=exchange,
            pair=pair,
            timeframes=timeframes,
            timerange=DEFAULT_TIMERANGE,
        )

    data_path = resolve_ohlcv_file(args.datadir, pair, timeframe)
    dataframe = load_timeframe_data(args.datadir, pair, timeframe)
    profile = background_profile(timeframe)
    background_fast = None
    background_slow = None
    if profile is not None:
        background_fast = load_timeframe_data(args.datadir, pair, profile.fast_timeframe, dataframe)
        background_slow = load_timeframe_data(args.datadir, pair, profile.slow_timeframe, dataframe)
    signals, summary = evaluate_signals(
        dataframe=dataframe,
        timeframe=timeframe,
        background_fast=background_fast,
        background_slow=background_slow,
        eval_start=eval_start,
        eval_end=eval_end,
    )

    display_dataframe = filter_dataframe_by_timerange(dataframe, eval_start, eval_end)
    print_summary(pair, timeframe, data_path, display_dataframe, summary)

    if args.export_csv:
        args.export_csv.parent.mkdir(parents=True, exist_ok=True)
        signals.to_csv(args.export_csv, index=False)
        print_row("导出明细 CSV", args.export_csv)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
