use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub location: String,
    pub api_url: String,
    pub ws_url: String,
}

/// Fetches VPN server list from ENS subdomains.
///
/// Each server is published as an ENS subdomain under `vpntee.eth`:
///   e.g. `us1.vpntee.eth` -> TXT record with JSON server info
///
/// For now this returns mock data. Replace with real ENS resolution
/// using an Ethereum RPC endpoint (e.g. via ethers-rs or a REST wrapper).
pub async fn fetch_servers_from_ens() -> Result<Vec<Server>, String> {
    // TODO: Replace with real ENS resolution.
    //
    // Real implementation would:
    // 1. Connect to an Ethereum node (e.g. https://mainnet.infura.io)
    // 2. Resolve each known subdomain under vpntee.eth via ENS contenthash or TXT records
    // 3. Parse the returned JSON into Server structs
    //
    // Example using a REST ENS resolver API:
    //   GET https://api.ensdata.net/text/{name}/vpntee-server
    //
    // let servers = resolve_ens_subdomains().await?;

    // Mock data matching real expected format
    let servers = vec![
        Server {
            id: "us1".into(),
            name: "US East 1".into(),
            location: "New York, USA".into(),
            api_url: "https://cb1282bd500d5cbcc9f76590667deb22f73b4cc9-8080.dstack-pha-prod9.phala.network".into(),
            ws_url: "wss://cb1282bd500d5cbcc9f76590667deb22f73b4cc9-8080.dstack-pha-prod9.phala.network".into(),
        },
        Server {
            id: "eu1".into(),
            name: "EU Central 1".into(),
            location: "Frankfurt, Germany".into(),
            api_url: "https://eu1.vpntee.eth.limo/api".into(),
            ws_url: "wss://eu1.vpntee.eth.limo/ws".into(),
        },
        Server {
            id: "ap1".into(),
            name: "Asia Pacific 1".into(),
            location: "Singapore".into(),
            api_url: "https://ap1.vpntee.eth.limo/api".into(),
            ws_url: "wss://ap1.vpntee.eth.limo/ws".into(),
        },
        Server {
            id: "eu2".into(),
            name: "EU West 1".into(),
            location: "Amsterdam, Netherlands".into(),
            api_url: "https://eu2.vpntee.eth.limo/api".into(),
            ws_url: "wss://eu2.vpntee.eth.limo/ws".into(),
        },
    ];

    Ok(servers)
}
