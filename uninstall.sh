#!/usr/bin/env bash
# uninstall.sh — remove aurora-kvm-agent from this host
# Usage:
#   sudo ./uninstall.sh              # keep service user & group
#   sudo ./uninstall.sh --purge      # also delete service user & group
#   sudo ./uninstall.sh --dry-run    # print what would be done
set -euo pipefail

SERVICE_NAME="aurora-kvm-agent"
SERVICE_USER="aurora-kvm-agent"
SERVICE_GROUP="aurora"
INSTALL_BIN="/usr/local/bin/${SERVICE_NAME}"
CONFIG_DIR="/etc/${SERVICE_NAME}"
TLS_DIR="${CONFIG_DIR}/tls"
STATE_DIR="/var/lib/${SERVICE_NAME}"
LOG_DIR="/var/log/${SERVICE_NAME}"
SYSTEMD_UNIT="/etc/systemd/system/${SERVICE_NAME}.service"

PURGE="false"
DRY_RUN="false"

# ─── args ────────────────────────────────────────────────────────────────────

usage() {
  cat <<'EOF'
Usage:
  uninstall.sh [options]

Options:
  --purge      Also delete the service user and group
  --dry-run    Print actions without executing them
  -h, --help   Show this help text
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --purge)   PURGE="true";   shift ;;
    --dry-run) DRY_RUN="true"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown argument: $1" >&2; usage; exit 1 ;;
  esac
done

# ─── helpers ─────────────────────────────────────────────────────────────────

step() { echo "  [uninstall] $*"; }

run() {
  if [ "$DRY_RUN" = "true" ]; then
    echo "  [dry-run]   $*"
  else
    "$@"
  fi
}

sudo_run() {
  if [ "$DRY_RUN" = "true" ]; then
    echo "  [dry-run]   sudo $*"
  else
    sudo "$@"
  fi
}

# Require root (or sudo) for real runs
if [ "$DRY_RUN" = "false" ] && [ "$(id -u)" -ne 0 ]; then
  exec sudo "$0" "$@"
fi

# ─── banner ──────────────────────────────────────────────────────────────────

cat <<EOF

  Aurora KVM Agent — Uninstaller
  ────────────────────────────────────────────
  service:    ${SERVICE_NAME}
  binary:     ${INSTALL_BIN}
  config:     ${CONFIG_DIR}
  state:      ${STATE_DIR}
  logs:       ${LOG_DIR}
  systemd:    ${SYSTEMD_UNIT}
  purge user: ${PURGE}
  dry run:    ${DRY_RUN}
  ────────────────────────────────────────────

EOF

if [ "$DRY_RUN" = "false" ]; then
  read -r -p "Proceed with uninstallation? [y/N] " confirm
  case "$confirm" in
    [yY][eE][sS]|[yY]) ;;
    *) echo "Aborted."; exit 0 ;;
  esac
  echo
fi

# ─── 1. stop & disable service ───────────────────────────────────────────────

step "Stopping and disabling ${SERVICE_NAME} service..."

if systemctl list-unit-files "${SERVICE_NAME}.service" &>/dev/null; then
  if systemctl is-active --quiet "${SERVICE_NAME}.service"; then
    sudo_run systemctl stop "${SERVICE_NAME}.service"
  fi
  if systemctl is-enabled --quiet "${SERVICE_NAME}.service" 2>/dev/null; then
    sudo_run systemctl disable "${SERVICE_NAME}.service"
  fi
else
  step "Service unit not found, skipping stop/disable."
fi

# ─── 2. remove systemd unit ──────────────────────────────────────────────────

step "Removing systemd unit..."
if [ -f "$SYSTEMD_UNIT" ]; then
  sudo_run rm -f "$SYSTEMD_UNIT"
  sudo_run systemctl daemon-reload
  sudo_run systemctl reset-failed 2>/dev/null || true
else
  step "Unit file ${SYSTEMD_UNIT} not found, skipping."
fi

# ─── 3. remove binary ────────────────────────────────────────────────────────

step "Removing binary..."
if [ -f "$INSTALL_BIN" ]; then
  sudo_run rm -f "$INSTALL_BIN"
else
  step "Binary ${INSTALL_BIN} not found, skipping."
fi

# ─── 4. remove config & TLS directory ────────────────────────────────────────

step "Removing config directory (includes TLS certs)..."
if [ -d "$CONFIG_DIR" ]; then
  sudo_run rm -rf "$CONFIG_DIR"
else
  step "Config dir ${CONFIG_DIR} not found, skipping."
fi

# ─── 5. remove state directory ───────────────────────────────────────────────

step "Removing state directory..."
if [ -d "$STATE_DIR" ]; then
  sudo_run rm -rf "$STATE_DIR"
else
  step "State dir ${STATE_DIR} not found, skipping."
fi

# ─── 6. remove log directory ─────────────────────────────────────────────────

step "Removing log directory..."
if [ -d "$LOG_DIR" ]; then
  sudo_run rm -rf "$LOG_DIR"
else
  step "Log dir ${LOG_DIR} not found, skipping."
fi

# ─── 7. optionally remove service user & group ───────────────────────────────

if [ "$PURGE" = "true" ]; then
  step "Removing service user '${SERVICE_USER}'..."
  if id "$SERVICE_USER" &>/dev/null; then
    sudo_run userdel --remove "$SERVICE_USER" 2>/dev/null || sudo_run userdel "$SERVICE_USER"
  else
    step "User '${SERVICE_USER}' not found, skipping."
  fi

  # Only remove the group if no other users belong to it
  step "Removing service group '${SERVICE_GROUP}' (if empty)..."
  if getent group "$SERVICE_GROUP" &>/dev/null; then
    members="$(getent group "$SERVICE_GROUP" | cut -d: -f4)"
    if [ -z "$members" ]; then
      sudo_run groupdel "$SERVICE_GROUP" 2>/dev/null || true
    else
      step "Group '${SERVICE_GROUP}' still has members (${members}), skipping deletion."
    fi
  else
    step "Group '${SERVICE_GROUP}' not found, skipping."
  fi
else
  step "Skipping user/group removal (pass --purge to also remove them)."
fi

# ─── done ────────────────────────────────────────────────────────────────────

echo
if [ "$DRY_RUN" = "true" ]; then
  echo "Dry run complete. No changes were made."
else
  echo "aurora-kvm-agent has been uninstalled."
  if [ "$PURGE" = "false" ]; then
    echo "Note: service user '${SERVICE_USER}' and group '${SERVICE_GROUP}' were kept."
    echo "      Run with --purge to also remove them."
  fi
fi
