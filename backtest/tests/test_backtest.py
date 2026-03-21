from __future__ import annotations

import sys
import tempfile
import textwrap
import unittest
from datetime import datetime
from decimal import Decimal
from pathlib import Path
from types import SimpleNamespace

PACKAGE_ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = PACKAGE_ROOT / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from backtest import (
    default_snapshots_root,
    list_snapshot_csvs,
    load_rows_for_args,
    maybe_write_reports,
    resolve_csv_paths,
)
from data.snapshot_csv import load_snapshot_csv
from domain.models import Position, SnapshotRow
from engine.engine import BacktestConfig, BacktestEngine
from report import write_html_report
from reports.reporting import (
    StrategyDescriptor,
    build_batch_report,
    build_group_summary,
    build_run_artifact,
    format_group_summary,
)
from scan import (
    run_parameter_scan,
    scan_results_to_dataframe,
    write_scan_outputs,
)
from strategies.mean_reversion import MeanReversionZScoreStrategy
from strategies.registry import parse_decimal_grid, parse_int_grid, strategy_scan_variants_from_args


CSV_FIXTURE = """\
timestamp,binance_mid_price,chainlink_price,spread_binance_chainlink,spread_delta,chainlink_start_delta,up_bid_price,up_bid_size,up_ask_price,up_ask_size,down_bid_price,down_bid_size,down_ask_price,down_ask_size,z_score,vel_spread,up_mid_price_slope,binance_sigma,chainlink_change_30s_pct,chainlink_change_60s_pct,chainlink_run
2026-03-20 10:00:00.000,85000,84990,10,1,0,0.48,100,0.52,100,0.47,100,0.53,100,-1.8,0.1,0.0,12,0.1,0.2,1
2026-03-20 10:00:01.000,85001,84991,10,0,1,0.54,100,0.56,100,0.44,100,0.46,100,0.1,0.0,0.0,12,0.1,0.2,1
2026-03-20 10:00:02.000,85002,84992,10,0,2,0.58,100,0.60,100,0.40,100,0.42,100,1.9,0.0,0.0,12,0.1,0.2,1
"""


class BacktestTests(unittest.TestCase):
    def test_format_group_summary_formats_interval_summary(self) -> None:
        runs = [
            build_run_artifact(
                csv_path=Path("data/snapshots/btc/5m/a.csv"),
                rows=10,
                result=type(
                    "CompatResult",
                    (),
                    {
                        "starting_cash": Decimal("0"),
                        "ending_cash": Decimal("0"),
                        "ending_equity": Decimal("1000"),
                        "total_return_pct": Decimal("1.0000"),
                        "max_drawdown_pct": Decimal("2.0000"),
                        "trade_count": 2,
                        "trades": [],
                        "equity_curve": [],
                    },
                )(),
                data_root=None,
            ),
            build_run_artifact(
                csv_path=Path("data/snapshots/btc/5m/b.csv"),
                rows=30,
                result=type(
                    "CompatResult",
                    (),
                    {
                        "starting_cash": Decimal("0"),
                        "ending_cash": Decimal("0"),
                        "ending_equity": Decimal("1010"),
                        "total_return_pct": Decimal("3.0000"),
                        "max_drawdown_pct": Decimal("4.0000"),
                        "trade_count": 4,
                        "trades": [],
                        "equity_curve": [],
                    },
                )(),
                data_root=None,
            ),
        ]

        line = format_group_summary(build_group_summary("5m", runs))

        self.assertEqual(
            "汇总 分组=5m 文件数=2 总行数=40 总成交数=6 平均每文件行数=20.00 平均每文件成交数=3.00 平均收益率=2.0000% 平均最大回撤=3.0000% 平均期末权益=1005.0000 最佳收益率=3.0000% 最差收益率=1.0000%",
            line,
        )

    def test_list_snapshot_csvs_returns_all_csvs(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            root = Path(tmp_dir)
            first = root / "btc" / "5m" / "a.csv"
            second = root / "eth" / "15m" / "b.csv"
            ignore = root / "notes.txt"
            first.parent.mkdir(parents=True, exist_ok=True)
            second.parent.mkdir(parents=True, exist_ok=True)
            first.write_text("a\n", encoding="utf-8")
            second.write_text("b\n", encoding="utf-8")
            ignore.write_text("c\n", encoding="utf-8")

            self.assertEqual([first, second], list_snapshot_csvs(root))

    def test_resolve_csv_paths_without_locator_uses_all_snapshots(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            root = Path(tmp_dir)
            first = root / "btc" / "5m" / "older.csv"
            second = root / "eth" / "15m" / "newer.csv"
            first.parent.mkdir(parents=True, exist_ok=True)
            second.parent.mkdir(parents=True, exist_ok=True)
            first.write_text("a\n", encoding="utf-8")
            second.write_text("b\n", encoding="utf-8")

            args = SimpleNamespace(
                csv=None,
                symbol=None,
                interval=None,
                market_slug=None,
                data_root=str(root),
            )
            self.assertEqual([first, second], resolve_csv_paths(args))

    def test_resolve_csv_paths_from_explicit_csv(self) -> None:
        args = SimpleNamespace(
            csv="tmp/sample.csv",
            symbol=None,
            interval=None,
            market_slug=None,
            data_root=None,
        )
        self.assertEqual([Path("tmp/sample.csv")], resolve_csv_paths(args))

    def test_resolve_csv_paths_from_project_layout(self) -> None:
        args = SimpleNamespace(
            csv=None,
            symbol="btc",
            interval="5m",
            market_slug="demo-market",
            data_root=None,
        )
        expected = default_snapshots_root() / "btc" / "5m" / "demo-market.csv"
        self.assertEqual([expected], resolve_csv_paths(args))

    def test_load_snapshot_csv_parses_rows(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            path = Path(tmp_dir) / "sample.csv"
            path.write_text(textwrap.dedent(CSV_FIXTURE), encoding="utf-8")
            rows = load_snapshot_csv(path)

        self.assertEqual(3, len(rows))
        self.assertEqual(Decimal("-1.8"), rows[0].z_score)
        self.assertEqual(1, rows[0].chainlink_run)

    def test_load_rows_for_args_raises_clean_error_on_missing_csv(self) -> None:
        args = SimpleNamespace(
            csv="missing.csv",
            symbol=None,
            interval=None,
            market_slug=None,
            data_root=None,
        )

        with self.assertRaises(SystemExit) as ctx:
            load_rows_for_args(args)

        self.assertIn("snapshot csv not found", str(ctx.exception))

    def test_mean_reversion_strategy_trades_and_marks_equity(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            path = Path(tmp_dir) / "sample.csv"
            path.write_text(textwrap.dedent(CSV_FIXTURE), encoding="utf-8")
            rows = load_snapshot_csv(path)

        engine = BacktestEngine(
            BacktestConfig(
                starting_cash=Decimal("100"),
                fee_bps=Decimal("0"),
                max_position=Decimal("1"),
            )
        )
        strategy = MeanReversionZScoreStrategy(
            size=Decimal("1"),
            entry_z=Decimal("1.5"),
            exit_z=Decimal("0.5"),
        )
        result = engine.run(rows, strategy)

        self.assertEqual(3, result.trade_count)
        self.assertEqual(Decimal("99.6000"), result.ending_cash)
        self.assertEqual(Decimal("100.0100"), result.ending_equity)
        self.assertEqual(Decimal("0.0100"), result.total_return_pct)

    def test_mean_reversion_strategy_switches_between_up_and_down(self) -> None:
        strategy = MeanReversionZScoreStrategy(size=Decimal("1"))

        up_signal = strategy.on_snapshot(
            SnapshotRow(
                timestamp=datetime(2026, 3, 20, 10, 0, 0),
                binance_mid_price=Decimal("0"),
                chainlink_price=Decimal("0"),
                spread_binance_chainlink=Decimal("0"),
                spread_delta=Decimal("0"),
                chainlink_start_delta=Decimal("0"),
                up_bid_price=Decimal("0"),
                up_bid_size=Decimal("0"),
                up_ask_price=Decimal("0"),
                up_ask_size=Decimal("0"),
                down_bid_price=Decimal("0"),
                down_bid_size=Decimal("0"),
                down_ask_price=Decimal("0"),
                down_ask_size=Decimal("0"),
                z_score=Decimal("-2"),
                vel_spread=Decimal("0"),
                up_mid_price_slope=Decimal("0"),
                binance_sigma=Decimal("0"),
                chainlink_change_30s_pct=Decimal("0"),
                chainlink_change_60s_pct=Decimal("0"),
                chainlink_run=0,
            ),
            Position(up_quantity=Decimal("0"), down_quantity=Decimal("0")),
        )
        self.assertEqual("up", up_signal.target_side)

        down_signal = strategy.on_snapshot(
            SnapshotRow(
                timestamp=datetime(2026, 3, 20, 10, 0, 1),
                binance_mid_price=Decimal("0"),
                chainlink_price=Decimal("0"),
                spread_binance_chainlink=Decimal("0"),
                spread_delta=Decimal("0"),
                chainlink_start_delta=Decimal("0"),
                up_bid_price=Decimal("0"),
                up_bid_size=Decimal("0"),
                up_ask_price=Decimal("0"),
                up_ask_size=Decimal("0"),
                down_bid_price=Decimal("0"),
                down_bid_size=Decimal("0"),
                down_ask_price=Decimal("0"),
                down_ask_size=Decimal("0"),
                z_score=Decimal("2"),
                vel_spread=Decimal("0"),
                up_mid_price_slope=Decimal("0"),
                binance_sigma=Decimal("0"),
                chainlink_change_30s_pct=Decimal("0"),
                chainlink_change_60s_pct=Decimal("0"),
                chainlink_run=0,
            ),
            Position(up_quantity=Decimal("0"), down_quantity=Decimal("0")),
        )
        self.assertEqual("down", down_signal.target_side)

    def test_polymarket_fee_model_matches_taker_buy_and_sell_accounting(self) -> None:
        row = SnapshotRow(
            timestamp=datetime(2026, 3, 20, 10, 0, 0),
            binance_mid_price=Decimal("0"),
            chainlink_price=Decimal("0"),
            spread_binance_chainlink=Decimal("0"),
            spread_delta=Decimal("0"),
            chainlink_start_delta=Decimal("0"),
            up_bid_price=Decimal("0.60"),
            up_bid_size=Decimal("100"),
            up_ask_price=Decimal("0.50"),
            up_ask_size=Decimal("100"),
            down_bid_price=Decimal("0.40"),
            down_bid_size=Decimal("100"),
            down_ask_price=Decimal("0.50"),
            down_ask_size=Decimal("100"),
            z_score=Decimal("0"),
            vel_spread=Decimal("0"),
            up_mid_price_slope=Decimal("0"),
            binance_sigma=Decimal("0"),
            chainlink_change_30s_pct=Decimal("0"),
            chainlink_change_60s_pct=Decimal("0"),
            chainlink_run=0,
        )
        engine = BacktestEngine(
            BacktestConfig(
                starting_cash=Decimal("100"),
                fee_bps=Decimal("25"),
                max_position=Decimal("10"),
            )
        )
        trades = []
        cash = Decimal("100")
        position = Position(up_quantity=Decimal("0"), down_quantity=Decimal("0"))

        cash, position = engine._buy_up(
            row=row,
            cash=cash,
            position=position,
            quantity=Decimal("10"),
            trades=trades,
        )
        self.assertEqual(Decimal("95.0000"), cash)
        self.assertEqual(Decimal("9.8438"), position.up_quantity)
        self.assertEqual(Decimal("0.0781"), trades[-1].fee)

        cash, position = engine._sell_up(
            row=row,
            cash=cash,
            position=position,
            quantity=position.up_quantity,
            trades=trades,
        )
        self.assertEqual(Decimal("100.82118"), cash)
        self.assertEqual(Decimal("0.0000"), position.up_quantity)
        self.assertEqual(Decimal("0.0851"), trades[-1].fee)

    def test_scan_outputs_use_pandas_dataframe(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            csv_path = Path(tmp_dir) / "sample.csv"
            csv_path.write_text(textwrap.dedent(CSV_FIXTURE), encoding="utf-8")
            rows = load_snapshot_csv(csv_path)
            scan_results = run_parameter_scan(
                rows_by_path={csv_path: rows},
                data_root=None,
                starting_cash=Decimal("100"),
                fee_bps=Decimal("0"),
                strategy_name="mean_reversion_zscore",
                strategy_variants=strategy_scan_variants_from_args(
                    SimpleNamespace(
                        strategy="mean_reversion_zscore",
                        entry_z="1.5",
                        exit_z="0.5",
                        size="1",
                        max_run="0",
                        scan_entry_z_values="1.5",
                        scan_exit_z_values="0.5",
                        scan_size_values="1",
                        scan_max_run_values="0",
                    )
                ),
            )
            frame = scan_results_to_dataframe(scan_results)
            output_dir = Path(tmp_dir) / "out"
            written = write_scan_outputs(output_dir, scan_results)
            scan_csv_text = written["scan_csv"].read_text(encoding="utf-8")
            scan_json_text = written["scan_json"].read_text(encoding="utf-8")

            self.assertEqual(1, len(frame))
            self.assertIn("avg_return_pct", frame.columns)
            self.assertEqual(output_dir / "scan_results.csv", written["scan_csv"])
            self.assertEqual(output_dir / "scan_results.json", written["scan_json"])
            self.assertIn("entry_z", scan_csv_text)
            self.assertIn("\"entry_z\": \"1.5\"", scan_json_text)

    def test_html_report_is_written(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            path = Path(tmp_dir) / "sample.csv"
            path.write_text(textwrap.dedent(CSV_FIXTURE), encoding="utf-8")
            rows = load_snapshot_csv(path)
            engine = BacktestEngine(
                BacktestConfig(
                    starting_cash=Decimal("100"),
                    fee_bps=Decimal("0"),
                    max_position=Decimal("1"),
                )
            )
            strategy = MeanReversionZScoreStrategy(
                size=Decimal("1"),
                entry_z=Decimal("1.5"),
                exit_z=Decimal("0.5"),
            )
            report = build_batch_report(
                runs=[
                    build_run_artifact(
                        csv_path=path,
                        rows=len(rows),
                        result=engine.run(rows, strategy),
                        data_root=None,
                    )
                ],
                strategy=StrategyDescriptor(
                    name="mean_reversion_zscore",
                    parameters={"size": "1"},
                ),
                config=engine.config,
            )
            html_path = Path(tmp_dir) / "report.html"
            written = write_html_report(report, html_path)
            html_text = html_path.read_text(encoding="utf-8")

            self.assertEqual(html_path, written)
            self.assertIn("Backtest Report", html_text)

    def test_maybe_write_reports_without_flags_is_noop(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            path = Path(tmp_dir) / "sample.csv"
            path.write_text(textwrap.dedent(CSV_FIXTURE), encoding="utf-8")
            rows = load_snapshot_csv(path)
            engine = BacktestEngine(
                BacktestConfig(
                    starting_cash=Decimal("100"),
                    fee_bps=Decimal("0"),
                    max_position=Decimal("1"),
                )
            )
            strategy = MeanReversionZScoreStrategy(size=Decimal("1"))
            report = build_batch_report(
                runs=[
                    build_run_artifact(
                        csv_path=path,
                        rows=len(rows),
                        result=engine.run(rows, strategy),
                        data_root=None,
                    )
                ],
                strategy=StrategyDescriptor(name="test", parameters={}),
                config=engine.config,
            )
            output_dir = Path(tmp_dir) / "reports"
            maybe_write_reports(report, output_dir, html_report=False, quantstats_report=False)

        self.assertFalse(output_dir.exists())


if __name__ == "__main__":
    unittest.main()
