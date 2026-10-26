# Project plugin setup

Selected identities are listed in [project routing](README.md). Shared skill procedures, optional hooks and generic role
sources are provided by the central plugins, not locally copied procedure entrypoints. Only product-specific authority
and domain policy remain local. Canonical shared source records and license notices stay with the provider/cache;
this consumer does not maintain a duplicate skill-source/hash or common-license inventory.
A marketplace is a local/private catalog, not a requirement to publish to a public store.

## Register a source once per host


This project uses the host-registered `konglabs-agent-skills` source. For a **new** registration, review private Git
access and deliberately select the shared moving `main` channel (released baseline: `v0.1.1`):

```bash
codex plugin marketplace add KongPedia/agent-skills --ref main
claude plugin marketplace add 'KongPedia/agent-skills#main'
```

If that marketplace is already registered, inspect its source/ref/cache instead of re-adding or retargeting it blindly.
A registration pinned to `v0.1.1` stays frozen; it does not follow newer `main` versions. Each teammate/agent host needs
its own access, source registration and refresh. A deliberately reviewed local checkout is also supported, but Git
marketplace upgrade does not pull that checkout for you. Never copy auth/cache/trust into this repository.


## Automatic Codex installation and recovery

Native Copier runs the selected `codex plugin add` task when template execution is authorized with `--trust` or a
separately trusted template. `--skip-tasks` renders without installation. Python selects Common + Python;
HTTP/framework compositions add only their selected bundles. The Codex CLI and accessible marketplace must already
exist. No skill Markdown is copied into this project. The task does not run for guidance adoption or updates.

If installation reports that a plugin was not found in `konglabs-agent-skills`, first register the reviewed
source using the commands above. Registration is once per host, not once per project. If the same named source is
already registered, inspect its source/cache instead of blindly retargeting it. Then use the selected recovery commands
below. A missing Codex CLI or failed add prints a prerequisite/registration hint and stops the chain.

Copier can merge into existing directories and may clean up a new destination on failure. Always generate into a new
path, inspect any retained output before retrying, and never recopy over an existing project as recovery. Earlier host
installs are not rolled back; never uninstall unrelated/pre-existing host plugins as rollback.

Codex recovery: first verify the registered marketplace source and cached revision; these commands use that host index.

```bash
codex plugin add common@konglabs-agent-skills
codex plugin add robotics@konglabs-agent-skills
codex plugin add rust@konglabs-agent-skills
```

```bash
claude plugin install common@konglabs-agent-skills --scope project
claude plugin install robotics@konglabs-agent-skills --scope project
claude plugin install rust@konglabs-agent-skills --scope project
```

The host manages downloads/cache; Codex installation globally enables selected bundles. Copier `--trust` authorizes
template execution, not Codex project/hook trust. Trust this project through the native UI
before relying on its team true/false flags; until then host-wide enabled plugins can remain active. Untrusted Codex projects may ignore project overrides. Preserve unrelated
user plugins/settings. Codex startup synchronization is asynchronous; false disables loading, not necessarily fetching.
Verify effective installed versions and task-relevant skill discovery in a fresh session. Generated flags alone do not
prove provisioning, discovery or model adherence.

For guidance-only adoption, existing host configs are preserved: manually merge just the required plugin/source entries,
retaining unrelated permissions, models and integrations. New-project configs use Copier three-way merging; inspect conflicts.

## Hooks and agents

The optional `session-guidance` advisory is off. It performs only a short
SessionStart reminder. Review hook definitions and trust separately using `/hooks` where supported; respect hook-disable
policy. Installation/enablement is not hook execution or permission to run tests, apps or hardware.
This project does not maintain `.codex/agents` role copies. Generic role definitions belong to the shared Common
provider. Claude can expose Common's native plugin agents when installed/enabled; verify actual discovery separately.
For Codex, use available built-in/host agents with the selected Common procedures: inspected clients do not establish
custom-agent TOML autoload from plugins. Do not copy exported TOMLs, symlink caches or invent manifest registration.
Role labels describe responsibilities, not guaranteed native agent IDs. Preserve local domain/safety policy through
AGENTS/workflow/engineering pointers rather than duplicating it in role prompts; inherit the available session model.

## Updates

Shared skills: refresh the registered source, inspect installed versions, then reload/start a session.
For a reviewed Git source that follows `main`:

```bash
codex plugin marketplace upgrade konglabs-agent-skills
codex plugin list --marketplace konglabs-agent-skills --json
```

Verify every selected installed version and start a fresh session. Routine shared skill updates with unchanged bundle
identities need no product commit or Copier update. A newly selected bundle needs installation and a reviewed activation
change; a new skill inside an already enabled bundle becomes available through that bundle's next reviewed release. Claude needs
marketplace refresh and selected installed-plugin updates; source refresh alone is not installed-payload proof.
Source refs and caches are host-managed, not independent per-project version locks.

Project integration: with clean committed history and retained answers/prior source, review both template revisions, run `uvx copier update --trust --skip-tasks` and inspect
the diff. Copier task-metadata trust is separate from Codex project/hook trust; no installation runs on update.
The recorded prior template revision must remain available; an answers file alone is not update-ready enrollment.
Product seeds and root authority remain project-owned; a profile change does not migrate code or folders.
Earlier draft layouts/delivery modes require reviewed temporary generation/re-enrollment, not forced overwrite.
Keep local rules in project-owned files. Never edit host caches, credentials or trust records to bypass a failure.

[OpenAI packaging](https://developers.openai.com/plugins/build/plugins) ·
[Claude installation](https://code.claude.com/docs/en/plugins/cli-reference)

## Keep native integration state

`.copier-answers.yml` records the template source/revision for genuine Copier updates; it is not the retired
`skill-sources.json` or a per-project plugin-version lock. Do not remove or hand-rewrite it. `.codex/config.toml` and
`.claude/settings.json` record plugin enablement. Shared skill updates remain host-owned, separate from these files.
