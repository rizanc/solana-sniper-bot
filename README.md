## Solana Sniper Bot

This bot aims to efficiently swap the input token for the output token. 

Provide the amount of input tokens as an integer. 

For example 1 Solana would be given as 1000000000 because SOL has 9 decimals.

Simply give it the amount of input tokens you would like swapped and the bot will do the rest.

This includes 
- Getting the latest prices for each token.
- Calculating the exchange rate
- Getting information on each token from the blockchain, such as number of decimals.
- Housekeeping such as creating associated token accounts (if necessary)
- Wrapping the required SOL (if input token is SOL)
- Monitoring the transactions
- Reporting on failed transactions

You can span multiple concurrent tasks for the swap transaction if needed. 

### Inspired from

https://github.com/0xcrust/raydium-swap

I adapted this code for my own use. Fixed some bugs and most likely created others!

Use this for educational purposes. Do not trade real money or big positions.

This is alpha software at best.

### How to run

RUST_LOG=[info|debug|error] cargo run swap

To get help on usage

cargo run help


Usage: solana-sniper-bot <COMMAND>

Commands:
  swap         
  cache-pools  
  help         Print this message or the help of the given subcommand(s)

Options:
  -h, --help  Print help

### TODO

Subscribe to Raydium pool events and 