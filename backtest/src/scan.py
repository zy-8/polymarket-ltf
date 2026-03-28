from __future__ import annotations

import argparse
import json
from dataclasses import dataclass
from decimal import Decimal
from pathlib import Path
from typing import TYPE_CHECKING, Any

from backtest import (
    add_execution_args,
    add_locator_args,
    add_output_args,
    load_rows_for_args,
)
from engine import BacktestConfig, BacktestEngine
from report import write_html_report, write_quantstats_reports
from reporting import (
    BatchReport,
    StrategyDescriptor,
    build_batch_report,
    build_run_artifact,
)
from strategy_lib import (
    strategy_scan_variants_from_args,
)

if TYPE_CHECKING:
    import pandas as pd


@dataclass(frozen=True)
class ScanResult:
    entry_z: Decimal
    exit_z: Decimal
    size: Decimal
    max_chainlink_run: int
    report: BatchReport


def run_parameter_scan(
    rows_by_path: dict[Path, list],
    data_root: Path | None,
    starting_cash: Decimal,
    fee_bps: Decimal,
    strategy_name: str,
    strategy_variants,
) -> list[ScanResult]:
    results: list[ScanResult] = []

    for variant in strategy_variants:
        config = BacktestConfig(
            starting_cash=starting_cash,
            fee_bps=fee_bps,
            max_position=Decimal(variant.parameters["size"]),
        )
        engine = BacktestEngine(config)
        runs = [
            build_run_artifact(
                csv_path=csv_path,
                rows=len(rows),
                result=engine.run(rows, variant.strategy),
                data_root=data_root,
            )
            for csv_path, rows in rows_by_path.items()
        ]
        report = build_batch_report(
            runs=runs,
            strategy=StrategyDescriptor(
                name=strategy_name,
                parameters=variant.parameters,
            ),
            config=config,
        )
        results.append(
            ScanResult(
                entry_z=Decimal(variant.parameters["entry_z"]),
                exit_z=Decimal(variant.parameters["exit_z"]),
                size=Decimal(variant.parameters["size"]),
                max_chainlink_run=int(variant.parameters["max_chainlink_run"]),
                report=report,
            )
        )

    return sorted(
        results,
        key=lambda item: (
            item.report.overall_summary.avg_return_pct,
            -item.report.overall_summary.avg_max_drawdown_pct,
        ),
        reverse=True,
    )


def scan_results_to_dataframe(scan_results: list[ScanResult]) -> "pd.DataFrame":
    import pandas

    records = []
    for result in scan_results:
        summary = result.report.overall_summary
        records.append(
            {
                "entry_z": float(result.entry_z),
                "exit_z": float(result.exit_z),
                "size": float(result.size),
                "max_chainlink_run": result.max_chainlink_run,
                "run_count": summary.run_count,
                "total_rows": summary.total_rows,
                "total_trades": summary.total_trades,
                "avg_rows_per_run": float(summary.avg_rows_per_run),
                "avg_trades_per_run": float(summary.avg_trades_per_run),
                "avg_return_pct": float(summary.avg_return_pct),
                "avg_max_drawdown_pct": float(summary.avg_max_drawdown_pct),
                "avg_ending_equity": float(summary.avg_ending_equity),
                "best_return_pct": float(summary.best_return_pct),
                "worst_return_pct": float(summary.worst_return_pct),
            }
        )

    frame = pandas.DataFrame.from_records(records)
    if not frame.empty:
        frame = frame.sort_values(
            by=["avg_return_pct", "avg_max_drawdown_pct", "total_trades"],
            ascending=[False, True, False],
        ).reset_index(drop=True)
    return frame


def scan_results_to_dict(scan_results: list[ScanResult]) -> list[dict[str, Any]]:
    payload = []
    for result in scan_results:
        summary = result.report.overall_summary
        payload.append(
            {
                "entry_z": str(result.entry_z),
                "exit_z": str(result.exit_z),
                "size": str(result.size),
                "max_chainlink_run": result.max_chainlink_run,
                "summary": {
                    "run_count": summary.run_count,
                    "total_rows": summary.total_rows,
                    "total_trades": summary.total_trades,
                    "avg_rows_per_run": str(summary.avg_rows_per_run),
                    "avg_trades_per_run": str(summary.avg_trades_per_run),
                    "avg_return_pct": str(summary.avg_return_pct),
                    "avg_max_drawdown_pct": str(summary.avg_max_drawdown_pct),
                    "avg_ending_equity": str(summary.avg_ending_equity),
                    "best_return_pct": str(summary.best_return_pct),
                    "worst_return_pct": str(summary.worst_return_pct),
                },
            }
        )
    return payload


def write_scan_outputs(output_dir: Path, scan_results: list[ScanResult]) -> dict[str, Path]:
    output_dir.mkdir(parents=True, exist_ok=True)
    frame = scan_results_to_dataframe(scan_results)
    csv_path = output_dir / "scan_results.csv"
    json_path = output_dir / "scan_results.json"
    frame.to_csv(csv_path, index=False)
    json_path.write_text(
        json.dumps(scan_results_to_dict(scan_results), ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    return {
        "scan_csv": csv_path,
        "scan_json": json_path,
    }


def maybe_write_reports(
    report: BatchReport,
    output_dir: Path | None,
    html_report: bool,
    quantstats_report: bool,
    scan_frame=None,
) -> None:
    if output_dir is None:
        return

    if html_report:
        html_path = write_html_report(
            report,
            output_dir / "report.html",
            scan_frame=scan_frame,
        )
        print(f"html_report={html_path}")

    if quantstats_report:
        quantstats_paths = write_quantstats_reports(report, output_dir / "quantstats")
        for path in quantstats_paths:
            print(f"quantstats_report={path}")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="scan",
        description="Run parameter scans for Polymarket crypto backtests",
    )
    add_locator_args(parser)
    add_execution_args(parser, include_scan_args=True)
    add_output_args(parser)
    parser.add_argument(
        "--top-k",
        default="10",
        help="How many top parameter sets to print",
    )
    return parser


def main() -> None:
    args = build_parser().parse_args()
    output_dir = Path(args.output_dir) if args.output_dir else None
    data_root, rows_by_path = load_rows_for_args(args)
    strategy_variants = strategy_scan_variants_from_args(args)

    scan_results = run_parameter_scan(
        rows_by_path=rows_by_path,
        data_root=data_root,
        starting_cash=Decimal(args.starting_cash),
        fee_bps=Decimal(args.fee_bps),
        strategy_name=args.strategy,
        strategy_variants=strategy_variants,
    )
    scan_frame = scan_results_to_dataframe(scan_results)
    top_k = max(1, int(args.top_k))

    print("参数扫描：")
    print(scan_frame.head(top_k).to_string(index=False))

    if output_dir is not None:
        written = write_scan_outputs(output_dir, scan_results)
        print("扫描结果已写出：")
        for label, path in written.items():
            print(f"{label}={path}")

        if scan_results:
            maybe_write_reports(
                scan_results[0].report,
                output_dir,
                args.html_report,
                args.quantstats_report,
                scan_frame=scan_frame,
            )


if __name__ == "__main__":
    main()
