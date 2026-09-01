# Testnet RPC Edge

This Worker is the public availability layer for the four-validator testnet RPC
fleet. It routes JSON-RPC and WebSocket traffic only to authenticated HTTPS
origins in US, EU, SEA, and IN.

`testnet-api.lichen.network` is the canonical public endpoint. The
`testnet-rpc.lichen.network/*` zone route remains only as the rollback-client
transition route for already deployed `v0.5.223` software; new source and
documentation must not use it.

## Invariants

- Every origin uses a distinct `ORIGIN_AUTH_TOKEN_<REGION>` Wrangler secret.
- The matching origin token is stored in a root-owned file before approved
  host-local automation composes `/etc/caddy/Caddyfile` from the signed release
  fragments as `root:caddy` mode `0640`.
- Do not expose an origin token through a systemd environment. Caddy startup
  diagnostics enumerate environment variables.
- Raw RPC `8899` and WebSocket `8900` listeners remain loopback-only.
- `/edge-health` is strict: all four origins must be healthy and no origin may
  lag the highest origin by more than 64 slots.
- Normal requests probe their selected origin and fail over on unhealthy,
  unreachable, or gateway-failure responses. Replayable request bodies are
  capped at 2 MiB.

## Validator Origin Setup

Run the approved host-local provisioning workflow independently on each
validator with that host's own token file and public OVH hostname. It must
compose `deploy/Caddyfile.common` plus the matching signed network fragment,
insert the token without logging it, and pass `caddy validate` before install.
The public repository intentionally does not carry secret-bearing provisioning
automation.

```bash
sudo install -o root -g root -m 600 /secure/input/token \
  /etc/lichen/secrets/edge-origin-auth
sudo caddy validate --config /etc/caddy/Caddyfile
sudo systemctl reload caddy
```

Upload the same host-specific value directly to the corresponding Wrangler
secret. Never put secret values in `wrangler.toml`.

## Verification

```bash
node --test edge/testnet-rpc/test/index.test.mjs
npx wrangler deploy --dry-run --config edge/testnet-rpc/wrangler.toml
npx wrangler deploy --config edge/testnet-rpc/wrangler.toml

curl -fsS https://explorer.lichen.network/api/testnet/edge-health | jq
curl -fsS -X POST https://explorer.lichen.network/api/testnet \
  -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"getSlot"}'
```

The health response must list exactly `US`, `EU`, `SEA`, and `IN`, with every
entry reporting `healthy=true` and `ready=true`.
