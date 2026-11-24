// Run: node snapgent-extension/test-auth-transport.js
// Exercises the actual shipped service worker with an isolated mock WebSocket.
// Uses Node WebCrypto for the real protocol proof, not a mocked HMAC.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const { webcrypto, createHmac } = require("node:crypto");
const source = fs.readFileSync(__dirname + "/background.js", "utf8");
const sockets = [], listeners = [], timers = new Map();
let sequence = 0;
const timer = (fn) => { timers.set(++sequence, fn); return sequence; };
const clear = (id) => timers.delete(id);
class MockSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  constructor() { this.readyState = 0; this.sent = []; sockets.push(this); }
  send(value) { this.sent.push(JSON.parse(value)); }
  close() { this.readyState = 3; this.onclose?.(); }
  async open() { this.readyState = 1; await this.onopen?.(); }
  async recv(value) { await this.onmessage?.({ data: JSON.stringify(value) }); }
}
const chrome = {
  runtime: {
    id: "test", getURL: (p) => "chrome-extension://test/" + p,
    onMessage: { addListener: (f) => listeners.push(f) },
    onStartup: { addListener() {} }, onInstalled: { addListener() {} },
    sendMessage: () => Promise.resolve(),
  },
  tabs: { query: (_, cb) => cb([]) },
};
const token = "11".repeat(32);
const serverNonce = "44".repeat(32);
function proof(role, client) {
  return createHmac("sha256", Buffer.from(token, "hex"))
    .update("snapgent-auth-v1:" + role + ":" + client + ":" + serverNonce)
    .digest("hex");
}
const api = new Function("chrome", "WebSocket", "crypto", "TextEncoder",
  "setTimeout", "clearTimeout", "setInterval", "clearInterval", "ZSGuard",
  source + '\ngetCredential = async () => "' + token +
  '"; return { statusObj, send, connect, pending, popupSender };'
)(chrome, MockSocket, webcrypto, TextEncoder, timer, clear, timer, clear, {});
(async () => {
  let sock = sockets[0];
  await sock.open();
  assert.equal(api.statusObj().connected, false);
  assert.equal(sock.sent[0].type, "auth_hello");
  const client = sock.sent[0].nonce;
  assert.match(client, /^[0-9a-f]{64}$/);
  await sock.recv({ type: "auth_challenge", nonce: serverNonce, proof: proof("server", client) });
  assert.deepEqual(sock.sent[1], { type: "authenticate", proof: proof("client", client) });
  assert.equal(api.statusObj().connected, false);
  await sock.recv({ type: "authenticated", nonce: serverNonce, ui_owned: true });
  assert.equal(api.statusObj().connected, true);
  await sock.recv({ type: "connected", tools: [{ name: "zed/list_dir" }], servers: [{ id: "zed", alive: true, tools: 1 }] });
  assert.equal(api.statusObj().tools, 1);
  const pending = api.send({ type: "call_tool", name: "write_file", arguments: {} });
  sock.close();
  assert.equal((await pending).sent, true);
  assert.equal(api.statusObj().tools, 0);
  api.connect();
  sock = sockets.at(-1);
  await sock.open();
  await sock.recv({ type: "auth_challenge", nonce: serverNonce, proof: "99".repeat(32) });
  assert.equal(api.statusObj().connected, false);
  assert.equal(api.statusObj().connectionState, "pairing_required");
  assert.equal(sock.sent.length, 1);
  api.connect();
  sock = sockets.at(-1);
  await sock.open();
  const legacyClient = sock.sent[0].nonce;
  await sock.recv({ type: "auth_challenge", nonce: serverNonce, proof: proof("server", legacyClient) });
  await sock.recv({ type: "authenticated", nonce: serverNonce });
  assert.equal(api.statusObj().connected, false);
  assert.equal(api.statusObj().connectionState, "upgrade_required");
  assert.equal(api.statusObj().tools, 0);
  assert.equal(api.popupSender({ id: "test", url: chrome.runtime.getURL("popup.html"), tab: { id: 1 } }), false);
  assert.equal(api.popupSender({ id: "test", url: chrome.runtime.getURL("popup.html") }), true);
  console.log("PASS: authenticated transport, proof parity, disconnect ambiguity, popup authorization");
})().catch(error => { console.error(error); process.exitCode = 1; });
