const HEALTH_PATH = "/edge-health";
const EXPLORER_RPC_PREFIX = "/api/testnet";
const ALLOWED_METHODS = new Set(["GET", "HEAD", "POST", "OPTIONS"]);
const RETRYABLE_STATUS = new Set([502, 503, 504]);
const MAX_REQUEST_BODY_BYTES = 2 * 1024 * 1024;
const MAX_READY_SLOT_LAG = 64;
const ORIGIN_DEFINITIONS = [
    ["US", "RPC_ORIGIN_US", "ORIGIN_AUTH_TOKEN_US"],
    ["EU", "RPC_ORIGIN_EU", "ORIGIN_AUTH_TOKEN_EU"],
    ["SEA", "RPC_ORIGIN_SEA", "ORIGIN_AUTH_TOKEN_SEA"],
    ["IN", "RPC_ORIGIN_IN", "ORIGIN_AUTH_TOKEN_IN"],
];

function configuredOrigins(env) {
    return ORIGIN_DEFINITIONS.map(([region, urlKey, tokenKey]) => {
        const configuredUrl = env[urlKey];
        const token = env[tokenKey];
        if (!configuredUrl || !token) {
            throw new Error(`${region} RPC origin is not fully configured`);
        }
        const url = new URL(configuredUrl);
        if (url.protocol !== "https:") {
            throw new Error(`${region} RPC origin must use HTTPS`);
        }
        return { region, url: url.origin, token };
    });
}

function upstreamUrl(requestUrl, configuredOrigin) {
    const request = new URL(requestUrl);
    const origin = new URL(configuredOrigin);
    if (origin.protocol !== "https:") {
        throw new Error("RPC_ORIGIN must use HTTPS");
    }
    const explorerRpcPath = request.hostname === "explorer.lichen.network"
        && (request.pathname === EXPLORER_RPC_PREFIX
            || request.pathname.startsWith(`${EXPLORER_RPC_PREFIX}/`));
    origin.pathname = explorerRpcPath
        ? request.pathname.slice(EXPLORER_RPC_PREFIX.length) || "/"
        : request.pathname;
    origin.search = request.search;
    return origin;
}

async function rpc(origin, method) {
    const response = await fetch(origin.url, {
        method: "POST",
        headers: {
            "content-type": "application/json",
            "x-lichen-origin-auth": origin.token,
        },
        body: JSON.stringify({ jsonrpc: "2.0", id: 1, method }),
    });
    if (!response.ok) {
        throw new Error(`upstream ${method} returned HTTP ${response.status}`);
    }
    const body = await response.json();
    if (body.error) {
        throw new Error(`upstream ${method} returned an RPC error`);
    }
    return body.result;
}

function healthyResult(result) {
    if (typeof result === "string") {
        return ["ok", "healthy"].includes(result.toLowerCase());
    }
    return result?.status === "ok" && result?.disk?.critical !== true;
}

function orderedOrigins(request, origins) {
    const affinity = request.headers.get("cf-ray")
        || request.headers.get("cf-connecting-ip")
        || new URL(request.url).hostname;
    let hash = 2166136261;
    for (const character of affinity) {
        hash ^= character.charCodeAt(0);
        hash = Math.imul(hash, 16777619);
    }
    const start = (hash >>> 0) % origins.length;
    return [...origins.slice(start), ...origins.slice(0, start)];
}

async function health(origins) {
    const checks = await Promise.all(origins.map(async (origin) => {
        try {
            const [healthResult, slot] = await Promise.all([
                rpc(origin, "getHealth"),
                rpc(origin, "getSlot"),
            ]);
            return {
                region: origin.region,
                healthy: healthyResult(healthResult),
                health: healthResult,
                slot,
            };
        } catch (error) {
            return { region: origin.region, healthy: false, error: String(error) };
        }
    }));
    const slots = checks.filter((check) => Number.isSafeInteger(check.slot));
    const maxSlot = slots.length > 0 ? Math.max(...slots.map((check) => check.slot)) : null;
    for (const check of checks) {
        check.slot_lag = maxSlot === null || !Number.isSafeInteger(check.slot)
            ? null
            : maxSlot - check.slot;
        check.ready = check.healthy && check.slot_lag <= MAX_READY_SLOT_LAG;
    }
    const ok = checks.every((check) => check.ready);
    return Response.json(
        {
            ok,
            available: checks.some((check) => check.ready),
            max_slot: maxSlot,
            origins: checks,
        },
        {
            status: ok ? 200 : 503,
            headers: { "cache-control": "no-store" },
        },
    );
}

async function bufferedBody(request) {
    if (request.body === null) {
        return undefined;
    }
    const body = await request.arrayBuffer();
    if (body.byteLength > MAX_REQUEST_BODY_BYTES) {
        throw new RangeError("RPC request body exceeds the edge limit");
    }
    return body.byteLength === 0 ? undefined : body;
}

function responseWithOrigin(response, region) {
    if (response.status === 101 || response.webSocket) {
        return response;
    }
    const headers = new Headers(response.headers);
    headers.set("x-lichen-origin-region", region);
    return new Response(response.body, {
        status: response.status,
        statusText: response.statusText,
        headers,
    });
}

export async function handleRequest(request, env) {
    if (!ALLOWED_METHODS.has(request.method)) {
        return new Response("Method not allowed", {
            status: 405,
            headers: { allow: [...ALLOWED_METHODS].join(", ") },
        });
    }

    let origins;
    try {
        origins = configuredOrigins(env);
    } catch (error) {
        return Response.json({ ok: false, error: String(error) }, { status: 500 });
    }

    if (upstreamUrl(request.url, origins[0].url).pathname === HEALTH_PATH) {
        return health(origins);
    }

    let body;
    try {
        body = await bufferedBody(request);
    } catch (error) {
        return Response.json({ ok: false, error: String(error) }, { status: 413 });
    }

    const attempted = [];
    for (const origin of orderedOrigins(request, origins)) {
        attempted.push(origin.region);
        try {
            if (!healthyResult(await rpc(origin, "getHealth"))) {
                continue;
            }
            const headers = new Headers(request.headers);
            headers.delete("host");
            headers.set("x-lichen-edge", "testnet-rpc");
            headers.set("x-lichen-edge-origin", origin.region);
            headers.set("x-lichen-origin-auth", origin.token);
            const response = await fetch(new Request(
                upstreamUrl(request.url, origin.url),
                {
                    method: request.method,
                    headers,
                    body,
                    duplex: "half",
                    redirect: "manual",
                },
            ));
            if (RETRYABLE_STATUS.has(response.status)) {
                continue;
            }
            return responseWithOrigin(response, origin.region);
        } catch {
            // Try the next independently authenticated origin.
        }
    }
    return Response.json(
        { ok: false, error: "No healthy RPC origin available", attempted },
        { status: 503, headers: { "cache-control": "no-store" } },
    );
}

export default {
    fetch: handleRequest,
};
