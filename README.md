# weight-gurus-skill

`weight-gurus-skill` is a small Rust CLI plus agent wrappers for fetching and normalizing live Weight Gurus measurement data.

It ships three integration surfaces:

- `weight-gurus-cli`: standalone CLI for any terminal or agent.
- `skills/weight-gurus`: `SKILL.md`-based skill for Codex, Hermes, and other skill-aware agents.
- `integrations/claude/agents/weight-gurus.md`: Claude Code subagent.

## Scope

This project owns Weight Gurus access and JSON data shaping. It does not update markdown files, Obsidian vaults, spreadsheets, databases, or other downstream stores. Consumers can use the JSON output however they choose.

## Features

- `auth test`
- `auth status`
- `weights list`
- `weights aggregate`
- `setup`
- `WEIGHT_GURUS_CONFIG_PATH` config fallback
- optional macOS Keychain lookup via service `WeightGurus`
- JSON stdout for agent-friendly use
- source-unit inference plus explicit lb/kg conversion

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

## Configure Credentials

For human setup, run:

```bash
weight-gurus-cli setup
```

That opens an interactive wizard when attached to a terminal and writes local config to `~/.config/weight-gurus/config.json` by default.

For agents, scripts, SSH sessions without a TTY, or other non-interactive contexts, pass values explicitly:

```bash
weight-gurus-cli --email "you@example.com" --password "your-password" setup --non-interactive
```

Supported inputs:

- `WEIGHT_GURUS_EMAIL`
- `WEIGHT_GURUS_PASSWORD`
- `WEIGHT_GURUS_BASE_URL` for API overrides and tests
- `WEIGHT_GURUS_CONFIG_PATH` to point at a JSON config file
- macOS only: Keychain service `WeightGurus`

Cross-platform auth behavior:

- macOS: if no credentials are passed, the CLI can fall back to Keychain service `WeightGurus`
- Linux and Windows: pass `--email` and `--password`, or set `WEIGHT_GURUS_EMAIL` and `WEIGHT_GURUS_PASSWORD`

## CLI Examples

```bash
weight-gurus-cli auth test
weight-gurus-cli auth status
weight-gurus-cli setup
weight-gurus-cli --email "you@example.com" --password "your-password" setup --non-interactive
weight-gurus-cli weights list --start 2026-01-01 --end 2026-03-31
weight-gurus-cli weights list --unit kg --start 2026-01-01 --end 2026-03-31
weight-gurus-cli weights aggregate --bucket week --start 2026-01-01 --end 2026-03-31
weight-gurus-cli weights aggregate --bucket month --unit native
```

## Units

The Weight Gurus API does not return an explicit unit marker in the operation payload used here. The observed API values are tenths of the account display unit:

- app shows `178.2 lbs`
- API returns `weight: 1782.0`
- app shows BMI `25.5`
- API returns `bmi: 255`

By default, `weights list` and `weights aggregate` output pounds. Use `--unit kg` for kilograms or `--unit native` to keep the inferred account unit.

Source unit inference:

- `--source-unit auto` is the default.
- Auto inference first uses BMI and weight to decide whether lb or kg produces a plausible height.
- If BMI is unavailable, it falls back to magnitude heuristics.
- Use `--source-unit lb` or `--source-unit kg` to override inference.

Each listed measurement includes the raw API weight, normalized weight, output unit, inferred source unit, and confidence label.

## Command Shape

`weights list` returns normalized measurement entries between optional dates. Date-only `--start` begins at UTC midnight for that date, and date-only `--end` includes the full end date.

```bash
weight-gurus-cli weights list --start yyyy-mm-dd --end yyyy-mm-dd
```

`weights aggregate` groups normalized measurements by day, week, or month:

```bash
weight-gurus-cli weights aggregate --bucket day
weight-gurus-cli weights aggregate --bucket week
weight-gurus-cli weights aggregate --bucket month
```

Deletes are excluded by default. Use `--include-deleted` to include delete operations.

## Install in Codex

Build the CLI first, then run:

```bash
./scripts/install-weight-gurus-skill.sh codex
```

That installs the skill to `${CODEX_HOME:-~/.codex}/skills/weight-gurus` and symlinks the built binary into `bin/weight-gurus-cli`.

## Install in Hermes

Build the CLI first, then run:

```bash
./scripts/install-weight-gurus-skill.sh hermes
```

That installs the skill to `${HERMES_HOME:-~/.hermes}/skills/weight-gurus`.

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

## Testing

```bash
cargo fmt --check
cargo test
```

## Repo Layout

```text
src/                          Rust CLI
tests/                        CLI and behavior tests
skills/weight-gurus/          Codex and Hermes skill
integrations/claude/agents/   Claude Code subagent
scripts/install-weight-gurus-skill.sh
```
