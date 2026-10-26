# SDK structure and ownership

This is the existing `unitree-webrtc-rs` Cargo/PyO3 library, not a newly generated Rust application. Its separately owned
Git root may be checked out inside BattleBang; the parent gitlink and unrelated standalone SDK are separate changes.

| Path | Owner |
| --- | --- |
| `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` | Rust library targets, dependencies, feature matrix and pinned compiler |
| `pyproject.toml`, `uv.lock`, `build.rs`, `unitree_webrtc_rs.pyi` | Maturin/PyO3 build, locked Python development environment, linker setup and shipped native API stub |
| `src/lib.rs`, `src/protocol/` | Library/Python module entrypoint and framework-independent wire/port contracts |
| `src/connection/` | Owned session, adapter assembly, connect/validation/rollback/disconnect lifecycle |
| `src/datachannel/`, `src/audio/`, `src/video/` | Routing, request completion, bounded queues and codec/buffer ownership |
| `src/infrastructure/`, `src/interface/` | Concrete HTTP/RTC/platform adapters and thin Python conversion/callback bridges |
| `tests/`, `benches/` | Focused mock/native/offline contracts and bounded measurement workloads; not hardware proof |
| `examples/` | User examples that may connect to and command physical hardware; not ordinary offline checks |
| `Makefile`, `scripts/` | Feature-aware verification, release/measurement tooling and static guidance checks |
| `docs/engineering/`, `docs/architecture_analysis.md` | Maintained SDK rules, current architecture and resource contracts |
| `docs/specs/`, `docs/adr/` | Future scoped contract/decision records, linked to existing owners rather than duplicating them |
| `.github/` | Existing PR/issue forms and release/acceptance workflow configuration, not an executed CI claim |
| `.agents/` | Project routing/setup only; shared procedures, forms, provenance and licenses stay with the provider |
| `.codex/config.toml`, `.claude/settings.json` | Shared plugin activation only, not installation, discovery or trust proof |
| `.copier-answers.yml` | Genuine guidance-only template lineage, not a migration of application files or a plugin payload lock |

Use [architecture](architecture_analysis.md) for detailed dataflow and [resource policy](engineering/resource-policy.md)
for implemented ownership/limits. Native manifests, source and the [contributor check contract](../CONTRIBUTING.md)
prevail over illustrative shared layouts or commands. Do not add an application target, empty layers or another command catalog.
