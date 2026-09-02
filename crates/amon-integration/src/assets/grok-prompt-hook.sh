#!/bin/sh
# amon's own hook — NOT vendored from herdr, NOT rewritten by revendor.
# Reports grok's submitted prompt as a turn boundary (ADR-0021: a seam). grok
# merges every ~/.grok/hooks/*.json, so this hook's config is a dedicated amon
# file that edits nobody else's — the purest seam of all. herdr registers only
# session_start there; user_prompt_submit is amon's alone.
# Installed and removed by amon-integration/src/activity_hooks/grok.rs.
#
# installed by amon
# managed by amon; `amon remove grok` deletes it and its config.
# AMON_GROK_PROMPT_HOOK_VERSION=1

set -eu

hook_input_file="$(mktemp "${TMPDIR:-/tmp}/amon-grok-prompt.XXXXXX")" || exit 0
trap 'rm -f "$hook_input_file"' EXIT HUP INT TERM
cat >"$hook_input_file" 2>/dev/null || true

[ "${AMON_ENV:-}" = "1" ] || exit 0
[ -n "${AMON_SOCKET_PATH:-}" ] || exit 0
[ -n "${AMON_AGENT_ID:-}" ] || exit 0
command -v python3 >/dev/null 2>&1 || exit 0

AMON_HOOK_INPUT_FILE="$hook_input_file" python3 - <<'PY'
import json
import os
import socket
import time

agent_id = os.environ.get("AMON_AGENT_ID")
socket_path = os.environ.get("AMON_SOCKET_PATH")
hook_input_file = os.environ.get("AMON_HOOK_INPUT_FILE")
if not agent_id or not socket_path:
    raise SystemExit(0)

hook_input = {}
if hook_input_file:
    try:
        with open(hook_input_file, encoding="utf-8") as handle:
            content = handle.read()
        if content.strip():
            hook_input = json.loads(content)
    except Exception:
        hook_input = {}


def first_text(*keys):
    for key in keys:
        value = hook_input.get(key)
        if isinstance(value, str) and value.strip():
            return value
    return None


# grok emits event and field names in several casings; accept them all, the
# way herdr's vendored grok hook does.
event = first_text("hook_event_name", "hookEventName") or ""
if event not in ("user_prompt_submit", "userPromptSubmit", "UserPromptSubmit"):
    raise SystemExit(0)
prompt = first_text("prompt", "userPrompt", "user_prompt", "message", "text")
if not prompt:
    raise SystemExit(0)
session_id = os.environ.get("GROK_SESSION_ID") or first_text("session_id", "sessionId")
agent_session_id = session_id if isinstance(session_id, str) and session_id else None

params = {
    "agent_id": agent_id,
    "source": "amon:grok",
    "agent": "grok",
    "seq": time.time_ns(),
    "text": prompt,
    "kind": "prompt",
}
if agent_session_id:
    params["agent_session_id"] = agent_session_id
request = {"id": "amon-prompt:1", "method": "agent.report_activity", "params": params}

try:
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.settimeout(0.5)
    client.connect(socket_path)
    client.sendall((json.dumps(request) + "\n").encode())
    try:
        client.recv(4096)
    except Exception:
        pass
    client.close()
except Exception:
    pass
PY
