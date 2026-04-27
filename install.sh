#!/usr/bin/env bash
set -euo pipefail

SERVICE_NAME="aurora-kvm-agent"
SERVICE_USER="aurora-kvm-agent"
SERVICE_GROUP="aurora"
INSTALL_BIN="/usr/local/bin/${SERVICE_NAME}"
CONFIG_DIR="/etc/${SERVICE_NAME}"
ENV_FILE="${CONFIG_DIR}/.env"
TLS_DIR="${CONFIG_DIR}/tls"
STATE_DIR="/var/lib/${SERVICE_NAME}"
LOG_DIR="/var/log/${SERVICE_NAME}"
SYSTEMD_UNIT="/etc/systemd/system/${SERVICE_NAME}.service"

SERVER=""
TOKEN=""
BINARY_URL_AMD64=""
BINARY_URL_ARM64=""
VERSION="latest"
GRPC_BIND_ADDR="0.0.0.0:8081"
RUNTIME_DRIVER="kvm"
DRY_RUN="false"
GITHUB_REPO="phucle996/kvm-agent"

usage() {
  cat <<'EOF'
Usage:
  install.sh --server <grpc-endpoint> --token <bootstrap-token> [options]

Options:
  --server <value>            Hypervisor gRPC endpoint, for example https://hypervisor.example.com:9443
  --token <value>             One-time bootstrap token created by Hypervisor
  --binary-url <value>        Override release tarball URL for linux-amd64 (auto-resolved from --version by default)
  --binary-url-arm64 <value>  Optional release tarball URL for linux-arm64
  --version <value>           Agent version label persisted to .env
  --grpc-bind <value>         Local gRPC bind address for health checks
  --runtime-driver <value>    Runtime driver value persisted to .env
  --dry-run                   Validate inputs and print planned actions without installing
  -h, --help                  Show this help text
EOF
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "Missing required command: $1" >&2
    exit 1
  }
}

set_env_value() {
  local file="$1"
  local key="$2"
  local value="$3"
  local tmp_file
  tmp_file="$(mktemp)"
  if [ -f "$file" ] && grep -qE "^${key}=" "$file"; then
    awk -v k="$key" -v v="$value" 'BEGIN { line = k "=" "\"" v "\"" } $0 ~ "^" k "=" { print line; next } { print }' "$file" >"$tmp_file"
  else
    if [ -f "$file" ]; then
      cat "$file" >"$tmp_file"
    fi
    printf '%s="%s"\n' "$key" "$value" >>"$tmp_file"
  fi
  mv "$tmp_file" "$file"
}

detect_arch() {
  local machine
  machine="$(uname -m)"
  case "$machine" in
    x86_64|amd64) echo "amd64" ;;
    aarch64|arm64) echo "arm64" ;;
    *)
      echo "Unsupported architecture: ${machine}" >&2
      exit 1
      ;;
  esac
}

generate_node_id() {
  if [ -x "$INSTALL_BIN" ]; then
    "$INSTALL_BIN" --print-node-id
    return
  fi
  head -c 16 /dev/urandom | base32 | tr -d '=\n' | tr '[:lower:]' '[:upper:]' | cut -c1-26
}

write_service_file() {
  cat >"$1" <<'EOF'
[Unit]
Description=Aurora KVM Agent
After=network-online.target
Wants=network-online.target
ConditionPathExists=/usr/local/bin/aurora-kvm-agent

[Service]
Type=simple
User=aurora-kvm-agent
Group=aurora
EnvironmentFile=/etc/aurora-kvm-agent/.env
WorkingDirectory=/var/lib/aurora-kvm-agent
ExecStart=/usr/local/bin/aurora-kvm-agent
Restart=on-failure
RestartSec=5s
NoNewPrivileges=yes
PrivateTmp=yes
ProtectSystem=full
ProtectHome=yes
ReadWritePaths=/var/lib/aurora-kvm-agent /etc/aurora-kvm-agent

[Install]
WantedBy=multi-user.target
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --server)
      SERVER="${2:-}"
      shift 2
      ;;
    --token)
      TOKEN="${2:-}"
      shift 2
      ;;
    --binary-url)
      BINARY_URL_AMD64="${2:-}"
      shift 2
      ;;
    --binary-url-arm64)
      BINARY_URL_ARM64="${2:-}"
      shift 2
      ;;
    --version)
      VERSION="${2:-}"
      shift 2
      ;;
    --grpc-bind)
      GRPC_BIND_ADDR="${2:-}"
      shift 2
      ;;
    --runtime-driver)
      RUNTIME_DRIVER="${2:-}"
      shift 2
      ;;
    --dry-run)
      DRY_RUN="true"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if [ -z "$SERVER" ] || [ -z "$TOKEN" ]; then
  usage
  exit 1
fi

# Auto-resolve binary URLs from GitHub releases if not explicitly provided
if [ -z "$BINARY_URL_AMD64" ]; then
  BINARY_URL_AMD64="https://github.com/${GITHUB_REPO}/releases/download/${VERSION}/kvm-agent-linux-amd64.tar.gz"
fi
if [ -z "$BINARY_URL_ARM64" ]; then
  BINARY_URL_ARM64="https://github.com/${GITHUB_REPO}/releases/download/${VERSION}/kvm-agent-linux-arm64.tar.gz"
fi

ARCH="$(detect_arch)"
SELECTED_BINARY_URL="$BINARY_URL_AMD64"
if [ "$ARCH" = "arm64" ] && [ -n "$BINARY_URL_ARM64" ]; then
  SELECTED_BINARY_URL="$BINARY_URL_ARM64"
fi

# VERSION already defaults to "latest" if not set

if [ "$DRY_RUN" = "true" ]; then
  cat <<EOF
Dry run:
  service_name:      ${SERVICE_NAME}
  arch:              ${ARCH}
  server:            ${SERVER}
  binary_url:        ${SELECTED_BINARY_URL}
  grpc_bind_addr:    ${GRPC_BIND_ADDR}
  runtime_driver:    ${RUNTIME_DRIVER}
  config_dir:        ${CONFIG_DIR}
  systemd_unit:      ${SYSTEMD_UNIT}
EOF
  exit 0
fi

require_cmd curl
require_cmd tar
require_cmd install
require_cmd systemctl
require_cmd base32

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

echo "[1/7] Downloading ${SERVICE_NAME} release artifact for ${ARCH}..."
curl -fsSL "$SELECTED_BINARY_URL" -o "${TMP_DIR}/agent.tar.gz"

echo "[2/7] Extracting artifact..."
tar -xzf "${TMP_DIR}/agent.tar.gz" -C "$TMP_DIR"
if [ ! -f "${TMP_DIR}/${SERVICE_NAME}" ]; then
  echo "Artifact does not contain ${SERVICE_NAME}" >&2
  exit 1
fi

echo "[3/7] Ensuring service user and directories..."
if ! getent group "$SERVICE_GROUP" >/dev/null 2>&1; then
  sudo groupadd --system "$SERVICE_GROUP"
fi
if ! id "$SERVICE_USER" >/dev/null 2>&1; then
  sudo useradd -r -s /usr/sbin/nologin -g "$SERVICE_GROUP" "$SERVICE_USER"
fi
sudo mkdir -p "$CONFIG_DIR" "$TLS_DIR" "$STATE_DIR" "$LOG_DIR"
sudo chown -R "${SERVICE_USER}:${SERVICE_GROUP}" "$CONFIG_DIR" "$STATE_DIR" "$LOG_DIR"
sudo chmod 750 "$CONFIG_DIR" "$STATE_DIR" "$LOG_DIR"

echo "[4/7] Installing binary..."
sudo install -m 755 "${TMP_DIR}/${SERVICE_NAME}" "$INSTALL_BIN"

echo "[5/7] Writing environment file..."
TMP_ENV="$(mktemp)"
if [ -f "$ENV_FILE" ]; then
  sudo cp "$ENV_FILE" "$TMP_ENV"
fi
set_env_value "$TMP_ENV" "APP_NAME" "$SERVICE_NAME"
if grep -qE '^APP_NODE_ID=' "$TMP_ENV" 2>/dev/null; then
  NODE_ID="$(grep '^APP_NODE_ID=' "$TMP_ENV" | head -n1 | cut -d'=' -f2- | tr -d '"')"
else
  NODE_ID="$(generate_node_id)"
fi
set_env_value "$TMP_ENV" "APP_NODE_ID" "$NODE_ID"
set_env_value "$TMP_ENV" "SHUTDOWN_TIMEOUT_SEC" "15"
set_env_value "$TMP_ENV" "GRPC_BIND_ADDR" "$GRPC_BIND_ADDR"
set_env_value "$TMP_ENV" "AGENT_TARGET_ADDR" "$SERVER"
set_env_value "$TMP_ENV" "AGENT_SERVER_NAME" ""
set_env_value "$TMP_ENV" "AGENT_CA_PATH" "${TLS_DIR}/ca.crt"
set_env_value "$TMP_ENV" "AGENT_CERT_PATH" "${TLS_DIR}/client.crt"
set_env_value "$TMP_ENV" "AGENT_KEY_PATH" "${TLS_DIR}/client.key"
set_env_value "$TMP_ENV" "AGENT_BOOTSTRAP_TOKEN" "$TOKEN"
set_env_value "$TMP_ENV" "AGENT_HEARTBEAT_INTERVAL_SEC" "10"
set_env_value "$TMP_ENV" "AGENT_HYPERVISOR_TYPE" "kvm"
set_env_value "$TMP_ENV" "AGENT_VERSION" "$VERSION"
set_env_value "$TMP_ENV" "RUNTIME_DRIVER" "$RUNTIME_DRIVER"
set_env_value "$TMP_ENV" "WORKER_MAX" "4"
set_env_value "$TMP_ENV" "REDIS_URL" "redis://127.0.0.1:6379/0"
sudo cp "$TMP_ENV" "$ENV_FILE"
rm -f "$TMP_ENV"
sudo chmod 640 "$ENV_FILE"
sudo chown "${SERVICE_USER}:${SERVICE_GROUP}" "$ENV_FILE"

echo "[6/7] Installing systemd unit..."
TMP_UNIT="$(mktemp)"
write_service_file "$TMP_UNIT"
sudo cp "$TMP_UNIT" "$SYSTEMD_UNIT"
rm -f "$TMP_UNIT"
sudo chmod 644 "$SYSTEMD_UNIT"

echo "[7/7] Enabling and restarting ${SERVICE_NAME}..."
sudo systemctl daemon-reload
sudo systemctl enable "${SERVICE_NAME}.service"
sudo systemctl restart "${SERVICE_NAME}.service"
sudo systemctl status "${SERVICE_NAME}.service" --no-pager -l || true

cat <<EOF
Installed ${SERVICE_NAME}
  binary:      ${INSTALL_BIN}
  env:         ${ENV_FILE}
  tls_dir:     ${TLS_DIR}
  systemd:     ${SYSTEMD_UNIT}
  target:      ${SERVER}
  version:     ${VERSION}
EOF
