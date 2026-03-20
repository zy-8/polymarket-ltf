from __future__ import annotations

from datetime import datetime
from decimal import Decimal
from pathlib import Path

import pandas as pd

from domain.models import SnapshotRow

TIMESTAMP_FORMAT = "%Y-%m-%d %H:%M:%S.%f"


def _decimal(raw: str) -> Decimal:
    """把 CSV 字段安全转换成 Decimal。"""

    return Decimal(raw or "0")


def load_snapshot_csv(path: str | Path) -> list[SnapshotRow]:
    """读取 Rust 侧生成的 snapshot CSV。

    这里直接使用 pandas 负责 CSV 解析，方便后续在 `data/` 层继续扩展
    parquet、批量统计和更高吞吐的数据读取路径。
    """

    csv_path = Path(path)
    frame = pd.read_csv(csv_path, dtype=str).fillna("")
    rows: list[SnapshotRow] = []
    for row in frame.to_dict(orient="records"):
        rows.append(
            SnapshotRow(
                timestamp=datetime.strptime(row["timestamp"], TIMESTAMP_FORMAT),
                binance_mid_price=_decimal(row["binance_mid_price"]),
                chainlink_price=_decimal(row["chainlink_price"]),
                spread_binance_chainlink=_decimal(row["spread_binance_chainlink"]),
                spread_delta=_decimal(row["spread_delta"]),
                chainlink_start_delta=_decimal(row["chainlink_start_delta"]),
                up_bid_price=_decimal(row["up_bid_price"]),
                up_bid_size=_decimal(row["up_bid_size"]),
                up_ask_price=_decimal(row["up_ask_price"]),
                up_ask_size=_decimal(row["up_ask_size"]),
                down_bid_price=_decimal(row["down_bid_price"]),
                down_bid_size=_decimal(row["down_bid_size"]),
                down_ask_price=_decimal(row["down_ask_price"]),
                down_ask_size=_decimal(row["down_ask_size"]),
                z_score=_decimal(row["z_score"]),
                vel_spread=_decimal(row["vel_spread"]),
                up_mid_price_slope=_decimal(row["up_mid_price_slope"]),
                binance_sigma=_decimal(row["binance_sigma"]),
                chainlink_change_30s_pct=_decimal(row.get("chainlink_change_30s_pct", "0")),
                chainlink_change_60s_pct=_decimal(row.get("chainlink_change_60s_pct", "0")),
                chainlink_run=int(row.get("chainlink_run", "0") or "0"),
            )
        )
    return rows
