from __future__ import annotations

import argparse
import html
import json
from pathlib import Path
from typing import TYPE_CHECKING

from reports.reporting import BatchReport, load_batch_report, run_file_label

if TYPE_CHECKING:
    import pandas as pd


def write_html_report(
    report: BatchReport,
    output_path: Path,
    scan_frame: "pd.DataFrame | None" = None,
) -> Path:
    output_path.parent.mkdir(parents=True, exist_ok=True)

    run_rows = "\n".join(
        (
            "<tr>"
            f"<td>{html.escape(run.market_slug or run.csv_path.stem)}</td>"
            f"<td>{html.escape(run.interval or '')}</td>"
            f"<td>{run.rows}</td>"
            f"<td>{run.result.trade_count}</td>"
            f"<td>{run.result.ending_equity}</td>"
            f"<td>{run.result.total_return_pct}%</td>"
            f"<td>{run.result.max_drawdown_pct}%</td>"
            "</tr>"
        )
        for run in report.runs
    )
    group_rows = "\n".join(
        (
            "<tr>"
            f"<td>{html.escape(summary.label)}</td>"
            f"<td>{summary.run_count}</td>"
            f"<td>{summary.total_rows}</td>"
            f"<td>{summary.total_trades}</td>"
            f"<td>{summary.avg_return_pct}%</td>"
            f"<td>{summary.avg_max_drawdown_pct}%</td>"
            f"<td>{summary.avg_ending_equity}</td>"
            "</tr>"
        )
        for summary in report.group_summaries + [report.overall_summary]
    )

    scan_table = ""
    if scan_frame is not None and not scan_frame.empty:
        scan_table = (
            "<h2>参数扫描</h2>\n"
            + scan_frame.head(20).to_html(index=False, border=0, classes="scan-table")
        )

    html_text = f"""<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <title>Backtest Report</title>
  <style>
    body {{
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      margin: 24px;
      background: #f6f4ef;
      color: #1f2933;
    }}
    h1, h2 {{ margin-bottom: 12px; }}
    .meta {{
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
      gap: 12px;
      margin-bottom: 24px;
    }}
    .card {{
      background: white;
      border: 1px solid #d8dee9;
      border-radius: 10px;
      padding: 14px 16px;
    }}
    table {{
      width: 100%;
      border-collapse: collapse;
      margin-bottom: 24px;
      background: white;
      border-radius: 10px;
      overflow: hidden;
    }}
    th, td {{
      padding: 10px 12px;
      border-bottom: 1px solid #e5e9f0;
      text-align: left;
      font-size: 14px;
    }}
    th {{
      background: #2f4858;
      color: white;
    }}
    tr:last-child td {{ border-bottom: none; }}
    code {{
      font-family: "SFMono-Regular", Consolas, monospace;
      font-size: 13px;
    }}
  </style>
</head>
<body>
  <h1>Backtest Report</h1>
  <div class="meta">
    <div class="card"><strong>生成时间</strong><br><code>{html.escape(report.generated_at.isoformat())}</code></div>
    <div class="card"><strong>策略</strong><br>{html.escape(report.strategy.name)}</div>
    <div class="card"><strong>参数</strong><br><code>{html.escape(json.dumps(report.strategy.parameters, ensure_ascii=False))}</code></div>
    <div class="card"><strong>总体平均收益率</strong><br>{report.overall_summary.avg_return_pct}%</div>
    <div class="card"><strong>总体平均最大回撤</strong><br>{report.overall_summary.avg_max_drawdown_pct}%</div>
    <div class="card"><strong>总体平均期末权益</strong><br>{report.overall_summary.avg_ending_equity}</div>
  </div>

  <h2>分组汇总</h2>
  <table>
    <thead>
      <tr>
        <th>分组</th>
        <th>文件数</th>
        <th>总行数</th>
        <th>总成交数</th>
        <th>平均收益率</th>
        <th>平均最大回撤</th>
        <th>平均期末权益</th>
      </tr>
    </thead>
    <tbody>
      {group_rows}
    </tbody>
  </table>

  <h2>运行明细</h2>
  <table>
    <thead>
      <tr>
        <th>market</th>
        <th>interval</th>
        <th>rows</th>
        <th>trades</th>
        <th>ending_equity</th>
        <th>return</th>
        <th>max_drawdown</th>
      </tr>
    </thead>
    <tbody>
      {run_rows}
    </tbody>
  </table>
  {scan_table}
</body>
</html>
"""

    output_path.write_text(html_text, encoding="utf-8")
    return output_path


def write_quantstats_reports(report: BatchReport, output_dir: Path) -> list[Path]:
    import pandas

    try:
        import quantstats as qs
    except ImportError as exc:
        raise RuntimeError(
            "未安装 quantstats，无法生成 quantstats 报表。请安装 `quantstats` extra。"
        ) from exc

    output_dir.mkdir(parents=True, exist_ok=True)
    output_paths: list[Path] = []
    for index, run in enumerate(report.runs, start=1):
        frame = pandas.DataFrame(
            {
                "timestamp": [point.timestamp for point in run.result.equity_curve],
                "equity": [float(point.equity) for point in run.result.equity_curve],
            }
        )
        if frame.empty:
            continue

        frame["timestamp"] = pandas.to_datetime(frame["timestamp"], utc=False)
        frame = (
            frame.drop_duplicates(subset=["timestamp"])
            .set_index("timestamp")
            .sort_index()
        )
        returns = frame["equity"].pct_change().fillna(0.0)
        output_path = output_dir / f"{run_file_label(run, index)}.html"
        qs.reports.html(
            returns,
            output=str(output_path),
            title=f"QuantStats {run.market_slug or run.csv_path.stem}",
        )
        output_paths.append(output_path)

    return output_paths


def maybe_write_reports(
    report: BatchReport,
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
        prog="report",
        description="Render reports from an existing structured backtest report.json",
    )
    parser.add_argument(
        "--report-json",
        required=True,
        help="Path to a report.json previously generated by the backtest command",
    )
    parser.add_argument(
        "--output-dir",
        default=None,
        help="Optional output directory for regenerated reports, defaults to report.json parent",
    )
    parser.add_argument(
        "--html-report",
        action="store_true",
        help="Write an HTML report",
    )
    parser.add_argument(
        "--quantstats-report",
        action="store_true",
        help="Write per-run quantstats HTML reports",
    )
    return parser


def main() -> None:
    args = build_parser().parse_args()
    report_json = Path(args.report_json)
    if not report_json.exists():
        raise SystemExit(f"report json not found: {report_json}")

    report = load_batch_report(report_json)
    output_dir = Path(args.output_dir) if args.output_dir else report_json.parent
    maybe_write_reports(report, output_dir, args.html_report, args.quantstats_report)

    print("报告输入：")
    print(f"report_json={report_json}")
    print(f"output_dir={output_dir}")


if __name__ == "__main__":
    main()
