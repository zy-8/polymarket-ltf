from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal

from domain.models import BacktestResult, EquityPoint, Position, SnapshotRow, Trade
from domain.strategy import TARGET_DOWN, TARGET_FLAT, TARGET_UP, Strategy

RESULT_SCALE = Decimal("0.0001")


@dataclass(frozen=True)
class BacktestConfig:
    """回测配置。

    `starting_cash`：初始现金。
    `fee_bps`：Polymarket fee rate，单位为基点。
    `max_position`：允许持有的最大绝对仓位。
    """

    starting_cash: Decimal = Decimal("1000")
    fee_bps: Decimal = Decimal("0")
    max_position: Decimal = Decimal("1")


class BacktestEngine:
    """最小可用的 Polymarket 双腿回测引擎。

    当前假设：

    - `up` 买入使用 `up_ask_price`，卖出使用 `up_bid_price`
    - `down` 买入使用 `down_ask_price`，卖出使用 `down_bid_price`
    - 持仓估值使用 `up_mid_price` / `down_mid_price`
    - 策略给出的是“目标腿 + 目标数量”，不是直接买卖动作
    - 回测成交默认按 taker 口径记账
    - 买单手续费按 Polymarket 公式先算成 USDC，再折成 shares 扣减持仓
    - 卖单手续费按 Polymarket 公式直接从 USDC 收益中扣减
    """

    FEE_SCALE = Decimal("0.0001")
    CRYPTO_EXPONENT = 2

    def __init__(self, config: BacktestConfig) -> None:
        self.config = config

    def run(self, rows: list[SnapshotRow], strategy: Strategy) -> BacktestResult:
        """顺序回放 snapshot 数据并返回回测结果。"""

        cash = self.config.starting_cash
        position = Position(up_quantity=Decimal("0"), down_quantity=Decimal("0"))
        trades: list[Trade] = []
        equity_curve: list[EquityPoint] = []

        for row in rows:
            signal = strategy.on_snapshot(row, position)
            target_quantity = max(
                Decimal("0"),
                min(self.config.max_position, signal.target_quantity),
            )
            cash, position = self._rebalance(
                row=row,
                cash=cash,
                position=position,
                target_side=signal.target_side,
                target_quantity=target_quantity,
                trades=trades,
            )

            equity_curve.append(
                EquityPoint(
                    timestamp=row.timestamp,
                    # 权益 = 现金 + up 市值 + down 市值。
                    equity=(
                        cash
                        + (position.up_quantity * row.up_mid_price)
                        + (position.down_quantity * row.down_mid_price)
                    ),
                    cash=cash,
                    up_position=position.up_quantity,
                    down_position=position.down_quantity,
                    up_mark_price=row.up_mid_price,
                    down_mark_price=row.down_mid_price,
                )
            )

        ending_equity = equity_curve[-1].equity if equity_curve else cash
        total_return_pct = Decimal("0")
        if self.config.starting_cash > 0:
            total_return_pct = ((ending_equity - self.config.starting_cash) / self.config.starting_cash) * Decimal("100")

        return BacktestResult(
            starting_cash=self.config.starting_cash,
            ending_cash=cash.quantize(RESULT_SCALE),
            ending_equity=ending_equity.quantize(RESULT_SCALE),
            total_return_pct=total_return_pct.quantize(RESULT_SCALE),
            max_drawdown_pct=self._max_drawdown_pct(equity_curve),
            trade_count=len(trades),
            trades=trades,
            equity_curve=equity_curve,
        )

    def _rebalance(
        self,
        row: SnapshotRow,
        cash: Decimal,
        position: Position,
        target_side: str,
        target_quantity: Decimal,
        trades: list[Trade],
    ) -> tuple[Decimal, Position]:
        """把当前持仓调到目标腿。"""

        if target_side not in {TARGET_UP, TARGET_DOWN, TARGET_FLAT}:
            return cash, position

        if position.up_quantity > 0 and target_side != TARGET_UP:
            cash, position = self._sell_up(row, cash, position, position.up_quantity, trades)

        if position.down_quantity > 0 and target_side != TARGET_DOWN:
            cash, position = self._sell_down(row, cash, position, position.down_quantity, trades)

        if target_side == TARGET_UP and target_quantity > position.up_quantity:
            cash, position = self._buy_up(
                row, cash, position, target_quantity - position.up_quantity, trades
            )
        elif target_side == TARGET_DOWN and target_quantity > position.down_quantity:
            cash, position = self._buy_down(
                row, cash, position, target_quantity - position.down_quantity, trades
            )

        return cash, position

    def _buy_up(
        self,
        row: SnapshotRow,
        cash: Decimal,
        position: Position,
        quantity: Decimal,
        trades: list[Trade],
    ) -> tuple[Decimal, Position]:
        """按卖一价买入 `up`。"""

        if row.up_ask_price <= 0 or quantity <= 0:
            return cash, position

        gross = row.up_ask_price * quantity
        fee = self._buy_fee(quantity, row.up_ask_price)
        net_quantity = quantity - fee.fee_shares
        next_cash = cash - gross
        next_position = Position(
            up_quantity=position.up_quantity + net_quantity,
            down_quantity=position.down_quantity,
        )
        trades.append(
            Trade(
                timestamp=row.timestamp,
                side="buy",
                asset="up",
                quantity=net_quantity,
                price=row.up_ask_price,
                fee=fee.fee_usdc,
                up_position_after=next_position.up_quantity,
                down_position_after=next_position.down_quantity,
                cash_after=next_cash,
            )
        )
        return next_cash, next_position

    def _sell_up(
        self,
        row: SnapshotRow,
        cash: Decimal,
        position: Position,
        quantity: Decimal,
        trades: list[Trade],
    ) -> tuple[Decimal, Position]:
        """按买一价卖出 `up`。"""

        if row.up_bid_price <= 0 or quantity <= 0:
            return cash, position

        gross = row.up_bid_price * quantity
        fee = self._sell_fee(quantity, row.up_bid_price)
        next_cash = cash + gross - fee.fee_usdc
        next_position = Position(
            up_quantity=position.up_quantity - quantity,
            down_quantity=position.down_quantity,
        )
        trades.append(
            Trade(
                timestamp=row.timestamp,
                side="sell",
                asset="up",
                quantity=quantity,
                price=row.up_bid_price,
                fee=fee.fee_usdc,
                up_position_after=next_position.up_quantity,
                down_position_after=next_position.down_quantity,
                cash_after=next_cash,
            )
        )
        return next_cash, next_position

    def _buy_down(
        self,
        row: SnapshotRow,
        cash: Decimal,
        position: Position,
        quantity: Decimal,
        trades: list[Trade],
    ) -> tuple[Decimal, Position]:
        """按卖一价买入 `down`。"""

        if row.down_ask_price <= 0 or quantity <= 0:
            return cash, position

        gross = row.down_ask_price * quantity
        fee = self._buy_fee(quantity, row.down_ask_price)
        net_quantity = quantity - fee.fee_shares
        next_cash = cash - gross
        next_position = Position(
            up_quantity=position.up_quantity,
            down_quantity=position.down_quantity + net_quantity,
        )
        trades.append(
            Trade(
                timestamp=row.timestamp,
                side="buy",
                asset="down",
                quantity=net_quantity,
                price=row.down_ask_price,
                fee=fee.fee_usdc,
                up_position_after=next_position.up_quantity,
                down_position_after=next_position.down_quantity,
                cash_after=next_cash,
            )
        )
        return next_cash, next_position

    def _sell_down(
        self,
        row: SnapshotRow,
        cash: Decimal,
        position: Position,
        quantity: Decimal,
        trades: list[Trade],
    ) -> tuple[Decimal, Position]:
        """按买一价卖出 `down`。"""

        if row.down_bid_price <= 0 or quantity <= 0:
            return cash, position

        gross = row.down_bid_price * quantity
        fee = self._sell_fee(quantity, row.down_bid_price)
        next_cash = cash + gross - fee.fee_usdc
        next_position = Position(
            up_quantity=position.up_quantity,
            down_quantity=position.down_quantity - quantity,
        )
        trades.append(
            Trade(
                timestamp=row.timestamp,
                side="sell",
                asset="down",
                quantity=quantity,
                price=row.down_bid_price,
                fee=fee.fee_usdc,
                up_position_after=next_position.up_quantity,
                down_position_after=next_position.down_quantity,
                cash_after=next_cash,
            )
        )
        return next_cash, next_position

    @dataclass(frozen=True)
    class _TradeFee:
        fee_usdc: Decimal
        fee_shares: Decimal

    def _buy_fee(self, quantity: Decimal, price: Decimal) -> _TradeFee:
        fee_usdc = self._taker_fee_usdc(quantity, price)
        fee_shares = Decimal("0")
        if fee_usdc > 0 and price > 0:
            fee_shares = (fee_usdc / price).quantize(self.FEE_SCALE)
        return self._TradeFee(fee_usdc=fee_usdc, fee_shares=fee_shares)

    def _sell_fee(self, quantity: Decimal, price: Decimal) -> _TradeFee:
        return self._TradeFee(
            fee_usdc=self._taker_fee_usdc(quantity, price),
            fee_shares=Decimal("0"),
        )

    def _taker_fee_usdc(self, quantity: Decimal, price: Decimal) -> Decimal:
        """按 Polymarket crypto fee 公式计算 taker fee。"""

        if quantity <= 0 or price <= 0 or self.config.fee_bps <= 0:
            return Decimal("0")

        fee_rate = self.config.fee_bps / Decimal("100")
        factor = price * (Decimal("1") - price)
        fee = quantity * price * fee_rate * (factor**self.CRYPTO_EXPONENT)
        return fee.quantize(self.FEE_SCALE)

    def _max_drawdown_pct(self, equity_curve: list[EquityPoint]) -> Decimal:
        """计算权益曲线的最大回撤百分比。"""

        peak: Decimal | None = None
        max_drawdown = Decimal("0")
        for point in equity_curve:
            if peak is None or point.equity > peak:
                peak = point.equity
                continue
            if peak > 0:
                drawdown = ((peak - point.equity) / peak) * Decimal("100")
                if drawdown > max_drawdown:
                    max_drawdown = drawdown
        return max_drawdown.quantize(RESULT_SCALE)
