# Plugin and SDK licensing

The SDK declares `Proprietary` in `Cargo.toml`; guidance adoption does not change its license or authorize redistribution.
Audit existing upstream/native/dependency obligations before any distribution; see [contributor licensing](CONTRIBUTING.md#licensing)
and the [publishing guide](.github/PUBLISHING.md).

## Shared procedures and retained local material

This project selects Common, Rust and Robotics identities from `konglabs-agent-skills` through its
[routing](.agents/README.md) and [host setup](.agents/plugin-setup.md). Reusable plugin procedures/resources remain with
the provider and actual host installation, not as local copies. The reviewed baseline is provider `v0.1.1`
(`d56c0780a2d9ce5456d8594fd19d6a19f8bbbb7c`): its
[Common provenance](https://github.com/KongPedia/agent-skills/blob/d56c0780a2d9ce5456d8594fd19d6a19f8bbbb7c/plugins/common/provenance.json),
[Rust provenance](https://github.com/KongPedia/agent-skills/blob/d56c0780a2d9ce5456d8594fd19d6a19f8bbbb7c/plugins/rust/provenance.json)
and [Robotics notices](https://github.com/KongPedia/agent-skills/blob/d56c0780a2d9ce5456d8594fd19d6a19f8bbbb7c/plugins/robotics/THIRD-PARTY-NOTICES.md)
record the provider's selected sources/notices. A requested ref or activation setting does not attest installed bytes.

SDK conventions retain local compatibility/safety adaptations and attribution. The eight remaining local notice/optional-reference
files are inventoried with immutable source identities and original/local hashes in
[skill-sources.json](docs/engineering/skill-sources.json):

- [wshobson MIT notice](docs/engineering/upstream-license.txt): retained optional expert prompts and related guidance
- [Apollo MIT notice](.agents/skills/rust-best-practices/references/apollo-LICENSE.md): Rust guidance adaptations
- [Google license](.agents/skills/rust-best-practices/references/google-styleguide-LICENSE.md) and
  [attribution/licensing declaration](.agents/skills/rust-best-practices/references/google-styleguide-licensing.md):
  Google guide prose CC-BY-3.0 and code samples Apache-2.0; unchanged snapshot now supplied by the shared Rust plugin
- [Matt Pocock MIT notice](.agents/skills/codebase-design/references/matt-LICENSE.md): module-design adaptations
- [Addy Osmani MIT notice](.agents/skills/deprecation-and-migration/references/addy-LICENSE.md): explicit-migration adaptations

The [prior immutable local manifest](https://github.com/KongPedia/unitree-webrtc-rs/blob/a02f40be51d0da1e3726399d5ce83d0c2da8c00a/docs/engineering/skill-sources.json)
preserves the retired procedure-copy history. Retaining legal files does not install or duplicate skill entrypoints.
Preserve applicable attribution when modifying or redistributing material; plugin/private access and static checks
are not legal clearance, a blanket MIT license for this product, or physical safety validation.
