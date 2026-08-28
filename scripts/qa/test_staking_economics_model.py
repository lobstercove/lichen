#!/usr/bin/env python3

import importlib.util
import pathlib
import sys
import unittest


SCRIPT = pathlib.Path(__file__).resolve().parents[1] / "staking_economics_model.py"
SPEC = importlib.util.spec_from_file_location("staking_economics_model", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODEL = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODEL
SPEC.loader.exec_module(MODEL)


class StakingEconomicsModelTests(unittest.TestCase):
    def setUp(self) -> None:
        self.supply = MODEL.parse_licn("500000000")

    def test_below_target_base_apr_is_about_inflation_over_target(self) -> None:
        stake = MODEL.parse_licn("10000000")
        budget = MODEL.epoch_security_budget(self.supply, 400, 6_700, stake)
        apr_bps = MODEL.annualized_apr_bps(budget, stake)
        self.assertGreaterEqual(apr_bps, 596)
        self.assertLessEqual(apr_bps, 597)

    def test_doubling_stake_preserves_base_rate_below_target(self) -> None:
        first_stake = MODEL.parse_licn("10000000")
        second_stake = first_stake * 2
        first = MODEL.epoch_security_budget(self.supply, 400, 6_700, first_stake)
        second = MODEL.epoch_security_budget(self.supply, 400, 6_700, second_stake)
        self.assertLessEqual(abs(second - first * 2), 1)

    def test_budget_caps_at_existing_inflation_ceiling(self) -> None:
        target = MODEL.target_bonded_stake(self.supply, 6_700)
        ceiling = MODEL.epoch_inflation_ceiling(self.supply, 400)
        self.assertEqual(
            MODEL.epoch_security_budget(self.supply, 400, 6_700, target), ceiling
        )
        self.assertEqual(
            MODEL.epoch_security_budget(self.supply, 400, 6_700, self.supply), ceiling
        )

    def test_current_low_participation_does_not_receive_full_ceiling(self) -> None:
        current_stake = MODEL.parse_licn("1519674")
        budget = MODEL.epoch_security_budget(self.supply, 400, 6_700, current_stake)
        ceiling = MODEL.epoch_inflation_ceiling(self.supply, 400)
        self.assertLess(budget * 200, ceiling)

    def test_impossible_stake_fails_closed(self) -> None:
        with self.assertRaisesRegex(ValueError, "between zero and total supply"):
            MODEL.epoch_security_budget(self.supply, 400, 6_700, self.supply + 1)


if __name__ == "__main__":
    unittest.main()
