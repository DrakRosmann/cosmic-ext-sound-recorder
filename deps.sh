#!/bin/sh
# Install what building needs, with the system's package manager (apt, dnf,
# pacman or zypper), if anything is missing; and check that Rust is new enough.
#   ./deps.sh          # just the libraries and tools
#   ./deps.sh 1.93     # and Rust at least this version
set -e

missing=
for tool in git cc pkg-config cargo; do
    command -v "$tool" >/dev/null 2>&1 || missing="$missing $tool"
done
if command -v pkg-config >/dev/null 2>&1; then
    for lib in xkbcommon alsa; do
        pkg-config --exists "$lib" || missing="$missing $lib"
    done
fi

if [ -n "$missing" ]; then
    echo "Missing:$missing"
    rust="cargo rustc"
    # rustup provides cargo too; don't swap one for the other.
    command -v rustup >/dev/null 2>&1 && rust=
    if command -v apt-get >/dev/null 2>&1; then
        # Debian, Ubuntu, Pop!_OS, Mint...
        sudo apt-get install -y git build-essential pkgconf libxkbcommon-dev libasound2-dev $rust
    elif command -v dnf >/dev/null 2>&1; then
        # Fedora, Nobara, Ultramarine...
        [ -n "$rust" ] && rust="cargo rust"
        sudo dnf install -y git gcc pkgconf-pkg-config libxkbcommon-devel alsa-lib-devel $rust
    elif command -v pacman >/dev/null 2>&1; then
        # Arch, Manjaro, EndeavourOS, CachyOS...
        [ -n "$rust" ] && rust=rust
        sudo pacman -S --needed --noconfirm git base-devel pkgconf libxkbcommon alsa-lib $rust
    elif command -v zypper >/dev/null 2>&1; then
        # openSUSE
        [ -n "$rust" ] && rust="cargo rust"
        sudo zypper install -y git gcc pkg-config libxkbcommon-devel alsa-devel $rust
    else
        echo "Unknown package manager: install git, a C compiler, pkg-config," >&2
        echo "the development files of libxkbcommon and ALSA, and Rust yourself." >&2
        exit 1
    fi
fi

# Distributions often ship an older Rust than libcosmic asks for.
if [ -n "$1" ]; then
    have=$(rustc --version | cut -d' ' -f2)
    if [ "$(printf '%s\n%s\n' "$1" "$have" | sort -V | head -n1)" != "$1" ]; then
        if command -v rustup >/dev/null 2>&1; then
            rustup update stable
        else
            echo "Rust $have is too old: $1 or newer is needed. Install it with rustup" >&2
            echo "(https://rustup.rs), then run this again; or install a package from" >&2
            echo "the releases, which needs no Rust at all." >&2
            exit 1
        fi
    fi
fi
