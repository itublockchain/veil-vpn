#!/usr/bin/env node
// Client-side WebSocket-to-UDP bridge.
//
// Boringtun client sends UDP packets to a local port.
// This bridge forwards them over WebSocket to the server's WS proxy,
// and relays responses back over UDP.
//
// Usage: node ws-bridge.js [local_udp_port] [ws_server_url]
//   Default: node ws-bridge.js 51821 ws://127.0.0.1:8443
//
// Requires: npm install ws

const dgram = require("dgram");
const WebSocket = require("ws");

const LOCAL_PORT = parseInt(process.argv[2] || "51821", 10);
const WS_URL = process.argv[3] || "ws://127.0.0.1:8443";

console.log(`[WS Bridge] UDP :${LOCAL_PORT} <-> ${WS_URL}`);

const udpServer = dgram.createSocket("udp4");
let clientAddr = null;
let clientPort = null;
let ws = null;
let reconnecting = false;
let udpToWsCount = 0;
let wsToUdpCount = 0;

function ts() {
  return new Date().toISOString();
}

function connectWs() {
  console.log(`[WS Bridge] [${ts()}] Connecting to ${WS_URL}...`);
  ws = new WebSocket(WS_URL);

  ws.on("open", () => {
    console.log(`[WS Bridge] [${ts()}] WebSocket CONNECTED to ${WS_URL}`);
    reconnecting = false;
  });

  ws.on("message", (data) => {
    wsToUdpCount++;
    const buf = Buffer.from(data);
    console.log(
      `[WS Bridge] [${ts()}] WS→UDP #${wsToUdpCount} | ${buf.length} bytes | ` +
      `first4=[${buf.slice(0, 4).toString("hex")}] | ` +
      `forwarding to ${clientAddr}:${clientPort}`
    );

    if (clientAddr && clientPort) {
      udpServer.send(buf, clientPort, clientAddr, (err) => {
        if (err) {
          console.error(`[WS Bridge] [${ts()}] WS→UDP #${wsToUdpCount} | UDP send error: ${err.message}`);
        } else {
          console.log(`[WS Bridge] [${ts()}] WS→UDP #${wsToUdpCount} | sent ${buf.length} bytes to ${clientAddr}:${clientPort}`);
        }
      });
    } else {
      console.warn(`[WS Bridge] [${ts()}] WS→UDP #${wsToUdpCount} | DROPPED — no client address known yet`);
    }
  });

  ws.on("close", (code, reason) => {
    console.log(`[WS Bridge] [${ts()}] WebSocket CLOSED | code=${code} reason=${reason || "(none)"}`);
    console.log(`[WS Bridge] [${ts()}] Session stats | UDP→WS: ${udpToWsCount} packets | WS→UDP: ${wsToUdpCount} packets`);
    scheduleReconnect();
  });

  ws.on("error", (err) => {
    console.error(`[WS Bridge] [${ts()}] WebSocket ERROR: ${err.message}`);
    scheduleReconnect();
  });

  ws.on("ping", (data) => {
    console.log(`[WS Bridge] [${ts()}] WebSocket PING received (${data.length} bytes)`);
  });

  ws.on("pong", (data) => {
    console.log(`[WS Bridge] [${ts()}] WebSocket PONG received (${data.length} bytes)`);
  });
}

function scheduleReconnect() {
  if (reconnecting) return;
  reconnecting = true;
  console.log(`[WS Bridge] [${ts()}] Reconnecting in 2s...`);
  setTimeout(connectWs, 2000);
}

// Client -> Server: forward UDP to WebSocket
udpServer.on("message", (msg, rinfo) => {
  clientAddr = rinfo.address;
  clientPort = rinfo.port;
  udpToWsCount++;

  const wsState = ws ? ["CONNECTING", "OPEN", "CLOSING", "CLOSED"][ws.readyState] : "null";
  console.log(
    `[WS Bridge] [${ts()}] UDP→WS #${udpToWsCount} | ${msg.length} bytes from ${rinfo.address}:${rinfo.port} | ` +
    `first4=[${msg.slice(0, 4).toString("hex")}] | ` +
    `ws=${wsState}`
  );

  if (ws && ws.readyState === WebSocket.OPEN) {
    ws.send(msg, (err) => {
      if (err) {
        console.error(`[WS Bridge] [${ts()}] UDP→WS #${udpToWsCount} | WS send error: ${err.message}`);
      } else {
        console.log(`[WS Bridge] [${ts()}] UDP→WS #${udpToWsCount} | sent ${msg.length} bytes to WS`);
      }
    });
  } else {
    console.warn(`[WS Bridge] [${ts()}] UDP→WS #${udpToWsCount} | DROPPED — WebSocket not open (state=${wsState})`);
  }
});

udpServer.on("listening", () => {
  const addr = udpServer.address();
  console.log(`[WS Bridge] [${ts()}] UDP listening on ${addr.address}:${addr.port}`);
  connectWs();
});

udpServer.on("error", (err) => {
  console.error(`[WS Bridge] [${ts()}] UDP socket error: ${err.message}`);
});

udpServer.bind(LOCAL_PORT, "127.0.0.1");

// Periodic stats
setInterval(() => {
  const wsState = ws ? ["CONNECTING", "OPEN", "CLOSING", "CLOSED"][ws.readyState] : "null";
  console.log(
    `[WS Bridge] [${ts()}] STATS | UDP→WS: ${udpToWsCount} | WS→UDP: ${wsToUdpCount} | ` +
    `client=${clientAddr}:${clientPort} | ws=${wsState}`
  );
}, 10000);

process.on("SIGINT", () => {
  console.log(`\n[WS Bridge] [${ts()}] Shutting down...`);
  console.log(`[WS Bridge] Final stats | UDP→WS: ${udpToWsCount} | WS→UDP: ${wsToUdpCount}`);
  if (ws) ws.close();
  udpServer.close();
  process.exit(0);
});
