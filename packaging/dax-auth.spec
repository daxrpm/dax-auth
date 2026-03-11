Name:           dax-auth
Version:        0.1.0
Release:        1%{?dist}
Summary:        Facial authentication module for Linux (Windows Hello-style)
License:        GPL-3.0-or-later
URL:            https://github.com/daxrpm/dax-auth
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo >= 1.80
BuildRequires:  pam-devel
BuildRequires:  libv4l-devel

Requires:       pam
Requires:       libv4l
# ONNX Runtime shared library — install separately or via ort-rs bundle
Requires:       onnxruntime >= 1.17

%description
dax-auth provides Windows Hello-style facial recognition for Linux,
integrating with PAM to enable face authentication for login, sudo,
lock screens, and screensavers.

Features:
 - RetinaFace detection + ArcFace R100 recognition (ONNX)
 - MiniFASNetV2 2D anti-spoofing liveness detection
 - Hardware IR camera support
 - Secure (FAR ≤ 1e-4) and paranoid (FAR ≤ 1e-6) modes
 - GPU acceleration: ROCm, CUDA, OpenVINO (auto-detected)
 - Pure Rust — no Python runtime required

%prep
%autosetup

%build
cargo build --release --workspace

%install
install -d %{buildroot}%{_bindir}
install -d %{buildroot}%{_libdir}/security
install -d %{buildroot}%{_libdir}/dax-auth
install -d %{buildroot}%{_sysconfdir}/dax-auth
install -d %{buildroot}%{_unitdir}

install -m 755 target/release/dax-authd     %{buildroot}%{_bindir}/dax-authd
install -m 755 target/release/dax-auth      %{buildroot}%{_bindir}/dax-auth
install -m 644 target/release/libpam_dax_auth.so \
    %{buildroot}%{_libdir}/security/pam_dax_auth.so
install -m 640 config/config.toml \
    %{buildroot}%{_sysconfdir}/dax-auth/config.toml
install -m 644 packaging/pam-dax-auth.conf \
    %{buildroot}%{_sysconfdir}/dax-auth/pam-example.conf
install -m 755 scripts/setup-runtime-dir.sh \
    %{buildroot}%{_libdir}/dax-auth/setup-runtime-dir.sh
install -m 644 packaging/dax-authd.service \
    %{buildroot}%{_unitdir}/dax-authd.service

%pre
# Create system user and group
getent group dax-auth > /dev/null || groupadd --system dax-auth
getent passwd dax-auth > /dev/null || \
    useradd --system --no-create-home \
            --shell /sbin/nologin \
            --gid dax-auth \
            --comment "dax-auth facial authentication daemon" \
            dax-auth
usermod -a -G video dax-auth 2>/dev/null || true

%post
# Create data directories
install -d -m 750 -o dax-auth -g dax-auth /var/lib/dax-auth
install -d -m 750 -o dax-auth -g dax-auth /var/lib/dax-auth/models
install -d -m 700 -o dax-auth -g dax-auth /var/lib/dax-auth/users

# Generate master key if absent
if [ ! -f %{_sysconfdir}/dax-auth/master.key ]; then
    dd if=/dev/urandom bs=32 count=1 \
        of=%{_sysconfdir}/dax-auth/master.key 2>/dev/null
    chown root:dax-auth %{_sysconfdir}/dax-auth/master.key
    chmod 0640 %{_sysconfdir}/dax-auth/master.key
fi

%systemd_post dax-authd.service

echo ""
echo "dax-auth installed. Next steps:"
echo "  1. Download models and start daemon:"
echo "     sudo systemctl enable --now dax-authd"
echo "  2. Enroll your face:   dax-auth enroll"
echo "  3. Configure PAM:      see %{_sysconfdir}/dax-auth/pam-example.conf"

%preun
%systemd_preun dax-authd.service

%postun
%systemd_postun_with_restart dax-authd.service

%files
%license LICENSE
%doc README.md
%{_bindir}/dax-authd
%{_bindir}/dax-auth
%{_libdir}/security/pam_dax_auth.so
%{_libdir}/dax-auth/setup-runtime-dir.sh
%{_unitdir}/dax-authd.service
%config(noreplace) %{_sysconfdir}/dax-auth/config.toml
%{_sysconfdir}/dax-auth/pam-example.conf

%changelog
* Wed Mar 11 2026 dax-auth contributors <daxrpm@users.noreply.github.com> - 0.1.0-1
- Initial release: facial authentication daemon, PAM module, and CLI
- RetinaFace + ArcFace R100 + MiniFASNetV2 anti-spoofing pipeline
- Umeyama 5-point face alignment for improved recognition accuracy
- Secure (FAR ≤ 1e-4) and paranoid (FAR ≤ 1e-6) security modes
