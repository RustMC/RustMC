# Compatibility matrix

Statuses: `planned`, `partial`, `tested`, `unsupported`, `unknown`. `tested` requires exact version, scope, and test evidence. No game protocol version is selected in M0.

| Area | Java | Bedrock | M0 evidence |
| --- | --- | --- | --- |
| Connection and authentication | planned | planned | None |
| Rules and update ordering | planned | planned translation; native parity unknown | None |
| Inventory, commands, recipes, interactions | planned | planned translation | None |
| Generation and seed behavior | planned | unknown | None |
| Save/import/export | planned | unknown | None |
| Cross-edition translation differences | not applicable | planned | None |

The proposed shared world uses Java-style game rules. Bedrock support would translate client actions and results; it does not promise identical edition mechanics. Version targets and rules baseline need owner approval before implementation. This matrix must grow into versioned feature entries backed by black-box observations and tests. M0 contains no gameplay and cannot accept client connections.
