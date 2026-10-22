Name:           portscope
Version:        0.1.0
Release:        1%{?dist}
Summary:        Local port, process and service inspector

License:        MIT
URL:            https://example.invalid/portscope
# No upstream tarball yet: the spec is built from a source tree with
# `rpmbuild -bb --define "_sourcedir <repo>" ...` (see scripts/build-rpm.sh).
Source0:        %{name}-%{version}.tar.gz

# The binary links against the system GTK/libadwaita; the crate feature gates
# are v4_14 / v1_5, so those are the real floors. RPM's automatic dependency
# generator adds the soname-level requirements itself.
BuildRequires:  gcc
BuildRequires:  gtk4-devel >= 4.14
BuildRequires:  libadwaita-devel >= 1.5
BuildRequires:  gettext
BuildRequires:  systemd-rpm-macros

Requires:       gtk4 >= 4.14
Requires:       libadwaita >= 1.5
# PortScope starts without these and reports the missing tool, so they must not
# be hard dependencies.
Recommends:     iproute
Recommends:     systemd

%description
PortScope shows which local TCP/UDP endpoints are listening, which process owns
each one, which systemd unit that process belongs to, and what the journal says
about it.

Every endpoint-to-service link carries the evidence that produced it: a socket
inode seen in the process file descriptor table, a cgroup match, a MainPID
match, socket activation via Triggers=, or a Listen= match. PortScope never
infers an association from a port number, a process name or a unit description.

It runs entirely as the invoking user: no root, no polkit, no group changes, no
network access and no telemetry. Some data, such as the system journal, may be
unavailable depending on your permissions, and the interface says so explicitly
rather than showing an empty result.

%prep
%setup -q

%build
# Reproducible: SOURCE_DATE_EPOCH is provided by rpmbuild/rpm macros. The remap
# keeps the build directory out of the binary.
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$(pwd)=/portscope"
cargo build --release --offline -p portscope-gtk -p portscope-cli

%install
install -d %{buildroot}%{_bindir}
install -m 755 target/release/portscope %{buildroot}%{_bindir}/portscope
install -m 755 target/release/portscope-cli %{buildroot}%{_bindir}/portscope-cli

install -d %{buildroot}%{_datadir}/applications
install -m 644 data/desktop/io.github.portscope.app.desktop \
    %{buildroot}%{_datadir}/applications/

install -d %{buildroot}%{_datadir}/metainfo
install -m 644 data/metainfo/io.github.portscope.app.metainfo.xml \
    %{buildroot}%{_datadir}/metainfo/

for s in 16 24 32 48 64 128 256; do
    d=%{buildroot}%{_datadir}/icons/hicolor/${s}x${s}/apps
    install -d "$d"
    install -m 644 data/icons/hicolor/${s}x${s}/apps/io.github.portscope.app.png "$d/"
done
install -d %{buildroot}%{_datadir}/icons/hicolor/scalable/apps
install -m 644 data/icons/hicolor/scalable/apps/io.github.portscope.app.svg \
    %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/

install -d %{buildroot}%{_datadir}/man/man1
gzip -9n -c data/man/portscope.1 > %{buildroot}%{_datadir}/man/man1/portscope.1.gz
gzip -9n -c data/man/portscope-cli.1 > %{buildroot}%{_datadir}/man/man1/portscope-cli.1.gz

# Translations. English is the source language.
for po in crates/gtk/po/*.po; do
    lang=$(basename "$po" .po)
    case "$lang" in en*) continue ;; esac
    d=%{buildroot}%{_datadir}/locale/$lang/LC_MESSAGES
    install -d "$d"
    msgfmt -o "$d/portscope.mo" "$po"
done

%files
%license LICENSE
%doc README.md
%{_bindir}/portscope
%{_bindir}/portscope-cli
%{_datadir}/applications/io.github.portscope.app.desktop
%{_datadir}/metainfo/io.github.portscope.app.metainfo.xml
%{_datadir}/icons/hicolor/*/apps/io.github.portscope.app.*
%{_datadir}/man/man1/portscope.1.gz
%{_datadir}/man/man1/portscope-cli.1.gz
%{_datadir}/locale/*/LC_MESSAGES/portscope.mo

%changelog
* Thu Oct 22 2026 PortScope contributors <noreply@example.invalid> - 0.1.0-1
- Initial packaged development snapshot.
