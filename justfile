import 'recipes/common.just'

name := 'cosmos-applet-system-updates'
appid := 'dev.jp.CosmosAppletSystemUpdates'
profile := 'release'
prefix := env('HOME') / '.local'
rootdir := ''
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')
base-dir := absolute_path(clean(rootdir / prefix))

[default]
default: build-release

build-debug:
    cargo build --locked --target-dir {{quote(cargo-target-dir)}}

build-release:
    cargo build --release --locked --target-dir {{quote(cargo-target-dir)}}

check:
    cargo clippy --all-targets --locked --target-dir {{quote(cargo-target-dir)}} -- -D warnings

test:
    #!/usr/bin/env bash
    set -euo pipefail
    # Rootless containers may expose /tmp with an unmapped owner; keep trust tests private.
    cache_home="${XDG_CACHE_HOME:-$HOME/.cache}"
    mkdir -p -- "$cache_home"
    temporary=$(mktemp -d "$cache_home/cosmos-applet-system-updates-tests.XXXXXX")
    trap 'rm -rf -- "$temporary"' EXIT
    TMPDIR="$temporary" cargo test --locked --target-dir {{quote(cargo-target-dir)}}

# The installed applet needs a COSMIC panel; direct runs are for development.
[positional-arguments]
run *args:
    @printf '%s\n' 'Applet surfaces require a COSMIC session; add Cosmos System Updates through panel settings.'
    if [[ {{quote(profile)}} == debug ]]; then cargo run --locked --target-dir {{quote(cargo-target-dir)}} -- "$@"; else cargo run --locked --profile {{quote(profile)}} --target-dir {{quote(cargo-target-dir)}} -- "$@"; fi

# Build separately. Staging never changes the final desktop Exec path.
install:
    @just --justfile {{quote(justfile())}} _install-file {{quote(cargo-target-dir / profile / name)}} {{quote(base-dir / 'bin' / name)}} 0755
    @just --justfile {{quote(justfile())}} _install-desktop resources/app.desktop {{quote(base-dir / 'share/applications' / appid + '.desktop')}} {{quote(absolute_path(prefix / 'bin' / name))}}
    @just --justfile {{quote(justfile())}} _install-file resources/icon.svg {{quote(base-dir / 'share/icons/hicolor/scalable/apps' / appid + '-symbolic.svg')}} 0644

# Files only: remove the panel entry yourself. Cached metadata is retained.
uninstall:
    rm -f -- {{quote(base-dir / 'bin' / name)}} {{quote(base-dir / 'share/applications' / appid + '.desktop')}} {{quote(base-dir / 'share/icons/hicolor/scalable/apps' / appid + '-symbolic.svg')}}

# Stop both the old applet and its terminal updater before copying cache data.
migrate:
    #!/usr/bin/env bash
    set -euo pipefail
    cache_home="${XDG_CACHE_HOME:-$HOME/.cache}"
    just --justfile {{quote(justfile())}} _migrate-directory "$cache_home/cosmic-applet-system-updates" "$cache_home/cosmos-applet-system-updates"
