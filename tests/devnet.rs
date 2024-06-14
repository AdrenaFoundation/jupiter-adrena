use adrena::program::Adrena;
use jupiter_adrena::PoolAmm;
use jupiter_amm_interface::{Amm, AmmContext, ClockRef, KeyedAccount};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{clock::Clock, pubkey as key, pubkey::Pubkey, sysvar::SysvarId};

#[test]
fn test() {
    let client = RpcClient::new("https://api.devnet.solana.com");

    let clock = client.get_account(&Clock::id()).unwrap();
    let pool_key = key!("8Hgu4wTyMvdQk9gfXxoEtujfumMMWuPVdMWVrs73qgsa");
    let pool_acc = client.get_account(&pool_key).unwrap();

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
}
