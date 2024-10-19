mod add_liquidity;
mod remove_liquidity;

pub use add_liquidity::*;
pub use remove_liquidity::*;

use rust_decimal::Decimal;

pub struct ComputeResult {
    pub in_amount: u64,
    pub out_amount: u64,
    pub fee_amount: u64,
    pub fee_pct: Decimal,
}
