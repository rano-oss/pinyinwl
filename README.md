# pinyinwl

Pinyin input method for Wayland (COSMIC), using libpinyin and the input-method v3 protocol.

Includes `pinyinwl` (IME daemon). Panel mode/menu comes from cosmic-applet-input-sources via `ime-control`.

## Build

Depends on crates.io [`libpinyin`](https://crates.io/crates/libpinyin) `0.3` plus COSMIC/Wayland deps. Shared panel D-Bus is the separate [`ime-control`](https://github.com/rano-oss/ime-control) crate (not vendored here).

Lexicon data is resolved by libpinyin (`LIBPINYIN_DATA_DIR`, `/usr/share/libpinyin/data`, or a sibling `../libpinyin/data/converted/` checkout).

```bash
cargo build --release
# or: ./install.sh build
```

## Install

```bash
sudo ./install.sh          # build + install
# or: ./install.sh build && sudo ./install.sh install
```

Restart the IME (or log out/in) after installing.
