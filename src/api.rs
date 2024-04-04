use std::{collections::HashMap, sync::Arc, time::Duration};

use log::{debug, error, info};
use serde::{Deserialize, Serialize};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::signature::{Keypair, Signature};

use solana_sdk::pubkey::Pubkey;
use solana_transaction_status::option_serializer::OptionSerializer;
use spl_token_client::client::{ProgramClient, ProgramRpcClient, ProgramRpcClientSendTransaction};
use tokio::{fs::File, io::AsyncReadExt};

pub const RAYDIUM_POOL_INFO_ENDPOINT: &str = "https://api.raydium.io/v2/sdk/liquidity/mainnet.json";
pub const RAYDIUM_PRICE_INFO_ENDPOINT: &str = "https://api.raydium.io/v2/main/price";

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

#[derive(Debug, Deserialize)]
pub struct SwapParams {
    pub keypair: String,
    #[serde(with = "pubkey")]
    pub in_token: Pubkey,
    #[serde(with = "pubkey")]
    pub out_token: Pubkey,
    pub amount_in: u64,
    pub slippage: f32,
    pub threads: u8,
    pub rpc_url: String,
}

pub async fn read_swap_params(path: &str) -> anyhow::Result<SwapParams> {
    // Open the file in read-only mode
    let mut file = File::open(path).await?;

    // Read the file's contents into a string
    let mut contents = String::new();
    file.read_to_string(&mut contents).await?;

    // Deserialize the JSON string into SwapParams
    let swap_params: SwapParams = serde_json::from_str(&contents)?;

    Ok(swap_params)
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

fn deserialize_price_info(value: serde_json::Value) -> anyhow::Result<HashMap<String, f64>> {
    debug!("fn: deserialize_price_info(value={})", value);
    Ok(value
        .as_object()
        .ok_or(anyhow::format_err!("malformed content. expected object."))?
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.as_f64().expect("value is f64")))
        .collect())
}

pub async fn fetch_all_prices() -> anyhow::Result<HashMap<String, f64>> {
    debug!("fn: fetch_all_prices");
    debug!(
        "Fetching price infos from raydium api endpoint={}",
        RAYDIUM_PRICE_INFO_ENDPOINT
    );
    let price_info_result: serde_json::Value = reqwest::get(RAYDIUM_PRICE_INFO_ENDPOINT)
        .await?
        .json()
        .await?;
    deserialize_price_info(price_info_result)
}

pub async fn get_price(token: &Pubkey, cache_path: &Option<String>) -> anyhow::Result<f64> {
    debug!(
        "fn: get_price(token = {},cache_path = {:?})",
        token, cache_path
    );
    let pools = if let Some(path) = cache_path {
        debug!("Fetching price-information from price-cache. path={}", path);
        deserialize_price_info(serde_json::from_str(&std::fs::read_to_string(path)?)?)?
    } else {
        fetch_all_prices().await?
    };

    pools
        .into_iter()
        .find_map(|(tok, price)| {
            if token.to_string() == *tok {
                return Some(price);
            }
            None
        })
        .ok_or_else(|| {
            error!("Failed to find price for token {}", token);
            anyhow::anyhow!("Failed to find price for token {}", token)
        })
}

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

pub async fn log_transaction(signature: &Signature) {
    let rpc_url = "https://api.mainnet-beta.solana.com".to_string();
    let rpc_client = RpcClient::new(rpc_url);

    for _ in 0..10 {
        match rpc_client
            .get_transaction(
                &signature,
                solana_transaction_status::UiTransactionEncoding::Base64,
            )
            .await
        {
            Ok(transaction_status) => match transaction_status.transaction.meta {
                Some(meta) => {
                    match meta.log_messages {
                        OptionSerializer::Some(log) => {
                            for l in log.iter() {
                                error!("{}", l);
                            }
                            break;
                        }
                        _ => (),
                    };
                }
                None => {
                    error!("Transaction meta not found");
                    break;
                }
            },
            Err(_) => (),
        }

        error!("Waiting for Transaction Status. Retrying in 1 second");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

pub fn base_unit(input_decimals: u8) -> f64 {
    let base: f64 = 10.0;
    let exponent = input_decimals;
    base.powf(exponent as f64)
}
