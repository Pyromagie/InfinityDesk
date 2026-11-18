#!/bin/sh
# Builds InfinityDesk and installs it as a session for login managers (greetd, SDDM, GDM, ...).
# Run it again after changes to update the installed version.
set -eu
cd "$(dirname "$0")"

command -v cargo >/dev/null || { echo "cargo not found: install Rust first (https://rustup.rs)"; exit 1; }

echo "Building InfinityDesk..."
cargo build --release

echo "Installing (needs sudo)..."
sudo install -Dm755 target/release/infinitydesk resources/infinitydesk-session -t /usr/local/bin/
sudo install -Dm644 resources/infinitydesk.desktop -t /usr/share/wayland-sessions/

echo "Done. Log out and choose \"InfinityDesk\" in the login screen."
echo "Log of the session: ~/.local/state/infinitydesk.log"
