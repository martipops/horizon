#!/usr/bin/env bash
# "Horizon Dev.app": this branch's launcher as an app to open from Finder (dev only), beside the
# real Horizon.app, never in its place: its own bundle ID and name, no self-update (it would
# become a release), the release's public Microsoft client ID to sign in with. It uses your real
# launcher data (accounts, instances), as the real launcher does.
#
#   dev/make-launcher-app.sh            # builds into dev/out/Horizon Dev.app
#
# Ad hoc signed, as packaging/macos/package.sh signs forks' builds. To play this branch's game
# from it, give an instance dev/run-dev-game.sh as its wrapper (see dev/run-dev-game.sh).
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
client_id=331e08f8-172d-43b9-835a-3108c2fcdbf3

# runtime_shaders: no Metal compiler needed (Xcode's isn't installed everywhere).
features=""
xcrun metal --version > /dev/null 2>&1 || features="--features gpui/runtime_shaders"
(cd "$root/launcher" && cargo build --release $features)

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/launcher/Cargo.toml" | head -1)
app="$root/dev/out/Horizon Dev.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$root/launcher/target/release/horizon-launcher" "$app/Contents/MacOS/horizon-launcher"
cp "$root/packaging/icons/horizon.icns" "$app/Contents/Resources/horizon.icns"
printf 'APPL????' > "$app/Contents/PkgInfo"
# The release's Info.plist, as a separate app: its own ID and name, and the variables a Finder
# launch doesn't otherwise get (LSEnvironment).
sed -e "s/@VERSION@/$version-dev/g" \
    -e 's|<string>dev.horizon.launcher</string>|<string>dev.horizon.launcher.dev</string>|' \
    -e 's|<string>Horizon</string>|<string>Horizon Dev</string>|g' \
    "$root/packaging/macos/Info.plist" > "$app/Contents/Info.plist"
plutil -insert LSEnvironment -json \
  "{\"HORIZON_LAUNCHER_NO_SELF_UPDATE\":\"1\",\"HORIZON_MSA_CLIENT_ID\":\"$client_id\"}" \
  "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
codesign --force --sign - "$app"
echo "Built: $app"
