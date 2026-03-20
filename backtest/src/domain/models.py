from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime
from decimal import Decimal


@dataclass(frozen=True)
class SnapshotRow:
    """一行 snapshot CSV 对应的标准化数据。

    这里保持字段名与 Rust 侧 CSV 列尽量一致，方便在策略里直观引用。
    """

    timestamp: datetime
    binance_mid_price: Decimal
    chainlink_price: Decimal
    spread_binance_chainlink: Decimal
    spread_delta: Decimal
    chainlink_start_delta: Decimal
    up_bid_price: Decimal
    up_bid_size: Decimal
    up_ask_price: Decimal
    up_ask_size: Decimal
    down_bid_price: Decimal
    down_bid_size: Decimal
    down_ask_price: Decimal
    down_ask_size: Decimal
    z_score: Decimal
    vel_spread: Decimal
    up_mid_price_slope: Decimal
    binance_sigma: Decimal
    chainlink_change_30s_pct: Decimal
    chainlink_change_60s_pct: Decimal
    chainlink_run: int

    @property
    def up_mid_price(self) -> Decimal:
        """返回 `up` 合约的估值中间价。

        优先使用 `(bid + ask) / 2`。
        如果盘口只剩单边，就退化为现存那一侧价格，避免估值直接变成 0。
        """

        if self.up_bid_price > 0 and self.up_ask_price > 0:
            return (self.up_bid_price + self.up_ask_price) / Decimal("2")
        if self.up_bid_price > 0:
            return self.up_bid_price
        return self.up_ask_price

    @property
    def down_mid_price(self) -> Decimal:
        """返回 `down` 合约的估值中间价。"""

        if self.down_bid_price > 0 and self.down_ask_price > 0:
            return (self.down_bid_price + self.down_ask_price) / Decimal("2")
        if self.down_bid_price > 0:
            return self.down_bid_price
        return self.down_ask_price


@dataclass(frozen=True)
class Position:
    """当前持仓。

    `up_quantity` 与 `down_quantity` 都是非负数量。
    当前回测引擎不支持裸做空，而是通过在 `up` 与 `down` 之间切换来表达方向。
    """

    up_quantity: Decimal
    down_quantity: Decimal


@dataclass(frozen=True)
class Trade:
    """一次实际成交后的快照。"""

    timestamp: datetime
    side: str
    asset: str
    quantity: Decimal
    price: Decimal
    fee: Decimal
    up_position_after: Decimal
    down_position_after: Decimal
    cash_after: Decimal


@dataclass(frozen=True)
class EquityPoint:
    """某个时间点的账户权益状态。"""

    timestamp: datetime
    equity: Decimal
    cash: Decimal
    up_position: Decimal
    down_position: Decimal
    up_mark_price: Decimal
    down_mark_price: Decimal


@dataclass(frozen=True)
class BacktestResult:
    """单次回测的完整结果。"""

    starting_cash: Decimal
    ending_cash: Decimal
    ending_equity: Decimal
    total_return_pct: Decimal
    max_drawdown_pct: Decimal
    trade_count: int
    trades: list[Trade]
    equity_curve: list[EquityPoint]
