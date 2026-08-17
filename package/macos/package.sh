#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
RUST_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
TARGET_DIR=${CARGO_TARGET_DIR:-"$RUST_ROOT/target"}
OUT_DIR=${1:-"$RUST_ROOT/dist"}
APP="$OUT_DIR/Mr Crabs.app"
CONTENTS="$APP/Contents"
RESOURCES="$CONTENTS/Resources"
SIGN_IDENTITY=${MR_CRABS_SIGN_IDENTITY:--}

cargo build --manifest-path "$RUST_ROOT/Cargo.toml" --locked --release -p mr-crabs-app --bin mr-crabs
python3 "$RUST_ROOT/package/verify_resources.py" \
  --resources "$RUST_ROOT/resources" \
  --manifest "$RUST_ROOT/verification/manifests/s11-resources.json"
python3 "$RUST_ROOT/package/generate_notices.py" \
  --manifest-path "$RUST_ROOT/Cargo.toml" \
  --output "$RUST_ROOT/resources/THIRD_PARTY_NOTICES.txt"
rm -rf "$APP"
mkdir -p "$CONTENTS/MacOS" "$RESOURCES"
install -m 0755 "$TARGET_DIR/release/mr-crabs" "$CONTENTS/MacOS/mr-crabs"
install -m 0644 "$SCRIPT_DIR/Info.plist" "$CONTENTS/Info.plist"
printf 'APPL????' >"$CONTENTS/PkgInfo"
cp -R "$RUST_ROOT/resources/." "$RESOURCES/"

codesign --force --deep --options runtime --entitlements "$SCRIPT_DIR/MrCrabs.entitlements" --sign "$SIGN_IDENTITY" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"
/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$CONTENTS/Info.plist" | grep -qx 'dev.jamie.mr-crabs'
/usr/libexec/PlistBuddy -c 'Print :CFBundleName' "$CONTENTS/Info.plist" | grep -qx 'Mr Crabs'
/usr/libexec/PlistBuddy -c 'Print :CFBundleDisplayName' "$CONTENTS/Info.plist" | grep -qx 'Mr Crabs'
/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$CONTENTS/Info.plist" | grep -qx 'mr-crabs'
if otool -L "$CONTENTS/MacOS/mr-crabs" | grep -Eq 'libghostty|libzig|libswift'; then
  printf '%s\n' "forbidden Zig/Ghostty/Swift runtime dependency" >&2
  exit 1
fi
printf '%s\n' "$APP"
