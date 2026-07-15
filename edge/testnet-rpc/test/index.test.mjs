import assert from "node:assert/strict";
import test from "node:test";

import { handleRequest } from "../src/index.mjs";

const env = {
    RPC_ORIGIN_US: "https://vps-cdb47b12.vps.ovh.us",
    RPC_ORIGIN_EU: "https://vps-210edd4a.vps.ovh.net",
    RPC_ORIGIN_SEA: "https://vps-df7100d5.vps.ovh.ca",
    RPC_ORIGIN_IN: "https://vps-8709ee62.vps.ovh.ca",
    ORIGIN_AUTH_TOKEN_US: "test-origin-token-us",
    ORIGIN_AUTH_TOKEN_EU: "test-origin-token-eu",
    ORIGIN_AUTH_TOKEN_SEA: "test-origin-token-sea",
    ORIGIN_AUTH_TOKEN_IN: "test-origin-token-in",
};

function healthyProbe(options, slot = 1234) {
    const method = JSON.parse(options.body).method;
    return Response.json({
        jsonrpc: "2.0",
        id: 1,
        result: method === "getHealth" ? { status: "ok" } : slot,
    });
}

test("forwards RPC requests without changing the path or body", async () => {
    const originalFetch = globalThis.fetch;
    let forwarded;
    let forwardedBody;
    globalThis.fetch = async (input, options) => {
        if (!(input instanceof Request)) return healthyProbe(options);
        forwarded = input;
        forwardedBody = await input.text();
        return Response.json({ jsonrpc: "2.0", id: 1, result: 42 });
    };

    try {
        const request = new Request("https://testnet-api.lichen.network/api", {
            method: "POST",
            headers: { "content-type": "application/json", "cf-ray": "1" },
            body: '{"jsonrpc":"2.0","id":1,"method":"getSlot"}',
        });
        const response = await handleRequest(request, env);

        assert.equal(response.status, 200);
        assert.equal(forwarded.url, "https://vps-cdb47b12.vps.ovh.us/api");
        assert.equal(forwarded.headers.get("x-lichen-edge"), "testnet-rpc");
        assert.equal(forwarded.headers.get("x-lichen-edge-origin"), "US");
        assert.equal(forwarded.headers.get("x-lichen-origin-auth"), "test-origin-token-us");
        assert.equal(forwardedBody, '{"jsonrpc":"2.0","id":1,"method":"getSlot"}');
        assert.equal(response.headers.get("x-lichen-origin-region"), "US");
    } finally {
        globalThis.fetch = originalFetch;
    }
});

test("maps the explorer same-origin gateway to the RPC root", async () => {
    const originalFetch = globalThis.fetch;
    let forwarded;
    globalThis.fetch = async (input, options) => {
        if (!(input instanceof Request)) return healthyProbe(options);
        forwarded = input;
        return Response.json({ jsonrpc: "2.0", id: 1, result: 42 });
    };

    try {
        const response = await handleRequest(
            new Request("https://explorer.lichen.network/api/testnet", {
                method: "POST",
                headers: { "cf-ray": "2" },
                body: '{"jsonrpc":"2.0","id":1,"method":"getSlot"}',
            }),
            env,
        );

        assert.equal(response.status, 200);
        assert.equal(forwarded.url, "https://vps-210edd4a.vps.ovh.net/");
    } finally {
        globalThis.fetch = originalFetch;
    }
});

test("strict health checks every origin and reports slot parity", async () => {
    const originalFetch = globalThis.fetch;
    const slots = new Map([
        ["vps-cdb47b12.vps.ovh.us", 1237],
        ["vps-210edd4a.vps.ovh.net", 1236],
        ["vps-df7100d5.vps.ovh.ca", 1235],
        ["vps-8709ee62.vps.ovh.ca", 1234],
    ]);
    globalThis.fetch = async (url, options) => healthyProbe(options, slots.get(new URL(url).hostname));

    try {
        const response = await handleRequest(
            new Request("https://testnet-api.lichen.network/edge-health"),
            env,
        );
        const body = await response.json();
        assert.equal(response.status, 200);
        assert.equal(body.ok, true);
        assert.equal(body.max_slot, 1237);
        assert.equal(body.origins.length, 4);
        assert.deepEqual(body.origins.map((origin) => origin.slot_lag), [0, 1, 2, 3]);
    } finally {
        globalThis.fetch = originalFetch;
    }
});

test("fails over when the selected origin is unavailable", async () => {
    const originalFetch = globalThis.fetch;
    let forwarded;
    globalThis.fetch = async (input, options) => {
        if (!(input instanceof Request)) {
            if (new URL(input).hostname === "vps-cdb47b12.vps.ovh.us") {
                throw new Error("origin unavailable");
            }
            return healthyProbe(options);
        }
        forwarded = input;
        return Response.json({ jsonrpc: "2.0", id: 1, result: 42 });
    };

    try {
        const response = await handleRequest(
            new Request("https://testnet-api.lichen.network", {
                method: "POST",
                headers: { "cf-ray": "1" },
                body: '{"jsonrpc":"2.0","id":1,"method":"getSlot"}',
            }),
            env,
        );
        assert.equal(response.status, 200);
        assert.equal(forwarded.url, "https://vps-210edd4a.vps.ovh.net/");
        assert.equal(response.headers.get("x-lichen-origin-region"), "EU");
    } finally {
        globalThis.fetch = originalFetch;
    }
});

test("strict health rejects a reachable but lagging origin", async () => {
    const originalFetch = globalThis.fetch;
    globalThis.fetch = async (url, options) => {
        const lagging = new URL(url).hostname === "vps-210edd4a.vps.ovh.net";
        return healthyProbe(options, lagging ? 1100 : 1234);
    };

    try {
        const response = await handleRequest(
            new Request("https://testnet-api.lichen.network/edge-health"),
            env,
        );
        const body = await response.json();
        assert.equal(response.status, 503);
        assert.equal(body.ok, false);
        assert.equal(body.available, true);
        assert.equal(body.origins.find((origin) => origin.region === "EU").ready, false);
    } finally {
        globalThis.fetch = originalFetch;
    }
});

test("rejects unsafe methods and non-TLS origins", async () => {
    const methodResponse = await handleRequest(
        new Request("https://testnet-api.lichen.network", { method: "DELETE" }),
        env,
    );
    assert.equal(methodResponse.status, 405);

    const originResponse = await handleRequest(
        new Request("https://testnet-api.lichen.network"),
        { ...env, RPC_ORIGIN_US: "http://127.0.0.1:8899" },
    );
    assert.equal(originResponse.status, 500);
});

test("rejects oversized replayable RPC bodies", async () => {
    const response = await handleRequest(
        new Request("https://testnet-api.lichen.network", {
            method: "POST",
            body: "x".repeat((2 * 1024 * 1024) + 1),
        }),
        env,
    );
    assert.equal(response.status, 413);
});
