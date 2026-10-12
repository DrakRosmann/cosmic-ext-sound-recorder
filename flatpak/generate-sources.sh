#!/bin/sh
# Regenerate cargo-sources.json from Cargo.lock. Needs flatpak-cargo-generator.py
# (https://github.com/flatpak/flatpak-builder-tools/tree/master/cargo) and
# Python with aiohttp, PyYAML and tomlkit:
#   flatpak/generate-sources.sh path/to/flatpak-cargo-generator.py
set -e
cd "$(dirname "$0")/.."
python3 "$1" Cargo.lock -o flatpak/cargo-sources.json
