---
name: weight-gurus
description: Use when fetching Weight Gurus measurements, validating Weight Gurus auth, exporting dated weight entries, or producing JSON summaries from Weight Gurus data.
---

# Weight Gurus

Use `weight-gurus-cli` for live Weight Gurus measurement access. Keep command output as JSON and avoid printing secrets. This skill fetches and normalizes Weight Gurus data only; downstream note, spreadsheet, database, or dashboard updates are outside this skill.

## Key Inputs

- `WEIGHT_GURUS_EMAIL` / `WEIGHT_GURUS_PASSWORD`, or pass `--email` / `--password`.
- `WEIGHT_GURUS_CONFIG_PATH` for setup persistence and offline status checks.
- `WEIGHT_GURUS_BASE_URL` to override API base (default `https://api.weightgurus.com`).
- macOS only: Keychain service `WeightGurus` when explicit credentials are omitted.

## Command Map

- Run setup for a human user in a terminal:
  - `weight-gurus-cli setup`
- Run setup from an agent or script:
  - `weight-gurus-cli --email "$WEIGHT_GURUS_EMAIL" --password "$WEIGHT_GURUS_PASSWORD" setup --non-interactive`
- Check credential readiness:
  - `weight-gurus-cli auth status`
- Check credentials:
  - `weight-gurus-cli auth test`
- List normalized measurements:
  - `weight-gurus-cli weights list [--start yyyy-mm-dd] [--end yyyy-mm-dd] [--unit lb|kg|native]`
- Aggregate measurements:
  - `weight-gurus-cli weights aggregate [--bucket day|week|month] [--start yyyy-mm-dd] [--end yyyy-mm-dd] [--unit lb|kg|native]`

## Behavior Notes

- `--start`/`--end` are filtered client-side using `entryTimestamp`; date-only `--end` includes the full end date.
- Delete operations are excluded by default; pass `--include-deleted` to include them.
- API weights are normalized from tenths of the account display unit.
- `--source-unit auto` is the default and uses BMI when present to infer lb vs kg.
- Use `--source-unit lb` or `--source-unit kg` when a user explicitly knows the account unit.
- Default output unit is pounds. Use `--unit kg` for kilograms or `--unit native` for the inferred account unit.
- Weekly summaries are derived aggregates, not a Weight Gurus API endpoint.

## Agent Usage

- Prefer `weights list` when the user asks for their actual entries.
- Prefer `weights aggregate --bucket week` only when the user asks for grouped or summary data.
- Do not run interactive `setup` from non-TTY agent contexts; use `setup --non-interactive` with explicit credentials or environment-backed global flags.
- Use `weight-gurus-cli` installed under `bin/` when available, else PATH fallback.
- This skill folder is intentionally portable across agent tools that support `SKILL.md`; `agents/openai.yaml` is only for Codex UI metadata.
