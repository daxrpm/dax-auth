#!/usr/bin/env bash
# install.sh — one-shot dax-auth installer and hardening setup

set -euo pipefail

DAX_USER="dax-auth"
DAX_GROUP="dax-auth"
SERVICE_NAME="dax-authd"

ETC_DIR="/etc/dax-auth"
CONFIG_PATH="$ETC_DIR/config.toml"
MASTER_KEY_PATH="$ETC_DIR/master.key"

STATE_DIR="/var/lib/dax-auth"
MODELS_DIR="$STATE_DIR/models"
USERS_DIR="$STATE_DIR/users"
RUNTIME_DIR="/run/dax-auth"

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
DOWNLOAD_SCRIPT="${SCRIPT_DIR}/download_models.sh"

failures=0
SERVICE_CHECKS_ENABLED=0

log() {
    printf '[install] %s\n' "$*"
}

warn() {
    printf '[install][warn] %s\n' "$*" >&2
}

pass_check() {
    printf '  [PASS] %s\n' "$*"
}

fail_check() {
    printf '  [FAIL] %s\n' "$*"
    failures=$((failures + 1))
}

require_root() {
    if [[ "$(id -u)" -ne 0 ]]; then
        echo "ERROR: run as root (sudo ./scripts/install.sh)" >&2
        exit 1
    fi
}

ensure_group_user() {
    if ! getent group "$DAX_GROUP" >/dev/null 2>&1; then
        log "creating system group: $DAX_GROUP"
        groupadd --system "$DAX_GROUP"
    fi

    if ! getent passwd "$DAX_USER" >/dev/null 2>&1; then
        log "creating system user: $DAX_USER"
        useradd --system --no-create-home --shell /usr/sbin/nologin --gid "$DAX_GROUP" --comment "dax-auth facial authentication daemon" "$DAX_USER"
    fi

    if getent group video >/dev/null 2>&1; then
        if ! id -nG "$DAX_USER" | grep -qw video; then
            log "adding $DAX_USER to video group"
            usermod -a -G video "$DAX_USER"
        fi
    else
        warn "video group does not exist; camera access may fail"
    fi
}

write_default_config() {
    cat >"$CONFIG_PATH" <<'EOF'
[security]
mode = "secure"
max_attempts = 3
auth_timeout_secs = 30

[liveness]
strategy = "auto"
liveness_threshold = 0.5

[camera]
fps = 30
max_frames = 90

[models]
dir = "/var/lib/dax-auth/models"
detection_model = "det_10g.onnx"
recognition_model = "w600k_r50.onnx"
liveness_model = "minifasnet_v2.onnx"

[storage]
dir = "/var/lib/dax-auth/users"

[inference]
intra_threads = 0

[daemon]
socket_path = "/run/dax-auth/daemon.sock"
log_level = "info"
journald = true
EOF
}

ensure_filesystem_layout() {
    install -d -m 0750 -o root -g "$DAX_GROUP" "$ETC_DIR"

    if [[ ! -f "$CONFIG_PATH" ]]; then
        if [[ -f "$SCRIPT_DIR/config.default.toml" ]]; then
            install -m 0640 -o root -g "$DAX_GROUP" "$SCRIPT_DIR/config.default.toml" "$CONFIG_PATH"
        elif [[ -f "$SCRIPT_DIR/../config/config.toml" ]]; then
            install -m 0640 -o root -g "$DAX_GROUP" "$SCRIPT_DIR/../config/config.toml" "$CONFIG_PATH"
        else
            write_default_config
        fi
        log "installed default config at $CONFIG_PATH"
    fi
    chown root:"$DAX_GROUP" "$CONFIG_PATH"
    chmod 0640 "$CONFIG_PATH"

    if [[ ! -f "$MASTER_KEY_PATH" ]]; then
        dd if=/dev/urandom of="$MASTER_KEY_PATH" bs=32 count=1 status=none
        log "generated master key at $MASTER_KEY_PATH"
    fi
    chown root:"$DAX_GROUP" "$MASTER_KEY_PATH"
    chmod 0640 "$MASTER_KEY_PATH"

    install -d -m 0700 -o "$DAX_USER" -g "$DAX_GROUP" "$STATE_DIR"
    install -d -m 0750 -o "$DAX_USER" -g "$DAX_GROUP" "$MODELS_DIR"
    install -d -m 0700 -o "$DAX_USER" -g "$DAX_GROUP" "$USERS_DIR"
    install -d -m 0750 -o "$DAX_USER" -g "$DAX_GROUP" "$RUNTIME_DIR"
}

install_local_artifacts_if_available() {
    local repo_root release_dir needs_build
    repo_root="$(cd -- "$SCRIPT_DIR/.." && pwd)"
    release_dir="$repo_root/target/release"
    needs_build=0

    if [[ ! -x "$release_dir/dax-authd" || ! -x "$release_dir/dax-auth" || ! -f "$release_dir/libpam_dax_auth.so" ]]; then
        needs_build=1
    elif [[ -f "$repo_root/Cargo.toml" ]] && command -v find >/dev/null 2>&1; then
        # Rebuild if any Rust/package source is newer than release artifacts.
        if find "$repo_root/crates" "$repo_root/packaging" "$repo_root/scripts" -type f \
            \( -name '*.rs' -o -name 'Cargo.toml' -o -name '*.service' -o -name '*.toml' \) \
            -newer "$release_dir/dax-authd" | grep -q .; then
            needs_build=1
        fi
    fi

    if [[ "$needs_build" -eq 1 ]]; then
        if command -v cargo >/dev/null 2>&1 && [[ -f "$repo_root/Cargo.toml" ]]; then
            log "building fresh release artifacts from source"
            (
                cd "$repo_root"
                cargo build --release -p dax-auth-daemon -p dax-auth-cli -p dax-auth-pam
            )
        else
            warn "release artifacts missing/stale and cargo source build unavailable"
        fi
    fi

    if [[ -x "$release_dir/dax-authd" && -x "$release_dir/dax-auth" && -f "$release_dir/libpam_dax_auth.so" ]]; then
        log "installing local release artifacts from $release_dir"
        install -d -m 0755 /usr/bin
        install -d -m 0755 /usr/lib/security
        install -d -m 0755 /usr/lib/dax-auth

        install -m 0755 "$release_dir/dax-authd" /usr/bin/dax-authd
        install -m 0755 "$release_dir/dax-auth" /usr/bin/dax-auth
        install -m 0644 "$release_dir/libpam_dax_auth.so" /usr/lib/security/pam_dax_auth.so

        if [[ -f "$repo_root/packaging/dax-authd.service" ]]; then
            install -d -m 0755 /usr/lib/systemd/system
            install -m 0644 "$repo_root/packaging/dax-authd.service" /usr/lib/systemd/system/dax-authd.service
        fi

        if [[ -f "$repo_root/scripts/setup-runtime-dir.sh" ]]; then
            install -m 0755 "$repo_root/scripts/setup-runtime-dir.sh" /usr/lib/dax-auth/setup-runtime-dir.sh
        fi
        install -m 0755 "$repo_root/scripts/download_models.sh" /usr/lib/dax-auth/download_models.sh
        install -m 0755 "$repo_root/scripts/install.sh" /usr/lib/dax-auth/install.sh
    else
        warn "local release artifacts not found; install package or build release binaries to enable daemon service"
    fi
}

install_models() {
    if [[ ! -x "$DOWNLOAD_SCRIPT" ]]; then
        echo "ERROR: model downloader not found or not executable: $DOWNLOAD_SCRIPT" >&2
        exit 1
    fi

    log "downloading and validating models"
    DAX_AUTH_MODELS_DIR="$MODELS_DIR" "$DOWNLOAD_SCRIPT" --dir "$MODELS_DIR"
    chown "$DAX_USER":"$DAX_GROUP" "$MODELS_DIR"/*.onnx 2>/dev/null || true
    chmod 0640 "$MODELS_DIR"/*.onnx 2>/dev/null || true
}

maybe_enable_service() {
    if ! command -v systemctl >/dev/null 2>&1; then
        warn "systemctl not found; skipping daemon enable/start"
        return
    fi

    if [[ ! -f /usr/lib/systemd/system/${SERVICE_NAME}.service && ! -f /etc/systemd/system/${SERVICE_NAME}.service ]]; then
        warn "systemd unit not found for ${SERVICE_NAME}; skipping service checks"
        return
    fi

    if [[ ! -x /usr/bin/dax-authd ]]; then
        warn "daemon binary missing at /usr/bin/dax-authd"
        warn "next step: build and install daemon binary, then run: systemctl enable --now $SERVICE_NAME"
        return
    fi

    SERVICE_CHECKS_ENABLED=1

    log "enabling and starting $SERVICE_NAME"
    systemctl daemon-reload || true
    if ! systemctl enable --now "$SERVICE_NAME"; then
        warn "failed to enable/start $SERVICE_NAME (check: journalctl -u $SERVICE_NAME -e)"
    fi
}

check_path_perm() {
    local path="$1"
    local mode="$2"
    local owner="$3"
    local group="$4"
    local label="$5"

    if [[ ! -e "$path" ]]; then
        fail_check "$label missing: $path"
        return
    fi

    local actual_mode actual_owner actual_group
    actual_mode="$(stat -c '%a' "$path")"
    actual_owner="$(stat -c '%U' "$path")"
    actual_group="$(stat -c '%G' "$path")"

    if [[ "$actual_mode" == "$mode" && "$actual_owner" == "$owner" && "$actual_group" == "$group" ]]; then
        pass_check "$label ($path) mode=$actual_mode owner=$actual_owner:$actual_group"
    else
        fail_check "$label ($path) expected mode=$mode owner=$owner:$group, got mode=$actual_mode owner=$actual_owner:$actual_group"
    fi
}

check_hash() {
    local path="$1"
    local expected="$2"
    local label="$3"

    if [[ ! -f "$path" ]]; then
        fail_check "$label missing: $path"
        return
    fi

    local actual
    actual="$(sha256sum "$path" | awk '{print $1}')"
    if [[ "$actual" == "$expected" ]]; then
        pass_check "$label hash OK"
    else
        fail_check "$label hash mismatch expected=$expected actual=$actual"
    fi
}

health_summary() {
    echo
    log "health summary"

    check_path_perm "$ETC_DIR" "750" "root" "$DAX_GROUP" "config directory"
    check_path_perm "$CONFIG_PATH" "640" "root" "$DAX_GROUP" "config file"
    check_path_perm "$MASTER_KEY_PATH" "640" "root" "$DAX_GROUP" "master key"
    check_path_perm "$STATE_DIR" "700" "$DAX_USER" "$DAX_GROUP" "state directory"
    check_path_perm "$MODELS_DIR" "750" "$DAX_USER" "$DAX_GROUP" "models directory"
    check_path_perm "$USERS_DIR" "700" "$DAX_USER" "$DAX_GROUP" "users directory"
    check_path_perm "$RUNTIME_DIR" "750" "$DAX_USER" "$DAX_GROUP" "runtime directory"

    check_hash "$MODELS_DIR/det_10g.onnx" "5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91" "det_10g.onnx"
    check_hash "$MODELS_DIR/w600k_r50.onnx" "4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43" "w600k_r50.onnx"

    if [[ "$SERVICE_CHECKS_ENABLED" -eq 1 ]]; then
        if systemctl is-active --quiet "$SERVICE_NAME"; then
            pass_check "$SERVICE_NAME active"
        else
            fail_check "$SERVICE_NAME not active"
        fi

        if systemctl is-enabled --quiet "$SERVICE_NAME"; then
            pass_check "$SERVICE_NAME enabled"
        else
            fail_check "$SERVICE_NAME not enabled"
        fi
    else
        warn "service checks skipped (systemd/binary/unit not fully available)"
    fi

    if [[ -S "$RUNTIME_DIR/daemon.sock" ]]; then
        local mode owner group
        mode="$(stat -c '%a' "$RUNTIME_DIR/daemon.sock")"
        owner="$(stat -c '%U' "$RUNTIME_DIR/daemon.sock")"
        group="$(stat -c '%G' "$RUNTIME_DIR/daemon.sock")"
        if [[ "$mode" == "660" && "$owner" == "$DAX_USER" && "$group" == "$DAX_GROUP" ]]; then
            pass_check "daemon socket mode/owner OK"
        else
            fail_check "daemon socket expected mode=660 owner=$DAX_USER:$DAX_GROUP, got mode=$mode owner=$owner:$group"
        fi
    else
        warn "daemon socket not present (service may be stopped)"
    fi

    echo
    if [[ "$failures" -eq 0 ]]; then
        log "installation complete: all checks passed"
    else
        log "installation completed with $failures failed check(s)"
        return 1
    fi
}

main() {
    require_root
    ensure_group_user
    ensure_filesystem_layout
    install_local_artifacts_if_available
    install_models
    maybe_enable_service
    health_summary
}

main "$@"
