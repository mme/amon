#!/bin/sh
# amon's own hook — NOT vendored from herdr, NOT rewritten by revendor.
# Registered on Claude's UserPromptSubmit to report the submitted prompt as a
# turn boundary (ADR-0021: a seam). herdr collects state, not content, so this
# is amon's alone. Installed and removed by amon-integration/src/prompt_hook.rs.
#
# installed by amon
# managed by amon; `amon remove claude` deletes it and its registration.
# AMON_PROMPT_HOOK_VERSION=1

set -eu

hook_input_file="$(mktemp "${TMPDIR:-/tmp}/amon-claude-prompt.XXXXXX")" || exit 0
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

if str(hook_input.get("hook_event_name") or "") != "UserPromptSubmit":
    raise SystemExit(0)
prompt = hook_input.get("prompt")
if not isinstance(prompt, str) or not prompt.strip():
    raise SystemExit(0)
session_id = hook_input.get("session_id")
agent_session_id = session_id if isinstance(session_id, str) and session_id else None

params = {
    "agent_id": agent_id,
    "source": "amon:claude",
    "agent": "claude",
    "seq": time.time_ns(),
    "prompt": prompt,
}
if agent_session_id:
    params["agent_session_id"] = agent_session_id
request = {"id": "amon-prompt:1", "method": "agent.report_prompt", "params": params}

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
