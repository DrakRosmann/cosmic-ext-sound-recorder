#!/bin/sh
# Remove what install.sh installed, with the same prefix.
#   ./uninstall.sh            # from ~/.local
#   sudo ./uninstall.sh /usr
set -e
prefix="${1:-$HOME/.local}"
id=io.github.DrakRosmann.CosmicExtSoundRecorder
rm -f "$prefix/bin/cosmic-ext-sound-recorder" "$prefix/share/applications/$id.desktop" \
    "$prefix/share/icons/hicolor/scalable/apps/$id.svg" "$prefix/share/metainfo/$id.metainfo.xml"
echo "Removed. Your recordings stay in your Music folder, under Recordings."
