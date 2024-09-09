use jupiter_adrena::PoolAmm;
use jupiter_amm_interface::{Amm, AmmContext, ClockRef, KeyedAccount, QuoteParams, SwapMode};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{account::Account, clock::Clock, pubkey as key, pubkey::Pubkey, sysvar::SysvarId};
use std::collections::HashMap;

#[test]
fn test() {
    let client = RpcClient::new("https://api.devnet.solana.com");

    let pool_key = key!("2buhqUduNw7wNhZ1ixFxfvLRX3gAZkGmg8G1Rv5SEur7");
    let pool_acc = client.get_account(&pool_key).unwrap();

    let clock = client.get_account(&Clock::id()).unwrap();
    let clock: Clock = clock.deserialize_data().unwrap();

    let labels = HashMap::from([
        (key!("3jdYcGYZaQVvcvMQGqVpt37JegEoDDnX7k4gSGAeGRqG"), "USDC"),
        (key!("4kUrHxiMfeKPGDi6yFV7kte8JjN3NG3aqG7bui4pfMqz"), "BONK"),
        (key!("7MoYkgWVCEDtNR6i2WUH9LTUSFXkQCsD9tBHriHQvuP5"), "BTC"),
        (key!("So11111111111111111111111111111111111111112"), "SOL"),
    ]);

    let amount = HashMap::from([
        (("USDC", "BONK"), 0),
        (("USDC", "BTC"), 50000),
        (("USDC", "SOL"), 1000),
    ]);
    let keyed_pool = KeyedAccount {
        account: pool_acc,
        key: pool_key,
        params: None,
    };

    let mut amm = PoolAmm::from_keyed_account(
        &keyed_pool,
        &AmmContext {
            clock_ref: ClockRef::from(clock),
        },
    )
    .unwrap();

    let accounts = amm.get_accounts_to_update();

    let account_map: HashMap<Pubkey, Account> = accounts
        .into_iter()
        .map(|p| (p, client.get_account(&p).unwrap()))
        .collect();

    amm.update(&account_map).unwrap();

    let accounts = amm.get_accounts_to_update();

    let account_map: HashMap<Pubkey, Account> = accounts
        .into_iter()
        .map(|p| (p, client.get_account(&p).unwrap()))
        .collect();

    amm.update(&account_map).unwrap();

    let mints = amm.get_reserve_mints();

    println!("====================================");

    // USDC
    let input_mint = key!("3jdYcGYZaQVvcvMQGqVpt37JegEoDDnX7k4gSGAeGRqG");

    for output_mint in &mints {
        if &input_mint != output_mint {
            let input_label = labels[&input_mint];
            let output_label = labels[output_mint];

            println!("INPUT: {}, OUTPUT: {}", input_label, output_label);
            let amount = amount[&(input_label, output_label)];
            let quote = amm
                .quote(&QuoteParams {
                    amount,
                    input_mint: input_mint,
                    output_mint: *output_mint,
                    swap_mode: SwapMode::ExactIn, //TODO do exact out
                })
                .unwrap();
            println!("{quote:?}");
            println!("====================================");
        }
    }
}
