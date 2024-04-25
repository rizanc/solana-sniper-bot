use std::{sync::Arc, time::Duration};

use log::{debug, info};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::ClientBuilder;
use serde::{Deserialize, Serialize};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::signature::Keypair;

use solana_sdk::pubkey::Pubkey;
use spl_token_client::client::{ProgramClient, ProgramRpcClient, ProgramRpcClientSendTransaction};
use tokio::{fs::File, io::AsyncReadExt};

pub const RAYDIUM_POOL_INFO_ENDPOINT: &str = "https://api.raydium.io/v2/sdk/liquidity/mainnet.json";
pub const BIRDEYE_API_ENDPOINT: &str = "https://public-api.birdeye.so/defi/price?address=";

#[derive(Debug, Deserialize, Serialize)]
pub struct LiqPoolInformation {
    pub official: Vec<LiquidityPool>,

    #[serde(rename = "unOfficial")]
    pub unofficial: Vec<LiquidityPool>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiquidityPool {
    #[serde(with = "pubkey")]
    pub id: Pubkey,
    #[serde(with = "pubkey")]
    pub base_mint: Pubkey,
    #[serde(with = "pubkey")]
    pub quote_mint: Pubkey,
    #[serde(with = "pubkey")]
    pub lp_mint: Pubkey,
    pub base_decimals: u8,
    pub quote_decimals: u8,
    pub lp_decimals: u8,
    pub version: u8,
    #[serde(with = "pubkey")]
    pub program_id: Pubkey,
    #[serde(with = "pubkey")]
    pub authority: Pubkey,
    #[serde(with = "pubkey")]
    pub open_orders: Pubkey,
    #[serde(with = "pubkey")]
    pub target_orders: Pubkey,
    #[serde(with = "pubkey")]
    pub base_vault: Pubkey,
    #[serde(with = "pubkey")]
    pub quote_vault: Pubkey,
    #[serde(with = "pubkey")]
    pub withdraw_queue: Pubkey,
    #[serde(with = "pubkey")]
    pub lp_vault: Pubkey,
    pub market_version: u8,
    #[serde(with = "pubkey")]
    pub market_program_id: Pubkey,
    #[serde(with = "pubkey")]
    pub market_id: Pubkey,
    #[serde(with = "pubkey")]
    pub market_authority: Pubkey,
    #[serde(with = "pubkey")]
    pub market_base_vault: Pubkey,
    #[serde(with = "pubkey")]
    pub market_quote_vault: Pubkey,
    #[serde(with = "pubkey")]
    pub market_bids: Pubkey,
    #[serde(with = "pubkey")]
    pub market_asks: Pubkey,
    #[serde(with = "pubkey")]
    pub market_event_queue: Pubkey,
}

pub mod pubkey {
    use serde::{self, Deserialize, Deserializer, Serializer};
    pub use solana_sdk::pubkey::Pubkey;
    use std::str::FromStr;

    pub fn serialize<S>(pubkey: &Pubkey, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = format!("{}", pubkey);
        serializer.serialize_str(&s)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Pubkey, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Pubkey::from_str(&s).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BirdEyeResponse {
    pub data: Option<BirdEyeData>,
    pub message: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct BirdEyeData {
    value: f64,
    #[serde(rename = "updateUnixTime")]
    update_unix_time: u64,
    #[serde(rename = "updateHumanTime")]
    update_human_time: String,
}

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub keypair: String,
    #[serde(with = "pubkey")]
    pub in_token: Pubkey,
    #[serde(with = "pubkey")]
    pub out_token: Pubkey,
    pub amount_in: u64,
    pub slippage: f32,
    pub threads: u8,
    pub rpc_url: String,
    pub birdeye_key: String,
}

pub async fn read_swap_params(path: &str) -> anyhow::Result<Settings> {
    // Open the file in read-only mode
    let mut file = File::open(path).await?;

    // Read the file's contents into a string
    let mut contents = String::new();
    file.read_to_string(&mut contents).await?;

    // Deserialize the JSON string into SwapParams
    let swap_params: Settings = serde_json::from_str(&contents)?;

    Ok(swap_params)
}

pub async fn get_price_birdeye(token: &Pubkey, key: &str) -> anyhow::Result<f64> {
    let mut headers = HeaderMap::new();
    headers.insert("x-api-key", HeaderValue::from_str(key)?);

    let result = ClientBuilder::new()
        .default_headers(headers)
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
        .get(format!("{}{}", BIRDEYE_API_ENDPOINT, token.to_string()))
        .send()
        .await?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to fetch price from birdeye: {}", e))?;

    let response = serde_json::from_value::<BirdEyeResponse>(result)
        .map_err(|e| anyhow::anyhow!("Failed to deserialize birdeye response: {}", e))?;

    match response.data {
        None => {
            if let Some(message) = response.message {
                return Err(anyhow::anyhow!(
                    "Failed to fetch price from birdeye: {}",
                    message
                ));
            }
            return Err(anyhow::anyhow!("Failed to fetch price from birdeye"));
        }
        Some(data) => {
            return Ok(data.value);
        }
    }
}


pub fn keypair_clone(kp: &Keypair) -> Keypair {
    Keypair::from_bytes(&kp.to_bytes()).expect("failed to copy keypair")
}

pub fn rpc(rpc_url: &str) -> Arc<RpcClient> {
    Arc::new(RpcClient::new(rpc_url.to_string()))
}

pub fn program_rpc(rpc: Arc<RpcClient>) -> Arc<dyn ProgramClient<ProgramRpcClientSendTransaction>> {
    let program_client: Arc<dyn ProgramClient<ProgramRpcClientSendTransaction>> = Arc::new(
        ProgramRpcClient::new(rpc.clone(), ProgramRpcClientSendTransaction),
    );
    program_client
}

// pub async fn log_transaction(signature: &Signature) {
//     let rpc_url = "https://api.mainnet-beta.solana.com".to_string();
//     let rpc_client = RpcClient::new(rpc_url);

//     for _ in 0..10 {
//         match rpc_client
//             .get_transaction(
//                 &signature,
//                 solana_transaction_status::UiTransactionEncoding::Base64,
//             )
//             .await
//         {
//             Ok(transaction_status) => match transaction_status.transaction.meta {
//                 Some(meta) => {
//                     match meta.log_messages {
//                         OptionSerializer::Some(log) => {
//                             for l in log.iter() {
//                                 error!("{}", l);
//                             }
//                             break;
//                         }
//                         _ => (),
//                     };
//                 }
//                 None => {
//                     error!("Transaction meta not found");
//                     break;
//                 }
//             },
//             Err(_) => (),
//         }

//         error!("Waiting for Transaction Status. Retrying in 1 second");
//         tokio::time::sleep(Duration::from_secs(1)).await;
//     }
// }

pub fn base_unit(input_decimals: u8) -> f64 {
    let base: f64 = 10.0;
    let exponent = input_decimals;
    base.powf(exponent as f64)
}

pub mod pool {
    use super::*;

    
    pub async fn get_pool_info(
        token_a: &Pubkey,
        token_b: &Pubkey,
        cache_path: Option<String>,
        allow_unofficial: bool,
    ) -> anyhow::Result<Option<LiquidityPool>> {
        debug!(
            "fn: get_pool_info(token_a={},token_b={},cache_path={:?})",
            token_a, token_b, cache_path
        );
        let pools = if let Some(path) = cache_path {
            debug!("Fetching liq-pool-infos from pool-cache. path={}", path);
            serde_json::from_str(&std::fs::read_to_string(path)?)?
        } else {
            fetch_all_liquidity_pools().await?
        };
    
        let mut pools: Box<dyn Iterator<Item = _>> = if allow_unofficial {
            Box::new(
                pools
                    .official
                    .into_iter()
                    .chain(pools.unofficial.into_iter()),
            )
        } else {
            Box::new(pools.official.into_iter())
        };
    
        match pools.find(|pool| {
            (pool.base_mint == *token_b && pool.quote_mint == *token_a)
                || (pool.base_mint == *token_a && pool.quote_mint == *token_b)
        }) {
            Some(pool) => Ok(Some(pool)),
            None => Err(anyhow::anyhow!(
                "Failed to find pool for token_a={} and token_b={}",
                token_a,
                token_b
            )),
        }
    }
    

    pub async fn fetch_all_liquidity_pools() -> anyhow::Result<LiqPoolInformation> {
        debug!("fn: fetch_all_liquidity_pools");
        info!(
            "Fetching LP infos from raydium api endpoint={}",
            RAYDIUM_POOL_INFO_ENDPOINT
        );
        Ok(reqwest::get(RAYDIUM_POOL_INFO_ENDPOINT)
            .await?
            .json()
            .await?)
    }


    pub async fn fetch_pools(output_file: &str) -> anyhow::Result<()> {
        let pool_info = fetch_all_liquidity_pools().await?;
        std::fs::write(output_file, serde_json::to_string(&pool_info)?)?;

        Ok(())
    }

 
    pub async fn save_token_pool_to_file(
        pool_info: &LiquidityPool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let file_path = pool_info.base_mint.to_string() + ".json";
        let json = serde_json::to_string(pool_info)?;

        tokio::fs::write(file_path, json).await?;
        Ok(())
    }

    
    pub async fn load_token_pool_from_file(
        file_path: &str,
    ) -> Result<LiquidityPool, Box<dyn std::error::Error>> {
        // Check if the file exists
        if tokio::fs::metadata(file_path).await.is_ok() {
            let mut file = File::open(file_path).await?;
            let mut contents = String::new();
            file.read_to_string(&mut contents).await?;

            // Deserialize the JSON content into a Rust object
            let pool_info: LiquidityPool = serde_json::from_str(&contents)?;
            Ok(pool_info)
        } else {
            dbg!(&file_path);
            Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "File not found@",
            )))
        }
    }
}
