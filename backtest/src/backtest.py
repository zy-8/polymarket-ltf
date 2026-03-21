from __future__ import annotations

import argparse
from decimal import Decimal
from pathlib import Path

from data.registry import add_data_format_arg, load_rows_by_paths
from engine.engine import BacktestConfig, BacktestEngine
from report import write_html_report, write_quantstats_reports
from reports.reporting import (
    StrategyDescriptor,
    build_batch_report,
    build_run_artifact,
    format_group_summary,
    write_batch_report,
)
from strategies.registry import (
    add_strategy_arg,
    add_strategy_parameter_args,
    build_strategy_from_args,
    strategy_descriptor_from_args,
)


def default_snapshots_root() -> Path:
    return Path(__file__).resolve().parents[2] / "data" / "snapshots"


def add_locator_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--csv", help="Path to a snapshot CSV file")
    parser.add_argument("--symbol", help="Symbol slug, for example: btc")
    parser.add_argument("--interval", help="Interval slug, for example: 5m")
    parser.add_argument("--market-slug", help="Market slug used in data/snapshots")
    parser.add_argument(
        "--data-root",
        default=None,
        help="Optional snapshots root directory, defaults to <repo>/data/snapshots",
    )
    add_data_format_arg(parser)


def add_execution_args(
    parser: argparse.ArgumentParser,
    include_scan_args: bool = False,
) -> None:
    parser.add_argument("--starting-cash", default="1000", help="Initial cash balance")
    parser.add_argument(
        "--fee-bps",
        default="0",
        help="Polymarket fee rate in bps, applied with the taker fee formula",
    )
    add_strategy_arg(parser)
    add_strategy_parameter_args(parser, include_scan_args=include_scan_args)


def add_output_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--output-dir",
        default=None,
        help="Optional output directory for structured results and reports",
    )
    parser.add_argument(
        "--html-report",
        action="store_true",
        help="Write an HTML report into --output-dir",
    )
    parser.add_argument(
        "--quantstats-report",
        action="store_true",
        help="Write per-run quantstats HTML reports into --output-dir/quantstats",
    )


def list_snapshot_csvs(data_root: Path) -> list[Path]:
    return sorted(path for path in data_root.rglob("*.csv") if path.is_file())


def resolve_csv_paths(args: argparse.Namespace) -> list[Path]:
    if args.csv:
        return [Path(args.csv)]

    data_root = Path(args.data_root) if args.data_root else default_snapshots_root()

    if not args.symbol and not args.interval and not args.market_slug:
        csv_paths = list_snapshot_csvs(data_root)
        if not csv_paths:
            raise SystemExit(f"no snapshot csv found under: {data_root}")
        return csv_paths

    if args.market_slug and (not args.symbol or not args.interval):
        raise SystemExit("using --market-slug also requires --symbol and --interval")

    if args.symbol and args.interval and args.market_slug:
        return [data_root / args.symbol / args.interval / f"{args.market_slug}.csv"]

    if args.symbol and args.interval:
        csv_paths = list_snapshot_csvs(data_root / args.symbol / args.interval)
        if not csv_paths:
            raise SystemExit(
                f"no snapshot csv found under: {data_root / args.symbol / args.interval}"
            )
        return csv_paths

    if args.interval and not args.symbol:
        csv_paths = list_snapshot_csvs(data_root)
        filtered = [path for path in csv_paths if path.parent.name == args.interval]
        if not filtered:
            raise SystemExit(f"no snapshot csv found for interval: {args.interval}")
        return filtered

    if args.symbol and not args.interval:
        csv_paths = list_snapshot_csvs(data_root / args.symbol)
        if not csv_paths:
            raise SystemExit(f"no snapshot csv found under: {data_root / args.symbol}")
        return csv_paths

    raise SystemExit("unsupported argument combination")


def load_rows_for_args(args: argparse.Namespace) -> tuple[Path | None, dict[Path, list]]:
    data_root = Path(args.data_root) if args.data_root else None
    csv_paths = resolve_csv_paths(args)
    try:
        rows_by_path = load_rows_by_paths(csv_paths, getattr(args, "data_format", "snapshot_csv"))
    except FileNotFoundError as exc:
        raise SystemExit(f"snapshot csv not found: {exc.filename}") from exc
    return data_root, rows_by_path


def maybe_write_reports(
    report,
    output_dir: Path | None,
    html_report: bool,
    quantstats_report: bool,
) -> None:
    if output_dir is None:
        return

    if html_report:
        html_path = write_html_report(report, output_dir / "report.html")
        print(f"html_report={html_path}")

    if quantstats_report:
        quantstats_paths = write_quantstats_reports(report, output_dir / "quantstats")
        for path in quantstats_paths:
            print(f"quantstats_report={path}")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="backtest",
        description="Run Polymarket crypto backtests against snapshot CSV data",
    )
    add_locator_args(parser)
    add_execution_args(parser)
    add_output_args(parser)
    return parser


def main() -> None:
    args = build_parser().parse_args()
    output_dir = Path(args.output_dir) if args.output_dir else None
    data_root, rows_by_path = load_rows_for_args(args)

    config = BacktestConfig(
        starting_cash=Decimal(args.starting_cash),
        fee_bps=Decimal(args.fee_bps),
        max_position=Decimal(args.size),
    )
    engine = BacktestEngine(config)
    strategy = build_strategy_from_args(args)
    strategy_name, strategy_parameters = strategy_descriptor_from_args(args)

    runs = [
        build_run_artifact(
            csv_path=csv_path,
            rows=len(rows),
            result=engine.run(rows, strategy),
            data_root=data_root,
        )
        for csv_path, rows in rows_by_path.items()
    ]
    report = build_batch_report(
        runs=runs,
        strategy=StrategyDescriptor(
            name=strategy_name,
            parameters=strategy_parameters,
        ),
        config=config,
    )

    print("回测明细：")
    for run in report.runs:
        print(
            "文件={csv} | 行数={rows} | 成交数={trades} | 期末现金={ending_cash} "
            "| 期末权益={ending_equity} | 收益率={total_return_pct}% | 最大回撤={max_drawdown_pct}%".format(
                csv=run.csv_path,
                rows=run.rows,
                trades=run.result.trade_count,
                ending_cash=run.result.ending_cash,
                ending_equity=run.result.ending_equity,
                total_return_pct=run.result.total_return_pct,
                max_drawdown_pct=run.result.max_drawdown_pct,
            )
        )

    if not args.csv and not args.symbol and not args.interval and not args.market_slug:
        print("分组汇总：")
        for summary in report.group_summaries:
            print(format_group_summary(summary))

    print("总体汇总：")
    print(format_group_summary(report.overall_summary))

    if output_dir is not None:
        written = write_batch_report(output_dir, report)
        print("结构化输出：")
        for label, path in written.items():
            print(f"{label}={path}")
    maybe_write_reports(report, output_dir, args.html_report, args.quantstats_report)


if __name__ == "__main__":
    main()
