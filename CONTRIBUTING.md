# Contributing

Read [engineering conventions](docs/engineering/conventions.md) before changing code. Agents start at [AGENTS.md](AGENTS.md); repeatable task prompts and local skill usage are in [agent-workflow.md](docs/engineering/agent-workflow.md).

## Scope and compatibility

- Rust owns transport/session/codec work; Python remains a thin compatible bridge. Reuse the existing ports and helpers before adding layers or dependencies.
- Check callers, exported Python methods/aliases, `.pyi`, examples and wire contracts together. Do not rename topic strings or public camelCase aliases for style alone.
- New model support requires sourced model/firmware/capability evidence and offline fixtures. Upstream Python support does not establish support in this port.
- Use short, focused changes. Describe the common cause and preserved behavior before a refactor; include a regression test for changed nontrivial behavior.

## Setup and verification

See [README development setup](README.md#development-setup) and inspect the current `Makefile`. `make init` installs development dependencies/hooks and builds the extension; use it only when setting up an application development environment, not for documentation review.

- Application work: smallest relevant regression, applicable `make check`, Rust tests and `make test-python` with required native dependencies. `make test-python` first rebuilds the native extension; `make acceptance` combines guidance/release checks, core, Rust and fresh-native Python gates in order, even under `make -j`. Rust tests link Python using the locked development environment; extension builds enable the separate `extension-module` feature. Direct pytest commands must use `uv run --locked --no-sync pytest -c pyproject.toml ...` so a parent checkout's configuration does not override this independently locked SDK.
- Python binding work: include an **actual native-extension** offline check. Existing mock-based Python tests alone do not prove ABI/NumPy/callback behavior.
- Packaging work: install the built wheel in a clean environment outside the source tree and inspect helper/stub contents. Use `make wheel-smoke WHEEL=/absolute/path/package.whl`; it checks the real extension loader, installed stub and legacy aliases without connecting. A successful build is not a successful fresh import. The [release workflow](.github/PUBLISHING.md) requires offline acceptance and this isolated import before artifact upload/publication; it still depends on platform GStreamer/Opus libraries and is not hardware or general Jetson portability validation.
- Documentation/skill-only work: validate metadata, local references, instruction consistency and diffs. Run `python3 scripts/check-guidance.py` directly with an available Python, or `make check-guidance` in the existing locked environment, for the pinned single-line name/description metadata subset, manifest SHA-256 and retained local Markdown targets. Unsupported metadata syntax fails explicitly; no implicit PyYAML dependency is needed. No application tests/builds/CI/hardware are required solely for prose.

PRs (including fork PRs) and pushes to `main` are configured to run the reusable [offline acceptance workflow](.github/workflows/acceptance.yml). A feature-branch push with an open PR runs acceptance through the PR event only, avoiding a duplicate push run. Feature branches without a PR do not automatically run acceptance: run `make acceptance` locally, then open a PR to start hosted checks. Release builds reuse the full workflow after manifest/lock/tag preflight, then install wheels outside the checkout and run native offline contracts before publication. Local checks do not prove remote CI, every configured platform, administrator protections, publication or signed provenance. See the canonical [publishing policy](.github/PUBLISHING.md); manual dispatch is a production publication, not a dry run.

Do not run `examples/` as offline verification: examples may connect to and command a robot, including when their names/comments suggest state queries. Hardware checks need explicit task authorization and operator safety arrangements. Never commit credentials, per-device keys or unredacted captures.

## Offline measurements

With the checked-in toolchain and native dependencies available, run from the SDK root:

```bash
make -s benchmark > /tmp/unitree-transport-benchmark.json
```

This runs `cargo bench --locked --no-default-features --bench transport_contracts` in optimized mode. [The workload](benches/transport_contracts.rs) measures a sparse 128 KiB LiDAR bitmap decoder, a reused owned 64×48 event handoff and media saturation/recovery: 100 warmups and 2,000 timed samples each. A sample contains one decode, 128 handoffs or 32 saturation rounds. Nearest-rank p50/p95/p99/max describe whole batches; normalized channel values are batch averages, **not individual-event tail latency**. The JSON records counters, source revision/dirty state, crate/compiler versions, OS/architecture and timestamp. Preserve that output and disclose host/build conditions for any comparison; no baseline speedup is established merely by adding the benchmark.

These are local Rust microbenchmarks, not network/codec-to-Python latency, Python callback performance, robot throughput, full process RSS or fleet/reliability measurements. Event handoff reuses an allocation; saturation includes small allocations, and timing includes instrumentation/scheduling. Bounded offline fixtures do not establish long-running soak, portable-wheel, physical audio or free-threaded support. Current physical/platform limits remain in [README validation scope](README.md#validation-scope).

[The native measurement CLI](scripts/measure-native.py) additionally exercises disconnected JSON admission and callback registration/explicit release. For performance evidence, install an **optimized release wheel** into a separate environment; the default `maturin develop` build is not optimized:

```bash
measurement_env="$(mktemp -d)"
uv venv --python .venv/bin/python "$measurement_env/venv"
uv pip install --python "$measurement_env/venv/bin/python" /absolute/path/release.whl
"$measurement_env/venv/bin/python" -I scripts/measure-native.py \
    --iterations 100 --batches 4 --callback-cycles 100 \
    > /tmp/unitree-native-measurement.json
```

The CLI never calls `connect`: it constructs only a disconnected loopback session, warms `disconnect`, checks the expected not-open/budget errors, and verifies callback-owner retention and release on every cycle. It records binary/source hashes, whole-operation latency and before/after RSS/native-thread observations where supported. Callback execution and remote delivery are not measured. The fixed owner/global runtime remain alive in the resource baseline and final samples; allocator retention and OS sampling are not a leak detector, process memory quota or long-running soak proof. Preserve the report and wheel identity, then remove the temporary measurement environment when no longer needed.

## Change report

State the user-facing outcome, affected files/contracts, exact checks and results, unsupported platforms/models and hardware/performance gaps. Separate static inspection, mock tests, native tests and physical validation. Do not claim end-to-end zero-copy from a Rust-buffer-to-NumPy ownership transfer.

Only commit/push/publish when authorized. Existing ticketed commit subjects follow `[BTB-<id>] <type>. <summary>`; never invent an ID. Use a ticketless subject only when explicitly authorized. Explain intent, constraints and omitted checks in the body.

## Licensing

The current repository declares `Proprietary`; this guide does not change that license or make it an open-source release. Before redistribution or an OSS release, the maintainer must settle the intended license and audit upstream provenance/notices and native/dependency distribution obligations. A Rust rewrite alone does not establish that no upstream obligations remain.
