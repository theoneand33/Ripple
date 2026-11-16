// Minimal T3 Code client. Each device is { url, token } where token comes from
// `t3 auth session issue` on that machine, so any reachable T3 server works.
// Linked environments (T3's peer links) are only reachable through /mcp tools
// that take `environmentId`, so for those we sign in to /mcp as an MCP client.
import { createHash, randomBytes, randomUUID } from "node:crypto";
import fs from "node:fs";

const base = (device) => String(device.url || "").trim().replace(/\/+$/, "");
const auth = (device) => ({ authorization: `Bearer ${String(device.token || "").trim()}` });

async function request(url, { method = "GET", headers = {}, body } = {}) {
  const res = await fetch(url, { method, headers, body, signal: AbortSignal.timeout(15000) })
    .catch(() => { throw new Error(`Could not reach ${new URL(url).origin}`); });
  const text = await res.text();
  let data;
  try { data = JSON.parse(text); } catch { data = null; }
  if (!res.ok) throw new Error(data?.error_description || data?.message || `T3 ${new URL(url).pathname} failed (${res.status})`);
  return data;
}

const getJson = (device, path, init = {}) => request(base(device) + path, {
  ...init,
  headers: { ...auth(device), ...(init.body ? { "content-type": "application/json" } : {}) },
});

// One-shot Effect RPC call over T3's /ws endpoint.
async function rpc(device, tag, payload) {
  const { ticket } = await getJson(device, "/api/auth/websocket-ticket", { method: "POST" });
  const wsUrl = `${base(device).replace(/^http/, "ws")}/ws?orchestrationProtocol=2&wsTicket=${encodeURIComponent(ticket)}`;
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(wsUrl);
    const done = (fn, value) => { clearTimeout(timer); ws.close(); fn(value); };
    const timer = setTimeout(() => done(reject, new Error("T3 request timed out")), 15000);
    ws.onopen = () => ws.send(JSON.stringify({ _tag: "Request", id: "1", tag, payload, headers: [] }));
    ws.onerror = () => done(reject, new Error("Could not connect to T3"));
    ws.onmessage = (event) => {
      const msg = JSON.parse(String(event.data));
      if (msg._tag !== "Exit") return;
      if (msg.exit._tag === "Success") return done(resolve, msg.exit.value);
      const err = msg.exit.cause?.[0]?.error;
      done(reject, new Error(err?.cause?.cause || err?.cause?.message || err?.message || "T3 request failed"));
    };
  });
}

// MCP tokens last 30 days; persist them so each app start doesn't add another
// "Ripple" client to the device's Sharing & access list.
let tokenFile = null;
let mcpTokens = {};
export function setT3TokenFile(file) {
  tokenFile = file;
  try { mcpTokens = JSON.parse(fs.readFileSync(file, "utf8")); } catch { mcpTokens = {}; }
}
const saveTokens = () => {
  if (tokenFile) fs.writeFileSync(tokenFile, JSON.stringify(mcpTokens), { mode: 0o600 });
};
const tokenKey = (device) => createHash("sha256").update(`${base(device)}\n${String(device.token || "").trim()}`).digest("hex");

// Never opened: the authorization code is read straight from the redirect URL,
// the same way T3's own peer links sign in.
const REDIRECT_URI = "http://127.0.0.1/ripple";

// Mints a one-time pairing code with the device's bearer token, then spends it
// on /mcp's OAuth flow: register, decide, exchange.
async function signInMcp(device) {
  const origin = base(device);
  const resource = `${origin}/mcp`;
  const json = { "content-type": "application/json" };
  const { credential } = await getJson(device, "/api/auth/pairing-token", {
    method: "POST",
    body: JSON.stringify({ label: "Ripple", access: "full-access" }),
  });
  const client = await request(`${origin}/oauth/mcp/register`, {
    method: "POST", headers: json,
    body: JSON.stringify({ client_name: "Ripple", redirect_uris: [REDIRECT_URI] }),
  });
  const verifier = randomBytes(32).toString("base64url");
  const state = randomBytes(16).toString("base64url");
  const decided = await request(`${origin}/oauth/mcp/decision`, {
    method: "POST", headers: json,
    body: JSON.stringify({
      authorization: {
        response_type: "code",
        client_id: client.client_id,
        redirect_uri: REDIRECT_URI,
        code_challenge: createHash("sha256").update(verifier).digest("base64url"),
        code_challenge_method: "S256",
        state,
        resource,
      },
      decision: { _tag: "pairing-code", access: "full-access", code: credential },
    }),
  });
  const redirect = new URL(decided.redirectTo);
  const code = redirect.searchParams.get("code");
  if (!code || redirect.searchParams.get("state") !== state) {
    throw new Error(redirect.searchParams.get("error_description") || "T3 did not approve Ripple");
  }
  const token = await request(`${origin}/oauth/mcp/token`, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({ grant_type: "authorization_code", code, redirect_uri: REDIRECT_URI, client_id: client.client_id, code_verifier: verifier, resource }),
  });
  // Renew a day early so a call never races the expiry.
  mcpTokens[tokenKey(device)] = { token: token.access_token, expiresAt: Date.now() + (token.expires_in - 86400) * 1000 };
  saveTokens();
}

async function mcpToken(device) {
  const cached = mcpTokens[tokenKey(device)];
  if (!cached || cached.expiresAt < Date.now()) await signInMcp(device);
  return mcpTokens[tokenKey(device)].token;
}

// One tools/call over MCP streamable HTTP.
async function mcp(device, name, args, retried = false) {
  const token = await mcpToken(device);
  let sessionId;
  let id = 0;
  const post = async (message) => {
    const headers = {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
      accept: "application/json, text/event-stream",
      "mcp-protocol-version": "2025-06-18",
      ...(sessionId ? { "mcp-session-id": sessionId } : {}),
    };
    const res = await fetch(`${base(device)}/mcp`, { method: "POST", headers, body: JSON.stringify(message), signal: AbortSignal.timeout(30000) });
    if (res.status === 401) return { unauthorized: true };
    sessionId = res.headers.get("mcp-session-id") || sessionId;
    const text = await res.text();
    if (!res.ok) throw new Error(`T3 /mcp failed (${res.status})`);
    if (message.id === undefined) return {};
    const body = res.headers.get("content-type")?.includes("text/event-stream")
      ? text.split("\n").filter((l) => l.startsWith("data:")).map((l) => l.slice(5)).pop()
      : text;
    return JSON.parse(body);
  };
  const init = await post({ jsonrpc: "2.0", id: ++id, method: "initialize", params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "Ripple", version: "1" } } });
  if (init.unauthorized) {
    // Revoked in T3's Sharing & access: sign in again once.
    if (retried) throw new Error("T3 rejected Ripple's MCP sign-in");
    delete mcpTokens[tokenKey(device)];
    return mcp(device, name, args, true);
  }
  await post({ jsonrpc: "2.0", method: "notifications/initialized", params: {} });
  const res = await post({ jsonrpc: "2.0", id: ++id, method: "tools/call", params: { name, arguments: args } });
  if (res.error) throw new Error(res.error.message);
  if (res.result.isError) {
    const text = res.result.content?.find((c) => c.text)?.text || "T3 tool failed";
    let message = text;
    try { message = JSON.parse(text).message || text; } catch {}
    throw new Error(message);
  }
  return res.result.structuredContent;
}

export async function t3Load(device) {
  const config = await rpc(device, "server.getConfig", {});
  const providers = config.providers || [];
  const { links } = config.settings?.linkedEnvironmentsEnabled
    ? await mcp(device, "t3_environment_links", {}).catch(() => ({ links: [] }))
    : { links: [] };
  return {
    favorites: (config.settings?.favorites || []).map(({ provider, model }) => {
      const p = providers.find((x) => x.instanceId === provider);
      return { instanceId: provider, model, label: p?.models?.find((m) => m.slug === model)?.name || model };
    }),
    runtimeMode: config.settings?.defaultRuntimeMode || "full-access",
    links: (links || []).map((l) => ({ id: l.environmentId, label: l.label })),
  };
}

// environmentId empty = the device itself; otherwise one of its linked environments.
export async function t3Projects(device, environmentId) {
  let projects;
  if (!environmentId) {
    ({ projects } = await getJson(device, "/api/projects"));
  } else {
    projects = [];
    let cursor = 0;
    do {
      const page = await mcp(device, "t3_project_list", { environmentId, cursor, limit: 100 });
      projects.push(...page.projects);
      cursor = page.nextCursor;
    } while (cursor !== null && cursor !== undefined);
  }
  return projects
    .filter((p) => !p.archivedAt && !p.deletedAt)
    .map((p) => ({ id: p.id, title: p.title, path: p.workspaceRoot }));
}

export async function t3StartThread(device, { environmentId, projectId, instanceId, model, message, worktree, runtimeMode }) {
  const text = String(message || "").trim();
  if (!text) throw new Error("Message is empty");
  const launch = {
    projectId,
    title: text.split("\n")[0].slice(0, 80),
    modelSelection: { instanceId, model },
    runtimeMode: runtimeMode || "full-access",
    interactionMode: "default",
    workspaceStrategy: worktree ? { type: "worktree", baseRef: "HEAD" } : { type: "root" },
  };
  if (environmentId) {
    const result = await mcp(device, "t3_thread_launch", { ...launch, environmentId, message: text });
    return { threadId: result.threadId };
  }
  const result = await rpc(device, "orchestration.launchThread", {
    ...launch,
    commandId: randomUUID(),
    generateTitle: true,
    initialMessage: { text, attachments: [] },
  });
  return { threadId: result.threadId };
}
