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
        (key!("HNyQyAanLYHPjoXtw2V6pAdv8d925z1ytYpw1uRftv2N"), "ALP"),
    ]);

    let amount = HashMap::from([
        (("SOL", "BONK"), 10_000_000_000),
        (("SOL", "BTC"), 10_000_000_000),
        (("SOL", "USDC"), 10_000_000_000),
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

    println!("{:?}", accounts);

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

    // SOL
    let input_mint = key!("So11111111111111111111111111111111111111112");

    for output_mint in &mints {
        if &input_mint != output_mint {
            let input_label = labels[&input_mint];
            let output_label = labels[output_mint];

            if output_label != "BONK" {
                println!("INPUT: {}, OUTPUT: {}", input_label, output_label);
                let amount = amount[&(input_label, output_label)];
                let quote = amm
                    .quote(&QuoteParams {
                        amount,
                        input_mint,
                        output_mint: *output_mint,
                        swap_mode: SwapMode::ExactIn, //TODO do exact out
                    })
                    .unwrap();
                println!("{quote:?}");
                println!("====================================");
            }
        }
    }

    println!("INPUT: LP, OUTPUT: SOL");
    let quote = amm
        .quote(&QuoteParams {
            amount: 1_000_000_000,
            input_mint: key!("HNyQyAanLYHPjoXtw2V6pAdv8d925z1ytYpw1uRftv2N"),
            output_mint: key!("So11111111111111111111111111111111111111112"),
            swap_mode: SwapMode::ExactIn, //TODO do exact out
        })
        .unwrap();
    println!("{quote:?}");
    println!("====================================");

    println!("INPUT: SOL, OUTPUT: LP");
    let quote = amm
        .quote(&QuoteParams {
            amount: 1_000_000_000,
            input_mint: key!("So11111111111111111111111111111111111111112"),
            output_mint: key!("HNyQyAanLYHPjoXtw2V6pAdv8d925z1ytYpw1uRftv2N"),
            swap_mode: SwapMode::ExactIn, //TODO do exact out
        })
        .unwrap();
    println!("{quote:?}");
    println!("====================================");
}
