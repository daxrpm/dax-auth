#!/usr/bin/env bash
# Interactive installer for dax-auth.
#
# What it does:
#   - Detects your Linux distribution and the matching package manager.
#   - Detects the cameras you have (RGB / IR) so the install adapts
#     to whatever hardware is present.
#   - Offers to install the system prerequisites for that distro.
#   - Builds the release artefacts (binary + cdylib) on demand.
#   - Fetches the ONNX models on demand.
#   - Installs everything to standard system paths and writes
#     `/etc/dax-auth/config.toml` so the PAM module works without
#     env vars.
#   - Provides a separate menu to wire `libdax_pam.so` into a PAM
#     service of your choice (sudo, login, …) with automatic backup
#     and a one-line rollback path.
#
# Run it without arguments:
#
#     ./scripts/install.sh
#
# It is idempotent: re-running picks up where the previous run left
# off, and never edits a file unless you confirm.

set -Eeuo pipefail

# ─────────────────────────── styling ────────────────────────────
if [[ -t 1 ]] && command -v tput >/dev/null 2>&1; then
    BOLD=$(tput bold)
    DIM=$(tput dim)
    RESET=$(tput sgr0)
    RED=$(tput setaf 1)
    GREEN=$(tput setaf 2)
    YELLOW=$(tput setaf 3)
    MAGENTA=$(tput setaf 5)
    CYAN=$(tput setaf 6)
else
    BOLD="" DIM="" RESET="" RED="" GREEN="" YELLOW="" MAGENTA="" CYAN=""
fi

LOG_FILE="${TMPDIR:-/tmp}/dax-auth-install-$(date +%Y%m%d-%H%M%S).log"
: >"$LOG_FILE"
log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >>"$LOG_FILE"; }

# All log helpers write to stderr so they never leak into the stdout
# of a command substitution (e.g. `backup="$(backup_pam_file …)"`).
heading() { printf "\n%s━━%s %s%s%s\n" "$CYAN" "$RESET" "$BOLD" "$*" "$RESET" >&2; log "[step] $*"; }
substep() { printf "  %s∙%s %s\n" "$DIM" "$RESET" "$*" >&2; log "[..] $*"; }
ok()      { printf "  %s✓%s %s\n" "$GREEN" "$RESET" "$*" >&2; log "[ok] $*"; }
warn()    { printf "  %s!%s %s\n" "$YELLOW" "$RESET" "$*" >&2; log "[!!] $*"; }
err()     { printf "  %s✗%s %s\n" "$RED" "$RESET" "$*" >&2; log "[xx] $*"; }
note()    { printf "    %s%s%s\n" "$DIM" "$*" "$RESET" >&2; }

abort() {
    err "$1"
    note "Full log: $LOG_FILE"
    exit 1
}

on_err() {
    local rc=$?
    printf "\n%s✗ command failed at line %s: %s (exit %s)%s\n" \
        "$RED$BOLD" "${BASH_LINENO[0]}" "$BASH_COMMAND" "$rc" "$RESET" >&2
    printf "  see %s\n" "$LOG_FILE" >&2
}
trap on_err ERR

ask() {
    # ask "Question?" "default"
    local prompt="$1" default="${2:-}" reply
    if [[ -n "$default" ]]; then
        read -r -p "  $(printf "%s%s%s [%s]: " "$BOLD" "$prompt" "$RESET" "$default")" reply
        echo "${reply:-$default}"
    else
        read -r -p "  $(printf "%s%s%s: " "$BOLD" "$prompt" "$RESET")" reply
        echo "$reply"
    fi
}

confirm() {
    local reply
    reply="$(ask "$1 [y/N]" "N")"
    [[ "${reply,,}" == "y" || "${reply,,}" == "yes" ]]
}

run_root() {
    log "[sudo] $*"
    if [[ $EUID -eq 0 ]]; then
        "$@"
    else
        sudo "$@"
    fi
}

# ─────────────────────────── detection ────────────────────────────
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RELEASE_BIN="$REPO_ROOT/target/release/daxauth"
RELEASE_LIB="$REPO_ROOT/target/release/libdax_pam.so"
MODELS_DIR="$REPO_ROOT/models"

DISTRO_ID="" DISTRO_NAME="" DISTRO_FAMILY=""
PKG_MGR="" PKG_INSTALL=""
SECURITY_DIR=""
PAM_DEV_PKG=""
EXTRA_PKGS=()

INSTALL_PREFIX=/usr/local
INSTALL_BIN="$INSTALL_PREFIX/bin/daxauth"
INSTALL_LIB=""    # set after distro detection
INSTALL_SHARE=/usr/share/daxauth
INSTALL_VAULT_DIR=/var/lib/daxauth
INSTALL_VAULT_FILE="$INSTALL_VAULT_DIR/vault.bin"
INSTALL_CONFIG_DIR=/etc/dax-auth
INSTALL_CONFIG_FILE="$INSTALL_CONFIG_DIR/config.toml"
INSTALL_SECRET_FILE="$INSTALL_CONFIG_DIR/secret"

detect_distro() {
    if [[ -f /etc/os-release ]]; then
        # shellcheck disable=SC1091
        . /etc/os-release
        DISTRO_ID="${ID:-unknown}"
        DISTRO_NAME="${PRETTY_NAME:-$DISTRO_ID}"
    else
        DISTRO_ID="unknown"
        DISTRO_NAME="unknown Linux"
    fi

    case "$DISTRO_ID" in
        fedora|rhel|centos|rocky|almalinux)
            DISTRO_FAMILY=fedora
            PKG_MGR=dnf
            PKG_INSTALL="dnf install -y"
            SECURITY_DIR=/usr/lib64/security
            PAM_DEV_PKG=pam-devel
            EXTRA_PKGS=(pam-devel v4l-utils pamtester gcc cmake openssl-devel) ;;
        debian|ubuntu|pop|linuxmint|elementary|raspbian)
            DISTRO_FAMILY=debian
            PKG_MGR=apt
            PKG_INSTALL="apt-get install -y"
            SECURITY_DIR=/lib/x86_64-linux-gnu/security
            [[ -d /lib/aarch64-linux-gnu/security ]] && SECURITY_DIR=/lib/aarch64-linux-gnu/security
            PAM_DEV_PKG=libpam0g-dev
            EXTRA_PKGS=(libpam0g-dev v4l-utils pamtester build-essential pkg-config libssl-dev) ;;
        arch|manjaro|endeavouros|garuda)
            DISTRO_FAMILY=arch
            PKG_MGR=pacman
            PKG_INSTALL="pacman -S --noconfirm"
            SECURITY_DIR=/usr/lib/security
            PAM_DEV_PKG=pam
            EXTRA_PKGS=(pam v4l-utils pamtester base-devel) ;;
        opensuse-leap|opensuse-tumbleweed|sles|suse)
            DISTRO_FAMILY=suse
            PKG_MGR=zypper
            PKG_INSTALL="zypper install -y"
            SECURITY_DIR=/lib64/security
            PAM_DEV_PKG=pam-devel
            EXTRA_PKGS=(pam-devel v4l-utils pamtester gcc cmake libopenssl-devel) ;;
        alpine)
            DISTRO_FAMILY=alpine
            PKG_MGR=apk
            PKG_INSTALL="apk add"
            SECURITY_DIR=/lib/security
            PAM_DEV_PKG=linux-pam-dev
            EXTRA_PKGS=(linux-pam-dev v4l-utils build-base) ;;
        *)
            DISTRO_FAMILY=unknown
            for candidate in /usr/lib64/security /lib/x86_64-linux-gnu/security /usr/lib/security /lib64/security /lib/security; do
                if [[ -d "$candidate" ]]; then
                    SECURITY_DIR="$candidate"
                    break
                fi
            done ;;
    esac

    [[ -d "$SECURITY_DIR" ]] || abort "Could not locate the PAM security directory. Install your distro's PAM development package and re-run."
    INSTALL_LIB="$SECURITY_DIR/libdax_pam.so"
}

detect_hardware() {
    HW_RGB_DEVICES=()
    HW_IR_DEVICES=()
    HW_NON_CAPTURE=()
    if ! command -v v4l2-ctl >/dev/null 2>&1; then
        warn "v4l2-ctl not available; skipping hardware detection."
        warn "Install v4l-utils to let the installer probe your cameras."
        return 0
    fi

    local node desc formats fourccs is_ir has_color has_grey
    for node in /dev/video*; do
        [[ -c "$node" ]] || continue

        # Skip nodes that are not Video Capture (metadata / output).
        if ! v4l2-ctl --device="$node" --list-formats 2>/dev/null | grep -q "Type: Video Capture"; then
            HW_NON_CAPTURE+=("$node")
            continue
        fi

        # `awk -F': '` truncates "ASUS FHD webcam: ASUS IR camera" at the
        # first colon; sed keeps everything after `Card type :`.
        desc="$(v4l2-ctl --device="$node" --info 2>/dev/null \
            | sed -n 's/^[[:space:]]*Card type[[:space:]]*:[[:space:]]*//p' | head -n1)"
        desc="${desc:-unknown}"

        # FourCCs the device exposes (e.g. GREY, YUYV, MJPG).
        fourccs="$(v4l2-ctl --device="$node" --list-formats 2>/dev/null \
            | awk -F"'" '/\[[0-9]+\]:/ {print $2}' | tr '\n' ' ' | sed 's/[[:space:]]\+$//')"
        if [[ -z "$fourccs" ]]; then
            # Companion / metadata node: keep it visible so the user
            # understands why the index is "skipped".
            if grep -qiE '\b(ir|infrared)\b' <<<"$desc"; then
                HW_NON_CAPTURE+=("$node ($desc, no streamable formats)")
            else
                HW_NON_CAPTURE+=("$node ($desc, no streamable formats)")
            fi
            continue
        fi
        formats="$(echo "$fourccs" | tr ' ' ',')"

        # Heuristics, in priority order:
        #   1. Description mentions "IR" / "infrared".
        #   2. Only grayscale formats exposed (typical of Hello-class IR sensors).
        is_ir=0
        if grep -qiE '\b(ir|infrared)\b' <<<"$desc"; then
            is_ir=1
        else
            has_color=0; has_grey=0
            for fcc in $fourccs; do
                case "$fcc" in
                    GREY|Y8|Y16|Y10) has_grey=1 ;;
                    YUYV|MJPG|NV12|RGB*|BGR*|UYVY|YV12|YU12|H264) has_color=1 ;;
                esac
            done
            if (( has_grey == 1 && has_color == 0 )); then
                is_ir=1
            fi
        fi

        if (( is_ir == 1 )); then
            HW_IR_DEVICES+=("$node|$desc|$formats")
        else
            HW_RGB_DEVICES+=("$node|$desc|$formats")
        fi
    done
}

print_banner() {
    cat <<EOF
${BOLD}${MAGENTA}┌──────────────────────────────────────────────────┐${RESET}
${BOLD}${MAGENTA}│             dax-auth interactive installer       │${RESET}
${BOLD}${MAGENTA}└──────────────────────────────────────────────────┘${RESET}
${DIM}repo : $REPO_ROOT${RESET}
${DIM}log  : $LOG_FILE${RESET}

EOF
}

print_environment() {
    heading "Environment"
    note "Distribution    : $DISTRO_NAME ($DISTRO_ID, family=$DISTRO_FAMILY)"
    note "Package manager : ${PKG_MGR:-?}"
    note "PAM directory   : $SECURITY_DIR"
    if (( ${#HW_RGB_DEVICES[@]} > 0 )); then
        ok "RGB cameras (${#HW_RGB_DEVICES[@]}):"
        local IFS='|'; local node desc fmt
        for d in "${HW_RGB_DEVICES[@]}"; do
            read -r node desc fmt <<<"$d"
            note "  $node  ·  $desc  ·  formats=$fmt"
        done
        unset IFS
    else
        warn "No RGB camera detected. The pipeline will not work without one."
    fi
    if (( ${#HW_IR_DEVICES[@]} > 0 )); then
        ok "IR cameras (${#HW_IR_DEVICES[@]}):"
        local IFS='|'; local node desc fmt
        for d in "${HW_IR_DEVICES[@]}"; do
            read -r node desc fmt <<<"$d"
            note "  $node  ·  $desc  ·  formats=$fmt"
        done
        unset IFS
        note "The pipeline runs RGB-only today; IR is recorded in config for future cross-check."
    else
        note "No IR sensor detected — the pipeline runs RGB-only, that's fine."
    fi
    if (( ${#HW_NON_CAPTURE[@]} > 0 )); then
        note "Non-capture / companion nodes (skipped):"
        for n in "${HW_NON_CAPTURE[@]}"; do note "  $n"; done
    fi
}

# ─────────────────────────── prerequisites ────────────────────────────
check_rust() {
    if ! command -v cargo >/dev/null 2>&1; then
        err "cargo not found in PATH."
        note "Install via https://rustup.rs/ — typical command:"
        note "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
        return 1
    fi
    ok "cargo $(cargo --version | awk '{print $2}')"
}

offer_distro_deps() {
    if [[ -z "$PKG_MGR" ]]; then
        warn "Unknown distro — install the PAM development package and v4l-utils manually."
        return 0
    fi
    heading "System prerequisites"
    note "Recommended packages on $DISTRO_NAME:"
    for p in "${EXTRA_PKGS[@]}"; do note "  - $p"; done
    if confirm "Install them now via $PKG_MGR?"; then
        # shellcheck disable=SC2086
        run_root $PKG_INSTALL "${EXTRA_PKGS[@]}" || warn "Some packages failed to install (see above)."
    else
        note "Skipped. Make sure '$PAM_DEV_PKG' is present, otherwise the cdylib will not load."
    fi
}

ensure_release_built() {
    if [[ -x "$RELEASE_BIN" && -f "$RELEASE_LIB" ]]; then
        ok "Release artefacts already built."
        return 0
    fi
    warn "Release binary or cdylib missing."
    if confirm "Build them now (cargo build --release -p dax-cli -p dax-pam)?"; then
        (cd "$REPO_ROOT" && cargo build --release -p dax-cli -p dax-pam) || abort "Cargo build failed."
        ok "Build complete."
    else
        abort "Cannot install without the release artefacts."
    fi
}

ensure_models() {
    local need=0
    for f in buffalo_s/det_500m.onnx buffalo_s/w600k_mbf.onnx liveness/MiniFASNetV2.onnx; do
        [[ -f "$MODELS_DIR/$f" ]] || need=1
    done
    if [[ $need -eq 0 ]]; then
        ok "ONNX models already fetched."
        return 0
    fi
    warn "Some ONNX models are missing under $MODELS_DIR."
    if confirm "Run scripts/fetch-models.sh now?"; then
        "$REPO_ROOT/scripts/fetch-models.sh" || abort "fetch-models.sh failed."
    else
        abort "Cannot install without the models."
    fi
}

# ─────────────────────────── install / config ────────────────────────────
generate_config() {
    local rgb_dev=0 ir_line="# ir_device = 2   # no IR sensor was detected; uncomment when you add one"
    if (( ${#HW_RGB_DEVICES[@]} > 0 )); then
        rgb_dev="$(echo "${HW_RGB_DEVICES[0]}" | awk -F'|' '{print $1}' | sed 's|/dev/video||')"
    fi
    if (( ${#HW_IR_DEVICES[@]} > 0 )); then
        local ir_dev
        ir_dev="$(echo "${HW_IR_DEVICES[0]}" | awk -F'|' '{print $1}' | sed 's|/dev/video||')"
        ir_line="ir_device = $ir_dev"
    fi
    cat <<EOF
# Generated by scripts/install.sh
# Edit and re-run the installer (Verify) to validate.

[paths]
vault     = "$INSTALL_VAULT_FILE"
detector  = "$INSTALL_SHARE/models/buffalo_s/det_500m.onnx"
recognizer = "$INSTALL_SHARE/models/buffalo_s/w600k_mbf.onnx"
liveness  = "$INSTALL_SHARE/models/liveness/MiniFASNetV2.onnx"

[camera]
rgb_device = $rgb_dev
$ir_line

[security]
match_threshold = 0.6
EOF
}

generate_secret() {
    # 32 bytes, base64. Good enough for a vault passphrase.
    if command -v openssl >/dev/null 2>&1; then
        openssl rand -base64 32 | tr -d '\n'
    else
        head -c 32 /dev/urandom | base64 | tr -d '\n'
    fi
}

action_install() {
    check_rust || abort "Install Rust first."
    offer_distro_deps
    ensure_release_built
    ensure_models

    heading "Installation plan"
    cat <<EOF
  Files to install:
    $RELEASE_BIN        →  $INSTALL_BIN                      (0755 root:root)
    $RELEASE_LIB        →  $INSTALL_LIB                      (0755 root:root)
    $MODELS_DIR/        →  $INSTALL_SHARE/models/            (0644)
    new config          →  $INSTALL_CONFIG_FILE              (0644 root:root)
    new random secret   →  $INSTALL_SECRET_FILE              (0600 root:root)
    new vault dir       →  $INSTALL_VAULT_DIR/               (0700 root:root)

  This step does NOT touch /etc/pam.d/* — the PAM service integration
  is a separate menu so you can run it with a recovery shell open.

EOF
    confirm "Proceed with the install?" || { warn "Aborted."; return 0; }

    substep "Copying CLI binary"
    run_root install -m 0755 -D "$RELEASE_BIN" "$INSTALL_BIN"

    substep "Copying PAM module"
    run_root install -m 0755 -D "$RELEASE_LIB" "$INSTALL_LIB"

    substep "Copying models"
    run_root install -d -m 0755 "$INSTALL_SHARE/models/buffalo_s" "$INSTALL_SHARE/models/liveness"
    for f in buffalo_s/det_500m.onnx buffalo_s/w600k_mbf.onnx liveness/MiniFASNetV2.onnx; do
        run_root install -m 0644 "$MODELS_DIR/$f" "$INSTALL_SHARE/models/$f"
    done

    substep "Creating $INSTALL_CONFIG_DIR"
    run_root install -d -m 0755 "$INSTALL_CONFIG_DIR"

    if [[ -f "$INSTALL_CONFIG_FILE" ]]; then
        warn "$INSTALL_CONFIG_FILE already exists; not overwriting."
    else
        substep "Writing $INSTALL_CONFIG_FILE"
        generate_config | run_root tee "$INSTALL_CONFIG_FILE" >/dev/null
        run_root chmod 0644 "$INSTALL_CONFIG_FILE"
    fi

    if [[ -f "$INSTALL_SECRET_FILE" ]]; then
        warn "$INSTALL_SECRET_FILE already exists; not overwriting."
    else
        substep "Generating $INSTALL_SECRET_FILE (random 32-byte base64)"
        local secret
        secret="$(generate_secret)"
        printf '%s\n' "$secret" | run_root tee "$INSTALL_SECRET_FILE" >/dev/null
        run_root chmod 0600 "$INSTALL_SECRET_FILE"
        run_root chown root:root "$INSTALL_SECRET_FILE"
    fi

    substep "Creating vault directory"
    run_root install -d -m 0700 -o root -g root "$INSTALL_VAULT_DIR"

    ok "Files in place."
    action_verify || true
    print_post_install
}

print_post_install() {
    heading "Next steps"
    cat <<EOF
  Now that the binary, the PAM module, the models, the config and the
  secret are in place, the CLI runs without any flags or environment
  variables. The user defaults to whoever invoked sudo (\$SUDO_USER),
  paths come from $INSTALL_CONFIG_FILE, the passphrase from
  $INSTALL_SECRET_FILE.

  1) Enrol your face (5 captures, move slightly between them):

       sudo daxauth enroll

  2) Verify (must succeed before wiring PAM):

       sudo daxauth verify

  3) When step 2 prints "MATCH", come back and run this script again,
     pick option 2 ("Configure PAM service"). It backs up the file,
     inserts the dax-auth line, and offers a pamtester smoke test.

  Tip: every flag (--user, --vault, --detector, …) is still accepted
  if you want to override the defaults from the config file.
EOF
}

# ─────────────────────────── PAM service integration ────────────────────────────
PAM_LINE_TAG="# dax-auth (auto-installed)"

list_pam_services() {
    local svc
    [[ -d /etc/pam.d ]] || abort "/etc/pam.d does not exist on this system."
    while IFS= read -r svc; do
        printf "%s\n" "$(basename "$svc")"
    done < <(find /etc/pam.d -maxdepth 1 -type f | sort)
}

backup_pam_file() {
    local svc_path="$1"
    local backup
    backup="${svc_path}.bak.$(date +%Y%m%d-%H%M%S)"
    run_root cp "$svc_path" "$backup"
    ok "Backup: $backup"
    echo "$backup"
}

action_configure_pam() {
    heading "Configure a PAM service"
    note "Pick a service in /etc/pam.d/ to wire dax-auth into."
    note "Common picks: sudo, login, gdm-password, kde-screensaver, sshd."
    note "Anything else can be typed as a path."
    echo ""
    local choice
    choice="$(ask "Service name (or full path)" "sudo")"

    local svc_path
    if [[ "$choice" == /* ]]; then
        svc_path="$choice"
    else
        svc_path="/etc/pam.d/$choice"
    fi
    [[ -f "$svc_path" ]] || abort "$svc_path does not exist."

    if grep -Fq "$PAM_LINE_TAG" "$svc_path"; then
        warn "$svc_path already contains a dax-auth line."
        if confirm "Remove it?"; then
            substep "Removing dax-auth lines from $svc_path"
            backup_pam_file "$svc_path" >/dev/null
            run_root sed -i "/$PAM_LINE_TAG/,+1d" "$svc_path"
            ok "Removed. Backup created beside the original."
        fi
        return 0
    fi

    note "Current first 10 lines of $svc_path:"
    sed -n '1,10p' "$svc_path" | sed 's/^/    /'
    echo ""

    local mode
    mode="$(ask "Use 'sufficient' (recommended, password fallback stays) or 'required'?" "sufficient")"
    if [[ "$mode" != "sufficient" && "$mode" != "required" ]]; then
        abort "Invalid mode: $mode (must be sufficient or required)"
    fi
    if [[ "$mode" == "required" ]]; then
        warn "'required' means the user CANNOT log in if face auth fails."
        warn "Make absolutely sure you have a recovery shell open before continuing."
        confirm "Really use 'required'?" || { warn "Aborted."; return 0; }
    fi

    local backup
    backup="$(backup_pam_file "$svc_path")"
    substep "Inserting line at top of $svc_path"
    local tmp
    tmp="$(mktemp)"
    {
        printf '%s\n' "$PAM_LINE_TAG"
        printf 'auth %s %s\n' "$mode" "$INSTALL_LIB"
        cat "$svc_path"
    } >"$tmp"
    run_root install -m "$(stat -c %a "$svc_path")" -o root -g root "$tmp" "$svc_path"
    rm -f "$tmp"
    ok "Wrote new $svc_path."
    note "Rollback if anything breaks:  sudo cp '$backup' '$svc_path'"

    if confirm "Smoke-test with pamtester now (recommended)?"; then
        if ! command -v pamtester >/dev/null 2>&1; then
            warn "pamtester not installed; skipping the smoke test."
            return 0
        fi
        substep "Running: pamtester $(basename "$svc_path") $USER authenticate"
        if pamtester -v "$(basename "$svc_path")" "$USER" authenticate; then
            ok "PAM authentication succeeded."
        else
            err "PAM authentication failed."
            note "Restore the previous file with:"
            note "  sudo cp '$backup' '$svc_path'"
        fi
    fi
}

# ─────────────────────────── verify / uninstall ────────────────────────────
action_verify() {
    heading "Verifying installation"
    local errors=0

    if [[ -x "$INSTALL_BIN" ]]; then
        ok "daxauth binary at $INSTALL_BIN"
    else
        warn "daxauth binary missing"; errors=$((errors+1))
    fi

    if [[ -f "$INSTALL_LIB" ]]; then
        local syms
        syms=$(nm -D --defined-only "$INSTALL_LIB" 2>/dev/null | grep -c pam_sm || true)
        if [[ "$syms" -ge 6 ]]; then
            ok "PAM cdylib at $INSTALL_LIB (PAM symbols: $syms)"
        else
            warn "Only $syms PAM symbols exported in $INSTALL_LIB (expected 6)"
            errors=$((errors+1))
        fi
    else
        warn "PAM cdylib missing"; errors=$((errors+1))
    fi

    for f in models/buffalo_s/det_500m.onnx models/buffalo_s/w600k_mbf.onnx models/liveness/MiniFASNetV2.onnx; do
        if [[ -f "$INSTALL_SHARE/$f" ]]; then
            ok "Model: $f"
        else
            warn "Missing model: $INSTALL_SHARE/$f"; errors=$((errors+1))
        fi
    done

    if [[ -f "$INSTALL_CONFIG_FILE" ]]; then
        ok "Config: $INSTALL_CONFIG_FILE"
    else
        warn "Config file missing"; errors=$((errors+1))
    fi

    if [[ -f "$INSTALL_SECRET_FILE" ]]; then
        local mode
        mode=$(stat -c %a "$INSTALL_SECRET_FILE" 2>/dev/null || echo ?)
        if [[ "$mode" == "600" ]]; then
            ok "Secret file with 0600 perms"
        else
            warn "Secret file perms are $mode (expected 600)"
            errors=$((errors+1))
        fi
    else
        warn "Secret file missing"; errors=$((errors+1))
    fi

    if [[ -d "$INSTALL_VAULT_DIR" ]]; then
        ok "Vault directory $INSTALL_VAULT_DIR"
    else
        warn "Vault directory missing"; errors=$((errors+1))
    fi

    echo ""
    if [[ $errors -eq 0 ]]; then
        printf "  %s%s All good.%s\n" "$GREEN" "$BOLD" "$RESET"
        return 0
    else
        printf "  %s%s %d issue(s) found.%s\n" "$YELLOW" "$BOLD" "$errors" "$RESET"
        return 1
    fi
}

action_uninstall() {
    heading "Uninstall"
    cat <<EOF
  Will remove:
    $INSTALL_BIN
    $INSTALL_LIB
    $INSTALL_SHARE/         (entire models tree)
    $INSTALL_CONFIG_DIR/    (config + secret)

  /etc/pam.d/* lines added by this installer can be reverted by
  running this script again and picking "Configure PAM service".
  Backups (.bak.YYYYMMDD-HHMMSS) are NOT touched here.
EOF
    confirm "Proceed with uninstall?" || { warn "Aborted."; return 0; }

    substep "Removing files"
    run_root rm -f "$INSTALL_BIN" "$INSTALL_LIB"
    run_root rm -rf "$INSTALL_SHARE" "$INSTALL_CONFIG_DIR"
    ok "Removed."

    if [[ -d "$INSTALL_VAULT_DIR" ]] && confirm "Also delete the vault directory $INSTALL_VAULT_DIR (templates will be lost)?"; then
        run_root rm -rf "$INSTALL_VAULT_DIR"
        ok "Vault directory removed."
    fi
}

# ─────────────────────────── main menu ────────────────────────────
main_menu() {
    print_banner
    detect_distro
    detect_hardware
    print_environment

    while true; do
        heading "Main menu"
        echo "  1) Install dax-auth"
        echo "  2) Configure PAM service (add or remove)"
        echo "  3) Verify installation"
        echo "  4) Uninstall"
        echo "  5) Quit"
        local choice
        choice="$(ask "Choice" "5")"
        case "$choice" in
            1) action_install ;;
            2) action_configure_pam ;;
            3) action_verify || true ;;
            4) action_uninstall ;;
            5) ok "Bye."; exit 0 ;;
            *) warn "Unknown choice: $choice" ;;
        esac
    done
}

main_menu
