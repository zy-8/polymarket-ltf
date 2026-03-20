from __future__ import annotations

import csv
import json
import re
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from decimal import Decimal
from pathlib import Path
from typing import Any

from domain.models import BacktestResult, EquityPoint, Trade
from engine.engine import BacktestConfig

SUMMARY_SCALE = Decimal("0.0001")


@dataclass(frozen=True)
class StrategyDescriptor:
    name: str
    parameters: dict[str, str]


@dataclass(frozen=True)
class RunArtifact:
    csv_path: Path
    rows: int
    symbol: str | None
    interval: str | None
    market_slug: str | None
    result: BacktestResult


@dataclass(frozen=True)
class GroupSummary:
    label: str
    run_count: int
    total_rows: int
    total_trades: int
    avg_rows_per_run: Decimal
    avg_trades_per_run: Decimal
    avg_return_pct: Decimal
    avg_max_drawdown_pct: Decimal
    avg_ending_equity: Decimal
    best_return_pct: Decimal
    worst_return_pct: Decimal


@dataclass(frozen=True)
class BatchReport:
    generated_at: datetime
    strategy: StrategyDescriptor
    config: BacktestConfig
    runs: list[RunArtifact]
    group_summaries: list[GroupSummary]
    overall_summary: GroupSummary


def build_run_artifact(
    csv_path: Path,
    rows: int,
    result: BacktestResult,
    data_root: Path | None = None,
) -> RunArtifact:
    symbol, interval, market_slug = infer_run_scope(csv_path, data_root)
    return RunArtifact(
        csv_path=csv_path,
        rows=rows,
        symbol=symbol,
        interval=interval,
        market_slug=market_slug,
        result=result,
    )


def build_group_summary(label: str, runs: list[RunArtifact]) -> GroupSummary:
    run_count = len(runs)
    total_rows = sum(run.rows for run in runs)
    total_trades = sum(run.result.trade_count for run in runs)

    if not run_count:
        return GroupSummary(
            label=label,
            run_count=0,
            total_rows=0,
            total_trades=0,
            avg_rows_per_run=Decimal("0.00"),
            avg_trades_per_run=Decimal("0.00"),
            avg_return_pct=Decimal("0.0000"),
            avg_max_drawdown_pct=Decimal("0.0000"),
            avg_ending_equity=Decimal("0.0000"),
            best_return_pct=Decimal("0.0000"),
            worst_return_pct=Decimal("0.0000"),
        )

    returns = [run.result.total_return_pct for run in runs]
    drawdowns = [run.result.max_drawdown_pct for run in runs]
    ending_equities = [run.result.ending_equity for run in runs]

    return GroupSummary(
        label=label,
        run_count=run_count,
        total_rows=total_rows,
        total_trades=total_trades,
        avg_rows_per_run=(Decimal(total_rows) / Decimal(run_count)).quantize(
            Decimal("0.01")
        ),
        avg_trades_per_run=(Decimal(total_trades) / Decimal(run_count)).quantize(
            Decimal("0.01")
        ),
        avg_return_pct=(sum(returns) / Decimal(run_count)).quantize(SUMMARY_SCALE),
        avg_max_drawdown_pct=(sum(drawdowns) / Decimal(run_count)).quantize(
            SUMMARY_SCALE
        ),
        avg_ending_equity=(sum(ending_equities) / Decimal(run_count)).quantize(
            SUMMARY_SCALE
        ),
        best_return_pct=max(returns).quantize(SUMMARY_SCALE),
        worst_return_pct=min(returns).quantize(SUMMARY_SCALE),
    )


def build_batch_report(
    runs: list[RunArtifact],
    strategy: StrategyDescriptor,
    config: BacktestConfig,
) -> BatchReport:
    grouped: dict[str, list[RunArtifact]] = {}
    for run in runs:
        label = run.interval or "unknown"
        grouped.setdefault(label, []).append(run)

    group_summaries = [
        build_group_summary(label, grouped[label]) for label in sorted(grouped)
    ]

    return BatchReport(
        generated_at=datetime.now(tz=timezone.utc),
        strategy=strategy,
        config=config,
        runs=runs,
        group_summaries=group_summaries,
        overall_summary=build_group_summary("all", runs),
    )


def format_group_summary(summary: GroupSummary) -> str:
    return (
        "汇总 分组={label} 文件数={runs} 总行数={rows} 总成交数={trades} "
        "平均每文件行数={avg_rows} 平均每文件成交数={avg_trades} "
        "平均收益率={avg_return}% 平均最大回撤={avg_drawdown}% "
        "平均期末权益={avg_ending_equity} 最佳收益率={best_return}% 最差收益率={worst_return}%".format(
            label=summary.label,
            runs=summary.run_count,
            rows=summary.total_rows,
            trades=summary.total_trades,
            avg_rows=summary.avg_rows_per_run,
            avg_trades=summary.avg_trades_per_run,
            avg_return=summary.avg_return_pct,
            avg_drawdown=summary.avg_max_drawdown_pct,
            avg_ending_equity=summary.avg_ending_equity,
            best_return=summary.best_return_pct,
            worst_return=summary.worst_return_pct,
        )
    )


def write_batch_report(output_dir: Path, report: BatchReport) -> dict[str, Path]:
    output_dir.mkdir(parents=True, exist_ok=True)
    trades_dir = output_dir / "trades"
    equity_dir = output_dir / "equity"
    trades_dir.mkdir(exist_ok=True)
    equity_dir.mkdir(exist_ok=True)

    report_json_path = output_dir / "report.json"
    run_summary_csv_path = output_dir / "run_summaries.csv"
    group_summary_csv_path = output_dir / "group_summaries.csv"

    report_json_path.write_text(
        json.dumps(batch_report_to_dict(report), ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    _write_run_summary_csv(run_summary_csv_path, report.runs)
    _write_group_summary_csv(group_summary_csv_path, report.group_summaries + [report.overall_summary])

    for index, run in enumerate(report.runs, start=1):
        run_label = run_file_label(run, index)
        _write_trade_csv(trades_dir / f"{run_label}.csv", run.result.trades)
        _write_equity_csv(equity_dir / f"{run_label}.csv", run.result.equity_curve)

    return {
        "report_json": report_json_path,
        "run_summaries_csv": run_summary_csv_path,
        "group_summaries_csv": group_summary_csv_path,
        "trades_dir": trades_dir,
        "equity_dir": equity_dir,
    }


def batch_report_to_dict(report: BatchReport) -> dict[str, Any]:
    return {
        "generated_at": report.generated_at.isoformat(),
        "strategy": {
            "name": report.strategy.name,
            "parameters": report.strategy.parameters,
        },
        "config": {
            "starting_cash": str(report.config.starting_cash),
            "fee_bps": str(report.config.fee_bps),
            "max_position": str(report.config.max_position),
        },
        "group_summaries": [group_summary_to_dict(summary) for summary in report.group_summaries],
        "overall_summary": group_summary_to_dict(report.overall_summary),
        "runs": [run_artifact_to_dict(run) for run in report.runs],
    }


def load_batch_report(path: Path) -> BatchReport:
    payload = json.loads(path.read_text(encoding="utf-8"))
    return batch_report_from_dict(payload)


def batch_report_from_dict(payload: dict[str, Any]) -> BatchReport:
    return BatchReport(
        generated_at=datetime.fromisoformat(payload["generated_at"]),
        strategy=StrategyDescriptor(
            name=payload["strategy"]["name"],
            parameters=dict(payload["strategy"]["parameters"]),
        ),
        config=BacktestConfig(
            starting_cash=Decimal(payload["config"]["starting_cash"]),
            fee_bps=Decimal(payload["config"]["fee_bps"]),
            max_position=Decimal(payload["config"]["max_position"]),
        ),
        runs=[run_artifact_from_dict(run) for run in payload["runs"]],
        group_summaries=[
            group_summary_from_dict(summary) for summary in payload["group_summaries"]
        ],
        overall_summary=group_summary_from_dict(payload["overall_summary"]),
    )


def group_summary_to_dict(summary: GroupSummary) -> dict[str, Any]:
    payload = asdict(summary)
    for key, value in payload.items():
        if isinstance(value, Decimal):
            payload[key] = str(value)
    return payload


def group_summary_from_dict(payload: dict[str, Any]) -> GroupSummary:
    return GroupSummary(
        label=payload["label"],
        run_count=int(payload["run_count"]),
        total_rows=int(payload["total_rows"]),
        total_trades=int(payload["total_trades"]),
        avg_rows_per_run=Decimal(payload["avg_rows_per_run"]),
        avg_trades_per_run=Decimal(payload["avg_trades_per_run"]),
        avg_return_pct=Decimal(payload["avg_return_pct"]),
        avg_max_drawdown_pct=Decimal(payload["avg_max_drawdown_pct"]),
        avg_ending_equity=Decimal(payload["avg_ending_equity"]),
        best_return_pct=Decimal(payload["best_return_pct"]),
        worst_return_pct=Decimal(payload["worst_return_pct"]),
    )


def run_artifact_to_dict(run: RunArtifact) -> dict[str, Any]:
    return {
        "csv_path": str(run.csv_path),
        "rows": run.rows,
        "symbol": run.symbol,
        "interval": run.interval,
        "market_slug": run.market_slug,
        "result": {
            "starting_cash": str(run.result.starting_cash),
            "ending_cash": str(run.result.ending_cash),
            "ending_equity": str(run.result.ending_equity),
            "total_return_pct": str(run.result.total_return_pct),
            "max_drawdown_pct": str(run.result.max_drawdown_pct),
            "trade_count": run.result.trade_count,
            "trades": [trade_to_dict(trade) for trade in run.result.trades],
            "equity_curve": [equity_point_to_dict(point) for point in run.result.equity_curve],
        },
    }


def run_artifact_from_dict(payload: dict[str, Any]) -> RunArtifact:
    return RunArtifact(
        csv_path=Path(payload["csv_path"]),
        rows=int(payload["rows"]),
        symbol=payload.get("symbol"),
        interval=payload.get("interval"),
        market_slug=payload.get("market_slug"),
        result=backtest_result_from_dict(payload["result"]),
    )


def backtest_result_from_dict(payload: dict[str, Any]) -> BacktestResult:
    return BacktestResult(
        starting_cash=Decimal(payload["starting_cash"]),
        ending_cash=Decimal(payload["ending_cash"]),
        ending_equity=Decimal(payload["ending_equity"]),
        total_return_pct=Decimal(payload["total_return_pct"]),
        max_drawdown_pct=Decimal(payload["max_drawdown_pct"]),
        trade_count=int(payload["trade_count"]),
        trades=[trade_from_dict(item) for item in payload["trades"]],
        equity_curve=[equity_point_from_dict(item) for item in payload["equity_curve"]],
    )


def trade_to_dict(trade: Trade) -> dict[str, Any]:
    return {
        "timestamp": trade.timestamp.isoformat(),
        "side": trade.side,
        "asset": trade.asset,
        "quantity": str(trade.quantity),
        "price": str(trade.price),
        "fee": str(trade.fee),
        "up_position_after": str(trade.up_position_after),
        "down_position_after": str(trade.down_position_after),
        "cash_after": str(trade.cash_after),
    }


def trade_from_dict(payload: dict[str, Any]) -> Trade:
    return Trade(
        timestamp=datetime.fromisoformat(payload["timestamp"]),
        side=payload["side"],
        asset=payload["asset"],
        quantity=Decimal(payload["quantity"]),
        price=Decimal(payload["price"]),
        fee=Decimal(payload["fee"]),
        up_position_after=Decimal(payload["up_position_after"]),
        down_position_after=Decimal(payload["down_position_after"]),
        cash_after=Decimal(payload["cash_after"]),
    )


def equity_point_to_dict(point: EquityPoint) -> dict[str, Any]:
    return {
        "timestamp": point.timestamp.isoformat(),
        "equity": str(point.equity),
        "cash": str(point.cash),
        "up_position": str(point.up_position),
        "down_position": str(point.down_position),
        "up_mark_price": str(point.up_mark_price),
        "down_mark_price": str(point.down_mark_price),
    }


def equity_point_from_dict(payload: dict[str, Any]) -> EquityPoint:
    return EquityPoint(
        timestamp=datetime.fromisoformat(payload["timestamp"]),
        equity=Decimal(payload["equity"]),
        cash=Decimal(payload["cash"]),
        up_position=Decimal(payload["up_position"]),
        down_position=Decimal(payload["down_position"]),
        up_mark_price=Decimal(payload["up_mark_price"]),
        down_mark_price=Decimal(payload["down_mark_price"]),
    )


def infer_run_scope(csv_path: Path, data_root: Path | None) -> tuple[str | None, str | None, str | None]:
    if data_root is not None:
        try:
            relative = csv_path.resolve().relative_to(data_root.resolve())
        except ValueError:
            relative = None
        if relative is not None and len(relative.parts) >= 3:
            return relative.parts[0], relative.parts[1], csv_path.stem

    parts = csv_path.parts
    if len(parts) >= 3:
        return parts[-3], parts[-2], csv_path.stem
    return None, None, csv_path.stem


def run_file_label(run: RunArtifact, index: int) -> str:
    base = run.market_slug or run.csv_path.stem or f"run-{index}"
    return sanitize_filename(f"{index:03d}-{base}")


def sanitize_filename(value: str) -> str:
    sanitized = re.sub(r"[^A-Za-z0-9._-]+", "_", value).strip("._")
    return sanitized or "run"


def _write_run_summary_csv(path: Path, runs: list[RunArtifact]) -> None:
    with path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(
            [
                "csv_path",
                "symbol",
                "interval",
                "market_slug",
                "rows",
                "trade_count",
                "ending_cash",
                "ending_equity",
                "total_return_pct",
                "max_drawdown_pct",
            ]
        )
        for run in runs:
            writer.writerow(
                [
                    str(run.csv_path),
                    run.symbol or "",
                    run.interval or "",
                    run.market_slug or "",
                    run.rows,
                    run.result.trade_count,
                    run.result.ending_cash,
                    run.result.ending_equity,
                    run.result.total_return_pct,
                    run.result.max_drawdown_pct,
                ]
            )


def _write_group_summary_csv(path: Path, summaries: list[GroupSummary]) -> None:
    with path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(
            [
                "label",
                "run_count",
                "total_rows",
                "total_trades",
                "avg_rows_per_run",
                "avg_trades_per_run",
                "avg_return_pct",
                "avg_max_drawdown_pct",
                "avg_ending_equity",
                "best_return_pct",
                "worst_return_pct",
            ]
        )
        for summary in summaries:
            writer.writerow(
                [
                    summary.label,
                    summary.run_count,
                    summary.total_rows,
                    summary.total_trades,
                    summary.avg_rows_per_run,
                    summary.avg_trades_per_run,
                    summary.avg_return_pct,
                    summary.avg_max_drawdown_pct,
                    summary.avg_ending_equity,
                    summary.best_return_pct,
                    summary.worst_return_pct,
                ]
            )


def _write_trade_csv(path: Path, trades: list[Trade]) -> None:
    with path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(
            [
                "timestamp",
                "side",
                "asset",
                "quantity",
                "price",
                "fee",
                "up_position_after",
                "down_position_after",
                "cash_after",
            ]
        )
        for trade in trades:
            writer.writerow(
                [
                    trade.timestamp.isoformat(),
                    trade.side,
                    trade.asset,
                    trade.quantity,
                    trade.price,
                    trade.fee,
                    trade.up_position_after,
                    trade.down_position_after,
                    trade.cash_after,
                ]
            )


def _write_equity_csv(path: Path, equity_curve: list[EquityPoint]) -> None:
    with path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(
            [
                "timestamp",
                "equity",
                "cash",
                "up_position",
                "down_position",
                "up_mark_price",
                "down_mark_price",
            ]
        )
        for point in equity_curve:
            writer.writerow(
                [
                    point.timestamp.isoformat(),
                    point.equity,
                    point.cash,
                    point.up_position,
                    point.down_position,
                    point.up_mark_price,
                    point.down_mark_price,
                ]
            )
