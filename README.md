# RustMC

An independently developed Minecraft-compatible server project written in Rust. RustMC aims for a vanilla-style multiplayer world for Java and Bedrock clients, fast startup, measured capacity, and plugins later.

> **Foundation / pre-alpha:** this repository is a development scaffold, not a playable Minecraft server. No client login, world, or gameplay exists. Do not use it for production worlds.

| Capability | Current state |
| --- | --- |
| Bootstrap CLI and configuration validation | Implemented in M0 |
| Java and Bedrock login/gameplay | Planned |
| Vanilla survival and compatible storage | Planned |
| Pulse–Parcel parallel execution | Experimental design only |
| Plugin runtime/API | Deferred |
| Server performance results | None |

The proposed shared world uses Java-style rules with Bedrock client translation, pending owner approval. It does not promise identical edition behavior. See [compatibility](docs/COMPATIBILITY.md).

## Developer quick start

The tested toolchain is pinned in `rust-toolchain.toml`; Linux is the initially tested platform. From a clone:

```sh
cargo build --workspace --locked
cargo test --workspace --locked
cargo run -p rustmc-server --locked -- --help
cargo run -p rustmc-server --locked -- --version
cargo run -p rustmc-server --locked -- --check-config config/rustmc.example.toml
```

These commands build and validate the scaffold. Running with no arguments reports bootstrap-only status and exits without opening sockets or creating a world. See [development](docs/DEVELOPMENT.md).

## Design and roadmap

The proposed [architecture](docs/ARCHITECTURE.md) keeps Java and Bedrock gateways separate from an authoritative gameplay core. [Decisions](docs/PROJECT_DECISIONS.md), [testing](docs/TESTING.md), and [benchmark methodology](docs/BENCHMARKS.md) explain validation. Start with [M0](docs/milestones/M0-foundation.md), the [TODO list](TODO.md), and [roadmap](ROADMAP.md).

## Contributing and security

Read [contribution guidance](CONTRIBUTING.md) and [provenance policy](docs/PROVENANCE.md). External contributions await a selected license. RustMC is experimental; [security reporting guidance](SECURITY.md) records the pending private route. Do not post secrets or vulnerability details in public issues.

## License and disclaimer

The project license is awaiting maintainer selection; see [decisions](docs/PROJECT_DECISIONS.md). RustMC is independent, unofficial, and neither approved by nor associated with Mojang or Microsoft.
