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

export default function App() {
  const [status, setStatus] = useState<VpnStatus>("disconnected");
  const [assignedIp, setAssignedIp] = useState<string | null>(null);
  const [walletAddress, setWalletAddress] = useState<string | null>(null);
  const [balance, setBalance] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [health, setHealth] = useState<HealthEvent | null>(null);

  // Subscribe to backend events
  useEffect(() => {
    invoke<VpnStatus>("get_status").then((s) => setStatus(s));

    const unlistenState = listen<VpnStateEvent>("vpn-state", (e) => {
      setStatus(e.payload.status);
      if (e.payload.assigned_ip) setAssignedIp(e.payload.assigned_ip);
      if (e.payload.error) setError(e.payload.error);
      if (e.payload.status === "disconnected") {
        setAssignedIp(null);
        setWalletAddress(null);
        setBalance(null);
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
        await invoke("disconnect");
      } catch (e) {
        setError(String(e));
      }
    } else if (status === "disconnected") {
      try {
        setError(null);
        const info = await invoke<ConnectedInfo>("connect");
        setAssignedIp(info.assigned_ip);
        setWalletAddress(info.wallet_address);
        setBalance(info.gateway_balance);
      } catch (e) {
        setError(String(e));
        setStatus("disconnected");
      }
    }
  };

  const isLoading = status === "connecting" || status === "disconnecting";
  const isConnected = status === "connected";
  const isError = status === "error";
  const showDisconnect = isConnected || isError;

  const buttonLabel = () => {
    switch (status) {
      case "connecting":
        return "Connecting...";
      case "disconnecting":
        return "Disconnecting...";
      case "connected":
      case "error":
        return "Disconnect";
      default:
        return "Connect";
    }
  };

  const formatHandshake = (secs: number | null) => {
    if (secs === null) return "no handshake";
    if (secs < 60) return `${secs}s ago`;
    return `${Math.floor(secs / 60)}m ago`;
  };

  const shortAddr = (addr: string) =>
    `${addr.slice(0, 6)}...${addr.slice(-4)}`;

  return (
    <div className="app">
      <span className="corner corner-tl" />
      <span className="corner corner-tr" />
      <span className="corner corner-bl" />
      <span className="corner corner-br" />

      {/* Status */}
      <div className="status-section">
        <div
          className={`status-indicator ${isConnected ? "on" : ""} ${isError ? "err" : ""}`}
        />
        <span className="status-label">
          {isConnected
            ? "CONNECTED"
            : isError
              ? "ERROR"
              : isLoading
                ? "..."
                : "DISCONNECTED"}
        </span>
      </div>

      {/* Assigned IP */}
      {assignedIp && isConnected && (
        <div className="connected-info">
          <span className="connected-ip">{assignedIp}</span>
        </div>
      )}

      {/* Wallet info */}
      {walletAddress && isConnected && (
        <div className="wallet-section">
          <div className="wallet-address" title={walletAddress}>
            {shortAddr(walletAddress)}
          </div>
          <div className="wallet-balance">
            <span className="balance-value">{balance ?? "..."} USDC</span>
            <button className="btn-refresh" onClick={refreshBalance} title="Refresh balance">
              ↻
            </button>
          </div>
        </div>
      )}

      {/* Error */}
      {error && <div className="error-msg">! {error}</div>}

      {/* Connect / Disconnect button */}
      <button
        className={`btn-connect ${showDisconnect ? "connected" : ""} ${isLoading ? "loading" : ""}`}
        onClick={handleClick}
        disabled={isLoading}
      >
        {isLoading && <span className="spinner" />}
        <span>{buttonLabel()}</span>
      </button>

      {/* Health info */}
      {health && isConnected && (
        <div className="health-info">
          <span className={`health-dot ${health.connected ? "ok" : "stale"}`} />
          <span>handshake: {formatHandshake(health.handshake_age_secs)}</span>
        </div>
      )}

      <div className="server-label">37.27.29.160</div>
    </div>
  );
}
