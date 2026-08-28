#!/usr/bin/env python3
"""Model Lichen's stake-weighted epoch security budget with integer math."""

from __future__ import annotations

import argparse
import json
from dataclasses import asdict, dataclass
from decimal import Decimal, InvalidOperation
from typing import Iterable


SPORES_PER_LICN = 1_000_000_000
SLOTS_PER_EPOCH = 432_000
SLOTS_PER_YEAR = 78_840_000


def parse_licn(value: str) -> int:
    """Parse a decimal LICN amount into spores without binary floating point."""
    try:
        decimal = Decimal(value)
    except InvalidOperation as exc:
        raise argparse.ArgumentTypeError(f"invalid LICN amount: {value}") from exc
    spores = decimal * SPORES_PER_LICN
    if decimal < 0 or spores != spores.to_integral_value():
        raise argparse.ArgumentTypeError(
            f"LICN amount must be non-negative with at most 9 decimals: {value}"
        )
    return int(spores)


def parse_positive_int(value: str) -> int:
    try:
        parsed = int(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(f"expected an integer: {value}") from exc
    if parsed <= 0:
        raise argparse.ArgumentTypeError(f"expected a positive integer: {value}")
    return parsed


def parse_int_list(value: str) -> list[int]:
    values = [parse_positive_int(item.strip()) for item in value.split(",") if item.strip()]
    if not values:
        raise argparse.ArgumentTypeError("expected at least one comma-separated integer")
    return values


def parse_licn_list(value: str) -> list[int]:
    values = [parse_licn(item.strip()) for item in value.split(",") if item.strip()]
    if not values:
        raise argparse.ArgumentTypeError("expected at least one comma-separated LICN amount")
    return values


def ceil_div(numerator: int, denominator: int) -> int:
    if denominator <= 0:
        raise ValueError("denominator must be positive")
    return (numerator + denominator - 1) // denominator


def epoch_inflation_ceiling(supply: int, inflation_bps: int) -> int:
    return supply * inflation_bps * SLOTS_PER_EPOCH // (10_000 * SLOTS_PER_YEAR)


def target_bonded_stake(supply: int, target_bonded_bps: int) -> int:
    return ceil_div(supply * target_bonded_bps, 10_000) if supply else 0


def epoch_security_budget(
    supply: int,
    inflation_bps: int,
    target_bonded_bps: int,
    effective_stake: int,
) -> int:
    if effective_stake < 0 or effective_stake > supply:
        raise ValueError("effective stake must be between zero and total supply")
    ceiling = epoch_inflation_ceiling(supply, inflation_bps)
    target = target_bonded_stake(supply, target_bonded_bps)
    if ceiling == 0 or effective_stake == 0:
        return 0
    if target == 0 or effective_stake >= target:
        return ceiling
    return ceiling * effective_stake // target


def annualized_apr_bps(epoch_reward: int, stake: int) -> int:
    if epoch_reward == 0 or stake == 0:
        return 0
    return epoch_reward * SLOTS_PER_YEAR * 10_000 // (stake * SLOTS_PER_EPOCH)


def licn_string(spores: int) -> str:
    whole, fraction = divmod(spores, SPORES_PER_LICN)
    return f"{whole}.{fraction:09d}".rstrip("0").rstrip(".")


@dataclass(frozen=True)
class Scenario:
    validator_count: int
    staker_count: int
    average_user_stake_licn: str
    validator_self_stake_licn: str
    native_self_stake_licn: str
    user_stake_licn: str
    effective_stake_licn: str
    bonded_ratio_bps: int
    target_bonded_ratio_bps: int
    epoch_inflation_ceiling_licn: str
    epoch_security_budget_licn: str
    issuance_utilization_bps: int
    gross_base_apr_bps: int
    delegated_net_apr_bps: int
    validator_commission_bps: int


def build_scenario(
    *,
    supply: int,
    inflation_bps: int,
    target_bonded_bps: int,
    validator_count: int,
    validator_self_stake: int,
    staker_count: int,
    average_user_stake: int,
    validator_commission_bps: int,
) -> Scenario:
    native_self_stake = validator_count * validator_self_stake
    user_stake = staker_count * average_user_stake
    effective_stake = native_self_stake + user_stake
    if effective_stake > supply:
        raise ValueError(
            "scenario effective stake exceeds supply: "
            f"validators={validator_count}, average_user_stake={licn_string(average_user_stake)}"
        )
    ceiling = epoch_inflation_ceiling(supply, inflation_bps)
    budget = epoch_security_budget(
        supply, inflation_bps, target_bonded_bps, effective_stake
    )
    gross_apr_bps = annualized_apr_bps(budget, effective_stake)
    delegated_net_apr_bps = gross_apr_bps * (10_000 - validator_commission_bps) // 10_000

    return Scenario(
        validator_count=validator_count,
        staker_count=staker_count,
        average_user_stake_licn=licn_string(average_user_stake),
        validator_self_stake_licn=licn_string(validator_self_stake),
        native_self_stake_licn=licn_string(native_self_stake),
        user_stake_licn=licn_string(user_stake),
        effective_stake_licn=licn_string(effective_stake),
        bonded_ratio_bps=effective_stake * 10_000 // supply if supply else 0,
        target_bonded_ratio_bps=target_bonded_bps,
        epoch_inflation_ceiling_licn=licn_string(ceiling),
        epoch_security_budget_licn=licn_string(budget),
        issuance_utilization_bps=budget * 10_000 // ceiling if ceiling else 0,
        gross_base_apr_bps=gross_apr_bps,
        delegated_net_apr_bps=delegated_net_apr_bps,
        validator_commission_bps=validator_commission_bps,
    )


def scenarios(
    *,
    supply: int,
    inflation_bps: int,
    target_bonded_bps: int,
    validator_counts: Iterable[int],
    validator_self_stake: int,
    staker_count: int,
    average_user_stakes: Iterable[int],
    validator_commission_bps: int,
) -> list[Scenario]:
    return [
        build_scenario(
            supply=supply,
            inflation_bps=inflation_bps,
            target_bonded_bps=target_bonded_bps,
            validator_count=validator_count,
            validator_self_stake=validator_self_stake,
            staker_count=staker_count,
            average_user_stake=average_user_stake,
            validator_commission_bps=validator_commission_bps,
        )
        for validator_count in validator_counts
        for average_user_stake in average_user_stakes
    ]


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--supply-licn", type=parse_licn, default=parse_licn("502675532.0659"))
    result.add_argument("--inflation-bps", type=parse_positive_int, default=400)
    result.add_argument("--target-bonded-bps", type=parse_positive_int, default=6_700)
    result.add_argument("--validator-counts", type=parse_int_list, default=[50, 60, 70])
    result.add_argument(
        "--validator-self-stake-licn", type=parse_licn, default=parse_licn("100000")
    )
    result.add_argument("--staker-count", type=parse_positive_int, default=10_000)
    result.add_argument(
        "--average-user-stakes-licn",
        type=parse_licn_list,
        default=[parse_licn(value) for value in ("100", "500", "1000", "5000")],
    )
    result.add_argument("--validator-commission-bps", type=int, default=500)
    result.add_argument("--json", action="store_true")
    return result


def validate_args(args: argparse.Namespace) -> None:
    if not 1 <= args.inflation_bps <= 10_000:
        raise ValueError("inflation basis points must be between 1 and 10000")
    if not 1 <= args.target_bonded_bps <= 10_000:
        raise ValueError("target bonded basis points must be between 1 and 10000")
    if not 0 <= args.validator_commission_bps <= 1_000:
        raise ValueError("validator commission must be between 0 and 1000 basis points")


def main() -> int:
    args = parser().parse_args()
    validate_args(args)
    rows = scenarios(
        supply=args.supply_licn,
        inflation_bps=args.inflation_bps,
        target_bonded_bps=args.target_bonded_bps,
        validator_counts=args.validator_counts,
        validator_self_stake=args.validator_self_stake_licn,
        staker_count=args.staker_count,
        average_user_stakes=args.average_user_stakes_licn,
        validator_commission_bps=args.validator_commission_bps,
    )

    payload = {
        "policy": {
            "supply_licn": licn_string(args.supply_licn),
            "inflation_bps": args.inflation_bps,
            "target_bonded_bps": args.target_bonded_bps,
            "validator_commission_bps": args.validator_commission_bps,
            "unassigned_issuance": "not_minted",
        },
        "scenarios": [asdict(row) for row in rows],
    }
    if args.json:
        print(json.dumps(payload, indent=2, sort_keys=True))
        return 0

    print(
        "validators  avg_user  bonded%  budget/epoch  gross_apr  delegated_net  issuance_used"
    )
    for row in rows:
        print(
            f"{row.validator_count:>10}  {row.average_user_stake_licn:>8}  "
            f"{row.bonded_ratio_bps / 100:>7.2f}%  "
            f"{row.epoch_security_budget_licn:>12}  "
            f"{row.gross_base_apr_bps / 100:>8.2f}%  "
            f"{row.delegated_net_apr_bps / 100:>12.2f}%  "
            f"{row.issuance_utilization_bps / 100:>12.2f}%"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
