// amon's own pi extension — NOT vendored from herdr, NOT rewritten by revendor.
// Reports Activity over the wrapper socket (ADR-0021: a seam): the submitted
// prompt as a turn boundary, and tool calls / the reply's first line as
// narration. pi's screen tells prompts, tool calls and replies apart by
// background colour alone, which amon's text reader cannot see — these events
// are the only Narration pi has. herdr's own extension beside this one keeps
// reporting state and session identity; this file reports content and nothing
// else. Installed and removed by amon-integration/src/activity_hooks/pi.rs.
//
// installed by amon
// managed by amon; `amon remove pi` deletes it.
// AMON_PI_ACTIVITY_VERSION=1

import * as net from "node:net";

const source = "amon:pi-activity";
const AMON_ENV = process.env.AMON_ENV;
const socketPath = process.env.AMON_SOCKET_PATH;
const socketEndpoint =
  process.platform === "win32" && socketPath ? `\\\\.\\pipe\\${socketPath}` : socketPath;
const paneId = process.env.AMON_AGENT_ID;

function enabled() {
  return AMON_ENV === "1" && !!socketPath && !!paneId;
}

function sendRequestAttempt(request: unknown, timeoutMs: number): Promise<boolean> {
  if (!enabled()) {
    return Promise.resolve(true);
  }
  return new Promise((resolve) => {
    let done = false;
    let timeout: ReturnType<typeof setTimeout> | undefined;
    const finish = (delivered: boolean) => {
      if (done) return;
      done = true;
      if (timeout) clearTimeout(timeout);
      socket.destroy();
      resolve(delivered);
    };
    const socket = net.createConnection(socketEndpoint!);
    socket.on("error", () => finish(false));
    socket.on("connect", () => socket.write(`${JSON.stringify(request)}\n`));
    socket.on("data", () => finish(true));
    socket.on("end", () => finish(false));
    timeout = setTimeout(() => finish(false), timeoutMs);
    timeout.unref?.();
  });
}

async function sendRequest(request: unknown): Promise<void> {
  if (await sendRequestAttempt(request, 500)) {
    return;
  }
  await sendRequestAttempt(request, 1500);
}

let reportSeq = 0;
function nextReportSeq(): number {
  reportSeq += 1;
  return reportSeq;
}

let currentAgentSessionId: string | undefined;

function updateSessionRef(ctx: any): void {
  try {
    const id = ctx?.sessionManager?.getSessionId?.();
    currentAgentSessionId = typeof id === "string" && id.length > 0 ? id : undefined;
  } catch {
    currentAgentSessionId = undefined;
  }
}

function reportActivity(kind: "prompt" | "narration", text: string): Promise<void> {
  const trimmed = typeof text === "string" ? text.trim() : "";
  if (!trimmed) {
    return Promise.resolve();
  }
  const params: Record<string, unknown> = {
    agent_id: paneId,
    source,
    agent: "pi",
    seq: nextReportSeq(),
    text: trimmed,
    kind,
  };
  if (currentAgentSessionId) {
    params.agent_session_id = currentAgentSessionId;
  }
  return sendRequest({
    id: `${source}:${kind}:${Date.now()}:${Math.random().toString(36).slice(2)}`,
    method: "agent.report_activity",
    params,
  });
}

// The harness's own account of a tool call, from its structured event: the
// tool's name, and the most human of its arguments when one is obvious. This
// formats the harness's data, it does not compose prose about it.
function toolLine(toolName: unknown, args: unknown): string {
  const name = typeof toolName === "string" && toolName ? toolName : "tool";
  const record = args && typeof args === "object" ? (args as Record<string, unknown>) : {};
  for (const key of ["command", "cmd", "path", "file_path", "filePath", "pattern", "query", "url"]) {
    const value = record[key];
    if (typeof value === "string" && value.trim()) {
      const first = value.trim().split("\n", 1)[0].slice(0, 80);
      return `${name}(${first})`;
    }
  }
  return name;
}

// The reply's opening line, the way Claude's screen shows its ● reply.
function firstAssistantLine(message: any): string {
  const content = message?.content;
  if (typeof content === "string") {
    return content.split("\n", 1)[0];
  }
  if (Array.isArray(content)) {
    for (const block of content) {
      if (block && typeof block === "object" && typeof (block as any).text === "string") {
        const line = (block as any).text.split("\n", 1)[0].trim();
        if (line) return line;
      }
    }
  }
  return "";
}

export default function (pi: any) {
  if (!enabled()) {
    return;
  }

  // Only the root TUI session reports, the same gate herdr's own extension
  // uses: RPC/JSON/print modes are headless, and subagents must not narrate
  // over the row of the agent the user is watching.
  let rootSession = false;

  pi.on("session_start", async (_event: any, ctx: any) => {
    if (ctx?.mode !== "tui") {
      return;
    }
    rootSession = true;
    updateSessionRef(ctx);
  });

  pi.on("before_agent_start", async (event: any, ctx: any) => {
    if (!rootSession) {
      return;
    }
    updateSessionRef(ctx);
    await reportActivity("prompt", event?.prompt ?? "");
  });

  pi.on("tool_execution_start", async (event: any, _ctx: any) => {
    if (!rootSession) {
      return;
    }
    await reportActivity("narration", toolLine(event?.toolName, event?.args));
  });

  pi.on("message_end", async (event: any, _ctx: any) => {
    if (!rootSession || event?.message?.role !== "assistant") {
      return;
    }
    await reportActivity("narration", firstAssistantLine(event.message));
  });
}
