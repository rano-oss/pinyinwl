#!/bin/bash
# Install script for pinyinwl (Pinyin IME for Wayland/COSMIC)
#
# Usage:
#   ./install.sh          # Build + install (may need sudo for install)
#   ./install.sh build    # Build only (no root needed)
#   ./install.sh install  # Install only (assumes already built)
#
# Installs:
#   - pinyinwl binary (IME daemon)
#   - cosmic-applet-pinyin binary (panel applet)
#   - Lexicon data (simplified, traditional, emoji, english wordlist, addons)
#   - Desktop file for the applet

set -e

PREFIX="${PREFIX:-/usr}"
BINDIR="${BINDIR:-$PREFIX/bin}"
DATADIR="${DATADIR:-$PREFIX/share/libpinyin/data}"
APPDIR="${APPDIR:-$PREFIX/share/applications}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
LIBCHINESE_DIR="${LIBCHINESE_DIR:-$SCRIPT_DIR/../libchinese}"
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

    info "Building pinyinwl (${PROFILE})..."
    cargo build --profile "$PROFILE" --manifest-path "$SCRIPT_DIR/Cargo.toml"

    # Build data if needed
    CONVERTED_DIR="$LIBCHINESE_DIR/data/converted"
    if [ ! -d "$CONVERTED_DIR/simplified" ]; then
        info "Building lexicon data..."
        (cd "$LIBCHINESE_DIR" && cargo run --manifest-path tools/convert_table/Cargo.toml --release)
    fi

    info "Build complete."
}

do_install() {
    local TARGET_DIR="$SCRIPT_DIR/target/$PROFILE"
    [ "$PROFILE" = "dev" ] && TARGET_DIR="$SCRIPT_DIR/target/debug"
    local CONVERTED_DIR="$LIBCHINESE_DIR/data/converted"

    [ -f "$TARGET_DIR/pinyinwl" ] || error "pinyinwl not built. Run: ./install.sh build"
    [ -d "$CONVERTED_DIR/simplified" ] || error "Data not built. Run: ./install.sh build"

    info "Installing binaries to $DESTDIR$BINDIR..."
    install -Dm755 "$TARGET_DIR/pinyinwl" "$DESTDIR$BINDIR/pinyinwl"
    install -Dm755 "$TARGET_DIR/cosmic-applet-pinyin" "$DESTDIR$BINDIR/cosmic-applet-pinyin"

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

    # Emoji table
    if [ -f "$LIBCHINESE_DIR/data/emoji.table" ]; then
        install -Dm644 "$LIBCHINESE_DIR/data/emoji.table" "$DESTDIR$DATADIR/simplified/emoji.table"
        install -Dm644 "$LIBCHINESE_DIR/data/emoji.table" "$DESTDIR$DATADIR/traditional/emoji.table"
    fi

    # English mixed-input wordlist (混输)
    if [ -f "$LIBCHINESE_DIR/data/english.wordlist" ]; then
        install -Dm644 "$LIBCHINESE_DIR/data/english.wordlist" "$DESTDIR$DATADIR/simplified/english.wordlist"
        install -Dm644 "$LIBCHINESE_DIR/data/english.wordlist" "$DESTDIR$DATADIR/traditional/english.wordlist"
    fi

    # Addon dictionaries
    if [ -d "$CONVERTED_DIR/addon" ]; then
        for addon_dir in "$CONVERTED_DIR/addon"/*/; do
            [ -d "$addon_dir" ] || continue
            local addon_name="$(basename "$addon_dir")"
            install -Dm644 "$addon_dir/lexicon.fst" "$DESTDIR$DATADIR/simplified/addon/$addon_name/lexicon.fst"
            install -Dm644 "$addon_dir/lexicon.dat" "$DESTDIR$DATADIR/simplified/addon/$addon_name/lexicon.dat"
            install -Dm644 "$addon_dir/lexicon.fst" "$DESTDIR$DATADIR/traditional/addon/$addon_name/lexicon.fst"
            install -Dm644 "$addon_dir/lexicon.dat" "$DESTDIR$DATADIR/traditional/addon/$addon_name/lexicon.dat"
        done
    fi

    # Desktop file
    info "Installing desktop file..."
    install -Dm644 "$SCRIPT_DIR/data/com.system76.CosmicAppletPinyin.desktop" \
        "$DESTDIR$APPDIR/com.system76.CosmicAppletPinyin.desktop"

    echo ""
    info "Installation complete!"
    echo ""
    echo "  Binaries:  $DESTDIR$BINDIR/pinyinwl"
    echo "             $DESTDIR$BINDIR/cosmic-applet-pinyin"
    echo "  Data:      $DESTDIR$DATADIR/"
    echo "  Desktop:   $DESTDIR$APPDIR/com.system76.CosmicAppletPinyin.desktop"
    echo ""
    echo "  To configure: pinyinwl --settings"
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
