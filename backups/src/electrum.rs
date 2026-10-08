use reqwest::Client;
use serde_json::{json, Value};
use std::error::Error;

// Configuration for local Electrum Daemon JSON-RPC endpoint
const RPC_URL: &str = "http://127.0.0.1:50001";
const RPC_USER: &str = "user";
const RPC_PASS: &str = "sig5UKVGkK8AFdtoiEXSZw==";

/// Internal helper function to make async JSON-RPC calls to the Electrum daemon
async fn call_electrum_rpc(
    client: &Client,
    method: &str,
    params: Value,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": "1",
        "method": method,
        "params": params
    });

    let response = client
        .post(RPC_URL)
        .basic_auth(RPC_USER, Some(RPC_PASS))
        .json(&payload)
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(format!("HTTP request failed with status: {}", response.status()).into());
    }

    let body: Value = response.json().await?;

    // Handle JSON-RPC level errors returned by Electrum
    if let Some(err) = body.get("error").filter(|e| !e.is_null()) {
        return Err(format!("Electrum JSON-RPC Error: {}", err).into());
    }

    body.get("result")
        .cloned()
        .ok_or_else(|| "Missing 'result' field in Electrum RPC response".into())
}

/// Public function to fetch all wallet addresses
pub async fn get_all_addresses(
    client: &Client,
) -> Result<Vec<String>, Box<dyn Error + Send + Sync>> {
    let result = call_electrum_rpc(client, "listaddresses", json!([])).await?;
    let addresses: Vec<String> = serde_json::from_value(result)?;
    Ok(addresses)
}

/// Checks the confirmed balance of a specific address
pub async fn get_address_balance(
    client: &Client,
    address: &str,
) -> Result<f64, Box<dyn Error + Send + Sync>> {
    let result = call_electrum_rpc(client, "getaddressbalance", json!([address])).await?;

    let confirmed_val = &result["confirmed"];

    // Handles confirmed balance returned as either a String ("0.001") or Number (0.001)
    if let Some(s) = confirmed_val.as_str() {
        Ok(s.parse::<f64>()?)
    } else if let Some(f) = confirmed_val.as_f64() {
        Ok(f)
    } else {
        Err("Missing or invalid 'confirmed' balance field in Electrum response".into())
    }
}

/// Safely filters an address list asynchronously without losing addresses on partial failure
pub async fn retain_addresses_with_balance(
    client: &Client,
    addresses: &mut Vec<String>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut active_addresses = Vec::new();

    for addr in addresses.iter() {
        let balance = get_address_balance(client, addr).await?;
        if balance > 0.0 {
            active_addresses.push(addr.clone());
        }
    }

    // Replace old list only if all queries succeeded
    *addresses = active_addresses;
    Ok(())
}

/// Fetches a new unused address from the wallet
pub async fn get_unused_address(
    client: &Client,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let result = call_electrum_rpc(client, "getunusedaddress", json!([])).await?;
    let address: String = serde_json::from_value(result)?;
    Ok(address)
}

/// Creates and returns a brand new receiving address in the Electrum wallet

//Wallet Gap Limit: Generating multiple addresses without receiving funds on them can eventually exceed Electrum's default gap limit (usually 20), which may require bumping the gap limit in Electrum if these addresses are monitored remotely.

pub async fn create_new_address(
    client: &Client,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let result = call_electrum_rpc(client, "createnewaddress", json!([])).await?;
    let address: String = serde_json::from_value(result)?;
    Ok(address)
}
