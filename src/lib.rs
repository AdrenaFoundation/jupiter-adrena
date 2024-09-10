mod quote;

use adrena::state::{custody::Custody, oracle::OraclePrice, pool::Pool};
use anchor_lang::{system_program, AccountDeserialize};
use anyhow::Context;
use jupiter_amm_interface::{try_get_account_data, Amm, AmmContext, Quote, SwapAndAccountMetas};
use num_traits::FromPrimitive;
use quote::{
    calculate_add_liquidity, calculate_remove_liquidity, calculate_swap, get_add_liquidity_metas,
    get_remove_liquidity_metas, get_swap_metas, ComputeResult,
};
use rust_decimal::Decimal;
use solana_sdk::{account_info::IntoAccountInfo, pubkey as key, pubkey::Pubkey};
use spl_token::{solana_program::program_pack::Pack, state::Mint};
use std::collections::HashMap;

const SPL_TOKEN_ID: Pubkey = key!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

const PROTOCOL_FEE_RECIPIENT: Pubkey = key!("5STGJRnjLKbssEkk5AmKpqebPLt5yk71RMFmGtxWwjgG");
const FEE_REDISTRIBUTION_MINT: Pubkey = key!("3jdYcGYZaQVvcvMQGqVpt37JegEoDDnX7k4gSGAeGRqG");
const REWARD_ORACLE_ACCOUNT: Pubkey = key!("5SSkXsEKQepHHAewytPVwdej4epN1nxgLVM84L4KXgy7");
const LM_STAKING: Pubkey = key!("AUP8PVY9gC5VGmTdyZLVB2DskLeScKGxY5VeZtZN7hFR");

#[derive(Clone, Debug)]
pub enum UpdateType {
    Custodies,
    OraclesAndTokens,
}

pub struct CalculateFeesParams<'a> {
    in_oracle: &'a OraclePrice,
    in_decimals: u8,
    in_amount: u64,
    out_oracle: &'a OraclePrice,
    out_decimals: u8,
    out_amount: u64,
    fees: (u64, u64),
}

#[derive(Clone)]
pub struct PoolAmm {
    pool_key: Pubkey,
    pool: Pool,
    custodies: HashMap<Pubkey, Custody>,
    oracle_prices: HashMap<Pubkey, OraclePrice>,
    lp_token_mint: (Pubkey, Option<Mint>),
    program_id: Pubkey,
    update_type: UpdateType,
}

impl PoolAmm {
    fn pda(&self, seeds: &[&[u8]]) -> Pubkey {
        Pubkey::find_program_address(seeds, &self.program_id).0
    }

    fn get_custody_and_oracle(
        &self,
        mint: Pubkey,
    ) -> anyhow::Result<(Pubkey, &Custody, &OraclePrice)> {
        let custody_key = self.pda(&[b"custody", self.pool_key.as_ref(), mint.as_ref()]);

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

    fn calculate_swap_fees(
        &self,
        CalculateFeesParams {
            in_oracle,
            in_decimals,
            in_amount,
            out_oracle,
            out_decimals,
            out_amount,
            fees,
        }: CalculateFeesParams,
    ) -> anyhow::Result<(u64, Decimal)> {
        let (_, fees_custody, fees_price) = self.get_custody_and_oracle(FEE_REDISTRIBUTION_MINT)?;

        let fees_in_usd = in_oracle.get_asset_amount_usd(fees.0, in_decimals)?;
        let fees_out_usd = out_oracle.get_asset_amount_usd(fees.1, out_decimals)?;
        let in_usd = in_oracle.get_asset_amount_usd(in_amount, in_decimals)?;
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
        _amm_context: &AmmContext,
    ) -> anyhow::Result<Self> {
        let program_id = keyed_account.account.owner;
        let pool_key = keyed_account.key;
        let pool = Pool::try_deserialize(&mut &keyed_account.account.data[..])?;
        let lp_token_mint = Pubkey::create_program_address(
            &[b"lp_token_mint", pool_key.as_ref(), &[pool.lp_token_bump]],
            &program_id,
        )?;

        Ok(PoolAmm {
            pool_key: keyed_account.key,
            program_id,
            pool,
            custodies: HashMap::new(),
            oracle_prices: HashMap::new(),
            lp_token_mint: (lp_token_mint, None),
            update_type: UpdateType::Custodies,
        })
    }

    fn label(&self) -> String {
        self.pool.name.to_string()
    }

    fn program_id(&self) -> Pubkey {
        self.program_id
    }

    fn key(&self) -> Pubkey {
        self.pool_key
    }

    fn requires_update_for_reserve_mints(&self) -> bool {
        true
    }

    fn get_reserve_mints(&self) -> Vec<Pubkey> {
        // Should all custodies be considered reserves? (Stable too?)
        // Add ALP mint/redeem which is not a custody
        self.custodies.values().map(|c| c.mint).collect()
    }

    fn get_accounts_to_update(&self) -> Vec<Pubkey> {
        match self.update_type {
            UpdateType::Custodies => {
                let mut keys = vec![self.pool_key, self.lp_token_mint.0];

                keys.append(
                    &mut self
                        .pool
                        .custodies
                        .into_iter()
                        .filter(|acc| *acc != system_program::ID)
                        .collect(),
                );
                keys
            }
            UpdateType::OraclesAndTokens => self
                .custodies
                .values()
                .map(|c| c.oracle.oracle_account)
                .collect(),
        }
    }

    /// Indicates if get_accounts_to_update might return a non constant vec
    fn has_dynamic_accounts(&self) -> bool {
        true
    }

    fn update(&mut self, account_map: &jupiter_amm_interface::AccountMap) -> anyhow::Result<()> {
        match self.update_type {
            UpdateType::Custodies => {
                let pool =
                    Pool::try_deserialize(&mut try_get_account_data(account_map, &self.pool_key)?)?;

                self.pool = pool;

                self.lp_token_mint.1 = Some(Mint::unpack(&mut try_get_account_data(
                    account_map,
                    &self.lp_token_mint.0,
                )?)?);

                for custody_key in &self.pool.custodies {
                    if *custody_key != system_program::ID {
                        let custody = Custody::try_deserialize(&mut try_get_account_data(
                            account_map,
                            custody_key,
                        )?)?;
                        self.custodies.insert(*custody_key, custody);
                    }
                }

                self.update_type = UpdateType::OraclesAndTokens;
            }
            UpdateType::OraclesAndTokens => {
                let oracle_keys = self.custodies.values().map(|c| c.oracle.oracle_account);

                for oracle_key in oracle_keys {
                    let oracle_account = account_map
                        .get(&oracle_key)
                        .with_context(|| format!("Could not find address: {oracle_key}"))?
                        .to_owned();

                    let oracle_price = OraclePrice::new_from_pyth_price_update_v2_account_info(
                        &(oracle_key, oracle_account).into_account_info(),
                    )?;

                    self.oracle_prices.insert(oracle_key, oracle_price);
                }

                self.update_type = UpdateType::Custodies;
            }
        }

        Ok(())
    }

    fn quote(
        &self,
        quote_params: &jupiter_amm_interface::QuoteParams,
    ) -> anyhow::Result<jupiter_amm_interface::Quote> {
        let lp_token_mint_key = self.lp_token_mint.0;

        let ComputeResult {
            in_amount,
            out_amount,
            fee_amount,
            fee_pct,
        } = if lp_token_mint_key == quote_params.input_mint {
            calculate_remove_liquidity(&self, quote_params)
        } else if lp_token_mint_key == quote_params.output_mint {
            calculate_add_liquidity(&self, quote_params)
        } else {
            calculate_swap(&self, quote_params)
        }?;

        Ok(Quote {
            min_in_amount: None,
            min_out_amount: None,
            in_amount,
            out_amount,
            fee_amount,
            fee_mint: FEE_REDISTRIBUTION_MINT,
            fee_pct,
        })
    }

    fn get_swap_and_account_metas(
        &self,
        swap_params: &jupiter_amm_interface::SwapParams,
    ) -> anyhow::Result<jupiter_amm_interface::SwapAndAccountMetas> {
        let lp_token_mint_key = self.lp_token_mint.0;

        let account_metas = if lp_token_mint_key == swap_params.source_mint {
            get_remove_liquidity_metas(&self, swap_params)
        } else if lp_token_mint_key == swap_params.destination_mint {
            get_add_liquidity_metas(&self, swap_params)
        } else {
            get_swap_metas(&self, swap_params)
        }?;

        Ok(SwapAndAccountMetas {
            swap: jupiter_amm_interface::Swap::Saber, //TODO Switch to Adrena
            account_metas,
        })
    }

    fn clone_amm(&self) -> Box<dyn Amm + Send + Sync> {
        Box::new(self.clone())
    }
}
