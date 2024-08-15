#![allow(warnings)] 
mod api;
use std::{env, str::FromStr, sync::Arc};

use api::{read_swap_params, LiquidityPool};

use anyhow::anyhow;
use clap::Parser;
use env_logger::Env;
use futures::future::join_all;
use log::{debug, error, info};

use raydium_contract_instructions::amm_instruction as amm;
use solana_receiver::{raydium::get_price, requests::monitor_account};

pub const COMPUTE_UNIT_PRICE: u64 = 200_000;
pub const COMPUTE_UNIT_LIMIT: ComputeUnitLimit = ComputeUnitLimit::Default;

use solana_client::{nonblocking::rpc_client::RpcClient, rpc_config::RpcSendTransactionConfig};

use solana_sdk::{
    commitment_config::CommitmentConfig,
    instruction::Instruction,
    pubkey::Pubkey,
    signature::Keypair,
    signer::{EncodableKey, Signer},
    transaction::Transaction,
};

use api::Settings;
use tokio::time::Duration;
use tokio::{sync::RwLock, time::sleep};

use spl_token_client::token::{ComputeUnitLimit, Token, TokenError};

use crate::api::base_unit;

#[derive(Debug, Parser)]
pub struct Cli {
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Debug, Parser)]
pub enum Command {
    Initialize,
    SwapInAndOut {
        #[arg(
            long,
            help = "Path to configuration file",
            default_value = "settings.json"
        )]
        configuration_file: String,
        #[arg(long, help = "Path to pools cache", default_value = "pool_cache.json")]
        pools: String,
    },
    Monitor {
        #[arg(
            long,
            help = "Path to configuration file",
            default_value = "settings.json"
        )]
        configuration_file: String,
    },
    Swap {
        #[arg(
            long,
            help = "Path to configuration file",
            default_value = "settings.json"
        )]
        configuration_file: String,
        #[arg(long, help = "Path to pools cache", default_value = "pool_cache.json")]
        pools: String,
        #[arg(long, help = "Output token")]
        out_token: Option<Pubkey>,
    },
    SnipeOut {
        #[arg(
            long,
            help = "Path to configuration file",
            default_value = "settings.json"
        )]
        configuration_file: String,
        #[arg(long, help = "Path to pools cache", default_value = "pool_cache.json")]
        pools: String,
    },
    CachePools {
        #[arg(long, help = "Path to output file", default_value = "pool_cache.json")]
        output_file: String,
    },
    Exchange {
        #[arg(
            long,
            help = "Path to configuration file",
            default_value = "settings.json"
        )]
        configuration_file: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let env = Env::default()
        .filter_or("RUST_LOG", "error")
        .write_style_or("LOG_STYLE", "always");

    env_logger::init_from_env(env);
    let cli = Cli::parse();

    match cli.command {
        Command::Initialize => {
            initialize();
        }
        Command::SwapInAndOut { configuration_file, pools } => {
            swap_in_and_out(&configuration_file, &pools).await?;
        }
        Command::Monitor { configuration_file } => {
            monitor(&configuration_file).await?;
        }
        Command::Swap {
            configuration_file,
            pools,
            out_token,
        } => {
            swap(&configuration_file, &pools, out_token).await?;
        }
        Command::SnipeOut {
            configuration_file,
            pools,
        } => {
            snipe_out(&configuration_file, &pools, None).await?;
        }
        Command::CachePools { output_file } => {
            api::pool::fetch_pools(&output_file).await?;
        }
        Command::Exchange { configuration_file } => {
            exchange_rate(&configuration_file).await?;
        }
    }

    Ok(())
}

async fn monitor(configuration_file: &str) -> anyhow::Result<()> {
    info!("Monitoring..");
    // "7jDEo3xfREs28aoBrvDsChk5cAANesvWxXHAwbSjF6pz"
    let account_to_monitor = env::var("ADDRESS").expect("ADDRESS must be set");
    let account_to_monitor = Pubkey::from_str(&account_to_monitor).unwrap();

    let params: Settings = read_swap_params(&configuration_file).await?;

    env::set_var("WSS_URL", params.ws_url);
    env::set_var("RPC_URL", params.rpc_url);

    loop {
        match monitor_account(&account_to_monitor, "./pools").await {
            Ok(transaction_info) => {
                info!("{:?}", &transaction_info);

                let mut mint = Pubkey::from_str(transaction_info.accounts.get(8).unwrap()).unwrap();
                if mint == Pubkey::from_str("So11111111111111111111111111111111111111112")? {
                    mint = Pubkey::from_str(transaction_info.accounts.get(9).unwrap()).unwrap();
                }

                match swap(configuration_file, "pool_cache.json", Some(mint)).await {
                    Ok(_) => {
                        info!("Swap successful");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        // match snipe_out(configuration_file, "pool_cache.json", Some(mint)).await {
                        //     Ok(_) => {
                        //         info!("Snipe out successful");
                        //     }
                        //     Err(e) => {
                        //         error!("Error sniping out: {}", e);
                        //     }
                        // }
                    }
                    Err(e) => {
                        error!("Error swapping: {}", e);
                    }
                }
            }
            Err(e) => {
                error!("Error monitoring account: {}", e);
            }
        }
    }
}

async fn swap_in_and_out(configuration_file: &str, pools: &str) -> anyhow::Result<()> {
    info!("Swap In & Out..");

    match swap(configuration_file, pools, None).await {
        Ok(_) => {

            tokio::time::sleep(Duration::from_secs(15)).await;
            match snipe_out(configuration_file, pools, None).await{
                Ok(_) => {
                    info!("Snipe out successful");
                }
                Err(e) => {
                    return Err(anyhow!("Error swapping out"));
                }
            }
        },
        Err(e) => {
            return Err(anyhow!("Error swapping in"));
        }
    }

    Ok(())
}

fn initialize() {
    todo!()
}

async fn exchange_rate(configuration_file: &str) -> anyhow::Result<()> {
    let mut v = vec![1, 2, 3, 4, 5];
    v.sort_by(|a, b| b.cmp(a));

    let swap_params: Settings = read_swap_params(&configuration_file).await?;

    let keypair = Keypair::read_from_file(&swap_params.keypair).map_err(|_| {
        error!("Failed to read keypair from path={}", configuration_file);
        anyhow::anyhow!("failed reading keypair from path {}", configuration_file)
    })?;

    info!(
        "Read keypair from {} successfully. Address: {}",
        configuration_file,
        keypair.pubkey().to_string()
    );

    let client = api::rpc(&swap_params.rpc_url);
    let program_client = api::program_rpc(Arc::clone(&client));
    let in_token_client = Token::new(
        Arc::clone(&program_client),
        &spl_token::ID,
        &swap_params.in_token,
        None,
        Arc::new(api::keypair_clone(&keypair)),
    )
    .with_compute_unit_price(COMPUTE_UNIT_PRICE)
    .with_compute_unit_limit(COMPUTE_UNIT_LIMIT);

    let out_token_client = Token::new(
        Arc::clone(&program_client),
        &spl_token::ID,
        &swap_params.out_token,
        None,
        Arc::new(api::keypair_clone(&keypair)),
    );

    let in_token_price =
        api::get_price_birdeye(&swap_params.in_token, &swap_params.birdeye_key).await?;
    info!("Current price of 1 input token= {} USD", in_token_price);

    let out_token_price = 0.0;
    //api::get_price_birdeye(&swap_params.out_token, &swap_params.birdeye_key).await?;
    info!("Current price of 1 output token= {} USD", out_token_price);

    let input_decimals = in_token_client.get_mint_info().await?.base.decimals;
    debug!("input_decimals: {}", input_decimals);

    let output_decimals = out_token_client.get_mint_info().await?.base.decimals;
    debug!("output_decimals: {}", output_decimals);

    let out_in_rate = out_token_price / in_token_price;
    let swap_amount_in = swap_params.amount_in;
    let expected_output_amt = (swap_amount_in as f64 / base_unit(input_decimals)) / out_in_rate;

    debug!(
        "in_price={} out_price={} in_out_rate={} swap_amount_in={} expected_output_amt={}",
        in_token_price, out_token_price, out_in_rate, swap_amount_in as f64, expected_output_amt
    );

    if swap_params.slippage > 100.0 {
        error!("Invalid slippage percentage. > 100");
        return Err(anyhow!("Invalid slippage percentage. >100"));
    }
    let out_factor = ((100.0 - swap_params.slippage) / 100.0) as f64;

    let min_expected_out = ((expected_output_amt * out_factor) * base_unit(output_decimals)) as u64;

    debug!("min_expected_out ={}", min_expected_out);

    debug!("out_factor={}", out_factor);

    info!(
        "Exchange {} input tokens for {} output",
        swap_amount_in as f64 / base_unit(input_decimals),
        min_expected_out as f64 / base_unit(output_decimals)
    );

    Ok(())
}

async fn swap(
    configuration_file: &str,
    pools: &str,
    out_token: Option<Pubkey>,
) -> anyhow::Result<()> {
    let swap_params: Settings = read_swap_params(&configuration_file).await?;

    let swap_params = match out_token {
        Some(out_token) => {
            let in_token = swap_params.in_token;
            let out_token = out_token;

            let mut swap_params = swap_params;

            swap_params.in_token = in_token;
            swap_params.out_token = out_token;

            swap_params
        }
        None => swap_params,
    };

    let keypair = Keypair::read_from_file(&swap_params.keypair).map_err(|_| {
        error!("Failed to read keypair from path={}", configuration_file);
        anyhow::anyhow!("failed reading keypair from path {}", configuration_file)
    })?;

    info!(
        "Read keypair from {} successfully. Address: {}",
        configuration_file,
        keypair.pubkey().to_string()
    );

    let user = keypair.pubkey();

    let pool_info = get_pool(&swap_params, &pools).await?;

    let mut instructions = initialize_instructions(&user).await;
    let token_program_id = swap_params.token_program_id;

    let client =
        RpcClient::new_with_commitment(swap_params.rpc_url.clone(), CommitmentConfig::confirmed());

    let program_client = api::program_rpc(Arc::new(client));

    let client =
        RpcClient::new_with_commitment(swap_params.rpc_url.clone(), CommitmentConfig::finalized());

    let in_token_client = Token::new(
        Arc::clone(&program_client),
        &spl_token::ID,
        &swap_params.in_token,
        None,
        Arc::new(api::keypair_clone(&keypair)),
    )
    .with_compute_unit_price(COMPUTE_UNIT_PRICE)
    .with_compute_unit_limit(COMPUTE_UNIT_LIMIT);

    let out_token_client = Token::new(
        Arc::clone(&program_client),
        &spl_token::ID,
        &swap_params.out_token,
        None,
        Arc::new(api::keypair_clone(&keypair)),
    )
    .with_compute_unit_price(COMPUTE_UNIT_PRICE)
    .with_compute_unit_limit(COMPUTE_UNIT_LIMIT);

    let in_token_ata = in_token_client.get_associated_token_address(&user);
    info!("User input-tokens ATA={}", in_token_ata);

    match in_token_client.get_account_info(&in_token_ata).await {
        Ok(_) => debug!("User's ATA for input tokens exists. Skipping creation.."),
        Err(TokenError::AccountNotFound) | Err(TokenError::AccountInvalidOwner) => {
            info!("User's input-tokens ATA does not exist. Creating..");

            match in_token_client.create_associated_token_account(&user).await {
                Err(error) => {
                    error!("Error creating user's input-tokens ATA: {}", error);

                    return Err(anyhow!("Error creating user's input-tokens ATA"));
                }
                _ => (),
            }
        }
        Err(error) => error!("Error retrieving user's input-tokens ATA: {}", error),
    }

    let user_in_acct = in_token_client.get_account_info(&in_token_ata).await?;

    let in_ata_account_balance = user_in_acct.base.amount;

    info!(
        "User input-tokens ATA balance={} native={} amount_in={}",
        in_ata_account_balance,
        in_token_client.is_native(),
        swap_params.amount_in
    );

    if in_token_client.is_native() && in_ata_account_balance < swap_params.amount_in + 20_200_000 {
        info!("User's ATA balance is less than required amount. Transfering..");
        let transfer_amt: u64 = swap_params.amount_in + 20_200_000 - in_ata_account_balance;

        info!("Transfering {} native tokens to user's ATA", transfer_amt);
        info!("From {} To => {}", &user, &in_token_ata);

        let transfer_instruction =
            solana_sdk::system_instruction::transfer(&user, &in_token_ata, transfer_amt);

        let sync_instruction = spl_token::instruction::sync_native(&spl_token::ID, &in_token_ata)?;

        instructions.push(transfer_instruction);
        instructions.push(sync_instruction);
    }

    let out_token_ata = out_token_client.get_associated_token_address(&user);
    debug!("User's output-tokens ATA={}", out_token_ata);

    match out_token_client.get_account_info(&out_token_ata).await {
        Ok(_) => debug!("User's ATA for output tokens exists. Skipping creation.."),
        Err(TokenError::AccountNotFound) | Err(TokenError::AccountInvalidOwner) => {
            info!("User's output-tokens ATA does not exist. Creating..");

            instructions.push(
                spl_associated_token_account::instruction::create_associated_token_account(
                    &user,
                    &user,
                    &swap_params.out_token,
                    &token_program_id,
                ),
            );
        }
        Err(error) => error!("Error retrieving user's output-tokens ATA: {}", error),
    }

    env::set_var("RPC_URL", swap_params.rpc_url.clone());
    let out_token_price = get_price(&pool_info.id.to_string()).await.unwrap();
    info!("Current price of out token= {} USD", out_token_price);

    let in_token_decimals = in_token_client.get_mint_info().await?.base.decimals;
    let out_token_decimals = out_token_client.get_mint_info().await?.base.decimals;

    let mut expected_output_amt = (swap_params.amount_in as f64 / out_token_price) as u64;

    if swap_params.out_token == pool_info.quote_mint {
        expected_output_amt = (swap_params.amount_in as f64 * out_token_price) as u64;
    }

    info!(
        "out_price={} swap_amount_in={} expected_output_amt={}",
        out_token_price, swap_params.amount_in as f64, expected_output_amt
    );

    if swap_params.slippage > 100.0 {
        error!("Invalid slippage percentage. > 100");
        return Err(anyhow!("Invalid slippage percentage. >100"));
    }

    info!(
        "Initiating swap of {} input tokens for {} output.",
        swap_params.amount_in as f64 / base_unit(in_token_decimals),
        expected_output_amt as f64 / base_unit(out_token_decimals)
    );

    let threads = swap_params.threads;
    let instructions_arc = Arc::new(RwLock::new(instructions));
    let client_arc = Arc::new(RwLock::new(client));
    let key_pair_arc = Arc::new(RwLock::new(keypair));
    let pool_info_arc = Arc::new(RwLock::new(pool_info));
    let swap_params_arc = Arc::new(RwLock::new(swap_params));

    let mut jobs = vec![];

    for _ in 0..threads {
        let client_arc_clone = client_arc.clone();
        let instructions_arc_clone = instructions_arc.clone();
        let pool_info_arc_clone = pool_info_arc.clone();
        let swap_params_arc_clone = swap_params_arc.clone();

        let key_pair_arc_clone = key_pair_arc.clone();

        let t1 = execute_swap(
            client_arc_clone,
            instructions_arc_clone,
            pool_info_arc_clone,
            swap_params_arc_clone,
            key_pair_arc_clone,
            user.clone(),
            in_token_ata.clone(),
            out_token_ata.clone(),
            expected_output_amt,
        );

        jobs.push(t1);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    let results = join_all(jobs).await;

    for result in results {
        match result {
            Ok(_result) => {
                // if let Err(e) = result {
                //     println!("A task encountered an error: {}", e);
                // }
            }
            Err(e) => println!("A task encountered an error: {}", e),
        }
    }

    println!("All tasks completed");

    Ok(())
}

fn execute_swap(
    client_arc_clone: Arc<RwLock<RpcClient>>,
    instructions_arc_clone: Arc<RwLock<Vec<Instruction>>>,
    pool_info_arc_clone: Arc<RwLock<LiquidityPool>>,
    swap_params_arc_clone: Arc<RwLock<Settings>>,
    key_pair_arc_clone: Arc<RwLock<Keypair>>,
    user: Pubkey,
    in_token_ata: Pubkey,
    out_token_ata_account: Pubkey,
    min_expected_out: u64,
) -> tokio::task::JoinHandle<()> {
    let t1 = tokio::spawn(async move {
        let mut instructions = instructions_arc_clone.read().await.clone();
        let mut slippeage = 0.00;
        let mut done = false;

        while !done {
            if slippeage > 0.50 {
                error!("Slippeage too high. Exiting..");
                break;
            }

            instructions.push(
                swap_instruction(
                    &(*pool_info_arc_clone.read().await),
                    &(*swap_params_arc_clone.read().await),
                    &user,
                    &in_token_ata,
                    &out_token_ata_account,
                    (min_expected_out as f64 * (1.0 - slippeage)) as u64,
                )
                .await
                .unwrap(),
            );

            // instructions.push(system_instruction::transfer(
            //     &user,
            //     &Pubkey::from_str("ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt").unwrap(),
            //     000166500,
            // ));

            let keypair = key_pair_arc_clone.read().await;

            let (recent_blockhash, last_valid_block_height) = client_arc_clone
                .read()
                .await
                .get_latest_blockhash_with_commitment(CommitmentConfig::finalized())
                .await
                .unwrap();

            debug!("Last valid block height: {}", last_valid_block_height);

            let transaction = Transaction::new_signed_with_payer(
                &instructions,
                Some(&user),
                &vec![&keypair],
                recent_blockhash,
            );

            let mut latest_block_height = client_arc_clone
                .read()
                .await
                .get_block_height_with_commitment(CommitmentConfig::finalized())
                .await
                .unwrap();

            while latest_block_height < last_valid_block_height - 150 {
                debug!(
                    "Last valid block height: {} Latest block {}",
                    last_valid_block_height, latest_block_height
                );
                debug!("Sending transaction..");
                match client_arc_clone
                    .read()
                    .await
                    .send_transaction_with_config(
                        &transaction,
                        RpcSendTransactionConfig {
                            skip_preflight: false,
                            max_retries: Some(0),
                            preflight_commitment: Some(
                                solana_sdk::commitment_config::CommitmentLevel::Confirmed,
                            ),
                            ..RpcSendTransactionConfig::default()
                        },
                    )
                    .await
                {
                    Ok(_) => {
                        debug!("Transaction sent successfully");
                    }
                    Err(e) => {
                        if let Some(tx_err) = e.get_transaction_error() {
                            debug!("Transaction failed 1: {}", tx_err);

                            match tx_err {
                                solana_sdk::transaction::TransactionError::AlreadyProcessed => {
                                    info!("Processed!..");
                                    done = true;
                                    break;
                                }
                                solana_sdk::transaction::TransactionError::BlockhashNotFound => {
                                    error!("Blockhash not found!..");
                                    break;
                                }
                                solana_sdk::transaction::TransactionError::InstructionError(
                                    u,
                                    c,
                                ) => {
                                    debug!("Error processing Instruction {}: {}", u, c);
                                    match c {
                                        solana_sdk::instruction::InstructionError::Custom(30) => {
                                            debug!("Slippeage error 30");
                                            slippeage += 0.001;
                                            info!("(a) Slippeage:{}", slippeage);
                                            break;
                                        }
                                        solana_sdk::instruction::InstructionError::Custom(40) => {
                                            debug!("Insufficient Funds");
                                            slippeage += 0.001;
                                            info!("(b) Slippeage:{}", slippeage);
                                            //done = true;
                                            break;
                                        }
                                        _ => {
                                            error!("Custom Transaction failed: {}", c);
                                            dbg!(c);
                                        }
                                    }
                                }
                                _ => {
                                    error!("Custom Transaction failed: {}", tx_err);
                                    dbg!(tx_err);
                                }
                            }
                        } else {
                            error!("Transaction failed 2: {}", e);

                            dbg!(e);
                        }
                    }
                }

                latest_block_height = client_arc_clone
                    .read()
                    .await
                    .get_block_height_with_commitment(CommitmentConfig::finalized())
                    .await
                    .unwrap();

                tokio::time::sleep(Duration::from_millis(250)).await;
            }

            instructions.pop();
        }

        // Ok(())

        // let response = client_arc_clone
        //     .read()
        //     .await
        //     .simulate_transaction(&transaction)
        //     .await?;
        // if response.value.err.is_some() {
        //     error!("Transaction simulation failed: {:?}", response.value.err);
        //     return Err(anyhow!(
        //         "Transaction simulation failed: {:?}",
        //         response.value.err
        //     ));
        // }
        // let cu = response.value.units_consumed.unwrap();
        // info!(
        //     "Transaction simulation successful. Consumed {} compute units",
        //     cu
        // );
        // if cu > COMPUTE_UNIT_LIMIT as u64 {
        //     error!("Transaction simulation failed: Exceeded compute unit limit");
        //     return Err(anyhow!(
        //         "Transaction simulation failed: Exceeded compute unit limit"
        //     ));
        // }

        // let budget_ins =
        //     solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_limit(
        //         COMPUTE_UNIT_LIMIT,
        //     );

        // instructions(budget_ins);
    });

    t1
}

async fn snipe_out(
    configuration_file: &str,
    pools: &str,
    out_token: Option<Pubkey>,
) -> anyhow::Result<()> {
    let mut swap_params: Settings = read_swap_params(&configuration_file).await?;

    let in_token = swap_params.in_token;

    swap_params.in_token = swap_params.out_token;
    swap_params.out_token = in_token;

    match out_token {
        Some(out_token) => {
            swap_params.in_token = out_token;
        }
        None => (),
    }

    let keypair = Keypair::read_from_file(&swap_params.keypair).map_err(|_| {
        error!("Failed to read keypair from path={}", configuration_file);
        anyhow::anyhow!("failed reading keypair from path {}", configuration_file)
    })?;

    info!(
        "Read keypair from {} successfully. Address: {}",
        configuration_file,
        keypair.pubkey().to_string()
    );

    let user = keypair.pubkey();

    let pool_info = get_pool(&swap_params, &pools).await?;

    let instructions = initialize_instructions(&user).await;

    let client =
        RpcClient::new_with_commitment(swap_params.rpc_url.clone(), CommitmentConfig::confirmed());

    let program_client = api::program_rpc(Arc::new(client));

    let client =
        RpcClient::new_with_commitment(swap_params.rpc_url.clone(), CommitmentConfig::finalized());

    let in_token_client = Token::new(
        Arc::clone(&program_client),
        &spl_token::ID,
        &swap_params.in_token,
        None,
        Arc::new(api::keypair_clone(&keypair)),
    )
    .with_compute_unit_price(COMPUTE_UNIT_PRICE)
    .with_compute_unit_limit(COMPUTE_UNIT_LIMIT);

    let out_token_client = Token::new(
        Arc::clone(&program_client),
        &spl_token::ID,
        &swap_params.out_token,
        None,
        Arc::new(api::keypair_clone(&keypair)),
    )
    .with_compute_unit_price(COMPUTE_UNIT_PRICE)
    .with_compute_unit_limit(COMPUTE_UNIT_LIMIT);

    let in_token_ata = in_token_client.get_associated_token_address(&user);
    info!("User input-tokens ATA={}", in_token_ata);

    let user_in_acct = in_token_client.get_account_info(&in_token_ata).await?;

    let mut balance = user_in_acct.base.amount;
    balance = match swap_params.pct_to_snipe_out {
        Some(pct) => (balance as f64 * pct) as u64,
        None => balance,
    };

    //balance =(balance as f64 * 0.90) as u64;
    swap_params.amount_in = balance;
    info!("in_account balance:{}", balance);

    info!(
        "User input-tokens ATA balance={} native={}",
        balance,
        in_token_client.is_native()
    );

    let out_token_ata = out_token_client.get_associated_token_address(&user);
    info!("User's output-tokens ATA={}", out_token_ata);

    match out_token_client.get_account_info(&out_token_ata).await {
        Ok(_) => info!("User's ATA for output tokens exists. Skipping creation.."),
        Err(TokenError::AccountNotFound) | Err(TokenError::AccountInvalidOwner) => {
            info!("User's output-tokens ATA does not exist. Creating..");

            while let Err(e) = out_token_client
                .create_associated_token_account(&user)
                .await
            {
                error!("Error creating user's output-tokens ATA: {}", e);
            }
        }
        Err(error) => error!("Error retrieving user's output-tokens ATA: {}", error),
    }

    let mut price_too_low = true;
    let mut min_expected_out = 0;
    let mut input_decimals: u8;
    let mut output_decimals: u8;

    while price_too_low {
        env::set_var("RPC_URL", swap_params.rpc_url.clone());

        input_decimals = in_token_client.get_mint_info().await?.base.decimals;
        info!("input_decimals: {}", input_decimals);

        output_decimals = out_token_client.get_mint_info().await?.base.decimals;
        info!("output_decimals: {}", output_decimals);

        let in_token_price = get_price(&pool_info.id.to_string()).await.unwrap();
        info!("Current price of 1 input token= {} SOL", in_token_price);

        let mut expected_output_amt = (balance as f64 / base_unit(input_decimals)) / in_token_price;
        if swap_params.in_token == pool_info.base_mint {
            expected_output_amt = (balance as f64 / base_unit(input_decimals)) * in_token_price
        }

        info!(
            "in_price={} balance={} expected_output_amt={}",
            in_token_price,
            balance as f64 / base_unit(input_decimals),
            expected_output_amt
        );

        if swap_params.slippage > 100.0 {
            error!("Invalid slippage percentage. > 100");
            return Err(anyhow!("Invalid slippage percentage. >100"));
        }

        min_expected_out = (expected_output_amt * base_unit(output_decimals)) as u64;
        info!("min_expected_out ={}", min_expected_out);

        let my_min_expected_out = match swap_params.min_snipe_out_amount {
            Some(min) => min,
            None => min_expected_out,
        };

        info!(
            "Initiating swap of {} input tokens for {} output.",
            balance as f64 / base_unit(input_decimals),
            min_expected_out as f64 / base_unit(output_decimals),
        );

        if my_min_expected_out <= min_expected_out {
            price_too_low = false;
        } else {
            info!("Price too low. Retrying..");
            sleep(Duration::from_secs(5)).await;
        }
    }

    let threads = swap_params.threads;
    let instructions_arc = Arc::new(RwLock::new(instructions));
    let client_arc = Arc::new(RwLock::new(client));
    let key_pair_arc = Arc::new(RwLock::new(keypair));
    let pool_info_arc = Arc::new(RwLock::new(pool_info));
    let swap_params_arc = Arc::new(RwLock::new(swap_params));

    let mut jobs = vec![];

    for _ in 0..threads {
        tokio::time::sleep(Duration::from_secs(1)).await;

        let client_arc_clone = client_arc.clone();
        let instructions_arc_clone = instructions_arc.clone();
        let key_pair_arc_clone = key_pair_arc.clone();
        let pool_info_arc_clone = pool_info_arc.clone();
        let swap_params_arc_clone = swap_params_arc.clone();

        let t1 = execute_swap(
            client_arc_clone,
            instructions_arc_clone,
            pool_info_arc_clone,
            swap_params_arc_clone,
            key_pair_arc_clone,
            user.clone(),
            in_token_ata.clone(),
            out_token_ata.clone(),
            min_expected_out,
        );

        jobs.push(t1);
    }

    let results = join_all(jobs).await;

    for result in results {
        match result {
            Ok(_result) => {
                // if let Err(e) = result {
                //     println!("A task encountered an error: {}", e);
                // }
            }
            Err(e) => println!("A task encountered an error: {}", e),
        }
    }

    println!("All tasks completed");

    Ok(())
}

async fn initialize_instructions(_user: &Pubkey) -> Vec<Instruction> {
    let mut instructions: Vec<Instruction> = vec![];

    // let rpc_url = "https://api.mainnet-beta.solana.com".to_string();
    // let rpc_client = RpcClient::new(rpc_url);

    // let fees = rpc_client.get_recent_prioritization_fees(&[*user]).await;

    // if let Ok(fees) = fees {
    //     dbg!(&fees);

    //     let fee = fees.get(0).unwrap();
    //     // let fee_instruction = solana_sdk::system_instruction::transfer(
    //     //     &user,
    //     //     &solana_sdk::sysvar::fees::id(),
    //     //     fee.prioritization_fee,
    //     // );

    //     // instructions.push(fee_instruction);

    //     info!("Setting compute unit price to {}", fee.prioritization_fee);
    //     let price_ins =
    //         solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_price(
    //             fee.prioritization_fee,
    //         );

    //     //instructions.push(price_ins);
    // }

    let price_ins = solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_price(
        COMPUTE_UNIT_PRICE,
    );

    instructions.push(price_ins);

    let budget_ins = solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_limit(
        200_000,
    );

    instructions.push(budget_ins);

    instructions
}

async fn get_pool(
    swap_params: &Settings,
    pools: &str,
) -> anyhow::Result<LiquidityPool, anyhow::Error> {
    let file_name = format!("./pools/{}.json", swap_params.out_token.to_string());
    let cached_pool: Option<LiquidityPool> =
        match api::pool::load_token_pool_from_file(&file_name).await {
            Ok(pool_info) => Some(pool_info),
            Err(_) => None,
        };

    let cached_pool: Option<LiquidityPool> = match cached_pool {
        Some(cached_pool) => Some(cached_pool),
        None => {
            let file_name = format!("./pools/{}.json", swap_params.in_token.to_string());
            match api::pool::load_token_pool_from_file(&file_name).await {
                Ok(pool_info) => Some(pool_info),
                Err(_) => {
                    info!("Failed to load pool info from file, fetching from API.");
                    None
                }
            }
        }
    };

    let pool_info = match cached_pool {
        Some(pool_info) => pool_info,
        None => {
            let pool_info = api::pool::get_pool_info(
                &swap_params.in_token,
                &swap_params.out_token,
                Some(pools.to_owned()),
                true,
            )
            .await?;

            match pool_info {
                Some(pool_info) => {
                    info!("Saving pool info to file");
                    if let Err(e) =
                        api::pool::save_token_pool_to_file(&swap_params, &pool_info).await
                    {
                        error!("Failed to save pool info: {}", e);
                    }

                    pool_info
                }
                None => {
                    let error_msg = format!(
                        "Failed to find pool in any specified direction for {}/{} pair",
                        swap_params.in_token, swap_params.out_token
                    );
                    error!("{}", &error_msg);
                    return Err(anyhow!(error_msg));
                }
            }
        }
    };

    Ok(pool_info)
}

async fn swap_instruction(
    pool_info: &LiquidityPool,
    swap_params: &Settings,
    user: &Pubkey,
    in_token_ata: &Pubkey,
    out_token_ata: &Pubkey,
    min_expected_out: u64,
) -> anyhow::Result<Instruction> {
    info!("Creating swap instruction..");
    info!(
        "Swapping {} input tokens for {} output tokens",
        swap_params.amount_in, min_expected_out
    );

    if pool_info.base_mint == swap_params.in_token {
        debug_assert!(pool_info.quote_mint == swap_params.out_token);
        info!("Swapping base token for quote token");
        let swap_instruction = amm::swap_base_in(
            &amm::ID,
            &pool_info.id,
            &pool_info.authority,
            &pool_info.open_orders,
            &pool_info.target_orders,
            &pool_info.base_vault,
            &pool_info.quote_vault,
            &pool_info.market_program_id,
            &pool_info.market_id,
            &pool_info.market_bids,
            &pool_info.market_asks,
            &pool_info.market_event_queue,
            &pool_info.market_base_vault,
            &pool_info.market_quote_vault,
            &pool_info.market_authority,
            &in_token_ata,
            &out_token_ata,
            &user,
            swap_params.amount_in,
            min_expected_out,
        )?;

        Ok(swap_instruction)
    } else {
        debug_assert!(
            pool_info.quote_mint == swap_params.in_token
                && pool_info.base_mint == swap_params.out_token
        );
        info!("Swapping quote token for base token");
        let swap_instruction = amm::swap_base_out(
            &amm::ID,
            &pool_info.id,
            &pool_info.authority,
            &pool_info.open_orders,
            &pool_info.target_orders,
            &pool_info.base_vault,
            &pool_info.quote_vault,
            &pool_info.market_program_id,
            &pool_info.market_id,
            &pool_info.market_bids,
            &pool_info.market_asks,
            &pool_info.market_event_queue,
            &pool_info.market_base_vault,
            &pool_info.market_quote_vault,
            &pool_info.market_authority,
            &in_token_ata,
            &out_token_ata,
            &user,
            swap_params.amount_in,
            min_expected_out,
        )?;

        Ok(swap_instruction)
    }
}
