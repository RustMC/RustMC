# Compatibility matrix

Statuses: `planned`, `partial`, `tested`, `unsupported`, `unknown`. `tested` requires exact version, scope, and test evidence.

## M2 discovery targets (checked 29 September 2026)

Java Edition **26.3**, network protocol **777**, is the initial target. Mojang's [version manifest](https://piston-meta.mojang.com/mc/game/version_manifest_v2.json) lists 26.3 as the latest release; the [official 26.3 server download](https://piston-data.mojang.com/v1/objects/33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c/server.jar) contains `version.json` with `protocol_version: 777` and `stable: true`. The [release notes](https://www.minecraft.net/en-us/article/minecraft-java-edition-26-3) corroborate the release. The server archive was inspected as metadata only; no code or assets were copied.

Bedrock Edition **1.26.51**, network protocol **2193**, is the initial target. Mojang's [1.26.51 protocol release](https://github.com/Mojang/bedrock-protocol-docs/releases/tag/v1.26.51) states the network identifier. Mojang's [26.52 hotfix announcement](https://feedback.minecraft.net/hc/en-us/articles/49175370527501-Minecraft-Bedrock-Edition-26-52-Hotfix-Changelog) establishes that 26.52 is newer, but no matching stable Mojang protocol schema was published at this check. Choosing the latest version with a published stable protocol identifier keeps the first target auditable. No claim of 26.52 compatibility follows from 1.26.51 discovery.

These identifiers select discovery responses only. Java's handshake carries a client protocol number, so mismatches can receive a status naming 777; Bedrock's unconnected ping carries no game network version, so discovery cannot authenticate or reject a client version. Neither route enables login or play. The Java 26.3 status path was observed with the owner's client; Bedrock ping behavior still requires a real-client check.

| Area | Java 26.3 | Bedrock 1.26.51 | Current evidence |
| --- | --- | --- | --- |
| Discovery | tested: 26.3 server-list status and ping | partial: UDP ping/pong synthetic socket test | Java 26.3 client showed `RustMC discovery only; login unavailable` and `0/0` with a green connection indicator on 29 September 2026; RustMC logged completed status exchanges. Bedrock real-client observation remains open. |
| Connection and authentication | partial: opt-in loopback Creative terrain preview; configuration and initial play rendered in a real client, no authentication or authoritative gameplay | planned | A real Java 26.3 client accepted the local core-pack registry and tag exchange, entered Creative preview play, and visibly rendered original chunks on 29 September 2026. No account authentication or gameplay state exists. |
| Rules and update ordering | planned | planned translation; native parity unknown | None |
| Inventory, commands, recipes, interactions | planned | planned translation | None |
| Generation and seed behavior | partial: original preview plus an opt-in, operator-provisioned data-driven Overworld probe | unknown | Determinism, packet parity tests, and local client observation; the probe lacks feature-stage blocks, structures, and exact vanilla world-generation parity. Its disk cache stores immutable preview packets, not world edits. |
| Save/import/export | planned | unknown | None |
| Cross-edition translation differences | not applicable | planned | None |

The proposed shared world uses Java-style game rules. Bedrock support would translate client actions and results; it does not promise identical edition mechanics. The discovery version targets are selected; the gameplay rules baseline still needs owner approval. The matrix must grow into versioned feature entries backed by black-box observations and tests. The default loopback process answers discovery only. With explicit local preview configuration, Java 26.3 can inspect immutable generated chunks; it has no authenticated, persistent, or shared gameplay world.
