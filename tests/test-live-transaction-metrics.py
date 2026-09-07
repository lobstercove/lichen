import copy
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('metrics_gate', Path(__file__).with_name('live-transaction-metrics.py'))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class MetricsGateTests(unittest.TestCase):
    def setUp(self):
        self.before = {'last_observed_block_slot': 10, 'last_observed_block_at_ms': 86401000,
                       'total_transactions': 7, 'total_blocks': 11, 'daily_transactions': 2}
        self.after = {**self.before, 'last_observed_block_slot': 12,
                      'total_transactions': 9, 'total_blocks': 13, 'daily_transactions': 4}
        self.blocks = [{'slot': slot, 'hash': str(slot), 'parent_hash': str(slot - 1),
                        'timestamp': 86401, 'transaction_count': int(slot > 10),
                        'transactions': [{'type': 'OracleAttestation'}] if slot > 10 else []}
                       for slot in range(10, 13)]

    def test_native_oracle_advancement(self):
        self.assertEqual(gate.verify_delta(self.before, self.after, self.blocks)['transaction_delta'], 2)

    def test_frozen_double_counted_and_stale_daily_fail(self):
        for key, value in [('total_transactions', 7), ('total_transactions', 11),
                           ('daily_transactions', 2), ('total_blocks', 12)]:
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                gate.verify_delta(self.before, {**self.after, key: value}, self.blocks)

    def test_missing_conflicting_or_incomplete_block_fails(self):
        with self.assertRaises(ValueError):
            gate.verify_delta(self.before, self.after, self.blocks[:-1])
        for key, value in [('parent_hash', 'bad'), ('slot', 90), ('transaction_count', 0)]:
            blocks = copy.deepcopy(self.blocks)
            blocks[-1][key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                gate.verify_delta(self.before, self.after, blocks)

    def test_idle_chain_cannot_pass_transaction_acceptance(self):
        for block in self.blocks:
            block.update(transaction_count=0, transactions=[])
        with self.assertRaises(ValueError):
            gate.verify_delta(self.before, self.after, self.blocks)

    def test_midnight_uses_new_utc_day(self):
        self.before['last_observed_block_at_ms'] = 86399000
        self.after['daily_transactions'] = 2
        self.assertTrue(gate.verify_delta(self.before, self.after, self.blocks)['success'])

    def test_local_fixture_runs_after_sample_and_failure_aborts_acceptance(self):
        events = []
        def rpc(url, method, params=None):
            events.append(method)
            if method == 'getBlock':
                return self.blocks[params[0] - 10]
            return self.before if events.count('getMetrics') == 1 else self.after
        def fixture():
            self.assertEqual(events, ['getMetrics'])
            events.append('fixture')
        with patch.object(gate, 'rpc', side_effect=rpc), patch.object(gate.time, 'sleep'):
            result = gate.observe_and_verify(['local'], 10, fixture)
            self.assertFalse(result['read_only'])
            self.assertTrue(result['success'])
            events.clear()
            def failed_fixture():
                raise ValueError('fixture rejected')
            with self.assertRaisesRegex(ValueError, 'fixture rejected'):
                gate.observe_and_verify(['local'], 10, failed_fixture)
            self.assertEqual(events, ['getMetrics'])


if __name__ == '__main__':
    unittest.main()
