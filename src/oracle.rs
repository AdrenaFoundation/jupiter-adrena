use anyhow::bail;
use pyth_sdk_solana::state::SolanaPriceAccount;
use solana_sdk::account_info::AccountInfo;

use crate::{
    generated::{OracleParams, OracleType},
    math,
};

pub const BPS_POWER: u128 = 10u64.pow(BPS_DECIMALS as u32) as u128;

const BPS_DECIMALS: u8 = 4;
const PRICE_DECIMALS: u8 = 6;
const USD_DECIMALS: u8 = 6;

const ORACLE_EXPONENT_SCALE: i32 = -9;
const ORACLE_PRICE_SCALE: u128 = 1_000_000_000;
const ORACLE_MAX_PRICE: u64 = (1 << 28) - 1;

const MAX_PRICE_ERROR: u16 = 50;
const MAX_PRICE_AGE_SEC: u16 = 3;

#[derive(Copy, Clone, Eq, PartialEq, Default, Debug)]
pub struct OraclePrice {
    pub price: u64,
    pub exponent: i32,
    // in BPS
    pub conf: u64,
}

impl OraclePrice {
    pub fn new(price: u64, exponent: i32, conf: u64) -> Self {
        Self {
            price,
            exponent,
            conf,
        }
    }

    pub fn new_from_oracle(
        oracle_account: &AccountInfo,
        oracle_params: &OracleParams,
        current_time: i64,
    ) -> anyhow::Result<Self> {
        match OracleType::try_from(oracle_params.oracle_type)? {
            OracleType::Pyth => Self::get_pyth_price(oracle_account, current_time),
            _ => Err(anyhow::anyhow!("UnsupportedOracle")),
        }
    }

    // Converts token amount to USD with implied USD_DECIMALS decimals using oracle price
    pub fn get_asset_amount_usd(
        &self,
        token_amount: u64,
        token_decimals: u8,
    ) -> anyhow::Result<u64> {
        if token_amount == 0 || self.price == 0 {
            return Ok(0);
        }

        math::checked_decimal_mul(
            token_amount,
            -(token_decimals as i32),
            self.price,
            self.exponent,
            -(USD_DECIMALS as i32),
        )
    }

    // Converts USD amount with implied USD_DECIMALS decimals to token amount
    pub fn get_token_amount(
        &self,
        asset_amount_usd: u64,
        token_decimals: u8,
    ) -> anyhow::Result<u64> {
        if asset_amount_usd == 0 || self.price == 0 {
            return Ok(0);
        }

        math::checked_decimal_div(
            asset_amount_usd,
            -(USD_DECIMALS as i32),
            self.price,
            self.exponent,
            -(token_decimals as i32),
        )
    }

    /// Returns price with mantissa normalized to be less than ORACLE_MAX_PRICE
    pub fn normalize(&self) -> OraclePrice {
        let mut p = self.price;
        let mut e = self.exponent;

        while p > ORACLE_MAX_PRICE {
            p /= 10;
            e += 1;
        }

        OraclePrice {
            price: p,
            exponent: e,
            conf: self.conf,
        }
    }

    pub fn checked_div(&self, other: &OraclePrice) -> anyhow::Result<OraclePrice> {
        let base = self.normalize();
        let other = other.normalize();

        Ok(OraclePrice {
            price: math::checked_as_u64(
                (base.price as u128 * ORACLE_PRICE_SCALE) / other.price as u128,
            )?,
            exponent: (base.exponent + ORACLE_EXPONENT_SCALE) - other.exponent,
            conf: math::checked_as_u64(
                (base.conf as u128 * ORACLE_PRICE_SCALE) / other.conf as u128,
            )?,
        })
    }

    pub fn scale_to_exponent(&self, target_exponent: i32) -> anyhow::Result<OraclePrice> {
        if target_exponent == self.exponent {
            return Ok(*self);
        }

        let delta = target_exponent - self.exponent;

        if delta > 0 {
            Ok(OraclePrice {
                price: self.price / math::checked_pow(10, delta as usize)?,
                exponent: target_exponent,
                conf: self.conf,
            })
        } else {
            Ok(OraclePrice {
                price: self.price * math::checked_pow(10, (-delta) as usize)?,
                exponent: target_exponent,
                conf: self.conf,
            })
        }
    }

    fn get_pyth_price(
        pyth_price_info: &AccountInfo,
        current_time: i64,
    ) -> anyhow::Result<OraclePrice> {
        if pyth_price_info.try_data_is_empty()? || pyth_price_info.try_lamports()? == 0 {
            bail!("InvalidOracleAccount");
        }

        let price_feed = SolanaPriceAccount::account_info_to_feed(pyth_price_info)
            .map_err(|_| anyhow::anyhow!("InvalidOracleAccount"))?;

        let pyth_price = price_feed.get_price_unchecked();

        let last_update_age_sec = current_time - pyth_price.publish_time;

        if last_update_age_sec > MAX_PRICE_AGE_SEC as i64 {
            bail!("StaleOraclePrice");
        }

        // Turn confidence from value into BPS
        let conf = math::checked_as_u64(math::checked_ceil_div::<u128>(
            pyth_price.conf as u128 * BPS_POWER,
            pyth_price.price as u128,
        )?)?;

        if pyth_price.price <= 0 || conf > MAX_PRICE_ERROR as u64 {
            bail!("InvalidOraclePrice");
        }

        OraclePrice {
            // price is i64 and > 0 per check above
            price: pyth_price.price as u64,
            exponent: pyth_price.expo,
            conf,
        }
        .scale_to_exponent(-(PRICE_DECIMALS as i32))
    }
}
