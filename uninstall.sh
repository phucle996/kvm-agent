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
ENV_FILE="${CONFIG_DIR}/.env"
TLS_DIR="${CONFIG_DIR}/tls"
STATE_DIR="/var/lib/${SERVICE_NAME}"
LOG_DIR="/var/log/${SERVICE_NAME}"
SYSTEMD_UNIT="/etc/systemd/system/${SERVICE_NAME}.service"

PURGE="false"
DRY_RUN="false"
CONFIRM="false"

usage() {
  cat <<'EOF_USAGE'
Usage:
  uninstall.sh [options]

Options:
  --purge      Also delete the service user and group
  --yes, -y    Skip confirmation prompt
  --dry-run    Print actions without executing them
  -h, --help   Show this help text
EOF_USAGE
}

while [ $# -gt 0 ]; do
  case "$1" in
    --purge)   PURGE="true"; shift ;;
    --yes|-y)  CONFIRM="true"; shift ;;
    --dry-run) DRY_RUN="true"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown argument: $1" >&2; usage; exit 1 ;;
  esac
done

step() { echo "  [uninstall] $*"; }

sudo_run() {
  if [ "$DRY_RUN" = "true" ]; then
    echo "  [dry-run]   sudo $*"
  else
    sudo "$@"
  fi
}

cleanup_group_membership() {
  local group="$1"
  local user="$2"

  if ! getent group "$group" >/dev/null 2>&1; then
    step "Group '${group}' not found, skipping membership cleanup."
    return
  fi

  if ! id "$user" >/dev/null 2>&1; then
    step "User '${user}' not found, skipping membership cleanup for group '${group}'."
    return
  fi

  local current_members
  current_members="$(getent group "$group" | cut -d: -f4)"
  if [ -z "$current_members" ]; then
    step "Group '${group}' has no explicit members, skipping membership cleanup."
    return
  fi

  if ! printf '%s' "$current_members" | tr ',' '\n' | grep -Fxq "$user"; then
    step "User '${user}' is not an explicit member of '${group}', skipping."
    return
  fi

  local updated_members
  updated_members="$(printf '%s' "$current_members" | tr ',' '\n' | grep -Fxv "$user" | paste -sd, -)"
  step "Removing user '${user}' from group '${group}'..."
  sudo_run gpasswd -M "$updated_members" "$group"
}

if [ "$DRY_RUN" = "false" ] && [ "$(id -u)" -ne 0 ]; then
  exec sudo "$0" "$@"
fi

cat <<EOF_BANNER

  Aurora KVM Agent — Uninstaller
  ────────────────────────────────────────────
  service:    ${SERVICE_NAME}
  binary:     ${INSTALL_BIN}
  config:     ${CONFIG_DIR}
  env:        ${ENV_FILE}
  tls:        ${TLS_DIR}
  state:      ${STATE_DIR}
  logs:       ${LOG_DIR}
  systemd:    ${SYSTEMD_UNIT}
  purge user: ${PURGE}
  dry run:    ${DRY_RUN}
  ────────────────────────────────────────────

EOF_BANNER

if [ "$DRY_RUN" = "false" ] && [ "$CONFIRM" = "false" ]; then
  read -r -p "Proceed with uninstallation? [y/N] " confirm
  case "$confirm" in
    [yY][eE][sS]|[yY]) ;;
    *) echo "Aborted."; exit 0 ;;
  esac
  echo
fi

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

step "Removing systemd unit..."
if [ -f "$SYSTEMD_UNIT" ]; then
  sudo_run rm -f "$SYSTEMD_UNIT"
  sudo_run systemctl daemon-reload
  sudo_run systemctl reset-failed 2>/dev/null || true
else
  step "Unit file ${SYSTEMD_UNIT} not found, skipping."
fi

step "Removing binary..."
if [ -f "$INSTALL_BIN" ]; then
  sudo_run rm -f "$INSTALL_BIN"
else
  step "Binary ${INSTALL_BIN} not found, skipping."
fi

step "Removing TLS identity directory..."
if [ -d "$TLS_DIR" ]; then
  sudo_run rm -rf "$TLS_DIR"
else
  step "TLS dir ${TLS_DIR} not found, skipping."
fi

step "Removing remaining config directory..."
if [ -d "$CONFIG_DIR" ]; then
  sudo_run rm -rf "$CONFIG_DIR"
else
  step "Config dir ${CONFIG_DIR} not found, skipping."
fi

step "Removing state directory..."
if [ -d "$STATE_DIR" ]; then
  sudo_run rm -rf "$STATE_DIR"
else
  step "State dir ${STATE_DIR} not found, skipping."
fi

step "Removing log directory..."
if [ -d "$LOG_DIR" ]; then
  sudo_run rm -rf "$LOG_DIR"
else
  step "Log dir ${LOG_DIR} not found, skipping."
fi

if [ "$PURGE" = "true" ]; then
  cleanup_group_membership "kvm" "$SERVICE_USER"
  cleanup_group_membership "libvirt" "$SERVICE_USER"

  step "Removing service user '${SERVICE_USER}'..."
  if id "$SERVICE_USER" &>/dev/null; then
    sudo_run userdel --remove "$SERVICE_USER" 2>/dev/null || sudo_run userdel "$SERVICE_USER"
  else
    step "User '${SERVICE_USER}' not found, skipping."
  fi

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

echo
if [ "$DRY_RUN" = "true" ]; then
  echo "Dry run complete. No changes were made."
else
  echo "aurora-kvm-agent has been uninstalled."
  echo "Note: KVM/libvirt packages, services, and host-level access changes from install.sh are intentionally left in place."
  if [ "$PURGE" = "false" ]; then
    echo "Note: service user '${SERVICE_USER}' and group '${SERVICE_GROUP}' were kept."
    echo "      Run with --purge to also remove them."
  fi
fi
