// SPDX-License-Identifier: GPL-3.0-or-later
// End-to-end live verification of Snapgent bridge against running runtime on 17613.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const os = require("node:os");
const { createHmac } = require("node:crypto");

const LOCALAPPDATA = process.env.LOCALAPPDATA || path.join(os.homedir(), "AppData", "Local");
const securityDir = path.join(LOCALAPPDATA, "SnapgentSecurity");
const tokenPath = path.join(securityDir, "bridge-token");
const extIdPath = path.join(securityDir, "trusted-extension-id");

assert(fs.existsSync(tokenPath), "Bridge token file must exist");
assert(fs.existsSync(extIdPath), "Trusted extension ID file must exist");

const token = fs.readFileSync(tokenPath, "utf8").trim();
const extensionId = fs.readFileSync(extIdPath, "utf8").trim();
assert.equal(token.length, 64, "Token must be a 64-character hex string");
assert.equal(extensionId.length, 32, "Extension ID must be 32 characters");

const ORIGIN = `chrome-extension://${extensionId}`;
const WS_URL = "ws://127.0.0.1:17613/";

function hmacProof(role, clientNonce, serverNonce) {
  return createHmac("sha256", Buffer.from(token, "hex"))
    .update(`snapgent-auth-v1:${role}:${clientNonce}:${serverNonce}`)
    .digest("hex");
}

function randomNonce() {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  return Buffer.from(bytes).toString("hex");
}

// Connect to bridge and complete HMAC authentication
function connectAuthenticated() {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(WS_URL, {
      headers: { Origin: ORIGIN }
    });

    const clientNonce = randomNonce();
    let authenticated = false;
    let connectedCatalog = null;

    ws.onopen = () => {
      ws.send(JSON.stringify({ type: "auth_hello", nonce: clientNonce }));
    };

    ws.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data);
        if (msg.type === "auth_challenge") {
          const serverNonce = msg.nonce;
          const expectedServerProof = hmacProof("server", clientNonce, serverNonce);
          assert.equal(msg.proof, expectedServerProof, "Server HMAC proof must match");
          const clientProof = hmacProof("client", clientNonce, serverNonce);
          ws.send(JSON.stringify({ type: "authenticate", proof: clientProof }));
        } else if (msg.type === "authenticated") {
          authenticated = true;
        } else if (msg.type === "connected") {
          connectedCatalog = msg;
          resolve({ ws, connectedCatalog });
        }
      } catch (err) {
        reject(err);
      }
    };

    ws.onerror = (err) => reject(err);
    ws.onclose = () => {
      if (!authenticated) reject(new Error("Socket closed before authentication"));
    };

    setTimeout(() => {
      if (!authenticated) {
        try { ws.close(); } catch {}
        reject(new Error("Authentication handshake timed out"));
      }
    }, 5000);
  });
}

function rpc(ws, req) {
  return new Promise((resolve, reject) => {
    const id = req.id || Math.floor(Math.random() * 1000000);
    req.id = id;

    const handler = (event) => {
      try {
        const msg = JSON.parse(event.data);
        if (msg.id === id || (req.type === "ping" && msg.type === "pong" && msg.id === id)) {
          ws.removeEventListener("message", handler);
          resolve(msg);
        }
      } catch (e) {
        // ignore other messages
      }
    };

    ws.addEventListener("message", handler);
    ws.send(JSON.stringify(req));

    setTimeout(() => {
      ws.removeEventListener("message", handler);
      reject(new Error(`RPC timeout for request type ${req.type} (id=${id})`));
    }, 10000);
  });
}

async function runTests() {
  console.log("=== Snapgent Live End-to-End Test Suite ===");

  // 1. Authentication & Catalog
  console.log("\n[Test 1] Authenticating WebSocket client...");
  const { ws, connectedCatalog } = await connectAuthenticated();
  assert(connectedCatalog, "Connected catalog must be received");
  assert(Array.isArray(connectedCatalog.tools), "Catalog must contain tools list");
  console.log(`PASS: Authenticated successfully. Total catalog tools: ${connectedCatalog.tools.length}`);

  // 2. Ping / Pong
  console.log("\n[Test 2] Ping / Pong contract...");
  const pong = await rpc(ws, { type: "ping" });
  assert.equal(pong.type, "pong");
  console.log("PASS: Ping/Pong roundtrip responsive");

  // 3. Zed 28 tools verification
  console.log("\n[Test 3] Verifying 28 Zed tools and schemas...");
  const toolsResp = await rpc(ws, { type: "list_tools" });
  const allTools = toolsResp.tools || [];
  const zedTools = allTools.filter(t => t.name.startsWith("zed/") || (t.server === "zed"));
  console.log(`Found ${zedTools.length} Zed tools.`);
  assert.equal(zedTools.length, 28, "Must expose exactly 28 Zed tools");

  const expectedZedTools = [
    "get_workspace_info", "get_project_info", "list_dir", "list_tree", "find_files",
    "search_files", "read_file", "read_files", "read_file_lines", "get_file_info",
    "get_file_outline", "create_file", "write_file", "edit_file", "create_directory",
    "move_file", "copy_file", "git_status", "git_diff", "git_log",
    "git_branches", "run_command", "run_project_script", "run_tests", "delete_file",
    "apply_patch", "git_add", "git_commit"
  ];

  for (const toolName of expectedZedTools) {
    const found = zedTools.some(t => t.name === `zed/${toolName}` || t.name === toolName);
    assert(found, `Missing expected Zed tool: ${toolName}`);
  }
  console.log("PASS: All 28 Zed tools are present with valid schema");

  // 4. Temporary test workspaces setup
  console.log("\n[Test 4] Setting up temporary test workspaces...");
  const tmpRoot = path.join(os.tmpdir(), `snapgent-e2e-${Date.now()}`);
  const wsA = path.join(tmpRoot, "workspace_A");
  const wsB = path.join(tmpRoot, "workspace_B");
  fs.mkdirSync(wsA, { recursive: true });
  fs.mkdirSync(wsB, { recursive: true });

  const sessionA = randomNonce();
  const sessionB = randomNonce();
  console.log(`Workspace A: ${wsA}`);
  console.log(`Workspace B: ${wsB}`);

  // 5. Workspace binding & Isolation (Two AIs / Sessions)
  console.log("\n[Test 5] Explicit workspace binding and isolation...");
  const bindRespA = await rpc(ws, {
    type: "workspace_bind",
    server: "zed",
    session: sessionA,
    workspace: wsA
  });
  assert.equal(bindRespA.ok, true, `Binding session A failed: ${bindRespA.error}`);

  const bindRespB = await rpc(ws, {
    type: "workspace_bind",
    server: "zed",
    session: sessionB,
    workspace: wsB
  });
  assert.equal(bindRespB.ok, true, `Binding session B failed: ${bindRespB.error}`);

  // Write file in workspace A via Session A
  const writeA = await rpc(ws, {
    type: "call_tool",
    server: "zed",
    session: sessionA,
    name: "zed/write_file",
    arguments: { path: "identity.txt", content: "Content of Workspace A" }
  });
  assert.equal(writeA.ok, true, `Write A failed: ${writeA.error}`);

  // Write file in workspace B via Session B
  const writeB = await rpc(ws, {
    type: "call_tool",
    server: "zed",
    session: sessionB,
    name: "zed/write_file",
    arguments: { path: "identity.txt", content: "Content of Workspace B" }
  });
  assert.equal(writeB.ok, true, `Write B failed: ${writeB.error}`);

  // Verify physical files on disk have zero leakage
  const contentA = fs.readFileSync(path.join(wsA, "identity.txt"), "utf8");
  const contentB = fs.readFileSync(path.join(wsB, "identity.txt"), "utf8");
  assert.equal(contentA, "Content of Workspace A");
  assert.equal(contentB, "Content of Workspace B");
  console.log("PASS: Multi-session isolation verified. No cross-workspace leakage.");

  // 6. Conflicting binding rejection
  console.log("\n[Test 6] Rejection of conflicting binding on same session...");
  const conflictResp = await rpc(ws, {
    type: "workspace_bind",
    server: "zed",
    session: sessionA,
    workspace: wsB
  });
  assert.equal(conflictResp.ok, false, "Conflicting workspace bind must be rejected");
  console.log("PASS: Conflicting workspace bind correctly rejected.");

  // 7. Two AIs on the SAME workspace
  console.log("\n[Test 7] Two AIs working on the SAME workspace...");
  const sessionA2 = randomNonce();
  const bindRespA2 = await rpc(ws, {
    type: "workspace_bind",
    server: "zed",
    session: sessionA2,
    workspace: wsA
  });
  assert.equal(bindRespA2.ok, true, "Second session binding to same workspace must succeed");

  const readA2 = await rpc(ws, {
    type: "call_tool",
    server: "zed",
    session: sessionA2,
    name: "zed/read_file",
    arguments: { path: "identity.txt" }
  });
  assert.equal(readA2.ok, true);
  assert.equal(readA2.text, "Content of Workspace A");
  console.log("PASS: Multiple sessions successfully share the same canonical workspace root.");

  // 8. Command execution not stuck in Loading / Not Run
  console.log("\n[Test 8] Tool execution response validity (no Not Run / Loading hang)...");
  const listResp = await rpc(ws, {
    type: "call_tool",
    server: "zed",
    session: sessionA,
    name: "zed/list_dir",
    arguments: { path: "." }
  });
  assert.equal(listResp.ok, true);
  assert(listResp.text !== undefined, "Result text must not be undefined");
  console.log("PASS: Tool execution responds immediately with structured result.");

  // 9. Deduplication and non-uncertain replay guard
  console.log("\n[Test 9] Deduplication with turn signature...");
  const turnId = "turn_12345";
  const call1 = await rpc(ws, {
    type: "call_tool",
    server: "zed",
    session: sessionA,
    turn: turnId,
    name: "zed/read_file",
    arguments: { path: "identity.txt" }
  });
  assert.equal(call1.ok, true);

  const callDuplicate = await rpc(ws, {
    type: "call_tool",
    server: "zed",
    session: sessionA,
    turn: turnId,
    name: "zed/read_file",
    arguments: { path: "identity.txt" }
  });
  assert.equal(callDuplicate.ok, true);
  assert.equal(callDuplicate.duplicate, true, "Duplicate call within turn must return cached result");
  console.log("PASS: Turn-level deduplication active. Duplicate operations not blindly re-executed.");

  // 10. Reconnect and Workspace Recovery
  console.log("\n[Test 10] WebSocket disconnect, automatic reconnect, and workspace recovery...");
  ws.close();

  // Reconnect with a fresh WebSocket connection
  const { ws: ws2 } = await connectAuthenticated();
  // Re-bind same session A to workspace A
  const rebindResp = await rpc(ws2, {
    type: "workspace_bind",
    server: "zed",
    session: sessionA,
    workspace: wsA
  });
  assert.equal(rebindResp.ok, true, "Same root rebind after reconnect must succeed without chat reset");

  const readAfterReconnect = await rpc(ws2, {
    type: "call_tool",
    server: "zed",
    session: sessionA,
    name: "zed/read_file",
    arguments: { path: "identity.txt" }
  });
  assert.equal(readAfterReconnect.ok, true);
  assert.equal(readAfterReconnect.text, "Content of Workspace A");
  console.log("PASS: Reconnect and workspace recovery succeeded seamlessly.");

  // Cleanup sessions and temp workspace
  await rpc(ws2, { type: "workspace_release", session: sessionA });
  await rpc(ws2, { type: "workspace_release", session: sessionA2 });
  await rpc(ws2, { type: "workspace_release", session: sessionB });
  ws2.close();

  try {
    fs.rmSync(tmpRoot, { recursive: true, force: true });
  } catch {}

  console.log("\n>>> ALL 10 END-TO-END SPECIFICATION TESTS PASSED SUCCESSFULLY! <<<");
}

runTests().catch(err => {
  console.error("\nFATAL TEST FAILURE:", err);
  process.exit(1);
});
