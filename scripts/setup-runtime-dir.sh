#!/bin/sh
# setup-runtime-dir.sh — Called by systemd ExecStartPre before dax-authd starts.
#
# Creates /run/dax-auth/ with the correct ownership and permissions.
# systemd's RuntimeDirectory= already does this for the service user,
# but we run this explicitly to handle edge cases (e.g., manual service restart
# after the directory was removed by tmpfiles.d or a reboot without systemd).
#
# Expected to run as root (systemd ExecStartPre before User= is applied).

set -eu

RUNTIME_DIR="/run/dax-auth"
DAX_USER="dax-auth"
DAX_GROUP="dax-auth"

# Create directory if it doesn't exist
install -d -m 0750 -o "${DAX_USER}" -g "${DAX_GROUP}" "${RUNTIME_DIR}"

# Remove stale socket from a previous run (prevents "address already in use")
SOCK="${RUNTIME_DIR}/daemon.sock"
if [ -S "${SOCK}" ]; then
    rm -f "${SOCK}"
fi

exit 0
