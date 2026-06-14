#!/usr/bin/env bash
#
# MagnetBox one-line installer for Linux servers.
#
#   curl -fsSL https://raw.githubusercontent.com/KenjakuSoft/MAGNET-BOX/main/install.sh | sudo bash
#
# Downloads the latest prebuilt binary, creates a dedicated user, and installs a
# systemd service that starts on boot. No Rust toolchain required.

set -euo pipefail

REPO="KenjakuSoft/MAGNET-BOX"
USER_NAME="magnetbox"
INSTALL_DIR="/opt/magnetbox"
SERVICE="/etc/systemd/system/magnetbox.service"
PORT="8080"

say() { printf '\n\033[1;32m==>\033[0m %s\n' "$1"; }

if [ "$(id -u)" -ne 0 ]; then
  echo "Please run as root, e.g.:  curl -fsSL .../install.sh | sudo bash"
  exit 1
fi

arch="$(uname -m)"
case "$arch" in
  x86_64 | amd64) asset="magnetbox-linux-x64" ;;
  *)
    echo "No prebuilt binary for '$arch' yet — build from source instead (see README)."
    exit 1
    ;;
esac

command -v curl >/dev/null 2>&1 || { echo "curl is required"; exit 1; }

say "Downloading the latest MagnetBox ($asset)…"
url="https://github.com/$REPO/releases/latest/download/$asset"
tmp="$(mktemp)"
curl -fSL "$url" -o "$tmp"

say "Installing to $INSTALL_DIR…"
id -u "$USER_NAME" >/dev/null 2>&1 || useradd --system --create-home --home-dir "$INSTALL_DIR" "$USER_NAME"
install -d -o "$USER_NAME" -g "$USER_NAME" "$INSTALL_DIR" "$INSTALL_DIR/downloads" "$INSTALL_DIR/data"
install -m 0755 -o "$USER_NAME" -g "$USER_NAME" "$tmp" "$INSTALL_DIR/magnetbox"
rm -f "$tmp"

say "Creating the systemd service…"
cat > "$SERVICE" <<EOF
[Unit]
Description=MagnetBox
After=network.target

[Service]
User=$USER_NAME
WorkingDirectory=$INSTALL_DIR
Environment=MAGNETBOX_DIR=$INSTALL_DIR/downloads
Environment=MAGNETBOX_DATA=$INSTALL_DIR/data
Environment=MAGNETBOX_BIND=127.0.0.1
Environment=MAGNETBOX_PORT=$PORT
Environment=MAGNETBOX_NO_OPEN=1
ExecStart=$INSTALL_DIR/magnetbox
Restart=on-failure
NoNewPrivileges=true
ProtectSystem=strict
ReadWritePaths=$INSTALL_DIR
ProtectHome=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable --now magnetbox

# Give it a moment to write the first-run credentials.
sleep 2

say "Done! MagnetBox is running on http://127.0.0.1:$PORT"
echo
echo "  Your admin login:"
if [ -f "$INSTALL_DIR/data/FIRST-LOGIN.txt" ]; then
  sed 's/^/    /' "$INSTALL_DIR/data/FIRST-LOGIN.txt"
else
  echo "    Run:  sudo journalctl -u magnetbox | grep -A4 'First run'"
fi
echo
echo "  Next steps to use it from anywhere with HTTPS:"
echo "    1. Point a domain's DNS A-record at this server's IP."
echo "    2. Install Caddy and use deploy/Caddyfile (see the README deploy guide)."
echo "    3. Open ports 80 and 443 in your firewall."
echo
echo "  Manage it:  sudo systemctl {status|restart|stop} magnetbox"
