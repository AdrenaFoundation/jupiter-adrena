use crate::{
    math,
    oracle::{OraclePrice, BPS_POWER},
};
use std::fmt::Display;

include!(concat!(env!("OUT_DIR"), "/adrena.rs"));

impl Copy for LimitedString {}

impl From<LimitedString> for String {
    fn from(limited_string: LimitedString) -> Self {
        let mut string = String::new();
        for byte in limited_string.value.iter() {
            if *byte == 0 {
                break;
            }
            string.push(*byte as char);
        }
        string
    }
}

impl Display for LimitedString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", String::from(*self))
    }
}

impl TryFrom<u8> for OracleType {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> anyhow::Result<Self> {
        Ok(match value {
            0 => OracleType::None,
            1 => OracleType::Custom,
            2 => OracleType::Pyth,
            // Return an error if unknown value
            _ => Err(anyhow::anyhow!("InvalidOracleState"))?,
        })
    }
}

impl U128Split {
    pub fn to_u128(&self) -> u128 {
        ((self.high as u128) << 64) | (self.low as u128)
    }
}

impl Custody {
    pub fn is_stable(&self) -> bool {
        self.is_stable == 1
    }
}

impl Pool {
    pub fn get_token_id(&self, custody: &Pubkey) -> anyhow::Result<usize> {
        self.custodies
            .iter()
            .position(|&k| k == *custody)
            .ok_or_else(|| anyhow::anyhow!("UnsupportedToken"))
    }

    pub fn get_swap_price(
        &self,
        token_in_price: &OraclePrice,
        token_out_price: &OraclePrice,
    ) -> anyhow::Result<OraclePrice> {
        token_in_price.checked_div(token_out_price)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn get_swap_amount(
        &self,
        token_in_price: &OraclePrice,
        token_out_price: &OraclePrice,
        custody_in: &Custody,
        custody_out: &Custody,
        amount_in: u64,
    ) -> anyhow::Result<u64> {
        let swap_price = self.get_swap_price(token_in_price, token_out_price)?;

        math::checked_decimal_mul(
            amount_in,
            -(custody_in.decimals as i32),
            swap_price.price,
            swap_price.exponent,
            -(custody_out.decimals as i32),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn get_swap_fees(
        &self,
        token_id_in: usize,
        token_id_out: usize,
        amount_in: u64,
        amount_out: u64,
        custody_in: &Custody,
        token_price_in: &OraclePrice,
        custody_out: &Custody,
        token_price_out: &OraclePrice,
    ) -> anyhow::Result<(u64, u64)> {
        let stable_swap = custody_in.is_stable() && custody_out.is_stable();

        let swap_in_fee = self.get_fee(
            token_id_in,
            if stable_swap {
                custody_in.fees.stable_swap_in
            } else {
                custody_in.fees.swap_in
            },
            amount_in,
            0u64,
            custody_in,
            token_price_in,
        )?;

        let swap_out_fee = self.get_fee(
            token_id_out,
            if stable_swap {
                custody_out.fees.stable_swap_out
            } else {
                custody_out.fees.swap_out
            },
            0u64,
            amount_out,
            custody_out,
            token_price_out,
        )?;

        Ok((swap_in_fee, swap_out_fee))
    }

    fn get_new_ratio(
        &self,
        amount_add: u64,
        amount_remove: u64,
        custody: &Custody,
        token_price: &OraclePrice,
    ) -> anyhow::Result<u16> {
        let (new_token_aum_usd, new_pool_aum_usd) = if amount_add > 0 && amount_remove > 0 {
            return Err(ProgramError::InvalidArgument.into());
        } else if amount_add == 0 && amount_remove == 0 {
            (
                token_price.get_asset_amount_usd(custody.assets.owned, custody.decimals)? as u128,
                self.aum_usd.to_u128(),
            )
        } else if amount_add > 0 {
            let added_aum_usd =
                token_price.get_asset_amount_usd(amount_add, custody.decimals)? as u128;

            (
                token_price
                    .get_asset_amount_usd(custody.assets.owned + amount_add, custody.decimals)?
                    as u128,
                self.aum_usd.to_u128() + added_aum_usd,
            )
        } else {
            let removed_aum_usd =
                token_price.get_asset_amount_usd(amount_remove, custody.decimals)? as u128;

            if removed_aum_usd >= self.aum_usd.to_u128() || amount_remove >= custody.assets.owned {
                (0, 0)
            } else {
                (
                    token_price.get_asset_amount_usd(
                        custody.assets.owned - amount_remove,
                        custody.decimals,
                    )? as u128,
                    self.aum_usd.to_u128() - removed_aum_usd,
                )
            }
        };

        if new_token_aum_usd == 0 || new_pool_aum_usd == 0 {
            return Ok(0);
        }

        let ratio = math::checked_as_u16((new_token_aum_usd * BPS_POWER) / new_pool_aum_usd)?;

        Ok(std::cmp::min(ratio, BPS_POWER as u16))
    }

    pub fn get_fee_amount(fee: u16, amount: u64) -> anyhow::Result<u64> {
        if fee == 0 || amount == 0 {
            return Ok(0);
        }

        math::checked_as_u64(math::checked_ceil_div::<u128>(
            amount as u128 * fee as u128,
            BPS_POWER,
        )?)
    }

    fn get_fee(
        &self,
        token_id: usize,
        base_fee: u16,
        amount_add: u64,
        amount_remove: u64,
        custody: &Custody,
        token_price: &OraclePrice,
    ) -> anyhow::Result<u64> {
        let fee_max: i64 = custody.fees.fee_max as i64;
        let fee_min: i64 = 0;

        let target_ratio: i64 = self.ratios[token_id].target as i64;
        let min_ratio: i64 = self.ratios[token_id].min as i64;
        let max_ratio: i64 = self.ratios[token_id].max as i64;
        let new_ratio: i64 =
            self.get_new_ratio(amount_add, amount_remove, custody, token_price)? as i64;

        let base_fee: i64 = base_fee as i64;
        let slope_denominator: i64 = if new_ratio > target_ratio {
            max_ratio - target_ratio
        } else {
            target_ratio - min_ratio
        };

        // Calculates the rate at which the fee will change.
        let slope_numerator: i64 = if amount_add != 0 {
            if new_ratio > max_ratio {
                anyhow::bail!("TokenRatioOutOfRange");
            }

            fee_max - fee_min
        } else {
            if new_ratio < min_ratio {
                anyhow::bail!("TokenRatioOutOfRange");
            }

            fee_min - fee_max
        };

        let fee_adjustment: i64 = ((slope_numerator * new_ratio) + (fee_min * slope_denominator)
            - (target_ratio * slope_numerator))
            / slope_denominator;

        let final_fee = math::checked_as_u16(std::cmp::max(fee_adjustment + base_fee, 0))?;

        Self::get_fee_amount(final_fee, std::cmp::max(amount_add, amount_remove))
    }
}
