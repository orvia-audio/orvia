#!/usr/bin/env bash
# Build a native macOS disk image or Ubuntu package from the workspace root.
set -euo pipefail
version=$(cargo metadata --locked --no-deps --format-version 1 | jq -r '.packages[] | select(.name=="orvia") | .version')
host=$(rustc -vV | awk '/^host:/ { print $2 }')
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p dist
cargo build --workspace --locked --release --message-format=json-render-diagnostics > "$work/build.json"
cargo-about generate --locked --target "$host" -o "$work/dependencies" installer/notices.hbs
cat LICENSE installer/DeepFilterNet-NOTICE.txt assets/tabler/LICENSE "$work/dependencies" > "$work/THIRD-PARTY-NOTICES.txt"

case "$host" in
    *-apple-darwin)
        app="$work/image/Orvia.app"
        mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$work/Orvia.iconset"
        install -m755 target/release/orvia target/release/orvia-engine "$app/Contents/MacOS/"
        cp installer/macos/Info.plist "$app/Contents/"
        plutil -replace CFBundleShortVersionString -string "${version%%-*}" "$app/Contents/Info.plist"
        build=${version##*.}
        plutil -replace CFBundleVersion -string "$build" "$app/Contents/Info.plist"
        icon_dir=$(jq -r 'select(.reason=="build-script-executed" and (.package_id | contains("/crates/settings#"))) | .out_dir' "$work/build.json")
        for size in 16 32 128 256 512; do
            for scale in 1 2; do
                suffix=""; if [[ $scale == 2 ]]; then suffix="@2x"; fi
                pixels=$((size * scale))
                sips -z "$pixels" "$pixels" "$icon_dir/orvia-1024.png" --out "$work/Orvia.iconset/icon_${size}x${size}$suffix.png" >/dev/null
            done
        done
        iconutil -c icns "$work/Orvia.iconset" -o "$app/Contents/Resources/Orvia.icns"
        cp "$work/THIRD-PARTY-NOTICES.txt" "$app/Contents/Resources/"
        codesign --force --sign - "$app/Contents/MacOS/orvia-engine"
        codesign --force --sign - "$app"
        codesign --verify --deep --strict "$app"
        ln -s /Applications "$work/image/Applications"
        hdiutil create -quiet -ov -format UDZO -srcfolder "$work/image" -volname Orvia "dist/Orvia-$version-macos-${host%%-*}.dmg"
        ;;
    x86_64-unknown-linux-gnu)
        root="$work/package"
        install -Dm755 target/release/orvia "$root/usr/bin/orvia"
        install -Dm755 target/release/orvia-engine "$root/usr/bin/orvia-engine"
        install -Dm644 assets/orvia.svg "$root/usr/share/icons/hicolor/scalable/apps/orvia.svg"
        install -Dm644 installer/linux/orvia.desktop "$root/usr/share/applications/orvia.desktop"
        install -Dm644 installer/linux/60-orvia.conf "$root/usr/share/pipewire/pipewire-pulse.conf.d/60-orvia.conf"
        install -Dm644 "$work/THIRD-PARTY-NOTICES.txt" "$root/usr/share/doc/orvia/copyright"
        mkdir -p "$root/DEBIAN" "$work/debian"
        cat > "$work/debian/control" <<EOF
Source: orvia
Maintainer: Orvia contributors <sya-ri@users.noreply.github.com>

Package: orvia
Architecture: amd64
Description: Microphone mixer for games and recording
EOF
        libs=$(cd "$work" && dpkg-shlibdeps -O -e"$root/usr/bin/orvia" -e"$root/usr/bin/orvia-engine")
        cat > "$root/DEBIAN/control" <<EOF
Package: orvia
Version: ${version/-/\~}
Architecture: amd64
Maintainer: Orvia contributors <sya-ri@users.noreply.github.com>
Depends: ${libs#shlibs:Depends=}, pipewire-pulse, wireplumber, libvulkan1
Description: Microphone mixer for games and recording
EOF
        dpkg-deb --root-owner-group --build "$root" "dist/Orvia-$version-linux-amd64.deb"
        ;;
    *) echo "Unsupported packaging target: $host" >&2; exit 1 ;;
esac
