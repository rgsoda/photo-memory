#!/usr/bin/env bash
#
# Build and install photomem on Linux.
#
# Does three things and stops: checks the build dependencies, builds the release
# binary, copies it onto your PATH. It never edits your compositor config or
# your shell profile — it prints what to add and leaves the file to you, because
# those are files you own and a script guessing at them is how they get broken.
#
#   ./install.sh              build and install to ~/.local/bin
#   ./install.sh --deps       also install the system packages first (uses sudo)
#   ./install.sh --prefix DIR install somewhere else (binary goes in DIR/bin)
#   ./install.sh --check      only report what is missing, build nothing

set -euo pipefail

PREFIX="${PREFIX:-$HOME/.local}"
INSTALL_DEPS=0
CHECK_ONLY=0

while [ $# -gt 0 ]; do
    case "$1" in
        --deps) INSTALL_DEPS=1 ;;
        --check) CHECK_ONLY=1 ;;
        --prefix) PREFIX="${2:?--prefix needs a directory}"; shift ;;
        -h|--help) sed -n '3,14p' "$0" | cut -c3-; exit 0 ;;
        *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
    esac
    shift
done

# Colours only when a terminal is watching, so piping this to a log stays clean.
if [ -t 1 ]; then bold=$'\e[1m'; dim=$'\e[2m'; red=$'\e[31m'; off=$'\e[0m'
else bold=""; dim=""; red=""; off=""; fi

say()  { printf '%s==>%s %s\n' "$bold" "$off" "$*"; }
note() { printf '    %s%s%s\n' "$dim" "$*" "$off"; }
die()  { printf '%serror:%s %s\n' "$red" "$off" "$*" >&2; exit 1; }

cd "$(dirname "$0")"

# ── system packages ──────────────────────────────────────────────────────────
#
# The webview is the only real dependency: Tauri renders the UI with WebKitGTK
# rather than shipping a browser, which is what keeps the binary at ~11 MB.
# Names differ per distro, so the lists are spelled out rather than guessed.

detect_distro() {
    if   command -v pacman  >/dev/null 2>&1; then echo arch
    elif command -v apt-get >/dev/null 2>&1; then echo debian
    elif command -v dnf     >/dev/null 2>&1; then echo fedora
    elif command -v zypper  >/dev/null 2>&1; then echo suse
    else echo unknown
    fi
}

deps_command() {
    case "$1" in
        arch)   echo "sudo pacman -S --needed base-devel pkgconf webkit2gtk-4.1 gtk3 libsoup3 librsvg" ;;
        debian) echo "sudo apt-get install -y build-essential pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev librsvg2-dev libssl-dev" ;;
        fedora) echo "sudo dnf install -y @development-tools pkgconf-pkg-config webkit2gtk4.1-devel gtk3-devel libsoup3-devel librsvg2-devel openssl-devel" ;;
        suse)   echo "sudo zypper install -y -t pattern devel_basis && sudo zypper install -y webkit2gtk3-soup2-devel gtk3-devel libsoup-devel librsvg-devel" ;;
        *)      echo "" ;;
    esac
}

distro="$(detect_distro)"
deps_cmd="$(deps_command "$distro")"

if [ "$INSTALL_DEPS" = 1 ]; then
    [ -n "$deps_cmd" ] || die "unrecognised distribution; install the WebKitGTK 4.1 development files by hand"
    say "Installing system packages"
    note "$deps_cmd"
    eval "$deps_cmd"
fi

# pkg-config is the honest test: the package may be named anything, but if the
# compiler cannot find webkit2gtk-4.1 the build will fail several minutes in.
missing=0
if ! command -v pkg-config >/dev/null 2>&1; then
    echo "missing: pkg-config"
    missing=1
elif ! pkg-config --exists webkit2gtk-4.1; then
    echo "missing: webkit2gtk-4.1 development files"
    missing=1
fi

if ! command -v cargo >/dev/null 2>&1; then
    echo "missing: cargo — install Rust from https://rustup.rs"
    missing=1
fi

if [ "$missing" = 1 ]; then
    if [ -n "$deps_cmd" ]; then
        note "on $distro: $deps_cmd"
        note "or rerun as: ./install.sh --deps"
    fi
    [ "$CHECK_ONLY" = 1 ] && exit 1
    die "dependencies are missing; nothing was built"
fi

if [ "$CHECK_ONLY" = 1 ]; then
    say "All build dependencies are present."
    exit 0
fi

# ── build ────────────────────────────────────────────────────────────────────
#
# There is no npm step. The UI is plain HTML/CSS/JS in ui/ and is embedded into
# the binary at build time.

say "Building (this takes a few minutes the first time)"
cargo build --release

# ── install ──────────────────────────────────────────────────────────────────

bindir="$PREFIX/bin"
say "Installing to $bindir/photomem"
install -Dm755 target/release/photomem "$bindir/photomem"

case ":$PATH:" in
    *":$bindir:"*) ;;
    *) note "$bindir is not on your PATH — add it to your shell profile:"
       note "  export PATH=\"$bindir:\$PATH\"" ;;
esac

# ── what to do next ──────────────────────────────────────────────────────────
#
# Wayland has no protocol for an application to register a global hotkey, so the
# binding has to live in the compositor's own config. Which file that is depends
# on the compositor, and Omarchy configures Hyprland in Lua rather than the
# stock syntax, so print whichever one actually applies here.

cat <<EOF

$(printf '%s' "$bold")Next:$(printf '%s' "$off")

  1. Run photomem once to write ~/.config/photomem/config.toml, then set
     'vault' in it to where your notes should live. Make that directory a git
     repo if you want them synced — it is deliberately separate from this one.

  2. Bind the hotkey. 'photomem daemon' stays warm in the background and
     'photomem capture' wakes it, which is what makes the popup feel instant.
EOF

if [ -f "$HOME/.config/hypr/hyprland.lua" ]; then
    cat <<'EOF'

     Omarchy configures Hyprland in Lua, spread across three files:

       -- ~/.config/hypr/autostart.lua
       o.launch_on_start("photomem daemon")

       -- ~/.config/hypr/bindings.lua
       o.bind("SUPER + N", "Photo memory", "photomem capture")

       -- ~/.config/hypr/hyprland.lua
       -- Both windows float and centre...
       o.window({ class = "^photomem$" }, { float = true, center = true })
       -- ...but only the capture window has a fixed size. The image window
       -- sizes itself to the picture, and a class-wide size rule would
       -- silently override that.
       o.window({ class = "^photomem$", title = "^photomem$" }, { size = "720 400" })

     Then: hyprctl reload && hyprctl configerrors
EOF
elif [ -f "$HOME/.config/hypr/hyprland.conf" ]; then
    cat <<'EOF'

     In ~/.config/hypr/hyprland.conf:

       exec-once = photomem daemon
       bind = SUPER, N, exec, photomem capture
       windowrulev2 = float, class:^(photomem)$
       windowrulev2 = center, class:^(photomem)$

     Then: hyprctl reload && hyprctl configerrors
EOF
else
    cat <<'EOF'

     No Hyprland config found. Bind SUPER+N to 'photomem capture' and autostart
     'photomem daemon' however your desktop does it — README.md has the
     Hyprland version, which is the only one tested so far.
EOF
fi

echo
say "Done. See README.md for the keys, and DESIGN.md for what it is."
