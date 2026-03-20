from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from domain.models import SnapshotRow

from .snapshot_csv import load_snapshot_csv


@dataclass(frozen=True)
class DataFormat:
    name: str
    description: str
    load_rows: Callable[[str | Path], list[SnapshotRow]]


DATA_FORMATS: dict[str, DataFormat] = {
    "snapshot_csv": DataFormat(
        name="snapshot_csv",
        description="Rust snapshot CSV 标准格式",
        load_rows=load_snapshot_csv,
    ),
}


def add_data_format_arg(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--data-format",
        default="snapshot_csv",
        choices=sorted(DATA_FORMATS),
        help="Input data format loader",
    )


def get_data_format(name: str) -> DataFormat:
    try:
        return DATA_FORMATS[name]
    except KeyError as exc:
        raise SystemExit(f"unsupported data format: {name}") from exc


def load_rows_by_paths(
    paths: list[Path],
    data_format: str,
) -> dict[Path, list[SnapshotRow]]:
    loader = get_data_format(data_format).load_rows
    return {path: loader(path) for path in paths}
