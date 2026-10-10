# PhotoStore

Native, open-source image editing software written in Rust.

![Rust 1.95+](https://img.shields.io/badge/Rust-1.95%2B-b7410e?style=flat-square&logo=rust)
![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Windows%20%7C%20Linux-2f7bf5?style=flat-square)
![License](https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-3a3a3a?style=flat-square)

## Install

Download a release package when available. For a local install, use Rust stable 1.95+:

```sh
CARGO_BUILD_JOBS=1 cargo install --path apps/photocraft --bin photostore --locked
CARGO_BUILD_JOBS=1 cargo install --path apps/photocraft-cli --bin photostore-cli --locked
```

Linux native dependencies and optional fonts are documented in [`docs/development.md`](docs/development.md).

## Build and run

```sh
CARGO_BUILD_JOBS=1 cargo build --release -p photocraft -p photocraft-cli
./target/release/photostore
./target/release/photostore-cli --help
```

The web build uses Trunk; packaging scripts are under [`packaging/`](packaging/). The internal Cargo package names remain `photocraft` and `photocraft-cli` for compatibility, while the exposed executables are `photostore` and `photostore-cli`.

PhotoStore is dual-licensed under MIT or Apache-2.0. See [`LICENSE-MIT`](LICENSE-MIT), [`LICENSE-APACHE`](LICENSE-APACHE), [`NOTICE`](NOTICE), and [`ATTRIBUTION.md`](ATTRIBUTION.md).
