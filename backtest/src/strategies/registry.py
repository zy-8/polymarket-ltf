from __future__ import annotations

import argparse
from dataclasses import dataclass
from decimal import Decimal
from itertools import product
from typing import Any

from domain.strategy import Strategy

from .mean_reversion import MeanReversionZScoreStrategy


@dataclass(frozen=True)
class StrategyVariant:
    parameters: dict[str, str]
    strategy: Strategy


@dataclass(frozen=True)
class StrategyDefinition:
    name: str
    description: str

    def build(self, args: argparse.Namespace) -> Strategy:
        raise NotImplementedError

    def parameters(self, args: argparse.Namespace) -> dict[str, str]:
        raise NotImplementedError

    def scan_variants(self, args: argparse.Namespace) -> list[StrategyVariant]:
        raise NotImplementedError


class MeanReversionStrategyDefinition(StrategyDefinition):
    def build(self, args: argparse.Namespace) -> Strategy:
        return MeanReversionZScoreStrategy(
            size=Decimal(args.size),
            entry_z=Decimal(args.entry_z),
            exit_z=Decimal(args.exit_z),
            max_chainlink_run=int(args.max_run),
        )

    def parameters(self, args: argparse.Namespace) -> dict[str, str]:
        return {
            "size": args.size,
            "entry_z": args.entry_z,
            "exit_z": args.exit_z,
            "max_chainlink_run": args.max_run,
        }

    def scan_variants(self, args: argparse.Namespace) -> list[StrategyVariant]:
        entry_z_values = parse_decimal_grid(args.scan_entry_z_values or args.entry_z)
        exit_z_values = parse_decimal_grid(args.scan_exit_z_values or args.exit_z)
        size_values = parse_decimal_grid(args.scan_size_values or args.size)
        max_run_values = parse_int_grid(args.scan_max_run_values or args.max_run)
        variants = []
        for entry_z, exit_z, size, max_chainlink_run in product(
            entry_z_values,
            exit_z_values,
            size_values,
            max_run_values,
        ):
            variants.append(
                StrategyVariant(
                    parameters={
                        "size": str(size),
                        "entry_z": str(entry_z),
                        "exit_z": str(exit_z),
                        "max_chainlink_run": str(max_chainlink_run),
                    },
                    strategy=MeanReversionZScoreStrategy(
                        size=size,
                        entry_z=entry_z,
                        exit_z=exit_z,
                        max_chainlink_run=max_chainlink_run,
                    ),
                )
            )
        return variants


STRATEGIES: dict[str, StrategyDefinition] = {
    "mean_reversion_zscore": MeanReversionStrategyDefinition(
        name="mean_reversion_zscore",
        description="基于 z_score 的 Polymarket up/down 均值回归策略",
    ),
}


def add_strategy_arg(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--strategy",
        default="mean_reversion_zscore",
        choices=sorted(STRATEGIES),
        help="Strategy to run",
    )


def add_strategy_parameter_args(
    parser: argparse.ArgumentParser,
    include_scan_args: bool = False,
) -> None:
    parser.add_argument("--size", default="1", help="Absolute target position size")
    parser.add_argument(
        "--entry-z",
        default="1.5",
        help="Open long below -entry_z, open short above +entry_z",
    )
    parser.add_argument(
        "--exit-z",
        default="0.5",
        help="Exit when abs(z_score) is below this threshold",
    )
    parser.add_argument(
        "--max-run",
        default="0",
        help="Optional chainlink_run filter, 0 disables the filter",
    )
    if include_scan_args:
        parser.add_argument(
            "--scan-entry-z-values",
            default=None,
            help="Comma-separated entry_z grid, for example: 1.0,1.5,2.0",
        )
        parser.add_argument(
            "--scan-exit-z-values",
            default=None,
            help="Comma-separated exit_z grid, for example: 0.3,0.5,0.8",
        )
        parser.add_argument(
            "--scan-size-values",
            default=None,
            help="Comma-separated size grid, for example: 0.5,1,2",
        )
        parser.add_argument(
            "--scan-max-run-values",
            default=None,
            help="Comma-separated max_chainlink_run grid, for example: 0,2,3",
        )


def get_strategy_definition(name: str) -> StrategyDefinition:
    try:
        return STRATEGIES[name]
    except KeyError as exc:
        raise SystemExit(f"unsupported strategy: {name}") from exc


def build_strategy_from_args(args: argparse.Namespace) -> Strategy:
    return get_strategy_definition(args.strategy).build(args)


def strategy_descriptor_from_args(args: argparse.Namespace) -> tuple[str, dict[str, str]]:
    definition = get_strategy_definition(args.strategy)
    return definition.name, definition.parameters(args)


def strategy_scan_variants_from_args(args: argparse.Namespace) -> list[StrategyVariant]:
    return get_strategy_definition(args.strategy).scan_variants(args)


def parse_decimal_grid(raw: str) -> list[Decimal]:
    values = [value.strip() for value in raw.split(",") if value.strip()]
    if not values:
        raise ValueError("参数网格不能为空")
    return [Decimal(value) for value in values]


def parse_int_grid(raw: str) -> list[int]:
    values = [value.strip() for value in raw.split(",") if value.strip()]
    if not values:
        raise ValueError("参数网格不能为空")
    return [int(value) for value in values]
