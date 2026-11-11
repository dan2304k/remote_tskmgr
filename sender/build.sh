#!/usr/bin/env bash
# build.sh - build rtm-sender on Linux/macOS and copy the result to ../dist/<os>-<arch>/
#
#   ./build.sh            release build with the tray icon + settings window (gui feature)
#   ./build.sh headless   release build without the GUI
#   ./build.sh debug      debug build with gui
#   ./build.sh test       run the test suites (headless workspace + rtm-sender with gui)
#   ./build.sh clean      remove target/ and the dist folder
#
# Linux gui build deps: libgtk-3-dev libayatana-appindicator3-dev libxdo-dev libxkbcommon-dev libwayland-dev
set -euo pipefail
SENDER_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SENDER_DIR/.." && pwd)"
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"; ARCH="$(uname -m)"
DIST_DIR="$REPO_DIR/dist/${OS}-${ARCH}"

PROFILE=release; PROFILE_FLAG=--release; FEATURES=(--features gui); VARIANT=gui
for arg in "$@"; do
  case "$arg" in
    headless) FEATURES=(); VARIANT=headless ;;
    debug)    PROFILE=debug; PROFILE_FLAG= ;;
    test)     (cd "$SENDER_DIR" && cargo test --workspace && cargo test -p rtm-sender --features gui) && echo "[test] all tests passed"; exit $? ;;
    clean)    echo "[build] removing $SENDER_DIR/target and $DIST_DIR"; rm -rf "$SENDER_DIR/target" "$DIST_DIR"; exit 0 ;;
    help|-h|--help) sed -n '2,10p' "$0"; exit 0 ;;
    *) echo "unknown option: $arg"; sed -n '2,10p' "$0"; exit 2 ;;
  esac
done

command -v cargo >/dev/null || { echo "[build] cargo not found; install Rust from https://rustup.rs"; exit 1; }
echo "[build] $(rustc --version), profile=$PROFILE, variant=$VARIANT"
(cd "$SENDER_DIR" && cargo build $PROFILE_FLAG -p rtm-sender "${FEATURES[@]}")

EXE="$SENDER_DIR/target/$PROFILE/rtm-sender"
mkdir -p "$DIST_DIR"
cp -f "$EXE" "$DIST_DIR/rtm-sender"
cp -f "$REPO_DIR/packaging/README.md" "$DIST_DIR/INSTALL.md"
cp -f "$REPO_DIR/packaging/rtm-sender.service" "$DIST_DIR/" 2>/dev/null || true
cp -f "$REPO_DIR/LICENSE" "$DIST_DIR/LICENSE"
echo "[build] done: $DIST_DIR/rtm-sender ($(stat -c%s "$DIST_DIR/rtm-sender" 2>/dev/null || stat -f%z "$DIST_DIR/rtm-sender") bytes, $VARIANT)"
echo "[build] test it: $DIST_DIR/rtm-sender --once"
