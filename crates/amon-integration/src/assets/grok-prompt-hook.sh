#!/bin/sh
# amon's own hook — NOT vendored from herdr, NOT rewritten by revendor.
# Reports grok's submitted prompt as a turn boundary (ADR-0020: a seam). grok
# merges every ~/.grok/hooks/*.json, so this hook's config is a dedicated amon
# file that edits nobody else's — the purest seam of all. herdr registers only
# session_start there; user_prompt_submit is amon's alone.
# Installed and removed by amon-integration/src/activity_hooks/grok.rs.
#
# installed by amon
# managed by amon; `amon remove grok` deletes it and its config.
# AMON_GROK_PROMPT_HOOK_VERSION=2

if [ "${AMON_ENV:-}" = "1" ] && [ -n "${AMON_AGENT_ID:-}" ] && [ -n "${AMON_SOCKET_PATH:-}" ]; then
  # The JSON on stdin is read by amon itself, in Rust (issue #77).
  "${AMON_BIN_PATH:-amon}" hook input grok-prompt 2>/dev/null && exit 0
fi
cat >/dev/null 2>&1
exit 0
