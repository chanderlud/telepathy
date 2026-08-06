#!/usr/bin/env bash

set -euo pipefail

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

if [[ "$(uname -s)" != "Darwin" ]]; then
  fail 'macOS is required to build this installer'
fi

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

for command_name in flutter xcodebuild rustup lipo hdiutil; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    fail "required command not found: $command_name"
  fi
done

PLIST_BUDDY="$(command -v /usr/libexec/PlistBuddy || true)"
if [[ -z "$PLIST_BUDDY" || ! -x "$PLIST_BUDDY" ]]; then
  fail 'required executable not found: /usr/libexec/PlistBuddy'
fi

rust_target_installed() {
  local required_target="$1"
  local installed_target
  local installed_targets

  if ! installed_targets="$(rustup target list --toolchain stable --installed)"; then
    fail 'Rust stable toolchain is unavailable; run: rustup toolchain install stable'
  fi
  while IFS= read -r installed_target; do
    if [[ "$installed_target" == "$required_target" ]]; then
      return 0
    fi
  done <<< "$installed_targets"
  return 1
}

for rust_target in aarch64-apple-darwin x86_64-apple-darwin; do
  if ! rust_target_installed "$rust_target"; then
    fail "required Rust target not installed under stable: $rust_target; run: rustup target add --toolchain stable $rust_target"
  fi
done

BUILD_DIR="$REPO_ROOT/build/macos"
APP_PATH="$BUILD_DIR/Build/Products/Release/Telepathy.app"
DMG_PATH="$REPO_ROOT/telepathy-macos-universal-unsigned.dmg"

printf 'Building unsigned universal macOS Release app...\n'
flutter pub get
flutter build macos --config-only --release
xcodebuild \
  -workspace macos/Runner.xcworkspace \
  -scheme Runner \
  -configuration Release \
  -derivedDataPath "$BUILD_DIR" \
  CODE_SIGNING_ALLOWED=NO \
  CODE_SIGNING_REQUIRED=NO \
  CODE_SIGN_IDENTITY="" \
  DEVELOPMENT_TEAM="" \
  MACOSX_DEPLOYMENT_TARGET=10.15 \
  -destination platform=macOS \
  'ARCHS=arm64 x86_64' \
  ONLY_ACTIVE_ARCH=NO \
  OBJROOT="$BUILD_DIR/Build/Intermediates.noindex" \
  SYMROOT="$BUILD_DIR/Build/Products" \
  build

if [[ ! -d "$APP_PATH" ]]; then
  fail "Release app not found: $APP_PATH"
fi

EXECUTABLE_NAME="$("$PLIST_BUDDY" -c 'Print :CFBundleExecutable' "$APP_PATH/Contents/Info.plist")"
EXECUTABLE_PATH="$APP_PATH/Contents/MacOS/$EXECUTABLE_NAME"
if [[ ! -f "$EXECUTABLE_PATH" ]]; then
  fail "app executable not found: $EXECUTABLE_PATH"
fi

lipo "$EXECUTABLE_PATH" -verify_arch arm64 x86_64
hdiutil create -volname Telepathy -srcfolder "$APP_PATH" -ov -format UDZO "$DMG_PATH"
hdiutil verify "$DMG_PATH"

printf 'App: %s\nDMG: %s\n' "$APP_PATH" "$DMG_PATH"
