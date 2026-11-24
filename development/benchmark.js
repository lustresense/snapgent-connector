// SPDX-License-Identifier: GPL-3.0-or-later
// Benchmarking script for Snapgent Rust runtime (Idle vs Active)
const fs = require("node:fs");
const path = require("node:path");
const os = require("node:os");
const { createHmac } = require("node:crypto");
const { execSync } = require("node:child_process");

const LOCALAPPDATA = process.env.LOCALAPPDATA || path.join(os.homedir(), "AppData", "Local");
const securityDir = path.join(LOCALAPPDATA, "SnapgentSecurity");
const token = fs.readFileSync(path.join(securityDir, "bridge-token"), "utf8").trim();
const extensionId = fs.readFileSync(path.join(securityDir, "trusted-extension-id"), "utf8").trim();
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

function getProcessMetrics() {
  const psCmd = `Get-Process snapgent | Select-Object -First 1 Id, WorkingSet64, PrivateMemorySize64, Threads, TotalProcessorTime | ConvertTo-Json`;
  try {
    const raw = execSync(`powershell -Command "${psCmd}"`, { encoding: "utf8" });
    const data = JSON.parse(raw);
    const threads = Array.isArray(data.Threads) ? data.Threads.length : 1;
    return {
      pid: data.Id,
      workingSetMiB: (data.WorkingSet64 / (1024 * 1024)).toFixed(2),
      privateMemoryMiB: (data.PrivateMemorySize64 / (1024 * 1024)).toFixed(2),
      threadCount: threads,
      rawCpu: data.TotalProcessorTime
    };
  } catch (e) {
    return null;
  }
}

function connectAuthenticated() {
  return new Promise((resolve, reject) => {
    const t0 = performance.now();
    const ws = new WebSocket(WS_URL, { headers: { Origin: ORIGIN } });
    const clientNonce = randomNonce();
    let authDone = 0;

    ws.onopen = () => {
      ws.send(JSON.stringify({ type: "auth_hello", nonce: clientNonce }));
    };

    ws.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data);
        if (msg.type === "auth_challenge") {
          const serverNonce = msg.nonce;
          const clientProof = hmacProof("client", clientNonce, serverNonce);
          ws.send(JSON.stringify({ type: "authenticate", proof: clientProof }));
        } else if (msg.type === "authenticated") {
          authDone = performance.now();
        } else if (msg.type === "connected") {
          const t1 = performance.now();
          resolve({ ws, totalConnectMs: t1 - t0, authHandshakeMs: authDone - t0 });
        }
      } catch (err) {
        reject(err);
      }
    };
    ws.onerror = (err) => reject(err);
  });
}

function rpc(ws, req) {
  return new Promise((resolve, reject) => {
    const id = Math.floor(Math.random() * 10000000);
    req.id = id;
    const t0 = performance.now();

    const handler = (event) => {
      try {
        const msg = JSON.parse(event.data);
        if (msg.id === id) {
          const t1 = performance.now();
          ws.removeEventListener("message", handler);
          resolve({ msg, latencyMs: t1 - t0 });
        }
      } catch (e) {}
    };

    ws.addEventListener("message", handler);
    ws.send(JSON.stringify(req));

    setTimeout(() => {
      ws.removeEventListener("message", handler);
      reject(new Error("RPC timeout"));
    }, 10000);
  });
}

async function runBenchmark() {
  console.log("=== Snapgent Real Runtime Performance Benchmark ===");

  // 1. Idle Measurements
  console.log("\n[1] Sampling Idle Runtime Metrics (over 3 seconds)...");
  const idle1 = getProcessMetrics();
  await new Promise(r => setTimeout(r, 3000));
  const idle2 = getProcessMetrics();

  console.log(`PID: ${idle2.pid}`);
  console.log(`Idle Working Set: ${idle2.workingSetMiB} MiB`);
  console.log(`Idle Private Memory: ${idle2.privateMemoryMiB} MiB`);
  console.log(`Idle Threads: ${idle2.threadCount}`);

  // 2. Reconnect Latency Measurement (5 iterations)
  console.log("\n[2] Measuring Reconnect & Handshake Latency (5 trials)...");
  const reconnectLatencies = [];
  const handshakeLatencies = [];

  for (let i = 0; i < 5; i++) {
    const res = await connectAuthenticated();
    reconnectLatencies.push(res.totalConnectMs);
    handshakeLatencies.push(res.authHandshakeMs);
    res.ws.close();
    await new Promise(r => setTimeout(r, 100));
  }

  const avgReconnect = (reconnectLatencies.reduce((a, b) => a + b, 0) / reconnectLatencies.length).toFixed(2);
  const minReconnect = Math.min(...reconnectLatencies).toFixed(2);
  const maxReconnect = Math.max(...reconnectLatencies).toFixed(2);
  console.log(`Full Reconnect Latency: min=${minReconnect}ms, avg=${avgReconnect}ms, max=${maxReconnect}ms`);

  // 3. Sustained Active Load & Tool Latency
  console.log("\n[3] Running Sustained Workload (100 active MCP tool calls)...");
  const { ws } = await connectAuthenticated();
  const tmpRoot = path.join(os.tmpdir(), `snapgent-bench-${Date.now()}`);
  fs.mkdirSync(tmpRoot, { recursive: true });
  const session = randomNonce();

  await rpc(ws, {
    type: "workspace_bind",
    server: "zed",
    session,
    workspace: tmpRoot
  });

  const toolLatencies = [];
  const startActive = performance.now();
  const preActiveMetrics = getProcessMetrics();

  for (let i = 0; i < 100; i++) {
    const filename = `test_${i}.txt`;
    const res = await rpc(ws, {
      type: "call_tool",
      server: "zed",
      session,
      name: "zed/write_file",
      arguments: { path: filename, content: `Benchmark payload ${i}` }
    });
    toolLatencies.push(res.latencyMs);
  }

  const endActive = performance.now();
  const postActiveMetrics = getProcessMetrics();
  ws.close();

  // Clean temp files
  try { fs.rmSync(tmpRoot, { recursive: true, force: true }); } catch {}

  toolLatencies.sort((a, b) => a - b);
  const minLat = toolLatencies[0].toFixed(2);
  const medianLat = toolLatencies[Math.floor(toolLatencies.length / 2)].toFixed(2);
  const p95Lat = toolLatencies[Math.floor(toolLatencies.length * 0.95)].toFixed(2);
  const maxLat = toolLatencies[toolLatencies.length - 1].toFixed(2);
  const meanLat = (toolLatencies.reduce((a, b) => a + b, 0) / toolLatencies.length).toFixed(2);

  console.log(`Tool Calls Completed: 100`);
  console.log(`Total Workload Duration: ${(endActive - startActive).toFixed(2)} ms`);
  console.log(`MCP Tool Latency: min=${minLat}ms, mean=${meanLat}ms, median=${medianLat}ms, p95=${p95Lat}ms, max=${maxLat}ms`);

  // Active Metrics & Memory Growth
  const memDiff = (parseFloat(postActiveMetrics.workingSetMiB) - parseFloat(idle1.workingSetMiB)).toFixed(2);
  const privDiff = (parseFloat(postActiveMetrics.privateMemoryMiB) - parseFloat(idle1.privateMemoryMiB)).toFixed(2);

  console.log(`\n[4] Active Runtime Metrics:`);
  console.log(`Active Working Set: ${postActiveMetrics.workingSetMiB} MiB (growth: ${memDiff > 0 ? "+" : ""}${memDiff} MiB)`);
  console.log(`Active Private Memory: ${postActiveMetrics.privateMemoryMiB} MiB (growth: ${privDiff > 0 ? "+" : ""}${privDiff} MiB)`);
  console.log(`Active Threads: ${postActiveMetrics.threadCount}`);

  const summary = {
    pid: idle2.pid,
    idleWorkingSetMiB: idle2.workingSetMiB,
    idlePrivateMemoryMiB: idle2.privateMemoryMiB,
    idleThreads: idle2.threadCount,
    activeWorkingSetMiB: postActiveMetrics.workingSetMiB,
    activePrivateMemoryMiB: postActiveMetrics.privateMemoryMiB,
    activeThreads: postActiveMetrics.threadCount,
    workingSetGrowthMiB: memDiff,
    privateMemoryGrowthMiB: privDiff,
    reconnectMinMs: minReconnect,
    reconnectAvgMs: avgReconnect,
    reconnectMaxMs: maxReconnect,
    mcpLatencyMinMs: minLat,
    mcpLatencyMeanMs: meanLat,
    mcpLatencyMedianMs: medianLat,
    mcpLatencyP95Ms: p95Lat,
    mcpLatencyMaxMs: maxLat
  };

  fs.writeFileSync(path.join(__dirname, "benchmark_results.json"), JSON.stringify(summary, null, 2));
  console.log("\nSaved benchmark results to development/benchmark_results.json");
}

runBenchmark().catch(console.error);
