# PhotoStore

Native, open-source image editing software written in Rust.

![Rust 1.95+](https://img.shields.io/badge/Rust-1.95%2B-b7410e?style=flat-square&logo=rust)
![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Windows%20%7C%20Linux-2f7bf5?style=flat-square)
![License](https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-3a3a3a?style=flat-square)

## Install

Download the macOS Apple Silicon package from the [PhotoStore releases](https://github.com/akiyamasho/PhotoStore/releases). For a local install, use Rust stable 1.95+:

```sh
CARGO_BUILD_JOBS=1 cargo install --path apps/photocraft --bin photostore --locked
CARGO_BUILD_JOBS=1 cargo install --path apps/photocraft-cli --bin photostore-cli --locked
```

Linux native dependencies and optional fonts are documented in [`docs/development.md`](docs/development.md).

Optional native movie import/export requires FFmpeg and FFprobe; install the Homebrew `ffmpeg-full` formula or set `PHOTOSTORE_FFMPEG` and `PHOTOSTORE_FFPROBE`.

## Build and run

```sh
CARGO_BUILD_JOBS=1 cargo build --release -p photocraft -p photocraft-cli
./target/release/photostore
./target/release/photostore-cli --help
open -a PhotoStore
```
