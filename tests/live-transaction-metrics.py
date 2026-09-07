#!/usr/bin/env python3
"""Read-only acceptance: RPC counter deltas must equal canonical block contents."""
import argparse
import concurrent.futures
import datetime
import json
import time
import urllib.request


def rpc(url, method, params=None):
    request = urllib.request.Request(
        url,
        data=json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': method,
                         'params': params or []}).encode(),
        headers={'Content-Type': 'application/json'},
    )
    with urllib.request.urlopen(request, timeout=20) as response:
        value = json.load(response)
    if 'error' in value or 'result' not in value:
        raise ValueError('RPC failed: ' + method)
    return value['result']


def utc_day(seconds):
    return datetime.datetime.fromtimestamp(seconds, datetime.timezone.utc).date()


def verify_delta(before, after, blocks):
    start = before['last_observed_block_slot']
    end = after['last_observed_block_slot']
    if not 0 < start < end or end - start > 2048:
        raise ValueError('metrics must observe an advancing, bounded live slot range')
    if len(blocks) != end - start + 1:
        raise ValueError('canonical block coverage is incomplete')
    before_day = utc_day(before['last_observed_block_at_ms'] / 1000)
    after_day = utc_day(after['last_observed_block_at_ms'] / 1000)
    total = daily = 0
    types = {}
    for index, block in enumerate(blocks):
        if block['slot'] != start + index:
            raise ValueError('canonical slots are not contiguous')
        if index and block['parent_hash'] != blocks[index - 1]['hash']:
            raise ValueError('canonical parent hash differs')
        if block['transaction_count'] != len(block['transactions']):
            raise ValueError('block transaction count differs from returned bodies')
        if not index:
            continue
        count = len(block['transactions'])
        total += count
        if utc_day(block['timestamp']) == after_day:
            daily += count
        for tx in block['transactions']:
            kind = tx['type']
            types[kind] = types.get(kind, 0) + 1
    if total == 0:
        raise ValueError('no user transactions observed; metrics acceptance is unproven')
    if after['total_transactions'] - before['total_transactions'] != total:
        raise ValueError('total transaction delta differs from canonical transactions')
    if after['total_blocks'] - before['total_blocks'] != end - start:
        raise ValueError('total block delta differs from canonical slots')
    expected_daily = daily + (before['daily_transactions'] if before_day == after_day else 0)
    if after['daily_transactions'] != expected_daily:
        raise ValueError('daily transaction counter differs from canonical UTC-day transactions')
    return {'first_slot': start, 'last_slot': end, 'last_hash': blocks[-1]['hash'],
            'transaction_delta': total, 'daily_transactions': expected_daily,
            'types': types, 'before_total': before['total_transactions'],
            'after_total': after['total_transactions'], 'success': True}


def observe_and_verify(urls, seconds, on_sampled=None):
    """Measure counters; the local harness may submit its own fixture after sampling."""
    if not 10 <= seconds <= 120:
        raise ValueError('use 10..120 observation seconds')
    def sample(url):
        return rpc(url, 'getMetrics')
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        before = list(pool.map(sample, urls))
        if on_sampled is not None:
            on_sampled()
        time.sleep(seconds)
        after = list(pool.map(sample, urls))
        def verify(item):
            index, url = item
            first, last = before[index], after[index]
            start, end = first['last_observed_block_slot'], last['last_observed_block_slot']
            if not 0 < start < end or end - start > 2048:
                raise ValueError('metrics range is stale, restarted or too large')
            blocks = [rpc(url, 'getBlock', [slot]) for slot in range(start, end + 1)]
            # Endpoint URLs can contain credentials; identify them only by index.
            return {'endpoint': index + 1, **verify_delta(first, last, blocks)}
        reports = list(pool.map(verify, enumerate(urls)))
    return {'success': True, 'read_only': on_sampled is None, 'reports': reports}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rpc-url', action='append', required=True)
    parser.add_argument('--observe-seconds', type=int, default=60)
    args = parser.parse_args()
    if not 10 <= args.observe_seconds <= 120:
        parser.error('use 10..120 observation seconds')
    print(json.dumps(observe_and_verify(args.rpc_url, args.observe_seconds), sort_keys=True))


if __name__ == '__main__':
    main()
