# Development

Linux/Fedora is the initially tested environment. Install Git and rustup from trusted sources; `rust-toolchain.toml` pins Rust 1.98.1 with rustfmt and clippy. Other platforms are unverified. The only dependency is the reviewed TOML parser in `Cargo.lock`. Do not update dependencies or toolchain merely to hide a failing check.

From the repository root run:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --locked
cargo doc --workspace --no-deps --locked
cargo run -p rustmc-server --locked -- --check-config config/rustmc.example.toml
```

The binary is bootstrap-only. `--help` and `--version` succeed; `--check-config PATH` checks a file without modifying it. Exit code 0 means a valid invocation; 2 means CLI usage error; 3 means configuration read/parse/validation error. No arguments print status and exit 0. For a missing toolchain, install the pinned release and components; do not silently substitute another compiler. If Cargo cannot fetch dependencies, report that network acquisition is blocked rather than claiming tests passed.
