// SPDX-License-Identifier: GPL-3.0-or-later
// background.js - service worker.
// Owns ONE resilient WebSocket to the local bridge (ws://127.0.0.1:PORT).
// Keeping the socket here (not in the content script) avoids https→ws mixed
// content issues and centralises reconnect / timeout logic.
//
// Contract with content.js: every sendMessage ALWAYS gets a response object,
// even when the bridge is offline. The agentic loop must never hang waiting.

// Shared execution policy, so the heartbeat runs the SAME rule the tests pin.
// core/guard.js is plain top-level script (no modules), so importing it here
// defines the identical `ZSGuard` binding in the worker global scope that the
// content scripts get from manifest.json. Without this the worker carried an
// inline copy of the rule while the tests covered the guard.js one - the very
// "tested logic is not the running logic" mistake the retry path already had.
//
// `importScripts` only exists in the worker; node-based tests inject ZSGuard
// themselves (see test-background.js), so the absence of it is not an error.
if (typeof importScripts === "function" && typeof ZSGuard === "undefined") {
  try { importScripts("core/guard.js"); } catch (e) { console.warn("guard.js not loaded:", e); }
}

const PORT = 17613;
const URLS = [`ws://127.0.0.1:${PORT}`, `ws://localhost:${PORT}`];
let currentUrlIndex = 0;

// Chat sites where a snapgentV3 provider content script runs. Status pushes go
// to every tab matching these. Add the new provider's URL pattern here (and in
// manifest.json content_scripts + host_permissions) when integrating another AI.
const PROVIDER_URLS = ["https://chat.deepseek.com/*", "https://chatgpt.com/*", "https://chat.openai.com/*", "https://claude.ai/*", "https://gemini.google.com/*", "https://www.kimi.ai/*", "https://kimi.ai/*", "https://chat.z.ai/*", "https://chat.qwen.ai/*", "https://coder.qwen.ai/*", "https://arena.ai/*", "https://www.meta.ai/*", "https://meta.ai/*", "https://ptero.pro/*", "https://grok.com/*", "https://chat.mistral.ai/*"];

const RECONNECT_MIN = 1000;
const RECONNECT_MAX = 5000;
const HEARTBEAT_MS = 10000;
// If no message (incl. pong) arrives within this window while we believe we're
// connected, the socket is half-open: force a reconnect instead of letting
// pending requests slowly time out. Kept at 60s so a very large frame in flight
// (big project data) does not delay the pong past the limit and cause a false
// reconnect. Must stay below the bridge ping_timeout (90s).
const STALE_SOCKET_MS = 60000;
const REQUEST_TIMEOUT_DEFAULT = 130000; // a bit above the 120s tool timeout
// Cheap probe types (liveness pings, status reads) must not inherit the full
// tool budget: a local bridge answers in single-digit milliseconds, so waiting
// two minutes for a ping only delays the "offline" report the UI needs.
//
// MEASURED on a live bridge (v3.9.4, port 17613, 16 Nov 2026), including the
// worst case of a target server that is NOT connected:
//
//   ping              0.4 ms     list_tools                   2.3 ms (0.8 ms again)
//   get_tools         0.4 ms     get_tools(roblox, offline)   0.3 ms
//   capabilities      0.3 ms     studio_status               46.3 ms (0.2 ms again)
//   list_mcp_servers  0.8 ms     get_servers                  0.5 ms
//
// The slowest observed answer is one cold studio_status at 46 ms - three orders
// of magnitude below the 6000 ms cap. A false timeout therefore needs a bridge
// that is genuinely stuck, which is exactly what the cap is meant to surface.
const FAST_TIMEOUT = 6000;
// Probes the bridge answers instantly (single-digit ms on a healthy local
// socket). They must not inherit the full tool budget, otherwise the loop sits
// waiting two minutes just to learn the bridge is offline.
const FAST_TYPES = new Set([
  "ping", "studio_status", "list_mcp_servers", "get_servers",
  "list_tools", "get_tools", "capabilities",
]);
// How long to wait for a socket that is not open before declaring it offline.
// The reconnect ceiling is 2s and connecting to a LOCAL socket takes a few ms,
// so a healthy bridge reconnects well inside this window; waiting longer is
// exactly the "bridge is unreachable" delay users complained about.
//
// DELIBERATELY shorter than the 4314 ms measured for a socket that was MID-
// RECONNECT: the first probe is expected to lose that race, and runTool's
// preflight allows one more attempt.
//
// Be precise about what the two attempts cover. The 8000 ms in the preflight is
// only the RACE timeout around bg({type:"ping"}); the second ping still waits at
// most CONNECT_WAIT_MS (2.5 s) for the socket itself. So the two attempts spend
// roughly 2 x 2.5 s = 5 s waiting on the connection, which is what makes the
// measured 4.3 s case land on the retry. Without the retry, blocking the FIRST
// attempt for 4.3 s would tax every ordinary cold start to cover a case the
// second attempt already handles.
const CONNECT_WAIT_MS = 2500;

let ws = null;
let connected = false;
let connectionState = "offline";
let connectionError = "";
let credentialRevision = 0;
let reconnectDelay = RECONNECT_MIN;
let reconnectTimer = null;
let heartbeatTimer = null;
let lastMessageAt = 0; // timestamp of the last frame received from the bridge
let nextId = 1;
const pending = new Map(); // id -> {resolve, timer}
let toolsCache = [];
let mcpAlive = false;
let serversCache = [];
let bridgeVersion = null;
let workspaceBinding = null;
let callRecovery = false;
const boundSessions = new Map();
const bindingRequests = new Map();
// true/false = a PLACE is loaded and usable in Roblox Studio; null = unknown.
// The MCP process stays alive when Studio is closed or its MCP option is off,
// so this is probed separately (bridge "studio_status").
let studioConnected = null;
// true/false = a Roblox Studio app is connected to the MCP server at all; null =
// unknown. studioApp=true with studioConnected=false means "Studio open but no
// place"; studioApp=false means "Studio closed OR its MCP option disabled".
let studioApp = null;
// true/false = a Roblox Studio WINDOW/PROCESS exists on this machine (checked
// bridge-side via tasklist); null = unknown/old bridge. Distinguishes the two
// studioApp=false sub-cases the UI must word differently: Studio genuinely not
// launched ("open Roblox Studio") vs Studio OPEN but its MCP plugin never
// registered with the bridge - the documented fix for the latter is opening
// Assistant Settings > MCP Servers inside Studio (validated live 3x), which
// "open Roblox Studio" wording completely fails to convey.
let studioProc = null;

function log(...a) {
  console.log("[zs-bg]", ...a);
}

// The token is kept in extension-origin IndexedDB, never content-script storage.
const validCredential = (value) => typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
function credentialStore(mode, value) {
  return new Promise((resolve, reject) => {
    const open = indexedDB.open("snapgent-credentials", 1);
    open.onupgradeneeded = () => open.result.createObjectStore("private");
    open.onerror = () => reject(new Error("Credential storage unavailable"));
    open.onsuccess = () => {
      const db = open.result;
      try {
        const tx = db.transaction("private", mode);
        const store = tx.objectStore("private");
        const req = mode === "readonly" ? store.get("bridge") : store.put(value, "bridge");
        tx.oncomplete = () => { db.close(); resolve(req.result); };
        tx.onabort = tx.onerror = () => { db.close(); reject(new Error("Credential storage failed")); };
      } catch (e) { db.close(); reject(e); }
    };
  });
}
async function getCredential() {
  const cached = await credentialStore("readonly");
  return validCredential(cached) ? cached : null;
}
const hex = (bytes) => Array.from(bytes, x => x.toString(16).padStart(2, "0")).join("");
const unhex = (value) => Uint8Array.from(value.match(/../g), x => parseInt(x, 16));
async function authKey(token) {
  return crypto.subtle.importKey("raw", unhex(token), { name: "HMAC", hash: "SHA-256" }, false, ["sign", "verify"]);
}
function authPayload(role, client, server) {
  return new TextEncoder().encode("snapgent-auth-v1:" + role + ":" + client + ":" + server);
}
function popupSender(sender) {
  return !!sender && sender.id === chrome.runtime.id &&
    sender.url === chrome.runtime.getURL("popup.html") && !sender.tab;
}
// ── WebSocket lifecycle ─────────────────────────────────────────────────
function connect() {
  if (ws && (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING)) return;
  clearTimeout(reconnectTimer);
  const targetUrl = URLS[currentUrlIndex % URLS.length];
  let socket;
  try { socket = new WebSocket(targetUrl); }
  catch {
    connectionState = "offline";
    currentUrlIndex++;
    scheduleReconnect();
    return;
  }
  ws = socket;
  connected = false;
  workspaceBinding = null;
  callRecovery = false;
  boundSessions.clear();
  bindingRequests.clear();
  connectionState = "connecting";
  const revision = credentialRevision;
  let stage = "opening", key, clientNonce, serverNonce;
  const current = () => ws === socket && credentialRevision === revision && socket.readyState === WebSocket.OPEN;
  const fail = (reason, state = "pairing_required") => {
    if (ws !== socket) return;
    connectionState = state;
    connectionError = reason;
    broadcastStatus();
    try { socket.close(); } catch {}
  };
  const deadline = setTimeout(() => {
    if (ws !== socket || connected) return;
    if (stage === "opening") { try { socket.close(); } catch {} }
    else fail("Authentication timed out. Check bridge and pairing.");
  }, 11000);
  socket.onopen = async () => {
    if (!current()) return;
    connectionState = "authenticating";
    broadcastStatus();
    try {
      const token = await getCredential();
      if (!current()) return;
      if (!token) return fail("Pair Snapgent with the local bridge in the extension popup.");
      key = await authKey(token);
      if (!current()) return;
      clientNonce = hex(crypto.getRandomValues(new Uint8Array(32)));
      stage = "challenge";
      socket.send(JSON.stringify({ type: "auth_hello", nonce: clientNonce }));
    } catch { if (current()) fail("Could not load local pairing credential."); }
  };
  socket.onmessage = async (ev) => {
    if (!current()) return;
    let msg;
    try { msg = JSON.parse(ev.data); } catch { return; }
    if (!msg || typeof msg !== "object") return;
    lastMessageAt = Date.now();
    if (stage === "ready") { handleBridgeMessage(msg); return; }
    try {
      if (stage === "challenge" && msg.type === "auth_challenge" &&
          validCredential(msg.nonce) && validCredential(msg.proof)) {
        stage = "verifying";
        serverNonce = msg.nonce;
        const verified = await crypto.subtle.verify("HMAC", key, unhex(msg.proof),
          authPayload("server", clientNonce, serverNonce));
        if (!current()) return;
        if (!verified) return fail("Bridge identity verification failed. Pair again.");
        const proof = hex(new Uint8Array(await crypto.subtle.sign("HMAC", key,
          authPayload("client", clientNonce, serverNonce))));
        if (!current()) return;
        stage = "confirm";
        socket.send(JSON.stringify({ type: "authenticate", proof }));
      } else if (stage === "confirm" && msg.type === "authenticated" && msg.nonce === serverNonce) {
        if (msg.ui_owned !== true) {
          return fail("Old or hidden bridge rejected. Close the old Snapgent process and open the new desktop application.", "upgrade_required");
        }
        stage = "ready";
        clearTimeout(deadline);
        connected = true;
        connectionState = "connected";
        connectionError = "";
        reconnectDelay = RECONNECT_MIN;
        startHeartbeat();
        broadcastStatus();
      } else {
        fail("Incompatible bridge or invalid authentication response.");
      }
    } catch { if (current()) fail("Bridge authentication failed."); }
  };
  socket.onclose = () => {
    clearTimeout(deadline);
    if (ws !== socket) return;
    ws = null;
    connected = false;
    if (connectionState === "upgrade_required") {
      // Preserve the useful explanation rather than showing a false pairing error.
    } else if (stage !== "opening" && stage !== "ready" && connectionState !== "pairing_required") {
      connectionState = "pairing_required";
      connectionError = "Bridge rejected pairing. Check trusted extension ID and credential.";
    } else if (connectionState !== "pairing_required") {
      connectionState = "offline";
      connectionError = "";
    }
    mcpAlive = false;
    studioConnected = null;
    studioApp = null;
    studioProc = null;
    serversCache = [];
    toolsCache = [];
    bridgeVersion = null;
    workspaceBinding = null;
    callRecovery = false;
    boundSessions.clear();
    bindingRequests.clear();
    stopHeartbeat();
    failAllPending("bridge connection closed");
    broadcastStatus();
    currentUrlIndex++;
    scheduleReconnect();
  };
  socket.onerror = () => { if (ws === socket) { try { socket.close(); } catch {} } };
}

function scheduleReconnect() {
  clearTimeout(reconnectTimer);
  reconnectTimer = setTimeout(connect, ["pairing_required", "upgrade_required"].includes(connectionState) ? 10000 : reconnectDelay);
  reconnectDelay = Math.min(reconnectDelay * 1.7, RECONNECT_MAX);
}

function startHeartbeat() {
  stopHeartbeat();
  heartbeatTimer = setInterval(async () => {
    if (connected) {
      const heartbeatSocket = ws;
      // Half-open socket: the WS still reports OPEN but nothing comes through.
      //
      // STALENESS IS PROVED, NOT GUESSED. Chrome suspends an idle MV3 service
      // worker and setInterval does not run while suspended, so on wake-up
      // lastMessageAt is ALWAYS older than STALE_SOCKET_MS. Closing on elapsed
      // time alone therefore killed a perfectly healthy socket, onclose
      // reconnected, the worker idled again and the cycle repeated (measured:
      // 203 connect/disconnect events in a day while a manual ping answered in
      // 1 ms). So ask the bridge for a real pong and close only when it does
      // not answer.
      //
      // The decision itself lives in ZSGuard.heartbeatDecision (imported at the
      // top of this file) so the tested rule is the rule that runs.
      const decision = ZSGuard.heartbeatDecision({
        now: Date.now(),
        lastMessageAt,
        staleMs: STALE_SOCKET_MS,
      });
      if (decision.shouldProbe) {
        const pong = await send({ type: "ping" }, 4000).catch(() => null);
        if (ws !== heartbeatSocket || !connected) return;
        const answered = !!(pong && pong.ok);
        const final = ZSGuard.heartbeatDecision({
          now: Date.now(),
          lastMessageAt,
          staleMs: STALE_SOCKET_MS,
          pongAnswered: answered,
        });
        if (connected && final.close) {
          log("socket stale after failed ping, forcing reconnect");
          try { ws.close(); } catch {}
        } else {
          // The bridge answered, so the socket is healthy: the gap was the
          // suspended worker, not a dead half-open socket.
          lastMessageAt = Date.now();
        }
        return;
      }
      // Keeps the MV3 service worker alive AND detects a half-open socket.
      send({ type: "ping" }).catch(() => {});
      refreshStudioStatus();
    }
  }, HEARTBEAT_MS);
}

function stopHeartbeat() {
  clearInterval(heartbeatTimer);
  heartbeatTimer = null;
}

// Resolve once the socket is OPEN, or false after `timeout` ms.
function waitForConnection(timeout = 8000) {
  return new Promise((resolve) => {
    if (connected && ws && ws.readyState === WebSocket.OPEN) return resolve(true);
    connect(); // nudge a (re)connection - important after a worker wake-up
    const t0 = Date.now();
    const iv = setInterval(() => {
      if (connected && ws && ws.readyState === WebSocket.OPEN) {
        clearInterval(iv);
        resolve(true);
      } else if (Date.now() - t0 > timeout) {
        clearInterval(iv);
        resolve(false);
      }
    }, 100);
  });
}

// ── request/response over the socket ────────────────────────────────────
async function send(obj, timeout = REQUEST_TIMEOUT_DEFAULT, allowed = () => true) {
  // The MV3 service worker can be suspended; the first message after a wake-up
  // arrives before the socket has re-opened. Wait for it instead of failing -
  // otherwise Kimi wrongly hears "bridge offline". Probe types fail fast so the
  // loop learns "offline" quickly instead of staring at a spinner; a real tool
  // run still waits the longer budget.
  const fast = FAST_TYPES.has(obj && obj.type);
  if (!connected || !ws || ws.readyState !== WebSocket.OPEN) {
    await waitForConnection(fast ? CONNECT_WAIT_MS : 8000);
  }
  return new Promise((resolve) => {
    if (!allowed()) { resolve({ok:false,kind:"cancelled",sent:false,error:"Cancelled before dispatch"}); return; }
    if (!connected || !ws || ws.readyState !== WebSocket.OPEN) {
      resolve({ ok: false, kind: "disconnected", sent: false, error: "bridge not connected" });
      return;
    }
    const id = nextId++;
    const payload = { ...obj, id };
    // A probe the bridge answers in milliseconds must not inherit the full tool
    // budget; otherwise "offline" takes two minutes to surface.
    const effectiveTimeout = fast ? Math.min(timeout, FAST_TIMEOUT) : timeout;
    const timer = setTimeout(() => {
      if (pending.has(id)) {
        pending.delete(id);
        resolve({ ok: false, kind: "timeout", sent: true, error: "bridge did not respond in time" });
      }
    }, effectiveTimeout);
    pending.set(id, { resolve, timer, sent: false });
    try {
      ws.send(JSON.stringify(payload));
      // Past this line the bridge may already have executed the request. A
      // caller that retries has to know the difference: retrying a write_file
      // or a git_commit that did land would apply it twice.
      const entry = pending.get(id);
      if (entry) entry.sent = true;
    } catch (e) {
      clearTimeout(timer);
      pending.delete(id);
      resolve({ ok: false, kind: "disconnected", sent: false, error: String(e) });
    }
  });
}

// ── fire-and-forget progress frames ─────────────────────────────────────
// A progress frame is advisory: it carries no result and the bridge answers it with
// silence (no id, no reply). It must therefore NEVER register a pending request -
// doing that would fabricate a "timeout" a minute later for something that worked.
function pushFrame(frame) {
  if (!connected || !ws || ws.readyState !== WebSocket.OPEN) return false;
  try {
    ws.send(JSON.stringify({ type: "stream", frame }));
    return true;
  } catch (e) {
    return false;
  }
}

// Ask the bridge whether a Roblox Studio instance is actually connected to the
// MCP server. Broadcasts only on change so the UI updates promptly but quietly.
let studioProbing = false;
async function refreshStudioStatus() {
  if (studioProbing || !connected || !serversCache.some(s => s.id === "roblox" && s.alive)) return;
  studioProbing = true;
  try {
    const r = await send({ type: "studio_status" }, 12000);
    const v = r && r.ok && typeof r.studio === "boolean" ? r.studio : null;
    if (v !== studioConnected) {
      studioConnected = v;
      broadcastStatus();
    }
  } finally {
    studioProbing = false;
  }
}

function handleBridgeMessage(msg) {
  if ("studio" in msg && (typeof msg.studio === "boolean" || msg.studio === null)) {
    studioConnected = msg.studio;
  }
  if ("studio_app" in msg && (typeof msg.studio_app === "boolean" || msg.studio_app === null)) {
    studioApp = msg.studio_app;
  }
  if ("studio_proc" in msg && (typeof msg.studio_proc === "boolean" || msg.studio_proc === null)) {
    studioProc = msg.studio_proc;
  }
  if (msg.type === "studio_status") {
    resolvePending(msg.id, { ok: true, studio: studioConnected });
    broadcastStatus();
    return;
  }
  if (msg.type === "connected") {
    bridgeVersion = typeof msg.bridge_version === "string" ? msg.bridge_version : null;
    workspaceBinding = msg.workspace_binding === true;
    callRecovery = msg.call_recovery === true;
    mcpAlive = !!msg.mcp_alive;
    if (Array.isArray(msg.tools)) toolsCache = msg.tools;
    if (Array.isArray(msg.servers)) serversCache = msg.servers;
    broadcastStatus();
    return;
  }
  if (msg.type === "pong") {
    resolvePending(msg.id, { ok: true });
    return;
  }
  if (msg.type === "tools") {
    if (typeof msg.workspace_binding === "boolean") workspaceBinding = msg.workspace_binding;
    if (typeof msg.call_recovery === "boolean") callRecovery = msg.call_recovery;
    if (Array.isArray(msg.tools)) toolsCache = msg.tools;
    if (Array.isArray(msg.servers)) serversCache = msg.servers;
    mcpAlive = !!msg.mcp_alive;
    resolvePending(msg.id, { ok: true, tools: toolsCache });
    broadcastStatus();
    return;
  }
  if (msg.type === "tool_result") {
    resolvePending(msg.id, msg.ok
      ? { ok: true, text: msg.text, images: msg.images || [], workspaces: msg.workspaces }
      : { ok: false, kind: msg.kind, error: msg.error });
    return;
  }
  if (msg.type === "mcp_status") {
    mcpAlive = !!msg.alive;
    if (Array.isArray(msg.tools)) toolsCache = msg.tools;
    if (Array.isArray(msg.servers)) serversCache = msg.servers;
    resolvePending(msg.id, { ok: !!msg.ok, alive: msg.alive, error: msg.error });
    broadcastStatus();
    return;
  }
  if (msg.type === "server_changed") {
    // The bridge acks, then restarts itself to reload config.json. The socket
    // will drop right after this - the content script shows a spinner until the
    // reconnect lands and a fresh status arrives.
    resolvePending(msg.id, { ok: !!msg.ok, error: msg.error, restarting: !!msg.restarting });
    return;
  }
  if (msg.type === "error") {
    resolvePending(msg.id, { ok: false, error: msg.error });
    return;
  }
}

function resolvePending(id, value) {
  const p = pending.get(id);
  if (!p) return;
  clearTimeout(p.timer);
  pending.delete(id);
  p.resolve(value);
}

function failAllPending(reason) {
  for (const [, p] of pending) {
    clearTimeout(p.timer);
    // `sent` tells the caller whether the frame left the extension. A request
    // still waiting on waitForConnection never reached the bridge, so it is
    // safe to re-send; one that had gone out may already have run.
    p.resolve({ ok: false, kind: "disconnected", sent: !!p.sent, error: reason });
  }
  pending.clear();
}

// ── status push to any open DeepSeek tab + popup ─────────────────────────
function statusObj() {
  const live = !!(connected && ws && ws.readyState === WebSocket.OPEN);
  return { type: "zs-status", connected: live, connectionState: !live && connectionState === "connected" ? "offline" : connectionState, connectionError, workspaceBinding: live ? workspaceBinding : null, mcpAlive: live && mcpAlive, bridgeVersion: live ? bridgeVersion : null, studio: live ? studioConnected : null, studioApp: live ? studioApp : null, studioProc: live ? studioProc : null, tools: live ? toolsCache.length : 0, servers: live ? serversCache : [] };
}
let statusProbe = null;
async function verifyStatus() {
  if (!statusObj().connected || Date.now() - lastMessageAt < 2000) return statusObj();
  if (!statusProbe) {
    const socket = ws;
    const probe = (async () => {
      const pong = await send({ type: "ping" }, 2000).catch(() => null);
      // Status UI must not tear down a healthy socket on one slow pong.
      // Heartbeat owns reconnect and tests stale sockets independently.
      if (ws === socket && !(pong && pong.ok)) log("status probe delayed; heartbeat will verify");
    })();
    statusProbe = probe;
    probe.finally(() => { if (statusProbe === probe) statusProbe = null; });
  }
  await statusProbe;
  return statusObj();
}

function broadcastStatus() {
  try {
    chrome.runtime.sendMessage(statusObj()).catch(() => {});
  } catch {}
  try {
    if (chrome.tabs && chrome.tabs.query) {
      chrome.tabs.query({ url: PROVIDER_URLS }, (tabs) => {
        if (!tabs || (chrome.runtime && chrome.runtime.lastError)) return;
        for (const t of tabs) {
          try {
            chrome.tabs.sendMessage(t.id, statusObj()).catch(() => {});
          } catch {}
        }
      });
    }
  } catch {}
}


async function ensureWorkspaceBound(msg, force = false) {
  const { server, session, workspace } = msg;
  if (server !== "local" || !validCredential(session) || typeof workspace !== "string" || !workspace.trim())
    return { ok:false, kind:"workspace_required", error:"Choose a workspace in the Snapgent bar.", sent:false };
  const key = server + ":" + session;
  if (!force && boundSessions.get(key) === workspace) return { ok:true, text:workspace };
  const existing = bindingRequests.get(key);
  if (existing) return existing.workspace === workspace ? existing.task : {ok:false,kind:"permission_denied",error:"Conflicting workspace selection."};
  const socket = ws;
  const task = (async () => {
    const reply = await send({type:"workspace_bind",server,session,workspace}, 100000);
    if (reply.ok && socket === ws && connected) boundSessions.set(key,workspace);
    return reply;
  })();
  bindingRequests.set(key, {workspace,task});
  try { return await task; }
  finally { if (bindingRequests.get(key)?.task === task) bindingRequests.delete(key); }
}
async function waitWorkspaceCapability() {
  if (!connected) await waitForConnection(30000);
  const start = Date.now();
  while (connected && workspaceBinding === null && Date.now() - start < 4000)
    await new Promise(resolve => setTimeout(resolve, 100));
  return connected ? workspaceBinding : null;
}

// Keep cancelled IDs while a call may still be awaiting a binding approval.
const cancelledCalls = new Map();
const tabWorkspaces = new Map();
async function releaseUnusedWorkspace(session) {
  if ([...tabWorkspaces.values()].includes(session)) return;
  boundSessions.delete("local:" + session);
  if (connected) await send({type:"workspace_release",session},5000);
}
function rememberTabWorkspace(sender, session) {
  if (!sender || !sender.tab || !Number.isInteger(sender.tab.id) || !validCredential(session)) return;
  const old = tabWorkspaces.get(sender.tab.id);
  tabWorkspaces.set(sender.tab.id, session);
  if (old && old !== session) releaseUnusedWorkspace(old).catch(() => {});
}
if (chrome.tabs && chrome.tabs.onRemoved) chrome.tabs.onRemoved.addListener(tabId => {
  const session = tabWorkspaces.get(tabId);
  tabWorkspaces.delete(tabId);
  if (session) releaseUnusedWorkspace(session).catch(() => {});
});
const cancellationKey = msg => msg.session + ":" + msg.call_id;
// Query the original receipt, never replay a potentially mutating command.
async function recoverLocalCall(msg, allowed = () => true, budget = 420000) {
  if (!validCredential(msg.session) || typeof msg.call_id !== "string") return {ok:false,kind:"outcome_unknown",error:"Missing recovery receipt"};
  const until = Date.now() + budget;
  while (allowed() && Date.now() < until) {
    if (!connected) await waitForConnection(Math.min(10000, Math.max(1, until - Date.now())));
    if (!allowed()) break;
    if (connected) {
      const capability = await waitWorkspaceCapability();
      if (capability && !callRecovery) return {ok:false,kind:"outcome_unknown",error:"Bridge has no receipt recovery; verify changes"};
      if (capability && callRecovery) {
        const result = await send({type:"recover_tool",session:msg.session,call_id:msg.call_id},5000,allowed);
        if (!["disconnected","timeout","recovering"].includes(result.kind)) return result;
      }
    }
    await new Promise(resolve=>setTimeout(resolve, 800));
  }
  return {ok:false,kind:allowed()?"outcome_unknown":"cancelled",error:"Original tool result unavailable; verify changes before retrying"};
}
function pruneCancelledCalls() {
  const cutoff = Date.now() - 600000;
  for (const [key, at] of cancelledCalls) if (at < cutoff) cancelledCalls.delete(key);
  while (cancelledCalls.size > 512) cancelledCalls.delete(cancelledCalls.keys().next().value);
}
// ── messages from content.js / popup.js ─────────────────────────────────
chrome.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
  if (msg && msg.type === "status") {
    if (!connected) connect(); // self-heal after a worker wake-up
    verifyStatus().then(sendResponse, () => sendResponse(statusObj()));
    return true;
  }
  (async () => {
    switch (msg && msg.type) {
      // Liveness probe for the content script. Without this case a
      // bg({type:"ping"}) falls through to `default:` and answers
      // {ok:false, error:"unknown message"} - which any caller reads as "the
      // bridge is down" even though it is perfectly healthy (measured: ping
      // 1 ms, 28 tools listed). Keeping the case here makes the answer
      // truthful: ok only when the bridge actually returned a pong.
      case "pair_bridge": {
        if (!popupSender(_sender)) { sendResponse({ok:false,error:"Pairing is restricted to the extension popup."}); break; }
        try {
          const token = typeof msg.token === "string" ? msg.token.trim() : "";
          if (!validCredential(token)) { sendResponse({ok:false,error:"Copy the 64-character pairing credential from the Snapgent desktop UI."}); break; }
          credentialRevision++;
          await credentialStore("readwrite", token);
          connectionState = "offline"; connectionError = "";
          const old = ws;
          ws = null; connected = false; stopHeartbeat(); failAllPending("Pairing changed");
          if (old) { try { old.close(); } catch {} }
          reconnectDelay = RECONNECT_MIN; currentUrlIndex = 0; connect();
          sendResponse({ok:true});
        } catch { sendResponse({ok:false,error:"Unable to save local pairing credential."}); }
        break;
      }
      case "ping":
        send({ type: "ping" }, FAST_TIMEOUT).then((r) => {
          sendResponse(r && r.ok
            ? { ok: true }
            : { ok: false, kind: (r && r.kind) || "no_pong", error: (r && r.error) || "bridge did not answer the ping" });
        }).catch((e) => {
          sendResponse({ ok: false, kind: "error", error: String(e) });
        });
        break;
      case "list_tools": {
        // Prefer a live refresh; fall back to cache so the loop never stalls.
        // 10s, not 25s: a catalogue request only blocks this long when one of the
        // MCP servers is dead (typically Roblox in a degraded, Blender-only
        // session), and in that exact case we already hold a perfectly good cached
        // catalogue. Waiting the full 25s just froze the boot for no new data.
        const r = await send({ type: "list_tools" }, 10000);
        if (r.ok) sendResponse({ ok: true, tools: r.tools });
        else sendResponse({ ok: toolsCache.length > 0, tools: toolsCache, error: r.error });
        break;
      }
      case "workspace_list":
        sendResponse(await send({type:"workspace_list",server:msg.server},10000)); break;
      case "workspace_probe":
        if (msg.server !== "local" || !validCredential(msg.session) || !connected) {
          sendResponse({ok:false,kind:"workspace_unbound"}); break;
        }
        sendResponse(await send({type:"workspace_probe",server:"local",session:msg.session},5000)); break;
      case "workspace_select": {
        const bound = await ensureWorkspaceBound(msg);
        if (bound.ok) rememberTabWorkspace(_sender, msg.session);
        sendResponse(bound); break;
      }
      case "workspace_release": {
        if (validCredential(msg.session)) {
          if (_sender && _sender.tab && tabWorkspaces.get(_sender.tab.id) === msg.session)
            tabWorkspaces.delete(_sender.tab.id);
          await releaseUnusedWorkspace(msg.session);
        }
        sendResponse({ok:true}); break;
      }
      case "cancel_tool": {
        if (validCredential(msg.session) && typeof msg.call_id === "string" && msg.call_id.length <= 128) {
          cancelledCalls.set(cancellationKey(msg), Date.now());
          pruneCancelledCalls();
          if (connected) await send({type:"cancel_tool", session:msg.session, call_id:msg.call_id}, 5000);
        }
        sendResponse({ok:true}); break;
      }
      case "recover_tool": {
        sendResponse(await recoverLocalCall(msg,()=>!cancelledCalls.has(cancellationKey(msg)))); break;
      }
      case "call_tool": {
        pruneCancelledCalls();
        const wasCancelled = () => msg.call_id && cancelledCalls.has(cancellationKey(msg));
        const cancelled = () => sendResponse({ok:false,kind:"cancelled",sent:false,error:"Cancelled before dispatch"});
        if (wasCancelled()) { cancelled(); break; }
        const timeout = (msg.timeout || 120000) + (msg.server === "local" ? 110000 : 10000);
        const localWorkspace = msg.server === "local";
        const payload = {type:"call_tool", name:msg.name, arguments:msg.arguments, timeout:msg.timeout, server:msg.server, turn:msg.turn};
        if (localWorkspace) {
          const capability = await waitWorkspaceCapability();
          if (capability === null) { sendResponse({ok:false,kind:"disconnected",sent:false,error:"Waiting for the authenticated workspace catalogue."}); break; }
          {
            if (!capability) { sendResponse({ok:false,kind:"workspace_required",sent:false,error:"Rebuild and restart the native bridge before using a saved workspace."}); break; }
            const bound = await ensureWorkspaceBound(msg);
            if (wasCancelled()) { cancelled(); break; }
            if (!bound.ok) { sendResponse({...bound,sent:false}); break; }
            rememberTabWorkspace(_sender, msg.session);
            payload.session = msg.session;
            payload.call_id = msg.call_id;
          }
        }
        if (wasCancelled()) { cancelled(); break; }
        const recoverable = localWorkspace && callRecovery && !!payload.call_id;
        let result = await send(payload,timeout,() => !wasCancelled());
        if (recoverable && ["disconnected","timeout"].includes(result.kind) && result.sent !== false && !wasCancelled()) {
          result = await recoverLocalCall(msg,()=>!wasCancelled());
        }
        // An explicit unbound error is emitted before execution, so recovery may retry safely.
        if (payload.session && result.kind === "workspace_unbound") {
          const bound = await ensureWorkspaceBound(msg,true);
          if (wasCancelled()) { cancelled(); break; }
          if (bound.ok) {
            const recoverableRetry = callRecovery && !!payload.call_id;
            // An unbound result is cached under the first receipt. Use a new
            // receipt only after explicit pre-execution rejection; otherwise
            // the caller must not repeat the command.
            if (recoverableRetry) {
              sendResponse({ok:false,kind:"workspace_unbound",sent:false,error:"Workspace reapproved; command not executed. Retry the command."}); break;
            }
            result = await send(payload,timeout,() => !wasCancelled());
          }
          else result = {...bound,sent:false};
        }
        sendResponse(result); break;
      }
      case "stream":
        // Loading/progress frame from the page. Answer immediately with whether it
        // went out; the caller uses it for UI only and never treats it as a result.
        sendResponse({ ok: pushFrame(msg.frame || {}) });
        break;
      case "restart_mcp": {
        const r = await send({ type: "restart_mcp" }, 30000);
        sendResponse(r);
        break;
      }
      case "add_server": {
        const r = await send({
          type: "add_server", server_id: msg.server_id,
          command: msg.command, args: msg.args, env: msg.env,
        }, 15000);
        sendResponse(r);
        break;
      }
      case "remove_server": {
        const r = await send({ type: "remove_server", server_id: msg.server_id }, 15000);
        sendResponse(r);
        break;
      }
      case "reconnect":
        reconnectDelay = RECONNECT_MIN;
        connect();
        sendResponse({ ok: true });
        break;
      default:
        sendResponse({ ok: false, error: "unknown message" });
    }
  })().catch(() => sendResponse({ok:false,kind:"error",error:"Extension request failed."}));
  return true; // async sendResponse
});

// Wake/keepalive hooks.
chrome.runtime.onStartup.addListener(connect);
chrome.runtime.onInstalled.addListener(connect);

connect();
