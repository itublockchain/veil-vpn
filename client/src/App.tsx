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
  name: string;
  region: string;
  ip: string;
}

const SERVERS: Server[] = [
  { name: "FRANKFURT", region: "EU", ip: "37.27.29.160" },
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
        <span className="header-id">v0.1.0</span>
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

      {/* Status bar */}
      <div className={`status-bar ${isConnected ? "on" : ""} ${isError ? "err" : ""}`}>
        <div className="status-left">
          <span className={`status-dot ${isConnected ? "on" : ""} ${isError ? "err" : ""}`} />
          <span className="status-text">
            {isConnected ? "SECURED" : isError ? "ERROR" : isLoading ? "ROUTING..." : "UNSECURED"}
          </span>
        </div>
        {assignedIp && isConnected && (
          <span className="status-ip">{assignedIp}</span>
        )}
        {health && isConnected && health.handshake_age_secs !== null && (
          <span className="status-ping">{health.handshake_age_secs}s</span>
        )}
      </div>

      {/* Server list */}
      <div className="section-label">NODES</div>
      <div className="server-list">
        {SERVERS.map((s) => (
          <div
            key={s.ip}
            className={`server-row ${selectedServer.ip === s.ip ? "active" : ""}`}
            onClick={() => !isConnected && setSelectedServer(s)}
          >
            <span className="server-name">{s.name}</span>
            <span className="server-region">{s.region}</span>
            <span className="server-ip">{s.ip}</span>
            {selectedServer.ip === s.ip && <span className="server-dot" />}
          </div>
        ))}
      </div>

      {/* ENS input */}
      <div className="ens-section">
        <input
          className="ens-input"
          type="text"
          placeholder="node.veil.eth"
          value={ensInput}
          onChange={(e) => setEnsInput(e.target.value)}
          disabled={isConnected || isLoading}
          spellCheck={false}
        />
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
