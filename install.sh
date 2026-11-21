#!/bin/sh
# Installs InfinityDesk as a session for login managers (greetd, SDDM, GDM, ...): installs the
# software it needs (Arch-based systems), builds it, and installs it if it is new or changed.
# Run it again after changes to update the installed version.
set -eu
cd "$(dirname "$0")"

# What InfinityDesk needs (Arch package names): compiler and libraries to build it, and the
# default terminal and launcher.
PACKAGES="gcc pkgconf wayland libxkbcommon libinput libdrm mesa libglvnd systemd-libs seatd
kitty fuzzel"

# 1. Required software
if command -v pacman >/dev/null; then
    # pacman -T lists the packages that are missing (also counts packages that provide them).
    missing=$(pacman -T $PACKAGES || true)
    command -v cargo >/dev/null || missing="$missing rust"
    if [ -n "$(echo $missing)" ]; then
        echo "Installing missing software:" $missing
        sudo pacman -S --needed $missing
    else
        echo "All required software is already installed."
    fi
elif ! command -v cargo >/dev/null; then
    echo "cargo not found: install Rust first (https://rustup.rs)."
    echo "Also needed (names differ per distribution): a C compiler, pkg-config, and the"
    echo "development files of wayland, libxkbcommon, libinput, libdrm, mesa (gbm, EGL),"
    echo "libudev and libseat; kitty and fuzzel for the default shortcuts."
    exit 1
fi

# 2. Build
echo "Building InfinityDesk..."
cargo build --release

# 3. Install, unless the installed version is already the same
installed() { cmp -s "$1" "$2"; }
if installed target/release/infinitydesk /usr/local/bin/infinitydesk &&
    installed resources/infinitydesk-session /usr/local/bin/infinitydesk-session &&
    installed resources/infinitydesk.desktop /usr/share/wayland-sessions/infinitydesk.desktop; then
    echo "InfinityDesk is already installed and up to date. Nothing to do."
    exit 0
fi
if [ -e /usr/local/bin/infinitydesk ]; then action=Updated; else action=Installed; fi
echo "Installing InfinityDesk (needs sudo)..."
sudo install -Dm755 target/release/infinitydesk resources/infinitydesk-session -t /usr/local/bin/
sudo install -Dm644 resources/infinitydesk.desktop -t /usr/share/wayland-sessions/

echo "$action InfinityDesk. Log out and choose \"InfinityDesk\" in the login screen."
echo "Log of the session: ~/.local/state/infinitydesk.log"
