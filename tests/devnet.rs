use std::collections::HashMap;

use jupiter_adrena::PoolAmm;
use jupiter_amm_interface::{Amm, AmmContext, ClockRef, KeyedAccount, QuoteParams, SwapMode};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{account::Account, clock::Clock, pubkey as key, pubkey::Pubkey, sysvar::SysvarId};

#[test]
fn test() {
    let client = RpcClient::new("https://api.devnet.solana.com");

    let pool_key = key!("22h3wdapjk9e4TPEtEmFcXB1dCZZEShCtGptdBCvWNQr");
    let pool_acc = client.get_account(&pool_key).unwrap();

    let clock = client.get_account(&Clock::id()).unwrap();
    let clock: Clock = clock.deserialize_data().unwrap();

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

    let mut account_map: HashMap<Pubkey, Account> = accounts
        .into_iter()
        .map(|p| (p, client.get_account(&p).unwrap()))
        .collect();

    let clock = client.get_account(&Clock::id()).unwrap();
    account_map.insert(Clock::id(), clock);

    amm.update(&account_map).unwrap();

    let accounts = amm.get_accounts_to_update();

    let mut account_map: HashMap<Pubkey, Account> = accounts
        .into_iter()
        .map(|p| (p, client.get_account(&p).unwrap()))
        .collect();
    let clock = client.get_account(&Clock::id()).unwrap();
    account_map.insert(Clock::id(), clock);

    amm.update(&account_map).unwrap();

    let mints = amm.get_reserve_mints();

    let quote = amm
        .quote(&QuoteParams {
            amount: 10000,
            input_mint: mints[0],
            output_mint: mints[1],
            swap_mode: SwapMode::ExactIn, //TODO do exact out
        })
        .unwrap();

    println!("{quote:?}");
}
