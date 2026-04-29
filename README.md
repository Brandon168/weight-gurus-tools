# weight-gurus-skill

`weight-gurus-skill` is a small Rust CLI plus agent wrappers for pulling live Weight Gurus data and updating a markdown weight log safely.

It ships three integration surfaces:

- `weight-gurus-cli`: standalone CLI for any terminal or agent.
- `skills/weight-gurus`: `SKILL.md`-based skill for Codex and Hermes.
- `integrations/claude/agents/weight-gurus.md`: Claude Code subagent.

## Is this "standard"?

Yes, for the skill bundle. OpenAI documents `SKILL.md` skills as following the Agent Skills open standard, and this repo packages the skill in that format.

- Codex: uses `~/.codex/skills/<name>/SKILL.md`
- Hermes: uses `~/.hermes/skills/<name>/SKILL.md`
- Any other `SKILL.md`-aware tool: install the same folder into that tool's skill location
- Claude Code: uses subagents in `~/.claude/agents/*.md` or slash commands in `~/.claude/commands/*.md`

This repo follows each tool's native extension point instead of forcing one layout everywhere.

## Features

- `auth test`
- `auth status`
- `weights raw`
- `weights weekly`
- `setup`
- `vault preview`
- `vault update --confirm`
- `WEIGHT_GURUS_CONFIG_PATH` config fallback (JSON file)
- optional macOS Keychain lookup via service `WeightGurus`
- JSON stdout for agent-friendly use
- explicit write confirmation for markdown updates

## Install the CLI

Recommended install path:

- Download the latest GitHub Release for your platform and extract the matching asset.

Platform assets:

- Linux: `weight-gurus-cli-linux-x86_64.tar.gz`
- macOS (Apple Silicon): `weight-gurus-cli-macos-aarch64.tar.gz`
- Windows: `weight-gurus-cli-windows-x86_64.zip`

```bash
gh release download --repo Brandon168/weight-gurus-tools --pattern "weight-gurus-cli-linux-x86_64.tar.gz"
gh release download --repo Brandon168/weight-gurus-tools --pattern "weight-gurus-cli-macos-aarch64.tar.gz"
gh release download --repo Brandon168/weight-gurus-tools --pattern "weight-gurus-cli-windows-x86_64.zip"
```

Unpack and run `weight-gurus-cli` from the extracted archive.

Or build locally:

```bash
cargo install --path . --locked
```

## Configure credentials

For human setup, run:

```bash
weight-gurus-cli setup
```

That opens an interactive wizard when attached to a terminal and writes local config to `~/.config/weight-gurus/config.json` by default.

For agents, scripts, SSH sessions without a TTY, or other non-interactive contexts, pass values explicitly:

```bash
weight-gurus-cli --email "you@example.com" --password "your-password" setup --non-interactive --note-path "/path/to/Weight Note.md"
```

Supported inputs:

- `WEIGHT_GURUS_EMAIL`
- `WEIGHT_GURUS_PASSWORD`
- `WEIGHT_GURUS_BASE_URL` for API overrides and tests
- `WEIGHT_GURUS_NOTE_PATH` for markdown update commands
- `WEIGHT_GURUS_CONFIG_PATH` to point at a JSON config file
- `setup` stores local settings to `~/.config/weight-gurus/config.json` by default
- macOS only: Keychain service `WeightGurus`

You can also pass `--email`, `--password`, and `--file` explicitly.

Cross-platform auth behavior:

- macOS: if no credentials are passed, the CLI can fall back to Keychain service `WeightGurus`
- Linux and Windows: pass `--email` and `--password`, or set `WEIGHT_GURUS_EMAIL` and `WEIGHT_GURUS_PASSWORD`
- All platforms: `--file` or `WEIGHT_GURUS_NOTE_PATH` is required for markdown update commands

## Install in Codex

Build the CLI first, then run:

```bash
./scripts/install-weight-gurus-skill.sh codex
```

That installs the skill to `${CODEX_HOME:-~/.codex}/skills/weight-gurus` and symlinks the built binary into `bin/weight-gurus-cli`.

Use it in Codex by selecting the `weight-gurus` skill when needed.

## Install in Hermes

Build the CLI first, then run:

```bash
./scripts/install-weight-gurus-skill.sh hermes
```

That installs the skill to `${HERMES_HOME:-~/.hermes}/skills/weight-gurus`.

In Hermes, installed skills become available as slash commands. Example:

```text
/weight-gurus pull my weekly Weight Gurus summary for the last 90 days
```

## Install in Claude Code

Claude Code does not use `SKILL.md` folders as its primary extension format. The supported fit here is a subagent.

1. Install the CLI onto your PATH:

```bash
cargo install --path . --locked
```

2. Install the Claude Code subagent:

```bash
./scripts/install-weight-gurus-skill.sh claude
```

That copies `integrations/claude/agents/weight-gurus.md` to `${CLAUDE_HOME:-~/.claude}/agents/weight-gurus.md`.

Then in Claude Code you can ask:

```text
Use the weight-gurus subagent to preview an update to my Weight Logs note.
```

## CLI examples

```bash
weight-gurus-cli auth test
weight-gurus-cli auth status
weight-gurus-cli setup
weight-gurus-cli --email "you@example.com" --password "your-password" setup --non-interactive --note-path "/path/to/Weight Note.md"
weight-gurus-cli weights raw --start 2026-01-01 --end 2026-03-31
weight-gurus-cli weights weekly --start 2026-01-01 --end 2026-03-31
weight-gurus-cli vault preview --file "/path/to/Weight Note.md"
weight-gurus-cli vault update --file "/path/to/Weight Note.md" --confirm
```

## Markdown updater assumptions

The updater is intentionally narrow:

- It looks for headings containing `Weight Log`, `Weight Logs`, or `Weight Log and DEXA`.
- It removes duplicate Mermaid charts in that section and inserts one canonical chart.
- It only writes when `--confirm` is present.
- It leaves content outside the matched section unchanged.

## Testing

```bash
cargo fmt --check
cargo test
```

## Repo layout

```text
src/                          Rust CLI
tests/                        CLI and behavior tests
skills/weight-gurus/          Codex and Hermes skill
integrations/claude/agents/   Claude Code subagent
scripts/install-weight-gurus-skill.sh
```
