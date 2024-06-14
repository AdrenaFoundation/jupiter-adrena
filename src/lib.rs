use adrena::{
    accounts::Swap,
    state::{
        custody::Custody,
        oracle::{OracleParams, OraclePrice},
        pool::Pool,
    },
};
use anchor_lang::{AccountDeserialize, AnchorDeserialize, ToAccountMetas};
use anyhow::anyhow;
use anyhow::Context;
use chrono::{DateTime, Utc};
use jupiter_amm_interface::{try_get_account_data, Amm, AmmContext, Quote, SwapAndAccountMetas};
use num_traits::FromPrimitive;
use rust_decimal::Decimal;
use solana_sdk::pubkey as key;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;

const SPL_TOKEN_ID: Pubkey = key!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

const PROTOCOL_FEE_RECIPIENT: Pubkey = key!("5STGJRnjLKbssEkk5AmKpqebPLt5yk71RMFmGtxWwjgG");
const FEE_REDISTRIBUTION_MINT: Pubkey = key!("3jdYcGYZaQVvcvMQGqVpt37JegEoDDnX7k4gSGAeGRqG");

//TODO: staking_reward_token_custody_oracle_account
// const REWARD_ORACLE_ACCOUNT: Pubkey = key!("")

#[derive(Clone, Debug)]
pub enum UpdateStatus {
    Custodies,
    OraclesAndTokens,
}

#[derive(Clone)]
pub struct PoolAmm {
    key: Pubkey,
    state: Pool,
    custodies: HashMap<Pubkey, Custody>,
    oracle_prices: HashMap<Pubkey, OraclePrice>,
    program_id: Pubkey,
    current_time: DateTime<Utc>,
    update_status: UpdateStatus,
}

impl PoolAmm {
    fn pda(&self, seeds: &[&[u8]]) -> Pubkey {
        Pubkey::find_program_address(seeds, &self.program_id).0
    }
}

impl Amm for PoolAmm {
    //TODO work with the new amm interface
    fn from_keyed_account(
        keyed_account: &jupiter_amm_interface::KeyedAccount,
        _amm_context: &AmmContext,
    ) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let pool = Pool::try_deserialize(&mut &keyed_account.account.data[..])?;

        Ok(PoolAmm {
            key: keyed_account.key,
            program_id: keyed_account.key,
            state: pool,
            custodies: HashMap::new(),
            oracle_prices: HashMap::new(),
            current_time: Utc::now(),
            update_status: UpdateStatus::Custodies,
        })
    }

    fn label(&self) -> String {
        self.state.name.to_string()
    }

    fn program_id(&self) -> Pubkey {
        self.program_id
    }

    fn key(&self) -> Pubkey {
        self.key
    }

    fn requires_update_for_reserve_mints(&self) -> bool {
        true
    }

    fn get_reserve_mints(&self) -> Vec<Pubkey> {
        self.custodies.values().map(|c| c.mint).collect()
    }

    fn get_accounts_to_update(&self) -> Vec<Pubkey> {
        match self.update_status {
            UpdateStatus::Custodies => {
                let mut keys = vec![self.key];

                keys.append(&mut self.state.custodies.to_vec());
                keys
            }
            UpdateStatus::OraclesAndTokens => self
                .custodies
                .values()
                .map(|c| c.oracle.oracle_account)
                .collect(),
        }
    }

    fn update(&mut self, account_map: &jupiter_amm_interface::AccountMap) -> anyhow::Result<()> {
        self.current_time = Utc::now();

        match self.update_status {
            UpdateStatus::Custodies => {
                let pool_state =
                    Pool::try_deserialize(&mut try_get_account_data(account_map, &self.key)?)?;

                self.state = pool_state;

                for custody_key in &self.state.custodies {
                    let custody = Custody::try_deserialize(&mut try_get_account_data(
                        account_map,
                        custody_key,
                    )?)?;
                    self.custodies.insert(*custody_key, custody);
                }

                self.update_status = UpdateStatus::OraclesAndTokens;
            }
            UpdateStatus::OraclesAndTokens => {
                let oracle_keys = self.custodies.values().map(|c| c.oracle.oracle_account);

                for oracle_key in oracle_keys {
                    let oracle_price = OraclePrice::try_from_slice(&try_get_account_data(
                        account_map,
                        &oracle_key,
                    )?)?;
                    self.oracle_prices.insert(oracle_key, oracle_price);
                }

                self.update_status = UpdateStatus::Custodies;
            }
        }

        Ok(())
    }

    fn quote(
        &self,
        quote_params: &jupiter_amm_interface::QuoteParams,
    ) -> anyhow::Result<jupiter_amm_interface::Quote> {
        let custody_in_mint = quote_params.input_mint;
        let custody_out_mint = quote_params.output_mint;

        let custody_in_pubkey =
            self.pda(&[b"custody", self.key.as_ref(), custody_in_mint.as_ref()]);
        let custody_out_pubkey =
            self.pda(&[b"custody", self.key.as_ref(), custody_out_mint.as_ref()]);

        let custody_in = self
            .custodies
            .get(&custody_in_pubkey)
            .ok_or(anyhow!("Custody does not exist: {}", custody_in_pubkey))?;

        let custody_out = self
            .custodies
            .get(&custody_out_pubkey)
            .ok_or(anyhow!("Custody does not exist {}", custody_out_pubkey))?;

        let token_price_in = self
            .oracle_prices
            .get(&custody_in.oracle.oracle_account)
            .ok_or(anyhow!(
                "Oracle does not exist: {}",
                custody_in.oracle.oracle_account
            ))?;

        let token_price_out = self
            .oracle_prices
            .get(&custody_out.oracle.oracle_account)
            .ok_or(anyhow!(
                "Oracle does not exist: {}",
                custody_out.oracle.oracle_account
            ))?;

        let token_id_in = self.state.get_token_id(&custody_in_pubkey)?;
        let token_id_out = self.state.get_token_id(&custody_out_pubkey)?;

        let out_amount = self.state.get_swap_amount(
            &token_price_in,
            &token_price_out,
            &custody_in,
            &custody_out,
            quote_params.amount,
        )?;

        let fees = self.state.get_swap_fees(
            token_id_in,
            token_id_out,
            quote_params.amount,
            out_amount,
            &custody_in,
            &token_price_in,
            &custody_out,
            &token_price_out,
        )?;

        let fee_amount = fees.0 + fees.1;

        let out_dec = Decimal::from_u64(out_amount).with_context(|| "Can't convert out_amount")?;
        let fee_dec = Decimal::from_u64(fee_amount).with_context(|| "Can't convert fee_amount")?;

        let fee_pct = Decimal::ONE_HUNDRED
            .checked_mul(out_dec)
            .and_then(|per| per.checked_div(fee_dec))
            .ok_or(anyhow!("Can't calculate fee percentage"))?;

        let quote = Quote {
            fee_amount,
            min_out_amount: None,
            min_in_amount: None,
            fee_mint: quote_params.input_mint,
            fee_pct,
            in_amount: quote_params.amount,
            out_amount,
        };
        Ok(quote)
    }

    fn get_swap_and_account_metas(
        &self,
        swap_params: &jupiter_amm_interface::SwapParams,
    ) -> anyhow::Result<jupiter_amm_interface::SwapAndAccountMetas> {
        let (dispensing_custody, dispensing_cust_state) = self
            .custodies
            .iter()
            .find(|c| c.1.mint == swap_params.source_mint)
            .ok_or(anyhow!(
                "Can't find the custody for the mint {}",
                swap_params.source_mint
            ))?;
        let (receiving_custody, receiving_cust_state) = self
            .custodies
            .iter()
            .find(|c| c.1.mint == swap_params.destination_mint)
            .ok_or(anyhow!(
                "Can't find the custody for the mint {}",
                swap_params.destination_mint
            ))?;

        let dispensing_custody_mint = swap_params.source_mint;
        let dispensing_custody_oracle_account = dispensing_cust_state.oracle.oracle_account;
        let dispensing_custody_token_account = dispensing_cust_state.token_account;
        let receiving_custody_mint = swap_params.destination_mint;
        let receiving_custody_oracle_account = receiving_cust_state.oracle.oracle_account;
        let receiving_custody_token_account = receiving_cust_state.token_account;
        let owner = swap_params.token_transfer_authority;
        let lp_token_mint = self.pda(&[b"lp_token_mint", self.key.as_ref()]);
        let lp_staking = self.pda(&[b"staking", lp_token_mint.as_ref()]);
        let lm_staking = self.pda(&[b"staking"]); //TODO staked token
        let cortex = self.pda(&[b"cortex"]);
        let user_profile = self.pda(&[b"user_profile", owner.as_ref()]);
        let lm_staking_reward_token_vault =
            self.pda(&[b"staking_reward_token_vault", lm_staking.as_ref()]);
        let lp_staking_reward_token_vault =
            self.pda(&[b"staking_reward_token_vault", lp_staking.as_ref()]);
        let staking_reward_token_custody = self.pda(&[
            b"custody",
            self.key.as_ref(),
            FEE_REDISTRIBUTION_MINT.as_ref(),
        ]);
        // let staking_reward_token_custody_oracle_account = pda();
        let staking_reward_token_custody_token_account = self.pda(&[
            b"custody_token_account",
            self.key.as_ref(),
            FEE_REDISTRIBUTION_MINT.as_ref(),
        ]);

        let account_metas = Swap {
            owner,
            funding_account: swap_params.source_token_account,
            receiving_account: swap_params.destination_token_account,
            transfer_authority: owner,
            cortex,
            lm_staking,
            lp_staking,
            pool: self.key,
            staking_reward_token_custody,
            staking_reward_token_custody_oracle_account: todo!(),
            staking_reward_token_custody_token_account,
            receiving_custody: *receiving_custody,
            receiving_custody_oracle_account,
            receiving_custody_token_account,
            dispensing_custody: *dispensing_custody,
            dispensing_custody_oracle_account,
            dispensing_custody_token_account,
            lm_staking_reward_token_vault,
            lp_staking_reward_token_vault,
            lp_token_mint,
            protocol_fee_recipient: PROTOCOL_FEE_RECIPIENT,
            user_profile: Some(user_profile),
            token_program: SPL_TOKEN_ID,
            adrena_program: self.program_id,
        }
        .to_account_metas(None);

        Ok(SwapAndAccountMetas {
            swap: jupiter_amm_interface::Swap::Saber, //TODO Switch to Adrena
            account_metas,
        })
    }

    fn clone_amm(&self) -> Box<dyn Amm + Send + Sync> {
        Box::new(self.clone())
    }
}
