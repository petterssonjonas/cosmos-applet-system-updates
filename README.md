# Cosmos System Updates

Check distribution updates from the COSMIC panel. Show the package count, package list, and last successful check.
Checks run at startup and every 60 minutes. Select **Check now** for a manual check.
A failed check keeps the previous result marked as stale. It does not show a false zero count.
Cosmos is independent of System76. COSMIC libraries and protocols keep their upstream names.

## Requirements

Use Linux with a working COSMIC session, a supported package manager, and an interactive terminal such as COSMIC Terminal.
Install native build packages for your distribution:

```sh
# Fedora
sudo dnf install gcc gcc-c++ cmake git pkgconf-pkg-config just desktop-file-utils expat-devel fontconfig-devel freetype-devel libxkbcommon-devel wayland-devel

# Debian 13 or newer
sudo apt update
sudo apt install build-essential cmake git pkgconf just desktop-file-utils libexpat1-dev libfontconfig-dev libfreetype-dev libxkbcommon-dev libwayland-dev

# Arch Linux: pacman-contrib supplies checkupdates; pacman supplies pacman-conf
sudo pacman -Syu --needed base-devel cmake git pkgconf just desktop-file-utils expat fontconfig freetype2 libxkbcommon wayland pacman-contrib fakeroot
```

The lists follow [libcosmic's native requirements](https://github.com/pop-os/libcosmic#dependencies) and distribution package metadata.
They are package examples, not a claim that all three distributions were tested.
Install current stable Rust with [rustup](https://rustup.rs), as your normal user. Distribution Rust can be too old.
After rustup setup, open a new terminal and run:

```sh
rustup update stable
rustup default stable
```

## Build and install

Use `sudo` only for package prerequisites above, never for these build or install commands:

```sh
git clone https://github.com/petterssonjonas/cosmos-applet-system-updates
cd cosmos-applet-system-updates
just build-release
just install
```

Build and install are separate. Installation uses the existing release build.
The executable goes to `~/.local/bin/cosmos-applet-system-updates`. The desktop entry and icon go to `~/.local/share`.
The desktop entry uses an absolute executable path. Keep `CARGO_TARGET_DIR` the same for both commands if you set it.
In a development container, use your host-mapped normal user, not root.

Open **COSMIC Settings → Desktop → Panel → Configure panel applets**.
Add **Cosmos System Updates** (`dev.jp.CosmosAppletSystemUpdates`). Remove and add it again after an upgrade.
Installation does not change the panel layout.

## Updates and authorization

**Update in terminal** shows the exact commands and asks for confirmation.
System updates use `sudo` at runtime. AUR helpers run as the normal user and manage their own authorization.
This is separate from the unprivileged applet installation. No automatic confirmation flags are added.
Press Ctrl+C to stop subsequent commands. A per-user lock prevents overlapping update runners.

Backend selection uses distribution identity and immutable-system detection, not merely installed command names.

| System | Backend and runtime needs |
| --- | --- |
| Fedora and related mutable systems | DNF5, DNF, or Yum. DNF5 must support JSON `check-update` output. |
| Debian and related systems | `apt` and `apt-get` with `--error-on=any`. Checks use private signed metadata. Updates run `apt-get update`, then `dist-upgrade`. |
| Arch and related systems | `checkupdates`, `fakeroot`, and `pacman-conf`. Optional `paru`, then `yay`, adds AUR checks and updates. |
| openSUSE/SUSE | Zypper. Tumbleweed uses `dup`; other supported variants use `update`. |
| Fedora Atomic and compatible OSTree systems | rpm-ostree. Background checks need existing permanent noninteractive Polkit authorization. Reboot separately to apply updates. |
| Alpine | APK with readable configuration and installed state. |
| Void | XBPS with metadata synchronization and a dry-run query. |
| Solus | `python3` and the installed `pisi` API for checks; eopkg for interactive updates. |

Background checks do not ask for passwords, change Polkit rules, install packages, or approve new repository keys.
If a repository needs initial trust approval, review its fingerprint with the normal package-manager workflow.
The applet never disables signature verification. Failed repository refreshes produce an error, not an incomplete success.
Zypper/APK checks copy private state snapshots, limited to 512 MiB. Each checking subprocess has a five-minute timeout.

Flatpak, Snap, firmware, distribution-release migrations, and automatic reboots are not included.
Unsupported systems, including SteamOS and unsupported declarative/transactional systems, receive no mutable upgrade commands.
Artwork alone does not mean that a distribution has a supported backend.
Only Fedora/DNF5 has been exercised against a live host during development. Other backends need validation on their distributions.

## Local data and migration

Checks contact configured repositories and, when selected, AUR through an installed helper. No accounts or telemetry are required.
APT/Arch metadata uses `~/.cache/cosmos-applet-system-updates`, or the corresponding `$XDG_CACHE_HOME` directory.
DNF retains its native user cache and repository trust. Results remain in memory. Temporary backend snapshots are discarded.

To migrate, stop the old applet and finish or cancel all old update terminals first. Do not run both versions together: their locks differ.
Before the new applet creates its cache, run `just migrate`.
It copies `~/.cache/cosmic-applet-system-updates` to `~/.cache/cosmos-applet-system-updates`, following `$XDG_CACHE_HOME` when set.
Migration keeps the source, refuses an existing destination, and rejects symlinks or untrusted paths. It does not copy runtime locks.
There are no settings or credentials to migrate. The new applet has no old-name runtime fallback.

Replace the old **System Updates** panel entry with **Cosmos System Updates**.
Remove legacy files separately under the old prefix: `bin/cosmic-applet-system-updates`,
`share/applications/io.github.cosmic-applet-system-updates.desktop`, and
`share/icons/hicolor/scalable/apps/io.github.cosmic-applet-system-updates-symbolic.svg`.

## Uninstall

Remove the applet from the panel. Run `just uninstall` from this source directory, without `sudo`.
Uninstall keeps cached metadata and does not remove legacy files or change package-manager state.

## License and attribution

Code and original artwork: MPL-2.0; see `LICENSE`.
Built with [libcosmic](https://github.com/pop-os/libcosmic) and the [COSMIC applet template](https://github.com/pop-os/cosmic-applet-template).
`resources/icons/fedora.svg` is the unmodified 24-pixel `fedora-logo-icon.svg` from the
[Pop icon theme](https://github.com/pop-os/icon-theme), by its contributors, under
[CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/); see its [upstream license](https://github.com/pop-os/icon-theme/blob/master/LICENSE).
Other distribution icons are original distro-inspired artwork. Distribution names and logos remain their owners' trademarks. No endorsement is implied.
