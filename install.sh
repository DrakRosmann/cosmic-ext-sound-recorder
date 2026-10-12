#!/bin/sh
# Build and install Sound Recorder, for the current user (default) or for everyone.
#   ./install.sh              # in ~/.local
#   sudo ./install.sh /usr    # for everyone
# From a release tarball (bin/ beside this script) nothing is built.
# Packagers and Flatpak: DESTDIR="$pkgdir" ./install.sh /usr --no-build
set -e
cd "$(dirname "$0")"
prefix="${1:-$HOME/.local}"
id=io.github.DrakRosmann.CosmicExtSoundRecorder
name=cosmic-ext-sound-recorder
dest="${DESTDIR:-}"

if [ -x "bin/$name" ]; then
    bin=bin
else
    bin=target/release
    if [ "$2" != "--no-build" ]; then
        ./deps.sh 1.93
        cargo build --release --locked
    fi
fi

# A user's own install uses the full path: the session may not have
# ~/.local/bin on its PATH.
case "$prefix" in
    "$HOME"/*) exec_path="$prefix/bin/$name" ;;
    *) exec_path="$name" ;;
esac

install -Dm0755 "$bin/$name" "$dest$prefix/bin/$name"
mkdir -p "$dest$prefix/share/applications"
sed "s|^Exec=@bindir@/$name|Exec=$exec_path|" "data/$id.desktop.in" > "$dest$prefix/share/applications/$id.desktop"
install -Dm0644 "data/icons/scalable/apps/$id.svg" "$dest$prefix/share/icons/hicolor/scalable/apps/$id.svg"
install -Dm0644 "data/$id.metainfo.xml" "$dest$prefix/share/metainfo/$id.metainfo.xml"

if [ -z "$dest" ]; then
    command -v update-desktop-database >/dev/null 2>&1 &&
        update-desktop-database -q "$prefix/share/applications" 2>/dev/null || true
    echo "Installed. Open \"Sound Recorder\" from the app library."
fi
