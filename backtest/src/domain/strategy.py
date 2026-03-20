from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal
from typing import Protocol

from .models import Position, SnapshotRow

TARGET_UP = "up"
TARGET_DOWN = "down"
TARGET_FLAT = "flat"


@dataclass(frozen=True)
class Signal:
    """策略输出。

    `target_side` 表示目标持仓方向：

    - `up`：持有 `up`
    - `down`：持有 `down`
    - `flat`：空仓

    `target_quantity` 表示目标腿想持有的数量。
    """

    target_side: str
    target_quantity: Decimal


class Strategy(Protocol):
    """策略协议。

    任何策略只要实现 `on_snapshot`，就可以被回测引擎消费。
    这样可以把“信号生成”与“成交模拟”明确分开。
    """

    def on_snapshot(self, row: SnapshotRow, current_position: Position) -> Signal:
        ...
