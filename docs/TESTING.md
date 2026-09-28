# Testing strategy

M0 unit tests validate schema, values, malformed input, and unknown fields. CLI integration tests use isolated temporary directories to exercise help, version, no-argument status, valid and invalid configuration, and usage errors. They require no account, network, Minecraft installation, or private files. Run `cargo test --workspace --locked`.

Later protocol tests need exact edition versions, malformed-input fixtures, authentication scenarios, and real client evidence. Gameplay needs versioned black-box observations. A future single-writer reference executor is an oracle for parallel equivalence, with replay recording initial state, rules/data, seed/random state, admission, ordered intents, and external results. Repartition, same-tick boundary, migration, and crash-recovery tests should reject divergence. None of these later tests exists in M0.
