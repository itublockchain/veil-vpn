import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type VpnStatus =
  | "disconnected"
  | "connecting"
  | "connected"
  | "disconnecting"
  | "error";

interface ConnectedInfo {
  assigned_ip: string;
  server_endpoint: string;
  wallet_address: string;
  gateway_balance: string;
}

interface VpnStateEvent {
  status: VpnStatus;
  assigned_ip: string | null;
  error: string | null;
}

interface HealthEvent {
  connected: boolean;
  process_alive: boolean;
  handshake_age_secs: number | null;
}

interface Server {
  ens: string;
  region: string;
  ip: string;
}

const SERVERS: Server[] = [
  { ens: "ethglobal.veilvpn.eth", region: "EU", ip: "37.27.29.160" },
  { ens: "silk.veilvpn.eth", region: "US", ip: "37.27.29.160" },
  { ens: "ghost.veilvpn.eth", region: "APAC", ip: "37.27.29.160" },
];

export default function App() {
  const [status, setStatus] = useState<VpnStatus>("disconnected");
  const [assignedIp, setAssignedIp] = useState<string | null>(null);
  const [walletAddress, setWalletAddress] = useState<string | null>(null);
  const [balance, setBalance] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [health, setHealth] = useState<HealthEvent | null>(null);
  const [selectedServer, setSelectedServer] = useState<Server>(SERVERS[0]);
  const [ensInput, setEnsInput] = useState("");
  const [copied, setCopied] = useState(false);
  const [nodesOpen, setNodesOpen] = useState(false);

  // Load wallet info on mount + auto-refresh every 3s
  useEffect(() => {
    const fetch = () => {
      invoke<{ address: string; balance: string }>("get_wallet_info").then((info) => {
        setWalletAddress(info.address);
        setBalance(info.balance);
      }).catch(() => {});
    };
    fetch();
    const interval = setInterval(fetch, 3_000);
    return () => clearInterval(interval);
  }, []);

  // Subscribe to backend events
  useEffect(() => {
    invoke<VpnStatus>("get_status").then((s) => setStatus(s));

    const unlistenState = listen<VpnStateEvent>("vpn-state", (e) => {
      setStatus(e.payload.status);
      if (e.payload.assigned_ip) setAssignedIp(e.payload.assigned_ip);
      if (e.payload.error) setError(e.payload.error);
      if (e.payload.status === "disconnected") {
        setAssignedIp(null);
        setHealth(null);
        setError(null);
      }
      if (e.payload.status === "connecting") {
        setError(null);
      }
    });

    const unlistenHealth = listen<HealthEvent>("vpn-health", (e) => {
      setHealth(e.payload);
      if (!e.payload.process_alive) {
        setStatus("error");
        setError("VPN process died unexpectedly");
      }
    });

    return () => {
      unlistenState.then((f) => f());
      unlistenHealth.then((f) => f());
    };
  }, []);

  const refreshBalance = useCallback(async () => {
    if (!walletAddress) return;
    try {
      const bal = await invoke<string>("refresh_balance", {
        walletAddress,
      });
      setBalance(bal);
    } catch {
      // silent
    }
  }, [walletAddress]);

  const handleClick = async () => {
    if (status === "connected" || status === "error") {
      try {
        setStatus("disconnecting");
        await invoke("disconnect");
        setStatus("disconnected");
        setAssignedIp(null);
        setHealth(null);
        setError(null);
      } catch (e) {
        setError(String(e));
        setStatus("disconnected");
      }
    } else if (status === "disconnected") {
      try {
        setError(null);
        setStatus("connecting");
        const info = await invoke<ConnectedInfo>("connect");
        setAssignedIp(info.assigned_ip);
        setWalletAddress(info.wallet_address);
        setBalance(info.gateway_balance);
        setStatus("connected");
      } catch (e) {
        setError(String(e));
        setStatus("disconnected");
      }
    }
  };

  const isLoading = status === "connecting" || status === "disconnecting";
  const isConnected = status === "connected";
  const isError = status === "error";

  const copyAddress = () => {
    if (!walletAddress) return;
    navigator.clipboard.writeText(walletAddress);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  const shortAddr = (addr: string) =>
    `${addr.slice(0, 6)}...${addr.slice(-4)}`;

  const formatBalance = (bal: string) => {
    const num = parseFloat(bal);
    if (isNaN(num)) return "0.00";
    return num.toFixed(2);
  };

  return (
    <div className={`app ${isConnected ? "secured" : ""}`}>
      {/* Header */}
      <div className="header">
        <span className="header-label">VEIL://VPN</span>
        <div className={`header-status ${isConnected ? "on" : ""} ${isError ? "err" : ""}`}>
          <span className={`status-dot ${isConnected ? "on" : ""} ${isError ? "err" : ""}`} />
          <span className="header-status-text">
            {isConnected ? "SECURED" : isError ? "ERROR" : isLoading ? "ROUTING..." : "UNSECURED"}
          </span>
        </div>
      </div>

      {/* Wallet section */}
      {walletAddress && (
        <div className="wallet-bar">
          <div className="wallet-left">
            <span className="wallet-label">WALLET</span>
            <span className="wallet-addr" onClick={copyAddress} title="Click to copy">
              {copied ? "COPIED" : shortAddr(walletAddress)}
            </span>
          </div>
          <div className="wallet-right">
            <span className="wallet-bal">{formatBalance(balance ?? "0")}</span>
            <span className="wallet-unit">USDC</span>
          </div>
        </div>
      )}

      {/* Connected info */}
      {assignedIp && isConnected && (
        <div className="connected-bar">
          <span className="connected-ip">{assignedIp}</span>
          {health && health.handshake_age_secs !== null && (
            <span className="connected-ping">{health.handshake_age_secs}s</span>
          )}
        </div>
      )}

      {/* Node selector */}
      <div className="node-selector">
        <div
          className={`node-selected ${nodesOpen ? "open" : ""}`}
          onClick={() => !isConnected && !isLoading && setNodesOpen(!nodesOpen)}
        >
          <div className="node-selected-left">
            <span className="node-selected-label">NODE</span>
            <span className="node-selected-ens">{selectedServer.ens}</span>
          </div>
          <div className="node-selected-right">
            <span className="node-selected-region">{selectedServer.region}</span>
            <span className={`node-chevron ${nodesOpen ? "open" : ""}`}>&#9662;</span>
          </div>
        </div>

        {nodesOpen && (
          <div className="node-dropdown">
            {SERVERS.map((s) => (
              <div
                key={s.ens}
                className={`node-row ${selectedServer.ens === s.ens ? "active" : ""}`}
                onClick={() => {
                  setSelectedServer(s);
                  setNodesOpen(false);
                }}
              >
                <span className="node-row-ens">{s.ens}</span>
                <span className="node-row-region">{s.region}</span>
              </div>
            ))}
            <div className="node-custom" onClick={(e) => e.stopPropagation()}>
              <div className="ens-input-wrap">
                <input
                  className="ens-input"
                  type="text"
                  placeholder="custom"
                  value={ensInput}
                  onChange={(e) => setEnsInput(e.target.value)}
                  spellCheck={false}
                />
                <span className="ens-suffix">.veilvpn.eth</span>
              </div>
            </div>
          </div>
        )}
      </div>

      {/* Error */}
      {error && <div className="error-bar">{error}</div>}

      {/* Action button */}
      <button
        className={`btn-action ${isConnected || isError ? "disconnect" : ""} ${isLoading ? "loading" : ""}`}
        onClick={handleClick}
        disabled={isLoading}
      >
        {isLoading ? (
          <>
            <span className="spinner" />
            <span>{status === "connecting" ? "ESTABLISHING" : "CLOSING"}</span>
          </>
        ) : isConnected || isError ? (
          <span>DISCONNECT</span>
        ) : (
          <span>CONNECT</span>
        )}
      </button>

      {/* Footer */}
      <div className="footer">
        <span>PAY-AS-YOU-GO</span>
        <span>0.01 USDC / 10MB</span>
      </div>
    </div>
  );
}
