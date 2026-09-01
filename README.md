# pinyinwl

Pinyin input method for Wayland (COSMIC), using libpinyin and the input-method v3 protocol.

Includes `pinyinwl` (IME daemon) and `cosmic-applet-pinyin` (panel indicator and settings).

## Build

Requires sibling checkouts from the [rano-oss](https://github.com/rano-oss) workspace (`libcosmic`, `libchinese`, `cosmic-comp`, `wayland-rs`, etc.).

```bash
cargo build --release
```

## Install

```bash
./install.sh
```
