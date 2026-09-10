# DEX live candle acceptance

The forming candle follows the same validated `ticker:<pair>` price stream as
the header. Persisted candle history still comes from the candle API. Quotes
update open/high/low/close within the current interval and add no trade volume;
trade events supply volume. Timestamped history and current-period snapshots
can advance recorded volume without replacing a newer live close.

Legacy `candleUpdate` messages contain the current chain slot, not the candle's
timestamp. Do not use that slot or receipt time to move a previous candle into
a new period. Delayed snapshots must not replace the live forming price.
Each symbol/resolution has separate state and subscribers; late subscription
acknowledgements must not revive a removed subscription after a switch or reconnect.

Before deploying DEX assets:

1. Run `node scripts/qa/test_dex_ui_readiness.js`, including its live candle
   regression suite, plus frontend asset integrity, shared-helper drift and RPC
   parity checks. Exercise rollover, history pagination, inverted pairs, two
   timeframes, switching symbols and reconnects.
2. Observe `ticker:12` and `candles:12:900` together on the public Testnet
   WebSocket. Confirm ticker messages drive chart callbacks without waiting for
   the slower consensus snapshots. A healthy socket alone does not establish this.
3. Export committed clean source, retain and verify the existing licensed
   chart bundle, and version JavaScript asset URLs by content hash. Preserve the
   previous Pages deployment for rollback.
4. Verify deployed asset hashes and the actual chart in a browser: observe the
   forming candle during several ticker changes, cross an interval boundary,
   switch pair/timeframe, and reconnect. Keep automated feed replay and visual
   acceptance separate in the deployment record.

On September 8, a 45-second public BTC/lUSD observation captured 194 ticker
changes but only three distinct candle closes. The old chart callback consumed
only the slower snapshots. The regression test includes that price mismatch;
the exact original observation is preserved in the deployment evidence.
