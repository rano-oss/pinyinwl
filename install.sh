#!/bin/bash
# Install script for pinyinwl (Pinyin IME for Wayland/COSMIC)
#
# Usage:
#   ./install.sh          # Build + install (may need sudo for install)
#   ./install.sh build    # Build only (no root needed)
#   ./install.sh install  # Install only (assumes already built)
#
# Installs:
#   - pinyinwl binary (IME daemon; depends on crates.io libpinyin)
#   - Lexicon data from a sibling libpinyin checkout (data/converted/)
#   - Desktop file for settings (panel icon: preferences-desktop-keyboard)

set -e

PREFIX="${PREFIX:-/usr}"
BINDIR="${BINDIR:-$PREFIX/bin}"
DATADIR="${DATADIR:-$PREFIX/share/libpinyin/data}"
APPDIR="${APPDIR:-$PREFIX/share/applications}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
LIBPINYIN_DIR="${LIBPINYIN_DIR:-$SCRIPT_DIR/../libpinyin}"
PROFILE="${PROFILE:-release}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
BOLD='\033[1m'
NC='\033[0m'

info()  { echo -e "${GREEN}==>${NC} ${BOLD}$1${NC}"; }
error() { echo -e "${RED}error:${NC} $1" >&2; exit 1; }

# Ensure cargo is in PATH
find_cargo() {
    if command -v cargo &>/dev/null; then
        return
    fi
    # Try sourcing cargo env from various locations
    for home_dir in "$HOME" "/home/${SUDO_USER:-}" "/root"; do
        [ -n "$home_dir" ] && [ -f "$home_dir/.cargo/env" ] && {
            . "$home_dir/.cargo/env"
            command -v cargo &>/dev/null && return
        }
    done
    # Try common paths directly
    for p in "$HOME/.cargo/bin" "/home/${SUDO_USER:-}/.cargo/bin" "/usr/local/bin"; do
        [ -x "$p/cargo" ] && { export PATH="$p:$PATH"; return; }
    done
    error "cargo not found. Install Rust: https://rustup.rs"
}

do_build() {
    find_cargo

    info "Building pinyinwl (${PROFILE}) against crates.io libpinyin..."
    cargo build --profile "$PROFILE" --manifest-path "$SCRIPT_DIR/Cargo.toml"

    local CONVERTED_DIR="$LIBPINYIN_DIR/data/converted"
    [ -d "$CONVERTED_DIR/simplified" ] || error \
        "Missing lexicon data at $CONVERTED_DIR (clone/build libpinyin data, or set LIBPINYIN_DIR)"

    info "Build complete."
}

do_install() {
    local TARGET_DIR="$SCRIPT_DIR/target/$PROFILE"
    [ "$PROFILE" = "dev" ] && TARGET_DIR="$SCRIPT_DIR/target/debug"
    local CONVERTED_DIR="$LIBPINYIN_DIR/data/converted"
    local RAW_DATA="$LIBPINYIN_DIR/data"

    [ -f "$TARGET_DIR/pinyinwl" ] || error "pinyinwl not built. Run: ./install.sh build"
    [ -d "$CONVERTED_DIR/simplified" ] || error "Missing data at $CONVERTED_DIR. Set LIBPINYIN_DIR."

    info "Installing binary to $DESTDIR$BINDIR..."
    install -Dm755 "$TARGET_DIR/pinyinwl" "$DESTDIR$BINDIR/pinyinwl"

    info "Installing data to $DESTDIR$DATADIR..."

    # Simplified lexicon
    install -Dm644 "$CONVERTED_DIR/simplified/lexicon.fst" "$DESTDIR$DATADIR/simplified/lexicon.fst"
    install -Dm644 "$CONVERTED_DIR/simplified/lexicon.dat" "$DESTDIR$DATADIR/simplified/lexicon.dat"
    install -Dm644 "$CONVERTED_DIR/simplified/word_bigram.dat" "$DESTDIR$DATADIR/simplified/word_bigram.dat"
    install -Dm644 "$CONVERTED_DIR/simplified/word_bigram_words.fst" "$DESTDIR$DATADIR/simplified/word_bigram_words.fst"

    # Traditional lexicon
    install -Dm644 "$CONVERTED_DIR/traditional/lexicon.fst" "$DESTDIR$DATADIR/traditional/lexicon.fst"
    install -Dm644 "$CONVERTED_DIR/traditional/lexicon.dat" "$DESTDIR$DATADIR/traditional/lexicon.dat"
    install -Dm644 "$CONVERTED_DIR/traditional/word_bigram.dat" "$DESTDIR$DATADIR/traditional/word_bigram.dat"
    install -Dm644 "$CONVERTED_DIR/traditional/word_bigram_words.fst" "$DESTDIR$DATADIR/traditional/word_bigram_words.fst"

    # Emoji / English (prefer converted copies, else raw data/)
    for name in emoji.table english.wordlist; do
        local src=""
        if [ -f "$CONVERTED_DIR/simplified/$name" ]; then
            src="$CONVERTED_DIR/simplified/$name"
        elif [ -f "$RAW_DATA/$name" ]; then
            src="$RAW_DATA/$name"
        fi
        if [ -n "$src" ]; then
            install -Dm644 "$src" "$DESTDIR$DATADIR/simplified/$name"
            install -Dm644 "$src" "$DESTDIR$DATADIR/traditional/$name"
        fi
    done

    # Addon dictionaries (simplified copies + traditional when available)
    if [ -d "$CONVERTED_DIR/addon" ]; then
        for addon_dir in "$CONVERTED_DIR/addon"/*/; do
            [ -d "$addon_dir" ] || continue
            local addon_name
            addon_name="$(basename "$addon_dir")"
            install -Dm644 "$addon_dir/lexicon.fst" "$DESTDIR$DATADIR/simplified/addon/$addon_name/lexicon.fst"
            install -Dm644 "$addon_dir/lexicon.dat" "$DESTDIR$DATADIR/simplified/addon/$addon_name/lexicon.dat"
            local trad_addon="$CONVERTED_DIR/traditional/addon/$addon_name"
            if [ -d "$trad_addon" ]; then
                install -Dm644 "$trad_addon/lexicon.fst" "$DESTDIR$DATADIR/traditional/addon/$addon_name/lexicon.fst"
                install -Dm644 "$trad_addon/lexicon.dat" "$DESTDIR$DATADIR/traditional/addon/$addon_name/lexicon.dat"
            else
                install -Dm644 "$addon_dir/lexicon.fst" "$DESTDIR$DATADIR/traditional/addon/$addon_name/lexicon.fst"
                install -Dm644 "$addon_dir/lexicon.dat" "$DESTDIR$DATADIR/traditional/addon/$addon_name/lexicon.dat"
            fi
        done
    fi

    info "Installing desktop file to $DESTDIR$APPDIR..."
    install -Dm644 "$SCRIPT_DIR/data/com.pinyinwl.Settings.desktop" \
        "$DESTDIR$APPDIR/com.pinyinwl.Settings.desktop"
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$DESTDIR$APPDIR" 2>/dev/null || true
    fi

    echo ""
    info "Installation complete!"
    echo ""
    echo "  Binary:    $DESTDIR$BINDIR/pinyinwl"
    echo "  Data:      $DESTDIR$DATADIR/"
    echo "  Desktop:   $DESTDIR$APPDIR/com.pinyinwl.Settings.desktop"
    echo ""
    echo "  To configure: pinyinwl --settings"
    echo "  Restart the IME (or log out/in) to pick up the new build."
}

# --- Main ---

case "${1:-all}" in
    build)   do_build ;;
    install) do_install ;;
    all)
        if [ "$(id -u)" -eq 0 ]; then
            # Running as root - try to find cargo, warn if not possible
            find_cargo
        fi
        do_build
        do_install
        ;;
    *)       echo "Usage: $0 [build|install|all]"; exit 1 ;;
esac
