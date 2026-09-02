// amon's own opencode plugin — NOT vendored from herdr, NOT rewritten by
// revendor. Reports Activity over the wrapper socket (ADR-0021: a seam): the
// submitted prompt as a turn boundary, and each tool call and the reply's
// first line as narration. opencode's screen amon has never read; its plugin
// API carries all of it, verified live against gpt-5.6-sol. herdr's own plugin
// beside this one keeps reporting state and session identity; this file
// reports content and nothing else. Installed and removed by
// amon-integration/src/activity_hooks/opencode.rs.
//
// installed by amon
// managed by amon; `amon remove opencode` deletes it.
// AMON_OPENCODE_ACTIVITY_VERSION=1

import * as net from "node:net";

const SOURCE = "amon:opencode-activity";
const AGENT = "opencode";

function enabled() {
  return (
    process.env.AMON_ENV === "1" &&
    !!process.env.AMON_SOCKET_PATH &&
    !!process.env.AMON_AGENT_ID
  );
}

let reportSeq = 0;
function nextReportSeq() {
  reportSeq += 1;
  return reportSeq;
}

function reportActivity(kind, text, sessionID) {
  const trimmed = typeof text === "string" ? text.trim() : "";
  if (!trimmed || !enabled()) {
    return Promise.resolve();
  }
  const paneId = process.env.AMON_AGENT_ID;
  const socketPath = process.env.AMON_SOCKET_PATH;
  const socketEndpoint =
    process.platform === "win32" ? `\\\\.\\pipe\\${socketPath}` : socketPath;

  const params = {
    agent_id: paneId,
    source: SOURCE,
    agent: AGENT,
    seq: nextReportSeq(),
    text: trimmed,
    kind,
  };
  if (sessionID) {
    params.agent_session_id = sessionID;
  }
  const request = {
    id: `${SOURCE}:${kind}:${Date.now()}:${Math.floor(Math.random() * 1e6)}`,
    method: "agent.report_activity",
    params,
  };

  return new Promise((resolve) => {
    const client = net.createConnection(socketEndpoint, () => {
      client.write(`${JSON.stringify(request)}\n`);
    });
    const finish = () => {
      client.destroy();
      resolve();
    };
    client.setTimeout(1000, finish);
    client.on("data", finish);
    client.on("error", finish);
    client.on("end", finish);
    client.on("close", resolve);
  });
}

// The first line of the user's prompt, from the text parts of the message the
// chat.message hook is about to send.
function promptText(output) {
  const parts = output && Array.isArray(output.parts) ? output.parts : [];
  for (const part of parts) {
    if (part && part.type === "text" && typeof part.text === "string" && part.text.trim()) {
      return part.text.split("\n", 1)[0];
    }
  }
  return "";
}

// The harness's own account of a tool call: its name, and the most human of
// its arguments when one is obvious. Formats structured data, composes nothing.
function toolLine(tool, args) {
  const name = typeof tool === "string" && tool ? tool : "tool";
  const record = args && typeof args === "object" ? args : {};
  for (const key of ["command", "cmd", "filePath", "file_path", "path", "pattern", "query", "url"]) {
    const value = record[key];
    if (typeof value === "string" && value.trim()) {
      return `${name}(${value.trim().split("\n", 1)[0].slice(0, 80)})`;
    }
  }
  return name;
}

export const AmonOpencodeActivity = async () => {
  if (!enabled()) {
    return {};
  }

  // opencode reports child (sub-agent) sessions through the same stream; a
  // parent id marks them, and they must not narrate over the root the user is
  // watching — the same guard herdr's own plugin uses.
  const childSessions = new Set();
  const isChild = (sessionID) => sessionID && childSessions.has(sessionID);

  // A text part arrives for the user's own message as well as the assistant's
  // reply; only the assistant's is narration. message.updated maps a message
  // id to its role, and every part carries its messageID, so a bounded map of
  // recent roles tells the two apart. Bounded because a long session must not
  // grow it without limit — the only lookups are for the message being
  // streamed right now.
  const roleByMessage = new Map();
  const rememberRole = (id, role) => {
    if (!id || !role) return;
    roleByMessage.set(id, role);
    if (roleByMessage.size > 64) {
      roleByMessage.delete(roleByMessage.keys().next().value);
    }
  };

  return {
    "chat.message": async (input, output) => {
      const sessionID = input && input.sessionID;
      if (isChild(sessionID)) {
        return;
      }
      if (output && output.message && output.message.role === "user") {
        await reportActivity("prompt", promptText(output), sessionID);
      }
    },
    "tool.execute.before": async (input, output) => {
      const sessionID = input && input.sessionID;
      if (isChild(sessionID)) {
        return;
      }
      await reportActivity(
        "narration",
        toolLine(input && input.tool, output && output.args),
        sessionID
      );
    },
    event: async ({ event }) => {
      const type = event && event.type;
      const props = (event && event.properties) || {};
      const sessionID = props.sessionID;

      // Track child sessions so their activity is dropped.
      const info = props.info;
      if (info && info.id && info.parentID) {
        childSessions.add(info.id);
      }
      if (isChild(sessionID)) {
        return;
      }

      // Track message roles so a part can be attributed to user or assistant.
      if (type === "message.updated" && info && info.id) {
        rememberRole(info.id, info.role);
        return;
      }

      // The assistant's reply, as its text part fills in — the first line, the
      // way Claude's screen shows its ● reply. The user's own message arrives
      // as a text part too; the prompt hook already reported it, so narrating
      // it here would overwrite the prompt with itself.
      if (type === "message.part.updated") {
        const part = props.part;
        if (
          part &&
          part.type === "text" &&
          typeof part.text === "string" &&
          part.text.trim() &&
          roleByMessage.get(part.messageID) === "assistant"
        ) {
          await reportActivity("narration", part.text.split("\n", 1)[0], sessionID);
        }
      }
    },
  };
};
