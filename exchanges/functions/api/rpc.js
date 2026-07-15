const UPSTREAM_RPC_URL = 'https://testnet-api.lichen.network';

const ALLOWED_METHODS = new Set([
    'getHealth',
    'getNetworkInfo',
    'getFeeConfig',
    'getSlot',
    'getLatestBlock',
    'getMetrics',
    'getIncidentStatus',
]);

const JSON_HEADERS = {
    'content-type': 'application/json; charset=utf-8',
    'cache-control': 'no-store',
    'x-content-type-options': 'nosniff',
};

function jsonResponse(body, status = 200) {
    return new Response(JSON.stringify(body), {
        status,
        headers: JSON_HEADERS,
    });
}

function validatePayload(payload) {
    if (!payload || typeof payload !== 'object' || Array.isArray(payload)) {
        return 'request body must be a JSON-RPC object';
    }
    if (payload.jsonrpc !== '2.0') {
        return 'jsonrpc must be 2.0';
    }
    if (!ALLOWED_METHODS.has(payload.method)) {
        return 'method is not allowed on the exchange status proxy';
    }
    if (payload.params !== undefined && !Array.isArray(payload.params)) {
        return 'params must be an array when present';
    }
    return null;
}

export async function onRequestOptions() {
    return new Response(null, {
        status: 204,
        headers: {
            'allow': 'POST, OPTIONS',
            'access-control-allow-methods': 'POST, OPTIONS',
            'access-control-allow-headers': 'content-type, accept',
            'cache-control': 'no-store',
        },
    });
}

export async function onRequestPost({ request }) {
    let payload;
    try {
        payload = await request.json();
    } catch (_err) {
        return jsonResponse({ error: 'invalid JSON body' }, 400);
    }

    const validationError = validatePayload(payload);
    if (validationError) {
        return jsonResponse({ error: validationError }, 400);
    }

    const upstreamPayload = {
        jsonrpc: '2.0',
        id: payload.id ?? 1,
        method: payload.method,
        params: payload.params ?? [],
    };

    const upstream = await fetch(UPSTREAM_RPC_URL, {
        method: 'POST',
        headers: {
            'accept': 'application/json',
            'content-type': 'application/json',
            'user-agent': 'lichen-exchange-status/1.0',
        },
        body: JSON.stringify(upstreamPayload),
    });

    const text = await upstream.text();
    return new Response(text, {
        status: upstream.status,
        headers: JSON_HEADERS,
    });
}

export async function onRequest() {
    return jsonResponse({ error: 'method not allowed' }, 405);
}
