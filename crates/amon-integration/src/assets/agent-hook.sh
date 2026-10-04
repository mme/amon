#!/bin/sh
# amon's own hook, in place of herdr's (ADR-0020: a supersession; issue #77).
# The JSON the agent hands its hook is read by `amon hook input`, in Rust, by
# the same rules herdr's script applied with an interpreter; nothing here
# needs one. Written over the file herdr's installer just wrote, keeping its
# markers, by amon-integration/src/superseded_hooks.rs.
#
# installed by amon
# managed by amon; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# AMON_INTEGRATION_ID=__ID__
# AMON_INTEGRATION_VERSION=__VERSION__
# AMON_HOOK_INPUT=1

if [ "${AMON_ENV:-}" = "1" ] && [ -n "${AMON_AGENT_ID:-}" ] && [ -n "${AMON_SOCKET_PATH:-}" ]; then
  "${AMON_BIN_PATH:-amon}" hook input __HOOK__ "$@" 2>/dev/null && exit 0
fi
cat >/dev/null 2>&1
__FALLBACK__exit 0
