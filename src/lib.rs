use adrena::{
    accounts::Swap,
    state::{
        custody::Custody,
        oracle::{OracleParams, OraclePrice, OracleType},
        pool::Pool,
    },
};
use anchor_lang::{system_program, AccountDeserialize, ToAccountMetas};
use anyhow::anyhow;
use anyhow::Context;
use jupiter_amm_interface::{
    try_get_account_data, Amm, AmmContext, ClockRef, Quote, SwapAndAccountMetas,
};
use num_traits::FromPrimitive;
use rust_decimal::Decimal;
use solana_sdk::{account_info::IntoAccountInfo, pubkey::Pubkey};
use solana_sdk::{clock::Clock, pubkey as key, sysvar::SysvarId};
use std::{collections::HashMap, sync::atomic::Ordering};

const SPL_TOKEN_ID: Pubkey = key!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

const PROTOCOL_FEE_RECIPIENT: Pubkey = key!("5STGJRnjLKbssEkk5AmKpqebPLt5yk71RMFmGtxWwjgG");
const FEE_REDISTRIBUTION_MINT: Pubkey = key!("3jdYcGYZaQVvcvMQGqVpt37JegEoDDnX7k4gSGAeGRqG");
const REWARD_ORACLE_ACCOUNT: Pubkey = key!("5SSkXsEKQepHHAewytPVwdej4epN1nxgLVM84L4KXgy7");
const LM_STAKING: Pubkey = key!("AUP8PVY9gC5VGmTdyZLVB2DskLeScKGxY5VeZtZN7hFR");

#[derive(Clone, Debug)]
pub enum UpdateStatus {
    Custodies,
    OraclesAndTokens,
}

pub struct CalculateFeesParams<'a> {
    in_oracle: &'a OraclePrice,
    in_decimals: u8,
    int_amount: u64,
    out_oracle: &'a OraclePrice,
    out_decimals: u8,
    out_amount: u64,
    fees: (u64, u64),
}

#[derive(Clone)]
pub struct PoolAmm {
    key: Pubkey,
    state: Pool,
    custodies: HashMap<Pubkey, Custody>,
    oracle_prices: HashMap<Pubkey, OraclePrice>,
    program_id: Pubkey,
    update_status: UpdateStatus,
    clock_ref: ClockRef,
}

impl PoolAmm {
    fn pda(&self, seeds: &[&[u8]]) -> Pubkey {
        Pubkey::find_program_address(seeds, &self.program_id).0
    }

    fn get_custody_and_oracle(
        &self,
        mint: Pubkey,
    ) -> anyhow::Result<(Pubkey, &Custody, &OraclePrice)> {
        let custody_key = self.pda(&[b"custody", self.key.as_ref(), mint.as_ref()]);

        let custody = self
            .custodies
            .get(&custody_key)
            .context(format!("Custody does not exist: {custody_key}"))?;

        let oracle_price = self
            .oracle_prices
            .get(&custody.oracle.oracle_account)
            .context(format!(
                "Oracle does not exist: {}",
                custody.oracle.oracle_account
            ))?;

        Ok((custody_key, custody, oracle_price))
    }

    fn calculate_fees(
        &self,
        CalculateFeesParams {
            in_oracle,
            in_decimals,
            int_amount,
            out_oracle,
            out_decimals,
            out_amount,
            fees,
        }: CalculateFeesParams,
    ) -> anyhow::Result<(u64, Decimal)> {
        let (_, fees_custody, fees_price) = self.get_custody_and_oracle(FEE_REDISTRIBUTION_MINT)?;

        let fees_in_usd = in_oracle.get_asset_amount_usd(fees.0, in_decimals)?;
        let fees_out_usd = out_oracle.get_asset_amount_usd(fees.1, out_decimals)?;
        let in_usd = in_oracle.get_asset_amount_usd(int_amount, in_decimals)?;
        let out_usd = out_oracle.get_asset_amount_usd(out_amount, out_decimals)?;

        let total_amount = in_usd + out_usd;
        let total_fees = fees_in_usd + fees_out_usd;

        let total_amount_dec =
            Decimal::from_u64(total_amount).context("Can't convert out_amount")?;
        let total_fees_dec = Decimal::from_u64(total_fees).context("Can't convert out_amount")?;

        let fee_pct = Decimal::ONE_HUNDRED
            .checked_mul(total_fees_dec)
            .and_then(|per| per.checked_div(total_amount_dec))
            .context("Can't calculate fee percentage")?;

        let reward_fees = fees_price.get_token_amount(total_fees, fees_custody.decimals)?;

        Ok((reward_fees, fee_pct))
    }
}

impl Amm for PoolAmm {
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
            program_id: keyed_account.account.owner,
            state: pool,
            custodies: HashMap::new(),
            oracle_prices: HashMap::new(),
            update_status: UpdateStatus::Custodies,
            clock_ref: amm_context.clock_ref.clone(),
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

                keys.append(
                    &mut self
                        .state
                        .custodies
                        .into_iter()
                        .filter(|acc| *acc != system_program::ID)
                        .collect(),
                );
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
        let clock = account_map
            .get(&Clock::id())
            .with_context(|| format!("Could not find address: {}", Clock::id()))?
            .deserialize_data()?;
        self.clock_ref.update(clock);

        match self.update_status {
            UpdateStatus::Custodies => {
                let pool_state =
                    Pool::try_deserialize(&mut try_get_account_data(account_map, &self.key)?)?;

                self.state = pool_state;

                for custody_key in &self.state.custodies {
                    if *custody_key != system_program::ID {
                        let custody = Custody::try_deserialize(&mut try_get_account_data(
                            account_map,
                            custody_key,
                        )?)?;
                        self.custodies.insert(*custody_key, custody);
                    }
                }

                self.update_status = UpdateStatus::OraclesAndTokens;
            }
            UpdateStatus::OraclesAndTokens => {
                let oracle_keys = self.custodies.values().map(|c| c.oracle.oracle_account);

                for oracle_key in oracle_keys {
                    let oracle_account = account_map
                        .get(&oracle_key)
                        .with_context(|| format!("Could not find address: {oracle_key}"))?
                        .to_owned();

                    let oracle_price = OraclePrice::new_from_oracle(
                        &(oracle_key, oracle_account).into_account_info(),
                        &OracleParams {
                            oracle_type: OracleType::Pyth.into(),
                            ..Default::default()
                        },
                        self.clock_ref.unix_timestamp.load(Ordering::Relaxed),
                    )?;

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

        let (custody_in_pubkey, custody_in, token_price_in) =
            self.get_custody_and_oracle(custody_in_mint)?;
        let (custody_out_pubkey, custody_out, token_price_out) =
            self.get_custody_and_oracle(custody_out_mint)?;

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

        let real_out_amount = out_amount - fees.1;

        let (fee_amount, fee_pct) = self.calculate_fees(CalculateFeesParams {
            fees,
            in_decimals: custody_in.decimals,
            in_oracle: token_price_in,
            int_amount: quote_params.amount,
            out_amount,
            out_decimals: custody_out.decimals,
            out_oracle: token_price_out,
        })?;

        let quote = Quote {
            fee_amount,
            min_out_amount: None,
            min_in_amount: None,
            fee_mint: FEE_REDISTRIBUTION_MINT,
            fee_pct,
            in_amount: quote_params.amount,
            out_amount: real_out_amount,
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

        let dispensing_custody_oracle_account = dispensing_cust_state.oracle.oracle_account;
        let dispensing_custody_token_account = dispensing_cust_state.token_account;
        let receiving_custody_oracle_account = receiving_cust_state.oracle.oracle_account;
        let receiving_custody_token_account = receiving_cust_state.token_account;
        let owner = swap_params.token_transfer_authority;
        let lp_token_mint = self.pda(&[b"lp_token_mint", self.key.as_ref()]);
        let lp_staking = self.pda(&[b"staking", lp_token_mint.as_ref()]);
        let cortex = self.pda(&[b"cortex"]);
        let user_profile = self.pda(&[b"user_profile", owner.as_ref()]);
        let lm_staking_reward_token_vault =
            self.pda(&[b"staking_reward_token_vault", LM_STAKING.as_ref()]);
        let lp_staking_reward_token_vault =
            self.pda(&[b"staking_reward_token_vault", lp_staking.as_ref()]);
        let staking_reward_token_custody = self.pda(&[
            b"custody",
            self.key.as_ref(),
            FEE_REDISTRIBUTION_MINT.as_ref(),
        ]);
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
            lm_staking: LM_STAKING,
            lp_staking,
            pool: self.key,
            staking_reward_token_custody,
            staking_reward_token_custody_oracle_account: REWARD_ORACLE_ACCOUNT,
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
