# RPM spec for HyprFetch — built in CI via rpmbuild (see .github/workflows/release.yml).
# The __VERSION__ placeholder is substituted with the release version at build time.
Name:           hyprfetch
Version:        __VERSION__
Release:        1%{?dist}
Summary:        Minimal-RAM, fast, resumable download manager with a web UI

License:        MIT
URL:            https://github.com/Local-DE-Coach/HyprFetch
Source0:        hyprfetch-%{version}-linux-x64.tar.gz
BuildArch:      x86_64

%description
HyprFetch is a minimal-RAM, fast, resumable internet download manager with a
browser-based control panel. A single Rust binary serves the web UI on
127.0.0.1:7780, runs segmented multi-connection downloads with byte-exact
resume (SQLite-backed), and enforces an optional engine-wide QoS bandwidth
cap. Idle RAM target: < 10 MB.

%prep
%setup -q -n hyprfetch-%{version}-x86_64-unknown-linux-gnu

%install
install -Dm755 hyprfetch                 %{buildroot}%{_bindir}/hyprfetch
install -Dm644 README.md                 %{buildroot}%{_docdir}/hyprfetch/README.md
install -Dm644 CHANGELOG.md              %{buildroot}%{_docdir}/hyprfetch/CHANGELOG.md
install -Dm644 LICENSE                   %{buildroot}%{_licensedir}/hyprfetch/LICENSE
install -Dm644 hyprfetch.desktop         %{buildroot}%{_datadir}/applications/hyprfetch.desktop

%files
%{_bindir}/hyprfetch
%{_datadir}/applications/hyprfetch.desktop
%doc %{_docdir}/hyprfetch/README.md
%doc %{_docdir}/hyprfetch/CHANGELOG.md
%license %{_licensedir}/hyprfetch/LICENSE

%changelog
* Tue Sep 30 2026 HyprFetch Contributors <Local-DE-Coach@users.noreply.github.com> - __VERSION__-1
- First RPM-packaged release (see upstream CHANGELOG.md for details).
