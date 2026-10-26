# SDK documentation

## Existing owners

- [Public README](../README.md): installation, API, examples and validation limits
- [Contributor guide](../CONTRIBUTING.md): setup and feature-aware offline check selection
- [Architecture map](architecture_analysis.md): current Rust composition, dataflow and extension seams
- [Engineering conventions](engineering/conventions.md): compatibility, ownership, FFI, logging and Rust decisions
- [Resource policy](engineering/resource-policy.md): lifecycle, queue/buffer bounds, teardown and native API migrations
- [SDK agent workflow](engineering/agent-workflow.md): procedure selection, bounded briefs and evidence requirements
- [Publishing guide](../.github/PUBLISHING.md): release matrix, authentication and publication gates

## Navigation and future records

- [Structure](structure.md): actual Cargo/PyO3 source and manifest/lock ownership
- [Specifications](specs/README.md) and [architecture decisions](adr/README.md): new maintained records when needed,
  not replacements for the existing owners above
- [Agent routing](../.agents/README.md) and [plugin setup](../.agents/plugin-setup.md): shared procedure selection

Shared methods/resources and optional blank forms remain in Common/Rust/Robotics plugins. Preserve existing SDK forms,
contracts and safety policy; add a document only when a real contract or decision needs one. Static guidance checks
and software tests do not establish deployed behavior or physical validation.
