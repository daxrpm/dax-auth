# dax-auth Makefile
#
# Targets:
#   make build        — compile all crates in release mode
#   make install      — install daemon, PAM module, CLI, config, and systemd unit
#   make uninstall    — remove installed files (does NOT remove user data)
#   make setup-key    — generate /etc/dax-auth/master.key if absent
#   make models       — download ONNX model files
#   make test         — run all workspace tests
#   make lint         — cargo clippy
#   make audit        — cargo audit (security advisories)
#   make clean        — cargo clean

# ── Install paths ──────────────────────────────────────────────────────────────
PREFIX          ?= /usr
BINDIR          ?= $(PREFIX)/bin
LIBDIR          ?= $(PREFIX)/lib
SYSCONFDIR      ?= /etc
DATADIR         ?= /var/lib
SYSTEMD_UNIT    ?= /etc/systemd/system
PAM_LIBDIR      ?= $(LIBDIR)/security

DAX_USER        ?= dax-auth
DAX_GROUP       ?= dax-auth

INSTALL         ?= install
CARGO           ?= cargo

# Release binary paths (after `make build`)
TARGET_DIR      := target/release
DAEMON_BIN      := $(TARGET_DIR)/dax-authd
CLI_BIN         := $(TARGET_DIR)/dax-auth
PAM_SO          := $(TARGET_DIR)/libpam_dax_auth.so

# ── Build ──────────────────────────────────────────────────────────────────────

.PHONY: build
build: ## Compile all crates in release mode
	$(CARGO) build --release --workspace

# ── Install ───────────────────────────────────────────────────────────────────

.PHONY: install
install: build install-bin install-pam install-config install-systemd install-data-dirs ## Full install (requires root)
	@echo ""
	@echo "✓ dax-auth installed successfully."
	@echo ""
	@echo "Next steps:"
	@echo "  1. Download models:    make models"
	@echo "  2. Generate master key: make setup-key"
	@echo "  3. Enable daemon:      systemctl enable --now dax-authd"
	@echo "  4. Enroll your face:   dax-auth enroll"
	@echo "  5. Test recognition:   dax-auth test"
	@echo "  6. Configure PAM:      see /etc/dax-auth/pam-example.conf"

.PHONY: install-bin
install-bin: ## Install daemon binary and CLI tool
	$(INSTALL) -d -m 755 $(DESTDIR)$(BINDIR)
	$(INSTALL) -m 755 $(DAEMON_BIN) $(DESTDIR)$(BINDIR)/dax-authd
	$(INSTALL) -m 755 $(CLI_BIN) $(DESTDIR)$(BINDIR)/dax-auth

.PHONY: install-pam
install-pam: ## Install PAM module .so
	$(INSTALL) -d -m 755 $(DESTDIR)$(PAM_LIBDIR)
	$(INSTALL) -m 644 $(PAM_SO) $(DESTDIR)$(PAM_LIBDIR)/pam_dax_auth.so

.PHONY: install-config
install-config: ## Install default config and PAM example
	$(INSTALL) -d -m 755 $(DESTDIR)$(SYSCONFDIR)/dax-auth
	# Only install default config if absent (don't overwrite user config)
	[ -f $(DESTDIR)$(SYSCONFDIR)/dax-auth/config.toml ] || \
		$(INSTALL) -m 640 config/config.toml $(DESTDIR)$(SYSCONFDIR)/dax-auth/config.toml
	$(INSTALL) -m 644 packaging/pam-dax-auth.conf \
		$(DESTDIR)$(SYSCONFDIR)/dax-auth/pam-example.conf
	# Setup script used by the systemd unit
	$(INSTALL) -d -m 755 $(DESTDIR)$(LIBDIR)/dax-auth
	$(INSTALL) -m 755 scripts/setup-runtime-dir.sh \
		$(DESTDIR)$(LIBDIR)/dax-auth/setup-runtime-dir.sh

.PHONY: install-systemd
install-systemd: ## Install systemd service unit
	$(INSTALL) -d -m 755 $(DESTDIR)$(SYSTEMD_UNIT)
	$(INSTALL) -m 644 packaging/dax-authd.service \
		$(DESTDIR)$(SYSTEMD_UNIT)/dax-authd.service
	@if [ -z "$(DESTDIR)" ]; then \
		systemctl daemon-reload; \
	fi

.PHONY: install-data-dirs
install-data-dirs: ## Create data directories and system user
	# Create system user if it doesn't exist
	@if ! id $(DAX_USER) > /dev/null 2>&1; then \
		useradd --system --no-create-home \
			--shell /usr/sbin/nologin \
			--comment "dax-auth facial authentication daemon" \
			$(DAX_USER); \
		usermod -a -G video $(DAX_USER); \
		echo "✓ Created system user: $(DAX_USER)"; \
	fi
	# Model + user-embedding directories
	$(INSTALL) -d -m 750 -o $(DAX_USER) -g $(DAX_GROUP) \
		$(DESTDIR)$(DATADIR)/dax-auth
	$(INSTALL) -d -m 750 -o $(DAX_USER) -g $(DAX_GROUP) \
		$(DESTDIR)$(DATADIR)/dax-auth/models
	$(INSTALL) -d -m 700 -o $(DAX_USER) -g $(DAX_GROUP) \
		$(DESTDIR)$(DATADIR)/dax-auth/users

# ── Master key setup ──────────────────────────────────────────────────────────

.PHONY: setup-key
setup-key: ## Generate /etc/dax-auth/master.key (32 random bytes, owner root:dax-auth mode 0640)
	@if [ -f $(SYSCONFDIR)/dax-auth/master.key ]; then \
		echo "master.key already exists — skipping (delete manually to regenerate)"; \
	else \
		dd if=/dev/urandom bs=32 count=1 of=$(SYSCONFDIR)/dax-auth/master.key 2>/dev/null; \
		chown root:$(DAX_GROUP) $(SYSCONFDIR)/dax-auth/master.key; \
		chmod 0640 $(SYSCONFDIR)/dax-auth/master.key; \
		echo "✓ Master key generated: $(SYSCONFDIR)/dax-auth/master.key"; \
	fi

# ── Models ────────────────────────────────────────────────────────────────────

.PHONY: models
models: ## Download ONNX model files to /var/lib/dax-auth/models/
	bash scripts/download_models.sh $(DATADIR)/dax-auth/models

# ── Uninstall ─────────────────────────────────────────────────────────────────

.PHONY: uninstall
uninstall: ## Remove installed files (does NOT touch user data or master key)
	@echo "Stopping and disabling daemon..."
	-systemctl stop dax-authd 2>/dev/null || true
	-systemctl disable dax-authd 2>/dev/null || true
	rm -f $(DESTDIR)$(BINDIR)/dax-authd
	rm -f $(DESTDIR)$(BINDIR)/dax-auth
	rm -f $(DESTDIR)$(PAM_LIBDIR)/pam_dax_auth.so
	rm -f $(DESTDIR)$(SYSTEMD_UNIT)/dax-authd.service
	rm -f $(DESTDIR)$(LIBDIR)/dax-auth/setup-runtime-dir.sh
	-rmdir $(DESTDIR)$(LIBDIR)/dax-auth 2>/dev/null || true
	@if [ -z "$(DESTDIR)" ]; then systemctl daemon-reload; fi
	@echo "✓ dax-auth uninstalled."
	@echo "  Config: $(SYSCONFDIR)/dax-auth/ — preserved (remove manually)"
	@echo "  Data:   $(DATADIR)/dax-auth/    — preserved (contains user enrollments)"

# ── Development ───────────────────────────────────────────────────────────────

.PHONY: test
test: ## Run all workspace tests
	$(CARGO) test --workspace

.PHONY: lint
lint: ## Run clippy with deny warnings
	$(CARGO) clippy --workspace --all-targets -- -D warnings

.PHONY: audit
audit: ## Run cargo-audit for known security advisories
	$(CARGO) audit

.PHONY: clean
clean: ## Remove build artifacts
	$(CARGO) clean

# ── Help ──────────────────────────────────────────────────────────────────────

.PHONY: help
help: ## Show this help message
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}'

.DEFAULT_GOAL := help
