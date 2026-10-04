#!/bin/sh
# amon's own hook — NOT vendored from herdr, NOT rewritten by revendor.
# Registered on Claude's UserPromptSubmit to report the submitted prompt as a
# turn boundary (ADR-0020: a seam). herdr collects state, not content, so this
# is amon's alone. Installed and removed by amon-integration/src/prompt_hook.rs.
#
# installed by amon
# managed by amon; `amon remove claude` deletes it and its registration.
# AMON_PROMPT_HOOK_VERSION=3

if [ "${AMON_ENV:-}" = "1" ] && [ -n "${AMON_AGENT_ID:-}" ] && [ -n "${AMON_SOCKET_PATH:-}" ]; then
  # The JSON on stdin is read by amon itself, in Rust (issue #77).
  "${AMON_BIN_PATH:-amon}" hook input claude-prompt 2>/dev/null && exit 0
fi
cat >/dev/null 2>&1
exit 0
