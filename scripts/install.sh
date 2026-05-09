#!/usr/bin/env bash
# Interactive installer for dax-auth. Detects your distribution, asks
# what to do, shows the plan, and only acts after explicit confirmation.
#
# Run from the repo root:
#   ./scripts/install.sh
#
# It never modifies /etc/pam.d/sudo. The PAM line for production use is
# printed at the end so you add it yourself with a recovery shell open.

set -euo pipefail

# ──────────────── styling ────────────────
BOLD=$(tput bold 2>/dev/null || true)
DIM=$(tput dim 2>/dev/null || true)
RESET=$(tput sgr0 2>/dev/null || true)
RED=$(tput setaf 1 2>/dev/null || true)
GREEN=$(tput setaf 2 2>/dev/null || true)
YELLOW=$(tput setaf 3 2>/dev/null || true)
CYAN=$(tput setaf 6 2>/dev/null || true)

heading()  { printf "\n%s==>%s %s%s%s\n" "$CYAN" "$RESET" "$BOLD" "$*" "$RESET"; }
step()     { printf " %s-->%s %s\n" "$CYAN" "$RESET" "$*"; }
ok()       { printf " %s[ok]%s %s\n" "$GREEN" "$RESET" "$*"; }
warn()     { printf " %s[!!]%s %s\n" "$YELLOW" "$RESET" "$*"; }
fail()     { printf " %s[xx]%s %s\n" "$RED" "$RESET" "$*" >&2; exit 1; }
note()     { printf "    %s%s%s\n" "$DIM" "$*" "$RESET"; }

ask() {
    # ask "Question?" "default" -> stdout the answer
    local prompt="$1" default="${2:-}" reply
    if [[ -n "$default" ]]; then
        read -r -p "$(printf "%s%s%s [%s]: " "$BOLD" "$prompt" "$RESET" "$default")" reply
        echo "${reply:-$default}"
    else
        read -r -p "$(printf "%s%s%s: " "$BOLD" "$prompt" "$RESET")" reply
        echo "$reply"
    fi
}

confirm() {
    # confirm "Proceed?" -> 0 if yes, 1 if no
    local reply
    reply="$(ask "$1 [y/N]" "N")"
    [[ "${reply,,}" == "y" || "${reply,,}" == "yes" ]]
}

run_root() {
    # Wrap a command in sudo only if not already root.
    if [[ $EUID -eq 0 ]]; then
        "$@"
    else
        sudo "$@"
    fi
}

# ──────────────── detection ────────────────
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RELEASE_BIN="$REPO_ROOT/target/release/daxauth"
RELEASE_LIB="$REPO_ROOT/target/release/libdax_pam.so"
MODELS_DIR="$REPO_ROOT/models"

DISTRO_ID=""
SECURITY_DIR=""

detect_distro() {
    if [[ -f /etc/os-release ]]; then
        # shellcheck disable=SC1091
        . /etc/os-release
        DISTRO_ID="${ID:-unknown}"
    else
        DISTRO_ID="unknown"
    fi
    case "$DISTRO_ID" in
        fedora|rhel|centos|rocky|almalinux)
            SECURITY_DIR=/usr/lib64/security ;;
        debian|ubuntu|pop|linuxmint)
            SECURITY_DIR=/lib/x86_64-linux-gnu/security ;;
        arch|manjaro|endeavouros)
            SECURITY_DIR=/usr/lib/security ;;
        *)
            for candidate in /usr/lib64/security /lib/x86_64-linux-gnu/security /usr/lib/security; do
                if [[ -d "$candidate" ]]; then
                    SECURITY_DIR="$candidate"
                    break
                fi
            done ;;
    esac
    [[ -d "$SECURITY_DIR" ]] || fail "Could not locate the PAM security directory; install libpam-dev / pam-devel?"
}

# ──────────────── prerequisites ────────────────
require_built() {
    [[ -x "$RELEASE_BIN" ]] || {
        warn "Release binary not found: $RELEASE_BIN"
        if confirm "Run 'cargo build --release -p dax-cli -p dax-pam' now?"; then
            (cd "$REPO_ROOT" && cargo build --release -p dax-cli -p dax-pam)
        else
            fail "Need release artefacts before installing."
        fi
    }
    [[ -f "$RELEASE_LIB" ]] || fail "Missing $RELEASE_LIB after build."
}

require_models() {
    if [[ ! -f "$MODELS_DIR/buffalo_s/det_500m.onnx" \
        || ! -f "$MODELS_DIR/buffalo_s/w600k_mbf.onnx" \
        || ! -f "$MODELS_DIR/liveness/MiniFASNetV2.onnx" ]]; then
        warn "Some ONNX models are missing under $MODELS_DIR"
        if confirm "Run scripts/fetch-models.sh now?"; then
            "$REPO_ROOT/scripts/fetch-models.sh"
        else
            fail "Need models before installing."
        fi
    fi
}

# ──────────────── actions ────────────────
INSTALL_BIN_DIR=/usr/local/bin
INSTALL_SHARE_DIR=/usr/share/daxauth
INSTALL_VAULT_DIR=/var/lib/daxauth

print_install_plan() {
    heading "Installation plan"
    note "Distribution : $DISTRO_ID"
    note "PAM dir      : $SECURITY_DIR"
    cat <<EOF
    Will install:
      - $RELEASE_BIN            -> $INSTALL_BIN_DIR/daxauth                  (mode 0755)
      - $RELEASE_LIB            -> $SECURITY_DIR/libdax_pam.so               (mode 0755)
      - $MODELS_DIR/buffalo_s/  -> $INSTALL_SHARE_DIR/models/buffalo_s/      (mode 0644)
      - $MODELS_DIR/liveness/   -> $INSTALL_SHARE_DIR/models/liveness/       (mode 0644)
      - mkdir                   -> $INSTALL_VAULT_DIR/                       (mode 0700, root:root)
EOF
    note "It does NOT touch /etc/pam.d/sudo or any other production stack."
}

do_install() {
    heading "Installing"

    step "Copying CLI binary"
    run_root install -m 0755 -D "$RELEASE_BIN" "$INSTALL_BIN_DIR/daxauth"

    step "Copying PAM module"
    run_root install -m 0755 -D "$RELEASE_LIB" "$SECURITY_DIR/libdax_pam.so"

    step "Copying models"
    run_root install -d -m 0755 "$INSTALL_SHARE_DIR/models/buffalo_s" "$INSTALL_SHARE_DIR/models/liveness"
    run_root install -m 0644 "$MODELS_DIR/buffalo_s/det_500m.onnx"   "$INSTALL_SHARE_DIR/models/buffalo_s/det_500m.onnx"
    run_root install -m 0644 "$MODELS_DIR/buffalo_s/w600k_mbf.onnx"  "$INSTALL_SHARE_DIR/models/buffalo_s/w600k_mbf.onnx"
    run_root install -m 0644 "$MODELS_DIR/liveness/MiniFASNetV2.onnx" "$INSTALL_SHARE_DIR/models/liveness/MiniFASNetV2.onnx"

    step "Creating vault directory ($INSTALL_VAULT_DIR)"
    run_root install -d -m 0700 -o root -g root "$INSTALL_VAULT_DIR"

    ok "Files in place."
}

print_post_install() {
    heading "Next steps"
    cat <<EOF
1) Pick a passphrase and export it for both shells where you enrol and verify:
       export DAX_VAULT_PASSPHRASE='choose-something-random'

2) Enrol your face (CLI uses the system models and vault dir):
       sudo -E daxauth enroll \\
           --user "\$USER" --vault $INSTALL_VAULT_DIR/vault.bin \\
           --captures 5 --device 0 \\
           --detector       $INSTALL_SHARE_DIR/models/buffalo_s/det_500m.onnx \\
           --recognizer     $INSTALL_SHARE_DIR/models/buffalo_s/w600k_mbf.onnx \\
           --liveness-model $INSTALL_SHARE_DIR/models/liveness/MiniFASNetV2.onnx

3) Verify (must succeed):
       sudo -E daxauth verify \\
           --user "\$USER" --vault $INSTALL_VAULT_DIR/vault.bin --device 0 \\
           --detector       $INSTALL_SHARE_DIR/models/buffalo_s/det_500m.onnx \\
           --recognizer     $INSTALL_SHARE_DIR/models/buffalo_s/w600k_mbf.onnx \\
           --liveness-model $INSTALL_SHARE_DIR/models/liveness/MiniFASNetV2.onnx

4) Smoke test the PAM module without touching sudo:
       DAX_VAULT_PATH=$INSTALL_VAULT_DIR/vault.bin \\
       DAX_VAULT_PASSPHRASE="\$DAX_VAULT_PASSPHRASE" \\
       TARGET_USER="\$USER" \\
       ./scripts/pamtest.sh

5) Only after step 4 prints 'successfully authenticated' should you wire it
   into a real PAM stack. Open a root shell in another terminal first, then
   prepend a single line to /etc/pam.d/sudo (or another service):

       auth sufficient $SECURITY_DIR/libdax_pam.so

   Keep the existing 'auth include system-auth' (or equivalent) right below
   so password authentication is still available as fallback.
EOF
}

do_uninstall() {
    heading "Uninstall plan"
    cat <<EOF
    Will remove:
      - $INSTALL_BIN_DIR/daxauth
      - $SECURITY_DIR/libdax_pam.so
      - $INSTALL_SHARE_DIR/      (entire tree)
      - /etc/pam.d/daxauth-test  (if present, used by pamtest.sh)

    The vault directory $INSTALL_VAULT_DIR is NOT removed automatically;
    enrolled templates would be lost.
EOF
    confirm "Proceed with uninstall?" || { warn "Aborted."; return 0; }

    run_root rm -f "$INSTALL_BIN_DIR/daxauth"
    run_root rm -f "$SECURITY_DIR/libdax_pam.so"
    run_root rm -rf "$INSTALL_SHARE_DIR"
    run_root rm -f /etc/pam.d/daxauth-test
    ok "Uninstalled."

    if [[ -d "$INSTALL_VAULT_DIR" ]] && confirm "Also delete the vault directory $INSTALL_VAULT_DIR?"; then
        run_root rm -rf "$INSTALL_VAULT_DIR"
        ok "Vault directory removed."
    fi
}

do_verify() {
    heading "Verifying installation"
    local errors=0

    if [[ -x "$INSTALL_BIN_DIR/daxauth" ]]; then
        ok "daxauth binary at $INSTALL_BIN_DIR/daxauth"
    else
        warn "daxauth binary missing"; errors=$((errors+1))
    fi

    if [[ -f "$SECURITY_DIR/libdax_pam.so" ]]; then
        ok "PAM module at $SECURITY_DIR/libdax_pam.so"
        local syms
        syms=$(nm -D --defined-only "$SECURITY_DIR/libdax_pam.so" 2>/dev/null | grep -c pam_sm || true)
        if [[ "$syms" -ge 6 ]]; then
            ok "$syms PAM hook symbols exported"
        else
            warn "Only $syms PAM symbols exported (expected 6)"
            errors=$((errors+1))
        fi
    else
        warn "PAM module missing"; errors=$((errors+1))
    fi

    for f in models/buffalo_s/det_500m.onnx models/buffalo_s/w600k_mbf.onnx models/liveness/MiniFASNetV2.onnx; do
        if [[ -f "$INSTALL_SHARE_DIR/$f" ]]; then
            ok "Model present: $f"
        else
            warn "Missing model: $INSTALL_SHARE_DIR/$f"; errors=$((errors+1))
        fi
    done

    if [[ -d "$INSTALL_VAULT_DIR" ]]; then
        ok "Vault directory $INSTALL_VAULT_DIR (perms $(stat -c %a "$INSTALL_VAULT_DIR" 2>/dev/null || echo ?))"
    else
        warn "Vault directory missing"; errors=$((errors+1))
    fi

    if [[ $errors -eq 0 ]]; then
        printf "\n%s%s All good.%s\n" "$GREEN" "$BOLD" "$RESET"
    else
        printf "\n%s%s %d issue(s) found.%s\n" "$YELLOW" "$BOLD" "$errors" "$RESET"
        return 1
    fi
}

# ──────────────── main menu ────────────────
print_banner() {
    cat <<EOF
${BOLD}dax-auth installer${RESET}
${DIM}Repo: $REPO_ROOT${RESET}

This script does not modify /etc/pam.d/sudo. It only places the
binary, the PAM module, the models, and prepares the vault directory.

EOF
}

main_menu() {
    print_banner
    detect_distro
    note "Detected distro: $DISTRO_ID"
    note "PAM directory  : $SECURITY_DIR"
    echo ""
    echo "What do you want to do?"
    echo "  1) Install"
    echo "  2) Verify install"
    echo "  3) Uninstall"
    echo "  4) Quit"
    local choice
    choice="$(ask "Choice" "1")"
    case "$choice" in
        1)
            require_built
            require_models
            print_install_plan
            confirm "Proceed?" || { warn "Aborted."; exit 0; }
            do_install
            do_verify || true
            print_post_install ;;
        2) do_verify ;;
        3) do_uninstall ;;
        4) exit 0 ;;
        *) fail "Unknown choice: $choice" ;;
    esac
}

main_menu
