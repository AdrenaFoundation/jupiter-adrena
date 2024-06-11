use adrena::{
    accounts::Swap,
    state::{custody::Custody, oracle::OraclePrice, pool::Pool},
};
use anchor_lang::{AccountDeserialize, ToAccountMetas};
use anyhow::anyhow;
use chrono::{DateTime, Utc};
use jupiter_amm_interface::{try_get_account_data, Amm, AmmContext, Quote, SwapAndAccountMetas};
use rust_decimal::Decimal;
use solana_sdk::pubkey as key;
use solana_sdk::{instruction::AccountMeta, pubkey::Pubkey};
use std::collections::HashMap;

// mod generated;
// mod math;
// mod oracle;

//TODO First iteration, let me to push the second before reviewing stuff and put TODO

const SPL_TOKEN_ID: Pubkey = key!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

#[derive(Clone, Debug)]
pub enum UpdateStatus {
    Custodies,
    OraclesAndTokens,
}

#[derive(Clone, Debug)]
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
        amm_context: &AmmContext,
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
                    // let oracle = OraclePrice::
                    // let custody = OraclePrice::try_deserialize(&mut try_get_account_data(
                    //     account_map,
                    //     custody_key,
                    // )?)?;
                    // self.custodies.insert(*custody_key, custody);
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

        let amount_out = self.state.get_swap_amount(
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
            amount_out,
            &custody_in,
            &token_price_in,
            &custody_out,
            &token_price_out,
        )?;

        let no_fee_amount = amount_out - fees.1;

        let quote = Quote {
            fee_amount: fees.1,
            min_out_amount: None,
            min_in_amount: None,
            fee_mint: quote_params.input_mint,
            fee_pct: Decimal::TEN,
            in_amount: quote_params.amount,
            out_amount: no_fee_amount,
        };
        Ok(quote)
    }

    fn get_swap_and_account_metas(
        &self,
        swap_params: &jupiter_amm_interface::SwapParams,
    ) -> anyhow::Result<jupiter_amm_interface::SwapAndAccountMetas> {
        let pda = |seeds: &[&[u8]]| Pubkey::find_program_address(seeds, &self.program_id).0;

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
        let lp_token_mint = pda(&[b"lp_token_mint", self.key.as_ref()]);
        let lp_staking = pda(&[b"staking", lp_token_mint.as_ref()]);
        let lm_staking = pda(&[b"staking"]); //TODO staked token
        let cortex = pda(&[b"cortex"]);
        let user_profile = pda(&[b"user_profile", owner.as_ref()]);
        let lm_staking_reward_token_vault =
            pda(&[b"staking_reward_token_vault", lm_staking.as_ref()]);
        let lp_staking_reward_token_vault =
            pda(&[b"staking_reward_token_vault", lp_staking.as_ref()]);

        let account_metas = Swap {
            owner,
            funding_account: swap_params.source_token_account,
            receiving_account: swap_params.destination_token_account,
            transfer_authority: owner,
            cortex,
            lm_staking,
            lp_staking,
            pool: self.key,
            staking_reward_token_custody: todo!(),
            staking_reward_token_custody_oracle_account: todo!(),
            staking_reward_token_custody_token_account: todo!(),
            receiving_custody: *receiving_custody,
            receiving_custody_oracle_account,
            receiving_custody_token_account,
            dispensing_custody: *dispensing_custody,
            dispensing_custody_oracle_account,
            dispensing_custody_token_account,
            lm_staking_reward_token_vault,
            lp_staking_reward_token_vault,
            lp_token_mint,
            protocol_fee_recipient: todo!(),
            user_profile: Some(user_profile),
            token_program: SPL_TOKEN_ID,
            adrena_program: adrena::ID,
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
