---
name: weight-gurus
description: Use when working with live Weight Gurus logs, including raw pulls and Markdown vault updates.
---

# Weight Gurus

Use `weight-gurus-cli` for live Weight Gurus pulls and markdown note updates. Keep command output as JSON and avoid printing secrets. Use this skill for extracting weight rows, weekly aggregates, and controlled note updates.

## Key Inputs

- `WEIGHT_GURUS_EMAIL` / `WEIGHT_GURUS_PASSWORD`, or pass `--email` / `--password`.
- `WEIGHT_GURUS_CONFIG_PATH` for setup persistence and offline status checks.
- `WEIGHT_GURUS_BASE_URL` to override API base (default `https://api.weightgurus.com`).
- `WEIGHT_GURUS_NOTE_PATH` for vault commands, or pass `--file /path/to/note.md`.
- macOS only: Keychain service `WeightGurus` when explicit credentials are omitted.

## Command Map

- Run setup:
  - `weight-gurus-cli setup`
  - `weight-gurus-cli setup --write`
- Check credential readiness:
  - `weight-gurus-cli auth status`
- Check credentials:
  - `weight-gurus-cli auth test`
- Pull raw operations:
  - `weight-gurus-cli weights raw [--start yyyy-mm-dd] [--end yyyy-mm-dd]`
- Pull weekly rollup:
  - `weight-gurus-cli weights weekly [--start yyyy-mm-dd] [--end yyyy-mm-dd]`
- Preview vault update:
  - `weight-gurus-cli vault preview [--file "/path/to/note"] [--start ...] [--end ...]`
- Apply vault update (explicit confirmation required):
  - `weight-gurus-cli vault update --confirm [--file "/path/to/note"] [--start ...] [--end ...]`

## Behavior Notes

- `--start`/`--end` are filtered client-side using `entryTimestamp`.
- Weight values are normalized with the existing heuristic:
  - if sampled value > 1000, divide by 10.
- `vault` update operations target only the section whose heading contains:
  - `Weight Log`, `Weight Logs`, optional `and DEXA`.
- Duplicate Mermaid charts are deduplicated and one canonical weekly chart is kept.
- Vault writes are never allowed without `--confirm`.

## Agent Usage

- Prefer `vault preview` before any update.
- Use `weight-gurus-cli` installed under `bin/` when available, else PATH fallback.
- This skill folder is intentionally portable across agent tools that support `SKILL.md`; `agents/openai.yaml` is only for Codex UI metadata.
