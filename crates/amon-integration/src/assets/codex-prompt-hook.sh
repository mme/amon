#!/bin/sh
# amon's own hook — NOT vendored from herdr, NOT rewritten by revendor.
# Registered on Codex's UserPromptSubmit to report the submitted prompt as a
# turn boundary (ADR-0020: a seam). Codex's hook engine mirrors Claude's —
# behind `[features] hooks = true`, registered in ~/.codex/hooks.json — and
# herdr registers only SessionStart there, so this event is amon's alone.
# Installed and removed by amon-integration/src/activity_hooks/codex.rs.
#
# installed by amon
# managed by amon; `amon remove codex` deletes it and its registration.
# AMON_CODEX_PROMPT_HOOK_VERSION=2

if [ "${AMON_ENV:-}" = "1" ] && [ -n "${AMON_AGENT_ID:-}" ] && [ -n "${AMON_SOCKET_PATH:-}" ]; then
  # The JSON on stdin is read by amon itself, in Rust (issue #77).
  "${AMON_BIN_PATH:-amon}" hook input codex-prompt 2>/dev/null && exit 0
fi
cat >/dev/null 2>&1
exit 0
