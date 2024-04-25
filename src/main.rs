mod api;
use api::read_swap_params;

use anyhow::anyhow;
use clap::Parser;
use env_logger::Env;
use futures::future::join_all;
use log::{debug, error, info};

use raydium_contract_instructions::amm_instruction as amm;
use serde_json::json;

pub const COMPUTE_UNIT_PRICE: u64 = 1_117_148;
pub const COMPUTE_UNIT_LIMIT: u32 = 447_568;

use std::{sync::Arc, vec};

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
use tokio::sync::RwLock;

use spl_token_client::token::{Token, TokenError};

use crate::api::base_unit;

#[derive(Debug, Parser)]
pub struct Cli {
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Debug, Parser)]
pub enum Command {
    Initialize,
    Swap {
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
        Command::Swap {
            configuration_file,
            pools,
        } => {
            swap(&configuration_file, &pools).await?;
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

fn initialize() {
    todo!()
}

async fn exchange_rate(configuration_file: &str) -> anyhow::Result<()> {
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

    let out_token_price =
        api::get_price_birdeye(&swap_params.out_token, &swap_params.birdeye_key).await?;
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

async fn swap(configuration_file: &str, pools: &str) -> anyhow::Result<()> {
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

    let user = keypair.pubkey();
    let pool_info = api::pool::load_token_pool_from_file(&format!("{}.json", swap_params.out_token.to_string())).await;

    let pool_info = match pool_info {
        Ok(pool_info) => pool_info,
        Err(e) => {
            let l = api::pool::get_pool_info(
                &swap_params.in_token,
                &swap_params.out_token,
                Some(pools.to_owned()),
                true,
            ).await?;

            match l{
                Some(pool_info) => {

                    dbg!(json!(&pool_info));

                    if let Err(e) = api::pool::save_token_pool_to_file(&pool_info).await {
                        error!("Failed to save pool info: {}", e);
                    }

                    pool_info

                },
                None => {

                    error!(
                        "Failed to find pool in any specified direction for {}/{} pair",
                        swap_params.in_token, swap_params.out_token
                    );
                    return Err(anyhow!(
                        "Failed to find pool for {}/{} pair",
                        swap_params.in_token,
                        swap_params.out_token
                    ));
                }
            }
            
        }
    };

    let mut instructions = initialize_instructions(&user).await;

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

    let user_in_token_account = in_token_client.get_associated_token_address(&user);
    info!("User input-tokens ATA={}", user_in_token_account);

    match in_token_client
        .get_account_info(&user_in_token_account)
        .await
    {
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

    let user_in_acct = in_token_client
        .get_account_info(&user_in_token_account)
        .await?;

    let balance = user_in_acct.base.amount;

    info!(
        "User input-tokens ATA balance={} native={} amount_in={}",
        balance,
        in_token_client.is_native(),
        swap_params.amount_in
    );

    if in_token_client.is_native() && balance < swap_params.amount_in + 20_200_000 {
        info!("User's ATA balance is less than required amount. Transfering..");
        let transfer_amt: u64 = swap_params.amount_in + 20_200_000 - balance;

        info!("Transfering {} native tokens to user's ATA", transfer_amt);
        info!("From {} To => {}", &user, &user_in_token_account);

        let transfer_instruction =
            solana_sdk::system_instruction::transfer(&user, &user_in_token_account, transfer_amt);

        let sync_instruction =
            spl_token::instruction::sync_native(&spl_token::ID, &user_in_token_account)?;

        instructions.push(transfer_instruction);
        instructions.push(sync_instruction);
    }

    let user_out_token_account = out_token_client.get_associated_token_address(&user);
    debug!("User's output-tokens ATA={}", user_out_token_account);

    match out_token_client
        .get_account_info(&user_out_token_account)
        .await
    {
        Ok(_) => debug!("User's ATA for output tokens exists. Skipping creation.."),
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

    let in_token_price = api::get_price_birdeye(&swap_params.in_token, &swap_params.birdeye_key).await?;
    info!("Current price of 1 input token= {} USD", in_token_price);

    let out_token_price = api::get_price_birdeye(&swap_params.out_token, &swap_params.birdeye_key).await?;
    info!("Current price of 1 output token= {} USD", out_token_price);

    let input_decimals = in_token_client.get_mint_info().await?.base.decimals;
    info!("input_decimals: {}", input_decimals);

    let output_decimals = out_token_client.get_mint_info().await?.base.decimals;
    let result: f64 = base_unit(input_decimals);

    let out_in_rate = out_token_price / in_token_price;
    let swap_amount_in = swap_params.amount_in;
    let expected_output_amt = (swap_amount_in as f64 / result) / out_in_rate;

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

    info!("min_expected_out ={}", min_expected_out);

    debug!("out_factor={}", out_factor);

    info!(
        "Initiating swap of {} input tokens for {} output. Rate= {} input-tokens/1 output-token",
        swap_amount_in as f64 / base_unit(input_decimals),
        min_expected_out as f64 / base_unit(output_decimals),
        out_in_rate
    );

    if pool_info.base_mint == swap_params.in_token {
        info!(
            "Initializing swap with input tokens as pool base token balance {}",
            balance
        );
        debug_assert!(pool_info.quote_mint == swap_params.out_token);
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
            &user_in_token_account,
            &user_out_token_account,
            &user,
            swap_amount_in,
            min_expected_out,
        )?;
        instructions.push(swap_instruction);
    } else {
        info!(
            "Initializing swap with input tokens as pool quote token balance {}",
            balance
        );
        debug_assert!(
            pool_info.quote_mint == swap_params.in_token
                && pool_info.base_mint == swap_params.out_token
        );
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
            &user_in_token_account,
            &user_out_token_account,
            &user,
            swap_amount_in,
            min_expected_out,
        )?;
        instructions.push(swap_instruction);
    }

    let instructions_arc = Arc::new(RwLock::new(instructions));
    let client_arc = Arc::new(RwLock::new(client));
    let key_pair_arc = Arc::new(RwLock::new(keypair));

    let mut jobs = vec![];

    for _ in 0..swap_params.threads {
        tokio::time::sleep(Duration::from_secs(1)).await;

        let client_arc_clone = client_arc.clone();
        let instructions_arc_clone = instructions_arc.clone();
        let key_pair_arc_clone = key_pair_arc.clone();

        let t1 = tokio::spawn(async move {
            let instructions = instructions_arc_clone.read().await;
            let keypair = key_pair_arc_clone.read().await;

            let (recent_blockhash, last_valid_block_height) = client_arc_clone
                .read()
                .await
                .get_latest_blockhash_with_commitment(CommitmentConfig::finalized())
                .await
                .unwrap();

            info!("Last valid block height: {}", last_valid_block_height);

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
                info!(
                    "Last valid block height: {} Latest block {}",
                    last_valid_block_height, latest_block_height
                );
                info!("Sending transaction..");
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
                    Ok(_) => info!("Transaction sent successfully"),
                    Err(e) => {
                        if let Some(tx_err) = e.get_transaction_error() {
                            error!("Transaction failed 1: {}", tx_err);
                            if tx_err == solana_sdk::transaction::TransactionError::AlreadyProcessed
                            {
                                info!("Processed!..");
                                break;
                            } else if tx_err
                                == solana_sdk::transaction::TransactionError::BlockhashNotFound
                            {
                                info!("Blockhash not found!..");
                                break;
                            }
                        } else {
                            error!("Transaction failed 2: {}", e);

                            dbg!(e);
                        }

                        //log_transaction(&transaction.signatures[0]).await;
                        //return Err(anyhow!("Transaction failed: {}", e));
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
        COMPUTE_UNIT_LIMIT,
    );

    instructions.push(budget_ins);

    instructions
}



