import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Server {
  id: string;
  name: string;
  location: string;
  api_url: string;
  ws_url: string;
}

interface ConnectedInfo {
  assigned_ip: string;
  server_name: string;
}

type ConnStatus = "idle" | "connecting" | "connected" | "disconnecting";

export default function App() {
  const [showServers, setShowServers] = useState(false);
  const [connStatus, setConnStatus] = useState<ConnStatus>("idle");
  const [selectedServer, setSelectedServer] = useState<Server | null>(null);
  const [servers, setServers] = useState<Server[]>([]);
  const [connectedInfo, setConnectedInfo] = useState<ConnectedInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loadingServers, setLoadingServers] = useState(false);

  const toggleServerList = async () => {
    const opening = !showServers;
    setShowServers(opening);
    if (opening && servers.length === 0) {
      setLoadingServers(true);
      try {
        const list = await invoke<Server[]>("fetch_servers");
        setServers(list);
      } catch (e) {
        setError(String(e));
      } finally {
        setLoadingServers(false);
      }
    }
  };

  const selectServer = (server: Server) => {
    setSelectedServer(server);
    setShowServers(false);
    setError(null);
  };

  const handleConnect = async () => {
    if (connStatus === "connected") {
      setConnStatus("disconnecting");
      try {
        await invoke("disconnect_vpn");
      } catch (e) {
        setError(String(e));
      } finally {
        setConnStatus("idle");
        setConnectedInfo(null);
      }
      return;
    }

    if (!selectedServer) return;
    setConnStatus("connecting");
    setError(null);
    try {
      const info = await invoke<ConnectedInfo>("connect_vpn", {
        serverId: selectedServer.id,
      });
      setConnectedInfo(info);
      setConnStatus("connected");
    } catch (e) {
      setError(String(e));
      setConnStatus("idle");
    }
  };

  const handleNoTunnel = async () => {
    setConnStatus("connecting");
    setError(null);
    try {
      const info = await invoke<ConnectedInfo>("connect_no_tunnel");
      setConnectedInfo(info);
      setConnStatus("connected");
    } catch (e) {
      setError(String(e));
      setConnStatus("idle");
    }
  };

  const isConnected = connStatus === "connected";
  const isLoading = connStatus === "connecting" || connStatus === "disconnecting";

  const buttonLabel = () => {
    if (connStatus === "connecting") return "Connecting...";
    if (connStatus === "disconnecting") return "Disconnecting...";
    if (connStatus === "connected") return "Disconnect";
    return "Connect";
  };

  return (
    <div className="app">
      <span className="corner corner-tl" />
      <span className="corner corner-tr" />
      <span className="corner corner-bl" />
      <span className="corner corner-br" />

      {/* Server selector */}
      <div className="server-selector-wrapper">
        <button
          className={`server-selector ${showServers ? "open" : ""}`}
          onClick={toggleServerList}
          disabled={isConnected || isLoading}
        >
          <span className="server-selector-name">
            {selectedServer ? selectedServer.name : "Select Server"}
          </span>
          <span className="server-selector-arrow">{showServers ? "\u25B2" : "\u25BC"}</span>
        </button>

        {/* Server dropdown */}
        {showServers && (
          <div className="server-dropdown">
            {loadingServers ? (
              <div className="loading-row">
                <span className="spinner" />
                <span>scanning...</span>
              </div>
            ) : (
              servers.map((s) => (
                <div
                  key={s.id}
                  className={`server-item ${selectedServer?.id === s.id ? "selected" : ""}`}
                  onClick={() => selectServer(s)}
                >
                  <span className="server-item-name">{s.name}</span>
                  <span className="server-item-loc">{s.location}</span>
                </div>
              ))
            )}
          </div>
        )}
      </div>

      {/* Error */}
      {error && <div className="error-msg">! {error}</div>}

      {/* Connected info */}
      {isConnected && connectedInfo && (
        <div className="connected-info">
          <span className="connected-ip">{connectedInfo.assigned_ip}</span>
        </div>
      )}

      {/* Connect button */}
      <button
        className={`btn-connect ${isConnected ? "connected" : ""} ${isLoading ? "loading" : ""}`}
        onClick={handleConnect}
        disabled={(!selectedServer && !isConnected) || isLoading}
      >
        {isLoading && <span className="spinner" />}
        <span>{buttonLabel()}</span>
        {isConnected && <span className="status-dot" />}
      </button>

      {/* No Tunnel button */}
      {!isConnected && (
        <button
          className={`btn-no-tunnel ${isLoading ? "loading" : ""}`}
          onClick={handleNoTunnel}
          disabled={isLoading}
        >
          {isLoading && <span className="spinner" />}
          <span>Connect (No Tunnel)</span>
        </button>
      )}
    </div>
  );
}
