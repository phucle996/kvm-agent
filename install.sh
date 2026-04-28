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
SERVER_NAME=""
CA_CERT_SRC=""
BINARY_URL_AMD64=""
BINARY_URL_ARM64=""
VERSION="latest"
GRPC_BIND_ADDR="0.0.0.0:8081"
DRY_RUN="false"
GITHUB_REPO="phucle996/kvm-agent"

usage() {
  cat <<'EOF'
Usage:
  install.sh --server <grpc-endpoint> --token <bootstrap-token> [options]

Options:
  --server <value>            Hypervisor gRPC endpoint, e.g. hypervisor.example.com:9443
  --token <value>             One-time bootstrap token created by Hypervisor
  --ca <path>                 Path to the Hypervisor CA certificate (PEM); auto-detected if omitted
  --server-name <value>       TLS SNI override; auto-derived from --server if omitted
  --binary-url <value>        Override release tarball URL for linux-amd64
  --binary-url-arm64 <value>  Optional release tarball URL for linux-arm64
  --version <value>           Agent version label persisted to .env
  --grpc-bind <value>         Local gRPC bind address for health checks (default: 0.0.0.0:8081)
  --dry-run                   Print planned actions without installing
  -h, --help                  Show this help text
EOF
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "Missing required command: $1" >&2
    exit 1
  }
}

current_login_user() {
  if [ -n "${SUDO_USER:-}" ] && [ "${SUDO_USER}" != "root" ]; then
    echo "$SUDO_USER"
    return
  fi
  id -un
}

# Extract hostname from a grpc endpoint (strips scheme and port)
auto_resolve_server_name() {
  local addr="$1"
  # Strip scheme
  addr="${addr#https://}"
  addr="${addr#http://}"
  # Strip path
  addr="${addr%%/*}"
  # Strip port
  echo "${addr%:*}"
}

# Ensure the given hostname resolves; if not, add 127.0.0.1 mapping to /etc/hosts
ensure_dns_resolution() {
  local hostname="$1"
  if [ -z "$hostname" ]; then
    return
  fi
  # Already resolves (getent covers /etc/hosts + DNS)
  if getent hosts "$hostname" >/dev/null 2>&1; then
    return
  fi
  echo "[dns] '$hostname' does not resolve; adding 127.0.0.1 entry to /etc/hosts"
  if grep -qF "$hostname" /etc/hosts 2>/dev/null; then
    echo "[dns] Entry already present in /etc/hosts (possibly stale); skipping."
    return
  fi
  echo "127.0.0.1 ${hostname}" | sudo tee -a /etc/hosts >/dev/null
  echo "[dns] Added: 127.0.0.1 ${hostname}"
}

# Well-known paths where a local Hypervisor installation stores its CA
HYPERVISOR_CA_SEARCH_PATHS=(
  "/etc/aurora-hypervisor/tls/app/ca/ca.crt"
  "/etc/aurora-hypervisor/tls/ca/ca.crt"
  "/etc/aurora-hypervisor/ca.crt"
)

# Copy the Hypervisor CA certificate into $TLS_DIR/ca.crt with correct ownership.
# Priority: --ca flag > auto-detected local hypervisor CA > skip with warning.
provision_ca_cert() {
  local dest="${TLS_DIR}/ca.crt"

  # 1. Explicit path supplied via --ca
  if [ -n "$CA_CERT_SRC" ]; then
    if [ ! -f "$CA_CERT_SRC" ]; then
      echo "[ca] ERROR: --ca file not found: ${CA_CERT_SRC}" >&2
      exit 1
    fi
    echo "[ca] Installing CA from ${CA_CERT_SRC}"
    sudo cp "$CA_CERT_SRC" "$dest"
    sudo chown "${SERVICE_USER}:${SERVICE_GROUP}" "$dest"
    sudo chmod 640 "$dest"
    return
  fi

  # 2. Auto-detect from local Hypervisor installation
  local found=""
  for path in "${HYPERVISOR_CA_SEARCH_PATHS[@]}"; do
    if [ -f "$path" ]; then
      found="$path"
      break
    fi
  done

  if [ -n "$found" ]; then
    echo "[ca] Auto-detected Hypervisor CA at ${found}"
    sudo cp "$found" "$dest"
    sudo chown "${SERVICE_USER}:${SERVICE_GROUP}" "$dest"
    sudo chmod 640 "$dest"
    return
  fi

  # 3. Not found — info; agent will use insecure TLS for automatic bootstrap
  echo "[ca] INFO: No CA certificate found. Agent will start in 'Automatic Bootstrap' mode"
  echo "[ca]       using Insecure TLS to enroll with the Hypervisor."
}

install_kvm_dependencies() {
  echo "[kvm] Installing KVM/libvirt dependencies..."
  if command -v apt-get >/dev/null 2>&1; then
    sudo apt-get update || echo "[kvm] Warning: apt-get update failed; continuing with existing package indexes." >&2
    local apt_packages=(qemu-kvm libvirt-daemon-system libvirt-clients bridge-utils virtinst)
    if ! sudo DEBIAN_FRONTEND=noninteractive apt-get install -y "${apt_packages[@]}"; then
      echo "[kvm] Warning: apt-get install failed; retrying packages individually." >&2
      echo "[kvm] Warning: your APT state may be broken. Example fix: sudo apt-get install --reinstall redisinsight or sudo apt-get remove redisinsight" >&2
      local pkg
      for pkg in "${apt_packages[@]}"; do
        if dpkg-query -W -f='${Status}' "$pkg" 2>/dev/null | grep -q "install ok installed"; then
          continue
        fi
        sudo DEBIAN_FRONTEND=noninteractive apt-get install -y "$pkg" || true
      done
    fi
  elif command -v dnf >/dev/null 2>&1; then
    sudo dnf install -y \
      qemu-kvm \
      libvirt \
      libvirt-client \
      bridge-utils \
      virt-install
  elif command -v yum >/dev/null 2>&1; then
    sudo yum install -y \
      qemu-kvm \
      libvirt \
      libvirt-client \
      bridge-utils \
      virt-install
  else
    echo "Unsupported package manager. Install qemu-kvm and libvirt manually before continuing." >&2
    exit 1
  fi
}

enable_libvirt_service() {
  echo "[kvm] Enabling libvirt service..."
  if systemctl list-unit-files libvirtd.service >/dev/null 2>&1 || [ -f /lib/systemd/system/libvirtd.service ] || [ -f /usr/lib/systemd/system/libvirtd.service ]; then
    sudo systemctl enable --now libvirtd.service
  elif systemctl list-unit-files virtqemud.service >/dev/null 2>&1 || [ -f /lib/systemd/system/virtqemud.service ] || [ -f /usr/lib/systemd/system/virtqemud.service ]; then
    sudo systemctl enable --now virtqemud.service
  else
    echo "[kvm] Error: cannot find libvirtd.service or virtqemud.service after dependency installation." >&2
    echo "[kvm] Fix package manager/libvirt first, then rerun installer. On Ubuntu/Debian try:" >&2
    echo "[kvm]   sudo apt-get install --reinstall libvirt-daemon-system libvirt-clients qemu-kvm" >&2
    echo "[kvm] If APT is blocked by a broken package, fix/remove that package first." >&2
    exit 1
  fi
}

ensure_group_exists() {
  local group="$1"
  if ! getent group "$group" >/dev/null 2>&1; then
    sudo groupadd --system "$group"
  fi
}

add_user_to_group_if_exists() {
  local user="$1"
  local group="$2"
  if id "$user" >/dev/null 2>&1 && getent group "$group" >/dev/null 2>&1; then
    sudo usermod -aG "$group" "$user"
  fi
}

configure_kvm_access() {
  local runner_user="$1"
  echo "[kvm] Configuring KVM/libvirt access for ${runner_user} and ${SERVICE_USER}..."
  ensure_group_exists kvm
  ensure_group_exists libvirt
  add_user_to_group_if_exists "$runner_user" kvm
  add_user_to_group_if_exists "$runner_user" libvirt
  add_user_to_group_if_exists "$SERVICE_USER" kvm
  add_user_to_group_if_exists "$SERVICE_USER" libvirt

  if [ -e /dev/kvm ]; then
    sudo chgrp kvm /dev/kvm || true
    sudo chmod 660 /dev/kvm || true
  fi
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
SupplementaryGroups=kvm libvirt
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
    --ca)
      CA_CERT_SRC="${2:-}"
      shift 2
      ;;
    --server-name)
      SERVER_NAME="${2:-}"
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

# Auto-derive server name (TLS SNI) from the server address if not explicitly set
if [ -z "$SERVER_NAME" ]; then
  SERVER_NAME="$(auto_resolve_server_name "$SERVER")"
fi

# VERSION already defaults to "latest" if not set

if [ "$DRY_RUN" = "true" ]; then
  cat <<EOF
Dry run:
  service_name:      ${SERVICE_NAME}
  arch:              ${ARCH}
  server:            ${SERVER}
  server_name:       ${SERVER_NAME}
  ca_cert_src:       ${CA_CERT_SRC:-auto-detect}
  binary_url:        ${SELECTED_BINARY_URL}
  grpc_bind_addr:    ${GRPC_BIND_ADDR}
  config_dir:        ${CONFIG_DIR}
  systemd_unit:      ${SYSTEMD_UNIT}
EOF
  exit 0
fi

RUNNER_USER="$(current_login_user)"

require_cmd curl
require_cmd tar
require_cmd install
require_cmd systemctl
require_cmd base32

install_kvm_dependencies
enable_libvirt_service

TMP_DIR="$(mktemp -d)"
# Ensure sudo/root can read files in this directory
chmod 755 "${TMP_DIR}"
trap 'rm -rf "${TMP_DIR}"' EXIT

echo "[1/7] Checking for ${SERVICE_NAME} binary..."
# Check for local binary first (useful for local development)
LOCAL_BIN_PATH="./target/release/${SERVICE_NAME}"
if [ -f "$LOCAL_BIN_PATH" ]; then
  echo "[1/7] Found local binary at ${LOCAL_BIN_PATH}, skipping download."
  cp "$LOCAL_BIN_PATH" "${TMP_DIR}/${SERVICE_NAME}"
else
  echo "[1/7] Downloading ${SERVICE_NAME} release artifact for ${ARCH}..."
  if ! curl -fsSL "$SELECTED_BINARY_URL" -o "${TMP_DIR}/agent.tar.gz"; then
    echo "ERROR: Failed to download binary from ${SELECTED_BINARY_URL}" >&2
    echo "Hint: If you are developing locally, run 'cargo build --release' first." >&2
    exit 1
  fi
  echo "[2/7] Extracting artifact..."
  tar -xzf "${TMP_DIR}/agent.tar.gz" -C "$TMP_DIR"
fi
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
configure_kvm_access "$RUNNER_USER"
sudo mkdir -p "$CONFIG_DIR" "$TLS_DIR" "$STATE_DIR" "$LOG_DIR"
sudo chown -R "${SERVICE_USER}:${SERVICE_GROUP}" "$CONFIG_DIR" "$STATE_DIR" "$LOG_DIR"
sudo chmod 750 "$CONFIG_DIR" "$STATE_DIR" "$LOG_DIR"
provision_ca_cert

echo "[4/7] Installing binary..."
# Stop service if running to avoid "file busy" errors during overwrite
sudo systemctl stop "${SERVICE_NAME}.service" >/dev/null 2>&1 || true
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
set_env_value "$TMP_ENV" "AGENT_SERVER_NAME" "$SERVER_NAME"
set_env_value "$TMP_ENV" "AGENT_CA_PATH" "${TLS_DIR}/ca.crt"
set_env_value "$TMP_ENV" "AGENT_CERT_PATH" "${TLS_DIR}/client.crt"
set_env_value "$TMP_ENV" "AGENT_KEY_PATH" "${TLS_DIR}/client.key"
set_env_value "$TMP_ENV" "AGENT_BOOTSTRAP_TOKEN" "$TOKEN"
set_env_value "$TMP_ENV" "AGENT_HEARTBEAT_INTERVAL_SEC" "10"
set_env_value "$TMP_ENV" "AGENT_VERSION" "$VERSION"
set_env_value "$TMP_ENV" "WORKER_MAX" "4"
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
ensure_dns_resolution "$SERVER_NAME"
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

Note: user ${RUNNER_USER} was added to kvm/libvirt groups when available.
      Log out and back in for group membership to apply to interactive shells.
EOF
