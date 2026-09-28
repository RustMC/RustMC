# M0 — Architecture and Project Foundation

Scope: truthful architecture and compatibility documentation, one bootstrap package, config validation, tests, and CI definition. No network listener, Minecraft login, world, scheduler, plugin runtime, or server performance claim.

| Task | Local evidence |
| --- | --- |
| M0-01 | Empty target inspected; local Git initialized; exclusions verified; no remote |
| M0-02 | Requirements and compatibility matrix mark unimplemented areas |
| M0-03 | Architecture v0.2 and ADR-0001 through ADR-0007, all proposed/experimental/deferred |
| M0-04 | README, roadmap, guides and project policies |
| M0-05 | Pinned one-package Rust workspace and config example |
| M0-06 | Unit and isolated CLI integration tests |
| M0-07 | Least-privilege pinned CI workflow; foundation run 36492068809 passed on published M0 commits |
| M0-08 | Review, provenance, and security policy; private route pending |
| M0-09 | Future replay, readiness and capacity methodology |
| M0-10 | Local checks and owner review completed; three M0 commits published without rewrite |

Acceptance requires all documented local commands, negative CLI/config tests, link and workflow checks, staged-content review, and fresh-checkout validation. GitHub foundation CI passed for the published M0 head; local checks also passed. Owner decisions: license and private security route before external contributions or public release; Java-style rules before gameplay; exact protocol versions before M2; source repository and visibility confirmed for M0 publication. M1 requires separate authorization.
