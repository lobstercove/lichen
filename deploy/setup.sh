#!/usr/bin/env bash
# ============================================================================
# Lichen Validator — Production Server Setup
# ============================================================================
#
# Run as root or with sudo on a fresh Debian/Ubuntu VPS.
#
# Usage:
#   sudo bash deploy/setup.sh testnet          # Setup for testnet
#   sudo bash deploy/setup.sh mainnet          # Setup for mainnet
#   sudo bash deploy/setup.sh testnet mainnet  # Setup for both networks
#
# This script:
#   1. Creates a 'lichen' system user
#   2. Creates /etc/lichen, /var/lib/lichen, /var/log/lichen
#   3. Copies binaries to /usr/local/bin
#   4. Generates /etc/lichen/env with correct port assignments
#   5. Installs and enables the systemd service
#
# Port assignments (matching lichen-start.sh / run-validator.sh V1):
#   Testnet: RPC=8899  WS=8900  P2P=7001  Signer=9201
#   Mainnet: RPC=9899  WS=9900  P2P=8001  Signer=9201
#
# ============================================================================

set -euo pipefail

INSTALL_DIR="/usr/local/bin"
CONFIG_DIR="/etc/lichen"
SECRETS_DIR="$CONFIG_DIR/secrets"
KEY_HIERARCHY_FILE="$CONFIG_DIR/key-hierarchy.md"
DRILL_REGISTER_FILE="$CONFIG_DIR/drill-register.md"
DATA_DIR="/var/lib/lichen"
LOG_DIR="/var/log/lichen"
SHARE_DIR="/usr/local/share/lichen"
CADDY_CONFIG_DIR="/etc/caddy"
CADDY_CONFIG_FILE="$CADDY_CONFIG_DIR/Caddyfile"
USER="lichen"
GROUP="lichen"

read_env_value() {
    local env_file="$1"
    local key="$2"

    if [ ! -f "$env_file" ]; then
        return 1
    fi

    grep "^${key}=" "$env_file" | tail -1 | cut -d= -f2- || true
}

upsert_env_value() {
    local env_file="$1"
    local key="$2"
    local value="$3"

    if grep -q "^${key}=" "$env_file"; then
        sed -i "s|^${key}=.*|${key}=${value}|" "$env_file"
    else
        printf '%s=%s\n' "$key" "$value" >> "$env_file"
    fi
}

delete_env_value() {
    local env_file="$1"
    local key="$2"
    sed -i "/^${key}=/d" "$env_file"
}

primary_ipv4() {
    hostname -I 2>/dev/null | awk '{print $1}'
}

ufw_is_active() {
    if ! command -v ufw >/dev/null 2>&1; then
        return 1
    fi

    [ "$(ufw status 2>/dev/null | head -n 1)" = "Status: active" ]
}

ensure_ufw_allow_rule() {
    local rule="$1"
    local comment="$2"

    if ! ufw_is_active; then
        return 0
    fi

    ufw allow "$rule" comment "$comment" >/dev/null
    echo "   ✅ Ensured ufw allows $rule ($comment)"
}

ensure_ufw_remove_allow_rule() {
    local rule="$1"
    local removed=0

    if ! ufw_is_active; then
        return 0
    fi

    while ufw status 2>/dev/null | grep -Fq "$rule"; do
        if ! ufw --force delete allow "$rule" >/dev/null 2>&1; then
            break
        fi
        removed=1
    done

    if [ "$removed" -eq 1 ]; then
        echo "   ✅ Removed legacy ufw allow $rule"
    fi
}

host_is_us_vps() {
    [ "$(primary_ipv4)" = "15.204.229.189" ]
}

append_caddy_fragment() {
    local source_file="$1"
    local target_file="$2"

    cat "$source_file" >> "$target_file"
    printf '\n' >> "$target_file"
}

ensure_caddy_installed() {
    if command -v caddy >/dev/null 2>&1; then
        echo "✅ Caddy already installed"
        return 0
    fi

    export DEBIAN_FRONTEND=noninteractive
    apt-get update -y >/dev/null
    apt-get install -y caddy >/dev/null
    echo "✅ Installed Caddy"
}

install_caddy_config() {
    local tmp_file=""
    local net=""

    tmp_file="$(mktemp)"
    append_caddy_fragment "deploy/Caddyfile.common" "$tmp_file"

    for net in "$@"; do
        case "$net" in
            testnet)
                append_caddy_fragment "deploy/Caddyfile.testnet" "$tmp_file"
                if host_is_us_vps; then
                    append_caddy_fragment "deploy/Caddyfile.testnet-us" "$tmp_file"
                fi
                ;;
            mainnet)
                append_caddy_fragment "deploy/Caddyfile.mainnet" "$tmp_file"
                if host_is_us_vps; then
                    append_caddy_fragment "deploy/Caddyfile.mainnet-us" "$tmp_file"
                fi
                ;;
        esac
    done

    mkdir -p "$CADDY_CONFIG_DIR"
    caddy validate --config "$tmp_file" --adapter caddyfile >/dev/null
    install -m 644 "$tmp_file" "$CADDY_CONFIG_FILE"
    rm -f "$tmp_file"

    systemctl enable caddy >/dev/null 2>&1 || true
    systemctl restart caddy
    echo "✅ Installed repo-managed Caddy ingress → $CADDY_CONFIG_FILE"
}

hash_contract_bundle() {
    local contracts_root="$1"
    local hash_tool=""
    local files=()
    local file=""

    if command -v sha256sum >/dev/null 2>&1; then
        hash_tool="sha256sum"
    elif command -v shasum >/dev/null 2>&1; then
        hash_tool="shasum -a 256"
    else
        return 1
    fi

    while IFS= read -r file; do
        files+=("$file")
    done < <(command find "$contracts_root" -maxdepth 2 -type f -name '*.wasm' | sort)

    if [ ${#files[@]} -eq 0 ]; then
        return 2
    fi

    if [ "$hash_tool" = "sha256sum" ]; then
        printf '%s\0' "${files[@]}" | xargs -0 sha256sum | sha256sum | awk '{print $1}'
        return 0
    fi

    printf '%s\0' "${files[@]}" | xargs -0 shasum -a 256 | shasum -a 256 | awk '{print $1}'
}

migrate_inline_secret_to_file() {
    local env_file="$1"
    local inline_key="$2"
    local file_key="$3"
    local secret_path="$4"
    local inline_value=""

    inline_value="$(read_env_value "$env_file" "$inline_key" || true)"
    if [ -n "$inline_value" ] && [ ! -f "$secret_path" ]; then
        printf '%s\n' "$inline_value" > "$secret_path"
        chmod 640 "$secret_path"
        chown root:"$GROUP" "$secret_path"
        echo "   ✅ Migrated $inline_key into $secret_path"
    fi

    if [ -n "$inline_value" ]; then
        delete_env_value "$env_file" "$inline_key"
    fi
    upsert_env_value "$env_file" "$file_key" "$secret_path"
}

write_key_hierarchy_template() {
    if [ -f "$KEY_HIERARCHY_FILE" ]; then
        return 0
    fi

    cat > "$KEY_HIERARCHY_FILE" <<EOF
# Lichen key hierarchy inventory

Complete this file before production cutover. Update it whenever ownership,
storage, rotation cadence, revocation procedure, or verification evidence changes.

Production rules:

- release signing stays offline and never lands on CI, VPS hosts, or browser-exposed machines
- contract governance, treasury execution, validator identity, bridge committee,
  oracle committee, and operator admin actions stay on separate roots
- treasury-grade authorities should use hardware-backed, HSM-backed, MPC, or
  governed-threshold custody rather than inline env secrets

| Role | Implementation / storage | Owner | Backup location | Rotation cadence | Revocation path | Last verification date |
| --- | --- | --- | --- | --- | --- | --- |
| Release signing | Offline ML-DSA key used for SHA256SUMS.sig | <owner> | <backup location> | <cadence> | Update trusted signer in validator/src/updater.rs and rotate offline key | <yyyy-mm-dd> |
| Contract governance | Governed signer set / hardware-backed threshold custody | <owner> | <backup location> | <cadence> | Rotate governance signer set through proposal flow | <yyyy-mm-dd> |
| Treasury execution | $SECRETS_DIR/custody-master-seed-<net>.txt or threshold executor | <owner> | <backup location> | <cadence> | Sweep treasury authority and revoke prior custody root | <yyyy-mm-dd> |
| Deposit derivation / sweeps | $SECRETS_DIR/custody-deposit-seed-<net>.txt on a distinct root | <owner> | <backup location> | <cadence> | Re-derive deposit path and revoke compromised derivation root | <yyyy-mm-dd> |
| Validator identity | /var/lib/lichen/state-<net>/home validator identity | <owner> | <backup location> | <cadence> | Replace validator identity and remove old validator from active set | <yyyy-mm-dd> |
| Bridge committee membership | Dedicated bridge operator key, separate from treasury and validator identity | <owner> | <backup location> | <cadence> | Remove bridge operator via governance flow and rotate committee secret | <yyyy-mm-dd> |
| Oracle committee membership | Dedicated feeder / attester key, separate from governance and treasury | <owner> | <backup location> | <cadence> | Remove oracle operator via governance flow and rotate oracle secret | <yyyy-mm-dd> |
| Operator admin actions | Per-environment admin token / control-plane credential | <owner> | <backup location> | <cadence> | Rotate admin secret and invalidate dashboards / scripts | <yyyy-mm-dd> |
EOF

    chmod 600 "$KEY_HIERARCHY_FILE"
    chown root:root "$KEY_HIERARCHY_FILE"
    echo "✅ Created key hierarchy inventory template → $KEY_HIERARCHY_FILE"
}

write_drill_register_template() {
    if [ -f "$DRILL_REGISTER_FILE" ]; then
        return 0
    fi

    cat > "$DRILL_REGISTER_FILE" <<EOF
# Lichen rotation and restore drill register

Complete this file before production cutover and update it after every exercise.

| Drill | Frequency | Owner | Preconditions | Evidence required | Pass criteria | Last run | Next due |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Custody root rotation drill | Quarterly | <owner> | Replacement custody root or threshold executor ready | rotation log + health checks | new custody root active, old root revoked | <yyyy-mm-dd> | <yyyy-mm-dd> |
| Validator key rotation drill | Quarterly | <owner> | Replacement validator identity prepared | validator replacement log | new validator identity active, old one removed | <yyyy-mm-dd> | <yyyy-mm-dd> |
| Admin token rotation drill | Monthly | <owner> | admin clients inventoried | invalidation proof + updated token record | old token rejected everywhere | <yyyy-mm-dd> | <yyyy-mm-dd> |
| Offline backup restore drill | Quarterly | <owner> | clean restore host + backup media available | restore transcript + checksums | backup restores without missing files | <yyyy-mm-dd> | <yyyy-mm-dd> |
| Contract-governance key recovery drill | Semi-annual | <owner> | signer replacement plan approved | proposal IDs + signer recovery proof | governance survives signer loss | <yyyy-mm-dd> | <yyyy-mm-dd> |
| Incident tabletop: signer compromise | Quarterly | <owner> | incident contacts + revocation plan current | timeline + containment notes | containment path works without ad hoc steps | <yyyy-mm-dd> | <yyyy-mm-dd> |
| Incident tabletop: frontend compromise | Quarterly | <owner> | frontend purge + status comms plan current | timeline + purge notes | compromised frontend can be isolated quickly | <yyyy-mm-dd> | <yyyy-mm-dd> |
EOF

    chmod 600 "$DRILL_REGISTER_FILE"
    chown root:root "$DRILL_REGISTER_FILE"
    echo "✅ Created drill register template → $DRILL_REGISTER_FILE"
}

write_incident_status_template() {
        local status_file="$1"
        local network="$2"

        if [ -f "$status_file" ]; then
                chmod 640 "$status_file"
                chown root:"$GROUP" "$status_file"
                echo "   ✅ Incident status manifest already exists → $status_file"
                return 0
        fi

        cat > "$status_file" <<EOF
{
    "schema_version": 1,
    "source": "operator",
    "network": "$network",
    "updated_at": null,
    "active_since": null,
    "mode": "normal",
    "severity": "info",
    "banner_enabled": false,
    "headline": "All systems operational",
    "summary": "No incident response mode is active.",
    "customer_message": "Deposits, bridge access, and wallet usage are operating normally.",
    "status_page_url": null,
    "actions": [],
    "components": {
        "bridge": {
            "status": "operational",
            "message": "Bridge deposits and mints are operating normally."
        },
        "contracts": {
            "status": "operational",
            "message": "No contract circuit breakers are active."
        },
        "deposits": {
            "status": "operational",
            "message": "Deposits and withdrawals are operating normally."
        },
        "wallet": {
            "status": "operational",
            "message": "Local wallet access remains available."
        }
    }
}
EOF

        chmod 640 "$status_file"
        chown root:"$GROUP" "$status_file"
        echo "   ✅ Created incident status manifest → $status_file"
}

write_service_fleet_config_template() {
        local config_file="$1"
        local network="$2"

        if [ -f "$config_file" ]; then
                chmod 640 "$config_file"
                chown root:"$GROUP" "$config_file"
                echo "   ✅ Service fleet config already exists → $config_file"
                return 0
        fi

        case "$network" in
                testnet)
                        cat > "$config_file" <<EOF
{
    "schema_version": 1,
    "network": "testnet",
    "probe_timeout_ms": 3000,
    "hosts": [
        {
            "id": "us",
            "label": "US VPS",
            "services": [
                {
                    "id": "custody",
                    "label": "Custody",
                    "service": "custody",
                    "probe": {
                        "kind": "http",
                        "url": "http://127.0.0.1:9105/health",
                        "body_contains_any": ["\"status\":\"ok\"", "\"status\": \"ok\""]
                    }
                },
                {
                    "id": "faucet",
                    "label": "Faucet",
                    "service": "faucet",
                    "probe": {
                        "kind": "http",
                        "url": "http://127.0.0.1:9100/health",
                        "body_contains_any": ["OK", "\"status\":\"ok\"", "\"status\": \"ok\""]
                    }
                }
            ]
        },
        {
            "id": "eu",
            "label": "EU VPS",
            "services": [
                {
                    "id": "custody",
                    "label": "Custody",
                    "service": "custody",
                    "expected": false,
                    "intentionally_absent_message": "Custody is only deployed on the US testnet footprint."
                },
                {
                    "id": "faucet",
                    "label": "Faucet",
                    "service": "faucet",
                    "probe": {
                        "kind": "http",
                        "url": "http://37.59.97.61:9100/health",
                        "body_contains_any": ["OK", "\"status\":\"ok\"", "\"status\": \"ok\""]
                    }
                }
            ]
        },
        {
            "id": "sea",
            "label": "SEA VPS",
            "services": [
                {
                    "id": "custody",
                    "label": "Custody",
                    "service": "custody",
                    "expected": false,
                    "intentionally_absent_message": "Custody is only deployed on the US testnet footprint."
                },
                {
                    "id": "faucet",
                    "label": "Faucet",
                    "service": "faucet",
                    "probe": {
                        "kind": "http",
                        "url": "http://15.235.142.253:9100/health",
                        "body_contains_any": ["OK", "\"status\":\"ok\"", "\"status\": \"ok\""]
                    }
                }
            ]
        }
    ]
}
EOF
                        ;;
                mainnet)
                        cat > "$config_file" <<EOF
{
    "schema_version": 1,
    "network": "mainnet",
    "hosts": []
}
EOF
                        ;;
        esac

        chmod 640 "$config_file"
        chown root:"$GROUP" "$config_file"
        echo "   ✅ Created service fleet config → $config_file"
}

# Parse network(s)
NETWORKS=()
for arg in "$@"; do
    case "$arg" in
        testnet|mainnet) NETWORKS+=("$arg") ;;
        *)
            echo "Usage: $0 <testnet|mainnet> [testnet|mainnet]"
            echo "Example: $0 testnet          # Setup testnet only"
            echo "         $0 testnet mainnet   # Setup both"
            exit 1
            ;;
    esac
done

if [ ${#NETWORKS[@]} -eq 0 ]; then
    echo "Usage: $0 <testnet|mainnet> [testnet|mainnet]"
    exit 1
fi

echo "=== Lichen Validator Setup ==="
echo "Networks: ${NETWORKS[*]}"
echo ""

# ── 1. Create system user ──
if ! id -u "$USER" &>/dev/null; then
    groupadd -r "$GROUP"
    useradd -r -g "$GROUP" -d /home/"$USER" -m -s /bin/false "$USER"
    echo "✅ Created system user: $USER"
else
    echo "✅ System user $USER already exists"
fi

# ── 2. Create directories ──
mkdir -p "$CONFIG_DIR" "$SECRETS_DIR" "$DATA_DIR" "$LOG_DIR" "$SHARE_DIR"
chown "$USER":"$GROUP" "$DATA_DIR" "$LOG_DIR"
chmod 750 "$SECRETS_DIR"
chown root:"$GROUP" "$SECRETS_DIR"
write_key_hierarchy_template
write_drill_register_template
echo "✅ Directories created"

ensure_caddy_installed

# ── 3. Copy binaries ──
for bin in lichen-validator lichen-genesis lichen-faucet lichen-custody zk-prove; do
    if [ -f "target/release/$bin" ]; then
        tmp_bin="$(mktemp "$INSTALL_DIR/.${bin}.XXXXXX")"
        install -m 755 "target/release/$bin" "$tmp_bin"
        mv "$tmp_bin" "$INSTALL_DIR/$bin"
        echo "   Installed: $bin → $INSTALL_DIR"
    else
        echo "   ⚠  $bin not found in target/release/ (skipping)"
    fi
done

# Install CLI binary (built as 'lichen', installed as 'lichen-cli')
if [ -f "target/release/lichen" ]; then
    tmp_cli="$(mktemp "$INSTALL_DIR/.lichen-cli.XXXXXX")"
    install -m 755 "target/release/lichen" "$tmp_cli"
    mv "$tmp_cli" "$INSTALL_DIR/lichen-cli"
    echo "   Installed: lichen → $INSTALL_DIR/lichen-cli"
else
    echo "   ⚠  lichen CLI not found in target/release/ (skipping)"
fi

# ── 3b. Copy seeds.json to config dir ──
if [ -f "seeds.json" ]; then
    cp seeds.json "$CONFIG_DIR/seeds.json"
    chmod 644 "$CONFIG_DIR/seeds.json"
    echo "✅ Installed seeds.json → $CONFIG_DIR/seeds.json"
else
    echo "   ❌ seeds.json not found (required for seed-file validator joins)"
    exit 1
fi

# ── 3c. Install contract artifacts for genesis/runtime bootstrap ──
if [ -d "contracts" ]; then
    rm -rf "$DATA_DIR/contracts"
    mkdir -p "$DATA_DIR/contracts"
    cp -R contracts/. "$DATA_DIR/contracts/"
    chown -R "$USER":"$GROUP" "$DATA_DIR/contracts"
    echo "✅ Installed contract artifacts → $DATA_DIR/contracts"

    CONTRACT_WASM_COUNT=$(command find "$DATA_DIR/contracts" -maxdepth 2 -type f -name '*.wasm' | wc -l | tr -d ' ')
    if [ "$CONTRACT_WASM_COUNT" = "0" ]; then
        echo "   ❌ No top-level contract WASM artifacts were installed under $DATA_DIR/contracts"
        echo "      Genesis replay uses contracts/<name>/<name>.wasm on every validator."
        exit 1
    fi

    if CONTRACT_BUNDLE_HASH=$(hash_contract_bundle "$DATA_DIR/contracts"); then
        echo "   Contract bundle hash: $CONTRACT_BUNDLE_HASH (${CONTRACT_WASM_COUNT} top-level .wasm files)"
        echo "   Every validator must use the same bundle hash before genesis creation or join."
    else
        echo "   ⚠  Could not compute a contract bundle hash for $DATA_DIR/contracts"
    fi
else
    echo "   ⚠  contracts/ directory not found (genesis bootstrap may skip contract deployment)"
fi

# ── 4. Generate env file + install systemd per network ──
for net in "${NETWORKS[@]}"; do
    echo ""
    echo "--- Setting up $net ---"

    case $net in
        testnet)
            NETWORK_LABEL="Testnet"
            RPC_PORT=8899; WS_PORT=8900; P2P_PORT=7001; SIGNER_PORT=9201
            ;;
        mainnet)
            NETWORK_LABEL="Mainnet"
            RPC_PORT=9899; WS_PORT=9900; P2P_PORT=8001; SIGNER_PORT=9201
            ;;
    esac

    ENV_FILE="$CONFIG_DIR/env-${net}"
    SIGNER_AUTH_TOKEN=""
    INCIDENT_STATUS_FILE=""
    SIGNED_METADATA_MANIFEST_FILE=""
    SIGNED_METADATA_KEYPAIR_FILE=""
    SERVICE_FLEET_CONFIG_FILE=""
    SERVICE_FLEET_UPSTREAM_RPC_URL=""
    SERVICE_FLEET_STATUS_FILE=""
    CURRENT_HOST_IPV4="$(primary_ipv4)"
    if [ -f "$ENV_FILE" ]; then
        SIGNER_AUTH_TOKEN="$(read_env_value "$ENV_FILE" "LICHEN_SIGNER_AUTH_TOKEN")"
        INCIDENT_STATUS_FILE="$(read_env_value "$ENV_FILE" "LICHEN_INCIDENT_STATUS_FILE")"
        SIGNED_METADATA_MANIFEST_FILE="$(read_env_value "$ENV_FILE" "LICHEN_SIGNED_METADATA_MANIFEST_FILE")"
        SIGNED_METADATA_KEYPAIR_FILE="$(read_env_value "$ENV_FILE" "LICHEN_SIGNED_METADATA_KEYPAIR_FILE")"
        SERVICE_FLEET_CONFIG_FILE="$(read_env_value "$ENV_FILE" "LICHEN_SERVICE_FLEET_CONFIG_FILE")"
        SERVICE_FLEET_UPSTREAM_RPC_URL="$(read_env_value "$ENV_FILE" "LICHEN_SERVICE_FLEET_UPSTREAM_RPC_URL")"
        SERVICE_FLEET_STATUS_FILE="$(read_env_value "$ENV_FILE" "LICHEN_SERVICE_FLEET_STATUS_FILE")"
    fi
    if [ -z "$SIGNER_AUTH_TOKEN" ]; then
        SIGNER_AUTH_TOKEN=$(openssl rand -hex 32 2>/dev/null || head -c 64 /dev/urandom | xxd -p -c 64)
    fi
    if [ -z "$INCIDENT_STATUS_FILE" ]; then
        INCIDENT_STATUS_FILE="$CONFIG_DIR/incident-status-${net}.json"
    fi
    if [ -z "$SIGNED_METADATA_MANIFEST_FILE" ]; then
        SIGNED_METADATA_MANIFEST_FILE="$CONFIG_DIR/signed-metadata-manifest-${net}.json"
    fi
    if [ -z "$SIGNED_METADATA_KEYPAIR_FILE" ]; then
        SIGNED_METADATA_KEYPAIR_FILE="$SECRETS_DIR/release-signing-keypair-${net}.json"
    fi
    if [ -z "$SERVICE_FLEET_CONFIG_FILE" ]; then
        SERVICE_FLEET_CONFIG_FILE="$CONFIG_DIR/service-fleet-${net}.json"
    fi
    if [ -z "$SERVICE_FLEET_STATUS_FILE" ]; then
        SERVICE_FLEET_STATUS_FILE="$DATA_DIR/service-fleet-status-${net}.json"
    fi
    if [ -z "$SERVICE_FLEET_UPSTREAM_RPC_URL" ] && [ "$net" = "testnet" ] && [ "$CURRENT_HOST_IPV4" != "15.204.229.189" ]; then
        SERVICE_FLEET_UPSTREAM_RPC_URL="http://15.204.229.189:8899"
    fi
    cat > "$ENV_FILE" <<EOF
# Lichen $net environment — auto-generated by deploy/setup.sh
LICHEN_NETWORK=$net
LICHEN_RPC_PORT=$RPC_PORT
LICHEN_WS_PORT=$WS_PORT
LICHEN_P2P_PORT=$P2P_PORT
# Required outside explicit local development. Set this before first boot so
# validator, genesis, treasury, and signer key files are encrypted at rest.
LICHEN_KEYPAIR_PASSWORD=
# P9-INF-09: Custody signer binds to loopback only (not all interfaces)
LICHEN_SIGNER_BIND=127.0.0.1:$SIGNER_PORT
LICHEN_SIGNER_AUTH_TOKEN=$SIGNER_AUTH_TOKEN
LICHEN_CONTRACTS_DIR=$DATA_DIR/contracts
RUST_LOG=info
LICHEN_INCIDENT_STATUS_FILE=$INCIDENT_STATUS_FILE
LICHEN_SIGNED_METADATA_MANIFEST_FILE=$SIGNED_METADATA_MANIFEST_FILE
LICHEN_SIGNED_METADATA_KEYPAIR_FILE=$SIGNED_METADATA_KEYPAIR_FILE
LICHEN_SERVICE_FLEET_CONFIG_FILE=$SERVICE_FLEET_CONFIG_FILE
LICHEN_SERVICE_FLEET_UPSTREAM_RPC_URL=$SERVICE_FLEET_UPSTREAM_RPC_URL
LICHEN_SERVICE_FLEET_STATUS_FILE=$SERVICE_FLEET_STATUS_FILE
# Extra CLI args passed to the validator.
# Production default stays fail-closed with auto-update disabled until signed
# canary rollout is proven.
LICHEN_EXTRA_ARGS=--auto-update=off
# LICHEN_ADMIN_TOKEN=your-secret-token-here
EOF
    chmod 600 "$ENV_FILE"
    echo "   ✅ Created $ENV_FILE"
    echo "   ⚠  Provision offline release signing key → $SIGNED_METADATA_KEYPAIR_FILE before first-boot deploy"

    write_incident_status_template "$INCIDENT_STATUS_FILE" "$net"
    write_service_fleet_config_template "$SERVICE_FLEET_CONFIG_FILE" "$net"

    # Create network-specific state dir
    mkdir -p "$DATA_DIR/state-${net}"
    chown "$USER":"$GROUP" "$DATA_DIR/state-${net}"
    install -m 644 -o "$USER" -g "$GROUP" "seeds.json" "$DATA_DIR/state-${net}/seeds.json"
    echo "   ✅ Installed state seed file: $DATA_DIR/state-${net}/seeds.json"
    touch "$SERVICE_FLEET_STATUS_FILE"
    chown "$USER":"$GROUP" "$SERVICE_FLEET_STATUS_FILE"
    chmod 640 "$SERVICE_FLEET_STATUS_FILE"

    # Create custody DB directory for this network
    CUSTODY_DB_NAME="custody-db"
    [ "$net" = "mainnet" ] && CUSTODY_DB_NAME="custody-db-mainnet"
    mkdir -p "$DATA_DIR/$CUSTODY_DB_NAME"
    chown "$USER":"$GROUP" "$DATA_DIR/$CUSTODY_DB_NAME"
    echo "   ✅ Created $DATA_DIR/$CUSTODY_DB_NAME"

    # Install network-specific systemd service (validator)
    SERVICE_NAME="lichen-validator-${net}"
    SERVICE_FILE="/etc/systemd/system/${SERVICE_NAME}.service"

    sed "s|EnvironmentFile=/etc/lichen/env|EnvironmentFile=/etc/lichen/env-${net}|" \
        deploy/lichen-validator.service > "$SERVICE_FILE"

    # Update SyslogIdentifier for this network
    sed -i "s|SyslogIdentifier=lichen-validator|SyslogIdentifier=${SERVICE_NAME}|" \
        "$SERVICE_FILE"

    systemctl enable "${SERVICE_NAME}" 2>/dev/null || true
    echo "   ✅ Installed + enabled systemd service: $SERVICE_NAME"

    # Install custody service for this network
    if [ "$net" = "testnet" ]; then
        cp deploy/lichen-custody.service /etc/systemd/system/lichen-custody.service
        systemctl enable lichen-custody 2>/dev/null || true
        echo "   ✅ Installed + enabled systemd service: lichen-custody"
    else
        cp deploy/lichen-custody-mainnet.service /etc/systemd/system/lichen-custody-mainnet.service
        systemctl enable lichen-custody-mainnet 2>/dev/null || true
        echo "   ✅ Installed + enabled systemd service: lichen-custody-mainnet"
    fi

    # Install faucet service (testnet only)
    if [ "$net" = "testnet" ]; then
        cp deploy/lichen-faucet.service /etc/systemd/system/lichen-faucet.service
        systemctl enable lichen-faucet 2>/dev/null || true
        echo "   ✅ Installed + enabled systemd service: lichen-faucet"
    fi

    # Generate custody env file if not present
    CUSTODY_ENV_NAME="custody-env"
    CUSTODY_RPC_URL="http://127.0.0.1:$RPC_PORT"
    CUSTODY_PORT=9105
    [ "$net" = "mainnet" ] && CUSTODY_ENV_NAME="custody-env-mainnet" && CUSTODY_PORT=9106
    MASTER_SEED_FILE="$SECRETS_DIR/custody-master-seed-$net.txt"
    DEPOSIT_SEED_FILE="$SECRETS_DIR/custody-deposit-seed-$net.txt"
    if [ ! -f "$CONFIG_DIR/$CUSTODY_ENV_NAME" ]; then
        CUSTODY_AUTH_TOKEN=$(openssl rand -hex 32 2>/dev/null || head -c 64 /dev/urandom | xxd -p -c 64)
        cat > "$CONFIG_DIR/$CUSTODY_ENV_NAME" <<CUSTEOF
# Lichen Custody Service — $net (auto-generated by deploy/setup.sh)
# Provision the file-backed seed paths below out-of-band from offline or
# hardware-backed custody before starting the service. Do not inline production
# treasury-grade seeds in this env file.
CUSTODY_DB_PATH=$DATA_DIR/$CUSTODY_DB_NAME
CUSTODY_API_AUTH_TOKEN=$CUSTODY_AUTH_TOKEN
CUSTODY_MASTER_SEED_FILE=$MASTER_SEED_FILE
CUSTODY_DEPOSIT_MASTER_SEED_FILE=$DEPOSIT_SEED_FILE
CUSTODY_LICHEN_RPC_URL=$CUSTODY_RPC_URL
CUSTODY_POLL_INTERVAL_SECS=15
CUSTODY_DEPOSIT_TTL_SECS=86400
CUSTODY_LISTEN_PORT=$CUSTODY_PORT
CUSTODY_SIGNER_AUTH_TOKEN=$SIGNER_AUTH_TOKEN
# Required outside explicit local development when CUSTODY_TREASURY_KEYPAIR
# points at canonical validator/genesis keypair JSON.
LICHEN_KEYPAIR_PASSWORD=
LICHEN_INCIDENT_STATUS_FILE=$INCIDENT_STATUS_FILE
RUST_LOG=info
CUSTODY_TREASURY_KEYPAIR=$CONFIG_DIR/custody-treasury-$net.json
CUSTEOF
        chmod 600 "$CONFIG_DIR/$CUSTODY_ENV_NAME"
        chown root:root "$CONFIG_DIR/$CUSTODY_ENV_NAME"
        echo "   ✅ Generated $CONFIG_DIR/$CUSTODY_ENV_NAME"
        echo "   ⚠  Provision $MASTER_SEED_FILE and $DEPOSIT_SEED_FILE before starting custody"
    else
        TREASURY_PATH="$CONFIG_DIR/custody-treasury-$net.json"
        if grep -q '^CUSTODY_TREASURY_KEYPAIR=' "$CONFIG_DIR/$CUSTODY_ENV_NAME"; then
            sed -i "s|^CUSTODY_TREASURY_KEYPAIR=.*|CUSTODY_TREASURY_KEYPAIR=$TREASURY_PATH|" "$CONFIG_DIR/$CUSTODY_ENV_NAME"
        else
            echo "CUSTODY_TREASURY_KEYPAIR=$TREASURY_PATH" >> "$CONFIG_DIR/$CUSTODY_ENV_NAME"
        fi
        if grep -q '^CUSTODY_SIGNER_AUTH_TOKEN=' "$CONFIG_DIR/$CUSTODY_ENV_NAME"; then
            sed -i "s|^CUSTODY_SIGNER_AUTH_TOKEN=.*|CUSTODY_SIGNER_AUTH_TOKEN=$SIGNER_AUTH_TOKEN|" "$CONFIG_DIR/$CUSTODY_ENV_NAME"
        else
            echo "CUSTODY_SIGNER_AUTH_TOKEN=$SIGNER_AUTH_TOKEN" >> "$CONFIG_DIR/$CUSTODY_ENV_NAME"
        fi
        if grep -q '^LICHEN_INCIDENT_STATUS_FILE=' "$CONFIG_DIR/$CUSTODY_ENV_NAME"; then
            sed -i "s|^LICHEN_INCIDENT_STATUS_FILE=.*|LICHEN_INCIDENT_STATUS_FILE=$INCIDENT_STATUS_FILE|" "$CONFIG_DIR/$CUSTODY_ENV_NAME"
        else
            echo "LICHEN_INCIDENT_STATUS_FILE=$INCIDENT_STATUS_FILE" >> "$CONFIG_DIR/$CUSTODY_ENV_NAME"
        fi
        migrate_inline_secret_to_file "$CONFIG_DIR/$CUSTODY_ENV_NAME" \
            "CUSTODY_MASTER_SEED" \
            "CUSTODY_MASTER_SEED_FILE" \
            "$MASTER_SEED_FILE"
        migrate_inline_secret_to_file "$CONFIG_DIR/$CUSTODY_ENV_NAME" \
            "CUSTODY_DEPOSIT_MASTER_SEED" \
            "CUSTODY_DEPOSIT_MASTER_SEED_FILE" \
            "$DEPOSIT_SEED_FILE"
        chmod 600 "$CONFIG_DIR/$CUSTODY_ENV_NAME"
        chown root:root "$CONFIG_DIR/$CUSTODY_ENV_NAME"
        echo "   ✅ $CONFIG_DIR/$CUSTODY_ENV_NAME already exists (updated treasury key path, signer auth token, incident manifest path, and file-backed seed paths)"
    fi

    ensure_ufw_allow_rule "80/tcp" "Caddy HTTP"
    ensure_ufw_allow_rule "443/tcp" "Caddy HTTPS"
    ensure_ufw_allow_rule "${P2P_PORT}/tcp" "${NETWORK_LABEL} P2P"
    ensure_ufw_allow_rule "${P2P_PORT}/udp" "${NETWORK_LABEL} P2P QUIC"
    ensure_ufw_remove_allow_rule "${RPC_PORT}/tcp"
    ensure_ufw_remove_allow_rule "${WS_PORT}/tcp"
    ensure_ufw_remove_allow_rule "9100/tcp"
    ensure_ufw_remove_allow_rule "9105/tcp"
    ensure_ufw_remove_allow_rule "9106/tcp"

    systemctl daemon-reload
done

install_caddy_config "${NETWORKS[@]}"

echo ""
echo "=== Setup Complete ==="
echo ""
echo "Quick reference:"
for net in "${NETWORKS[@]}"; do
    case $net in
        testnet) echo "  $net → EDGE https://testnet-rpc.lichen.network  P2P :7001  STATUS $CONFIG_DIR/incident-status-testnet.json  METADATA $CONFIG_DIR/signed-metadata-manifest-testnet.json  KEY $SECRETS_DIR/release-signing-keypair-testnet.json" ;;
        mainnet) echo "  $net → EDGE https://rpc.lichen.network  P2P :8001  STATUS $CONFIG_DIR/incident-status-mainnet.json  METADATA $CONFIG_DIR/signed-metadata-manifest-mainnet.json  KEY $SECRETS_DIR/release-signing-keypair-mainnet.json" ;;
    esac
done
echo ""
echo "Firewall (auto-opened when ufw is active; otherwise allow manually):"
echo "  sudo ufw allow 80/tcp    # Caddy HTTP -> HTTPS"
echo "  sudo ufw allow 443/tcp   # Caddy HTTPS"
for net in "${NETWORKS[@]}"; do
    case $net in
        testnet)
            echo "  sudo ufw allow 7001/tcp  # testnet P2P"
            echo "  sudo ufw allow 7001/udp  # testnet P2P QUIC"
            ;;
        mainnet)
            echo "  sudo ufw allow 8001/tcp  # mainnet P2P"
            echo "  sudo ufw allow 8001/udp  # mainnet P2P QUIC"
            ;;
    esac
done
echo ""
echo "⚠  VPS bootstrap — Genesis Node (run once on the first VPS):"
echo ""
echo "   0. Set LICHEN_KEYPAIR_PASSWORD in /etc/lichen/env-<net> and /etc/lichen/custody-env[-mainnet] before first boot."
echo "      Production validators now refuse plaintext validator, treasury, and signer key files."
echo ""
echo "   1. Prepare wallet artifacts:"
echo "      sudo -u lichen HOME=$DATA_DIR LICHEN_HOME=$DATA_DIR LICHEN_CONTRACTS_DIR=$DATA_DIR/contracts \\"
echo "        lichen-genesis --network <testnet|mainnet> --db-path $DATA_DIR/state-<net> \\"
echo "        --prepare-wallet --output-dir $DATA_DIR/genesis-keys-<net>"
echo ""
echo "   2. Start validator briefly to generate keypair, note publicKeyBase58, then stop and flush state:"
echo "      sudo systemctl start lichen-validator-<net>"
echo "      sudo python3 -c \"import json; print(json.load(open('$DATA_DIR/state-<net>/validator-keypair.json'))['publicKeyBase58'])\""
echo "      sudo systemctl stop lichen-validator-<net>"
echo "      sudo rm -rf $DATA_DIR/state-<net>"
echo ""
echo "   3. Fetch live prices and create genesis:"
echo "      PRICE_JSON=\$(curl -sf 'https://api.binance.com/api/v3/ticker/price?symbols=[\"SOLUSDT\",\"ETHUSDT\",\"BNBUSDT\"]')"
echo "      export GENESIS_SOL_USD=\$(echo \$PRICE_JSON | python3 -c \"import sys,json; [print(t['price']) for t in json.load(sys.stdin) if t['symbol']=='SOLUSDT']\")"
echo "      export GENESIS_ETH_USD=\$(echo \$PRICE_JSON | python3 -c \"import sys,json; [print(t['price']) for t in json.load(sys.stdin) if t['symbol']=='ETHUSDT']\")"
echo "      export GENESIS_BNB_USD=\$(echo \$PRICE_JSON | python3 -c \"import sys,json; [print(t['price']) for t in json.load(sys.stdin) if t['symbol']=='BNBUSDT']\")"
echo "      sudo -u lichen HOME=$DATA_DIR LICHEN_HOME=$DATA_DIR LICHEN_CONTRACTS_DIR=$DATA_DIR/contracts \\"
echo "        GENESIS_SOL_USD=\$GENESIS_SOL_USD GENESIS_ETH_USD=\$GENESIS_ETH_USD GENESIS_BNB_USD=\$GENESIS_BNB_USD \\"
echo "        lichen-genesis --network <net> --db-path $DATA_DIR/state-<net> \\"
echo "        --wallet-file $DATA_DIR/genesis-keys-<net>/genesis-wallet.json \\"
echo "        --initial-validator <PUBKEY_FROM_STEP_2>"
echo ""
echo "   4. Start the genesis validator:"
echo "      sudo systemctl start lichen-validator-<net>"
echo ""
echo "   5. On joining VPSes, verify /var/lib/lichen/state-<net>/seeds.json matches the staged release file, then start:"
echo "      sudo systemctl start lichen-validator-<net>"
echo ""
echo "   6. Install post-genesis key material from the live state:"
echo "      cd ~/lichen && bash scripts/vps-post-genesis.sh <net>"
echo ""
echo "   7. Provision the offline release-signing key on the validator host:"
echo "      sudo install -m 640 -o root -g lichen /secure/offline-mounted/release-signing-keypair-<net>.json $SECRETS_DIR/release-signing-keypair-<net>.json"
echo ""
echo "   8. Run post-genesis bootstrap from the repo checkout (auto-bootstraps Python deps if needed):"
echo "      cd ~/lichen && DEPLOY_NETWORK=<net> ./scripts/first-boot-deploy.sh --rpc http://127.0.0.1:<rpc-port> --skip-build"
echo "      testnet rpc-port=8899, mainnet rpc-port=9899"
echo ""
echo "   9. If signed metadata was generated into the repo checkout, install it into /etc/lichen:"
echo "      sudo install -m 640 -o root -g lichen ~/lichen/signed-metadata-manifest-<net>.json /etc/lichen/signed-metadata-manifest-<net>.json"
echo ""
echo "   10. Start custody and faucet after post-genesis key setup + first-boot deploy complete:"
echo "      testnet: sudo systemctl start lichen-custody && sudo systemctl start lichen-faucet"
echo "      mainnet: sudo systemctl start lichen-custody-mainnet"
echo ""
echo "   Genesis keys (treasury) will be in: $DATA_DIR/genesis-keys-<net>/genesis-keys/"
echo "   Keep those keys secure — they control the 1B LICN treasury."
