# pinyinwl

Pinyin input method for Wayland (COSMIC), using libpinyin and the input-method v3 protocol.

Includes `pinyinwl` (IME daemon). Panel mode/menu comes from cosmic-applet-input-sources via `ime-control`.

## Build

Depends on crates.io [`libpinyin`](https://crates.io/crates/libpinyin) plus COSMIC/Wayland deps. Lexicon install data comes from a sibling `../libpinyin` checkout (`data/converted/`).

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
