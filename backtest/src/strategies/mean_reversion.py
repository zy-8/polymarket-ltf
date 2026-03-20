from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal

from domain.models import SnapshotRow
from domain.strategy import Signal, TARGET_DOWN, TARGET_FLAT, TARGET_UP


@dataclass
class MeanReversionZScoreStrategy:
    """基于 `z_score` 的最小均值回归示例策略。

    规则：

    - `z_score <= -entry_z` 时做多 `up`
    - `z_score >= entry_z` 时买入 `down`
    - `abs(z_score) <= exit_z` 时平仓
    - 如果设置了 `max_chainlink_run`，当连续单边变动次数过高时直接空仓观望
    """

    size: Decimal
    entry_z: Decimal = Decimal("1.5")
    exit_z: Decimal = Decimal("0.5")
    max_chainlink_run: int = 0

    def on_snapshot(self, row: SnapshotRow, current_position) -> Signal:
        """根据当前快照和已有持仓输出目标腿与目标数量。"""

        if self.max_chainlink_run > 0 and row.chainlink_run > self.max_chainlink_run:
            return Signal(target_side=TARGET_FLAT, target_quantity=Decimal("0"))

        if row.z_score <= -self.entry_z:
            return Signal(target_side=TARGET_UP, target_quantity=self.size)
        if row.z_score >= self.entry_z:
            return Signal(target_side=TARGET_DOWN, target_quantity=self.size)
        if abs(row.z_score) <= self.exit_z:
            return Signal(target_side=TARGET_FLAT, target_quantity=Decimal("0"))
        if current_position.up_quantity > 0:
            return Signal(target_side=TARGET_UP, target_quantity=current_position.up_quantity)
        if current_position.down_quantity > 0:
            return Signal(target_side=TARGET_DOWN, target_quantity=current_position.down_quantity)
        return Signal(target_side=TARGET_FLAT, target_quantity=Decimal("0"))
