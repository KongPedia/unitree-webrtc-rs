# Contributing

Read [engineering conventions](docs/engineering/conventions.md) before changing code. Agents start at [AGENTS.md](AGENTS.md); repeatable task prompts and local skill usage are in [agent-workflow.md](docs/engineering/agent-workflow.md).

## Scope and compatibility

- Rust owns transport/session/codec work; Python remains a thin compatible bridge. Reuse the existing ports and helpers before adding layers or dependencies.
- Check callers, exported Python methods/aliases, `.pyi`, examples and wire contracts together. Do not rename topic strings or public camelCase aliases for style alone.
- New model support requires sourced model/firmware/capability evidence and offline fixtures. Upstream Python support does not establish support in this port.
- Use short, focused changes. Describe the common cause and preserved behavior before a refactor; include a regression test for changed nontrivial behavior.

## Setup and verification

See [README development setup](README.md#development-setup) and inspect the current `Makefile`. `make init` installs development dependencies/hooks and builds the extension; use it only when setting up an application development environment, not for documentation review.

- Application work: smallest relevant regression, applicable `make check`, Rust tests and `make test-python` with required native dependencies. `make test-python` first rebuilds the native extension; `make acceptance` combines guidance/checks, core, Rust and fresh-native Python gates in order, even under `make -j`. Rust tests link Python using the locked development environment; extension builds enable the separate `extension-module` feature.
- Python binding work: include an **actual native-extension** offline check. Existing mock-based Python tests alone do not prove ABI/NumPy/callback behavior.
- Packaging work: install the built wheel in a clean environment outside the source tree and inspect helper/stub contents. Use `make wheel-smoke WHEEL=/absolute/path/package.whl`; it checks the real extension loader, installed stub and legacy aliases without connecting. A successful build is not a successful fresh import. The [release workflow](.github/PUBLISHING.md) requires offline acceptance and this isolated import before artifact upload/publication; it still depends on platform GStreamer/Opus libraries and is not hardware or general Jetson portability validation.
- Documentation/skill-only work: validate metadata, local references, instruction consistency and diffs. Run `make check-guidance` (also a local pre-commit hook) for the pinned single-line name/description metadata subset, manifest SHA-256 and retained local Markdown targets. Unsupported metadata syntax fails explicitly; no implicit PyYAML dependency is needed. No application tests/builds/CI/hardware are required solely for prose.

Do not run `examples/` as offline verification: examples may connect to and command a robot, including when their names/comments suggest state queries. Hardware checks need explicit task authorization and operator safety arrangements. Never commit credentials, per-device keys or unredacted captures.

## Change report

State the user-facing outcome, affected files/contracts, exact checks and results, unsupported platforms/models and hardware/performance gaps. Separate static inspection, mock tests, native tests and physical validation. Do not claim end-to-end zero-copy from a Rust-buffer-to-NumPy ownership transfer.

Only commit/push/publish when authorized. Existing ticketed commit subjects follow `[BTB-<id>] <type>. <summary>`; never invent an ID. Use a ticketless subject only when explicitly authorized. Explain intent, constraints and omitted checks in the body.

## Licensing

The current repository declares `Proprietary`; this guide does not change that license or make it an open-source release. Before redistribution or an OSS release, the maintainer must settle the intended license and audit upstream provenance/notices and native/dependency distribution obligations. A Rust rewrite alone does not establish that no upstream obligations remain.
