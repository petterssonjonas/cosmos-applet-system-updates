# COSMIC System Updates

A standalone COSMIC panel applet for distribution package updates. It checks at startup and every **60 minutes**, including while its dropdown is closed. **Check now** performs a manual check; opening the dropdown does not trigger another scan.

The panel shows a distro-specific symbolic icon and the available update count. The dropdown lists package names, installed and available versions where the backend provides them, repository information, and the last successful check. Large results have a scrollable list with 30 packages per page. An unsuccessful refresh keeps the previous list explicitly marked as stale, never reports the failure as zero updates, and does not overlap another check. The timestamp is replaced by a spinner while a check is running.

**Update in terminal** opens an interactive terminal, displays the exact commands, and asks for confirmation before running them. System package operations use `sudo`; AUR helpers run as the desktop user and manage their own privilege elevation. No `-y` or `--noconfirm` flags are added. Ctrl+C stops subsequent commands. The terminal retains the completion or failure message until Enter is pressed. A per-user lock prevents overlapping update runners.

## Supported backends

Selection uses `/etc/os-release` (`/usr/lib/os-release` as fallback), `ID_LIKE`, available executables, and immutable-system detection. It does not pick a package manager merely because an unrelated executable is installed.

| Distribution family | Background check | Interactive update |
| --- | --- | --- |
| Fedora, RHEL, Rocky, AlmaLinux, CentOS and related systems | Prefer DNF5, then DNF, then Yum; refresh metadata and query native updates | `sudo dnf5 upgrade --refresh`, corresponding DNF command, or Yum upgrade with expired metadata |
| Debian, Ubuntu, Pop!_OS, Mint, elementary, Kali, Zorin and derivatives | APT refreshes signed metadata into a private list cache, then queries upgradable packages | `sudo apt-get update --error-on=any`, then `sudo apt-get dist-upgrade` |
| Arch, Manjaro, EndeavourOS, Garuda, CachyOS and derivatives | `checkupdates` uses a separate database; prefer `paru`, then `yay` for additional AUR updates | `paru -Syu`, `yay -Syu`, or `sudo pacman -Syu` |
| openSUSE/SUSE | Zypper refreshes a private host-state snapshot, then lists updates | Refresh repositories, then `sudo zypper dup` for Tumbleweed or `sudo zypper update` otherwise |
| Fedora Atomic and compatible OSTree systems | Authorized, noninteractive `rpm-ostree upgrade --check`; reports actual available OS deployments | `sudo rpm-ostree upgrade`; reboot separately to apply the deployment |
| Alpine | APK refreshes a private state snapshot and queries newer versions | `sudo apk update`, then interactive `apk upgrade` |
| Void | XBPS in-memory metadata synchronization and dry-run update query | `sudo xbps-install -S -u` |
| Solus | Official Python 3 `pisi` API refreshes metadata in a private state snapshot | `sudo eopkg update-repo`, then `sudo eopkg upgrade` |

### Requirements and limitations

- **DNF5:** requires a version supporting JSON output for `check-update`. JSON mode can return exit status 0 even when updates exist, so the structured package data determines the result. Packages repeated in upgrades and obsoleting sections are counted once; nested obsoleted packages are not additional updates. Legacy DNF/Yum text checks retain their 0/100 consistency checks. DNF-family checks reuse the native configured user cache and its existing repository trust; they do not silently approve new signing keys. If a repository needs initial trust approval, the error explains how to review its fingerprint in an ordinary-user terminal metadata check. Signature verification is not disabled.
- **APT:** requires `apt-get` with `--error-on=any` and `apt`. The private check clears update hooks so system maintenance hooks are not run from an unprivileged scan. Repository authentication remains enabled.
- **Arch:** install `pacman-contrib` (`checkupdates`), `fakeroot`, and `pacman-conf`. The applet does not install these automatically or refresh the live pacman database without upgrading. AUR development-package checks follow the selected helper's configuration.
- **Zypper/APK:** need readable package-manager configuration and installed-package state. Copies are bounded to 512 MiB and discarded after the scan. These private checks can require more I/O than managers with native nonroot metadata support.
- **rpm-ostree:** background checking must already have permanent noninteractive polkit authorization. The applet does not request passwords, alter polkit rules, or reuse temporary authorization grants for background scans. Unavailable authorization produces an explicit error; the interactive terminal updater remains available. Deployment items are not claimed to be individual package updates.
- **Solus:** needs `python3` and the installed `pisi` API. The CLI is not used for background refresh because it can invoke `pkexec`; the API is isolated to a private snapshot instead. No default repositories are invented or added.
- **Unsupported systems:** SteamOS, unsupported transactional/declarative systems, and distributions without a supported backend are identified explicitly. The applet does not attempt mutable package upgrades on a managed read-only OS. NixOS, Gentoo and Mageia have artwork but no automated update backend here.
- This applet manages the host's distribution updates. It does not include Flatpak, Snap, firmware updates, distro-release migrations, or automatic rebooting.
- A failed repository refresh makes the check fail visibly rather than presenting an incomplete result as “up to date.” Subprocess output is bounded to 8 MiB per stream and each checking subprocess has a five-minute timeout. Update commands themselves are not subject to that timeout.
- Only Fedora/DNF5 has been exercised against a live host during development. Other backends have deterministic parser/selection/command tests and need validation on their respective distributions.

## Icons

Includes **28 distro SVGs**: Fedora, Ubuntu, Arch, Debian, Pop!_OS, Mint, openSUSE, Manjaro, EndeavourOS, Garuda, CachyOS, Zorin, elementary, Kali, RHEL, Rocky, AlmaLinux, CentOS, Alpine, Void, Solus, NixOS, Gentoo, Mageia, Fedora Atomic, Bazzite, Bluefin and Aurora. Related variants share their family's artwork. Unknown derivatives use the first recognized `ID_LIKE` family; entirely unknown systems use a generic update icon.

The SVGs are embedded and require no special fonts. Fedora uses the recognizable blue badge and white Fedora mark from [System76's Pop icon theme](https://github.com/pop-os/icon-theme), retaining its original colors rather than applying symbolic recoloring. `resources/icons/fedora.svg` is the unmodified 24-pixel `fedora-logo-icon.svg` artwork, distributed under [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/) by the Pop icon theme contributors; [upstream license](https://github.com/pop-os/icon-theme/blob/master/LICENSE). Fedora's name and logo remain trademarks of their respective owners; no endorsement is implied. The other 27 icons are original distro-inspired symbolic artwork using native theme tinting. An icon identifies the distribution; it does not imply that every distribution's update mechanism is supported.

## Build and install

Requires Rust, `just`, `desktop-file-utils`, and libcosmic's native Wayland/font/graphics development dependencies.

```sh
just build-release
just install
```

The install recipe puts the executable, desktop registration and generic icon under `~/.local`. It respects `CARGO_TARGET_DIR`; use the same value for build and install when sharing a target directory.

Add **System Updates** through **COSMIC Settings → Desktop → Panel → Applets**. Installation does not alter the panel configuration or other applets.

```sh
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cosmic-applet-system-updates --check
```

`--check` performs a real, non-installing metadata check and prints the result. An error returns a nonzero exit status. The internal `--run-updates BACKEND` mode refuses noninteractive input or execution as root; only the package commands inside the ordinary-user terminal invoke sudo.

Supported terminal preference: COSMIC Terminal, GNOME Console, GNOME Terminal, Konsole, Xfce Terminal, kitty, Alacritty, then xterm. A missing terminal produces an actionable error. Panel-private sockets and variables are removed from the terminal child's environment.

## Local data and privacy

- No accounts, credentials, telemetry, or applet daemon are required.
- Checks contact the repositories already configured on the machine, plus AUR through an installed helper where selected.
- DNF-family metadata and approved repository keys remain in the native package manager's configured user cache.
- APT/Arch metadata and per-backend check locks live under `$XDG_CACHE_HOME/cosmic-applet-system-updates`, defaulting to `~/.cache/cosmic-applet-system-updates`. Native metadata is retained between checks to allow conditional downloads instead of fetching every index from scratch each hour.
- Zypper, APK and eopkg private snapshots are temporary. Update-runner locking uses the private user runtime directory. No packages are downloaded or installed by the applet's checking code.
- Update results remain in memory. Restarting the applet performs a fresh check.

Code and original artwork: MPL-2.0; see `LICENSE`. The bundled Fedora logo is separately licensed CC BY-SA 4.0, as attributed above.
