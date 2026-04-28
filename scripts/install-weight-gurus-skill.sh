#!/bin/sh
set -eu

REPO_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SKILL_SRC="$REPO_ROOT/skills/weight-gurus"
CLAUDE_AGENT_SRC="$REPO_ROOT/integrations/claude/agents/weight-gurus.md"
CLI_SRC="$REPO_ROOT/target/release/weight-gurus-cli"

TARGET="${1:-codex}"
case "$TARGET" in
  codex)
    if [ -n "${SKILLS_HOME:-}" ]; then
      SKILL_DEST="$SKILLS_HOME/weight-gurus"
    elif [ -n "${CODEX_HOME:-}" ]; then
      SKILL_DEST="$CODEX_HOME/skills/weight-gurus"
    else
      SKILL_DEST="$HOME/.codex/skills/weight-gurus"
    fi
    DEST_KIND="skill"
    ;;
  claude)
    SKILL_DEST="${CLAUDE_HOME:-$HOME/.claude}/agents/weight-gurus.md"
    DEST_KIND="file"
    ;;
  hermes)
    SKILL_DEST="${HERMES_HOME:-$HOME/.hermes}/skills/weight-gurus"
    DEST_KIND="skill"
    ;;
  *)
    echo "Usage: $0 [codex|claude|hermes]" >&2
    exit 2
    ;;
esac

if [ -n "${2:-}" ]; then
  echo "Usage: $0 [codex|claude|hermes]" >&2
  exit 2
fi

if [ -x "$CLI_SRC" ]; then
  CLI_PATH="$CLI_SRC"
elif command -v weight-gurus-cli >/dev/null 2>&1; then
  CLI_PATH=$(command -v weight-gurus-cli)
else
  echo "weight-gurus-cli was not found." >&2
  echo "Build it first with: cargo build --release --bin weight-gurus-cli" >&2
  exit 1
fi

mkdir -p "$(dirname "$SKILL_DEST")"
if [ "$DEST_KIND" = "skill" ]; then
  rm -rf "$SKILL_DEST"
  cp -R "$SKILL_SRC" "$SKILL_DEST"
  mkdir -p "$SKILL_DEST/bin"
  ln -sf "$CLI_PATH" "$SKILL_DEST/bin/weight-gurus-cli"
  echo "Installed Weight Gurus skill to $SKILL_DEST"
  echo "Linked weight-gurus-cli from $CLI_PATH"
else
  cp "$CLAUDE_AGENT_SRC" "$SKILL_DEST"
  echo "Installed Claude Code agent to $SKILL_DEST"
  echo "Agent expects weight-gurus-cli on PATH: $CLI_PATH"
fi
