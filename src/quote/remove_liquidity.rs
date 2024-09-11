use adrena::{accounts::RemoveLiquidity, math};
use anchor_lang::{prelude::AccountMeta, ToAccountMetas};
use anyhow::{anyhow, Context};
use jupiter_amm_interface::{QuoteParams, SwapParams};
use num_traits::FromPrimitive;
use rust_decimal::Decimal;

use crate::{
    PoolAmm, FEE_REDISTRIBUTION_MINT, LM_STAKING, PROTOCOL_FEE_RECIPIENT, REWARD_ORACLE_ACCOUNT,
    SPL_TOKEN_ID,
};

use super::ComputeResult;

pub fn calculate_remove_liquidity(
    amm: &PoolAmm,
    params: &QuoteParams,
) -> anyhow::Result<ComputeResult> {
    let in_amount = params.amount;

    let lp_token_mint = amm
        .lp_token_mint
        .1
        .context("The mint of lp_token is not found.")?;

    let (custody_pubkey, custody, token_price) = amm.get_custody_and_oracle(params.output_mint)?;

    let token_id = amm.pool.get_token_id(&custody_pubkey)?;

    let remove_amount_usd = math::checked_as_u64(
        (amm.pool.aum_usd.to_u128() * in_amount as u128) / lp_token_mint.supply as u128,
    )?;

    let remove_amount = token_price.get_token_amount(remove_amount_usd, custody.decimals)?;
    let fee_amount =
        amm.pool
            .get_remove_liquidity_fee(token_id, remove_amount, custody, &token_price)?;
    let out_amount = remove_amount - fee_amount;

    let fee_amount_usd = token_price.get_asset_amount_usd(fee_amount, custody.decimals)?;

    let total_amount_dec = Decimal::from_u64(remove_amount).context("Can't convert out_amount")?;
    let total_fees_dec = Decimal::from_u64(fee_amount_usd).context("Can't convert out_amount")?;

    let fee_pct = Decimal::ONE_HUNDRED
        .checked_mul(total_fees_dec)
        .and_then(|per| per.checked_div(total_amount_dec))
        .context("Can't calculate fee percentage")?;

    Ok(ComputeResult {
        in_amount,
        out_amount,
        fee_amount,
        fee_pct,
    })
}

pub fn get_remove_liquidity_metas(
    amm: &PoolAmm,
    params: &SwapParams,
) -> anyhow::Result<Vec<AccountMeta>> {
    let (receiving_custody, receiving_custody_state) = amm
        .custodies
        .iter()
        .find(|c| c.1.mint == params.destination_mint)
        .ok_or(anyhow!(
            "Can't find the custody for the mint {}",
            params.destination_mint
        ))?;

    let lp_token_mint = amm.lp_token_mint.0;
    let lp_staking = amm.pda(&[b"staking", lp_token_mint.as_ref()]);
    let cortex = amm.pda(&[b"cortex"]);

    let lm_staking_reward_token_vault =
        amm.pda(&[b"staking_reward_token_vault", LM_STAKING.as_ref()]);
    let lp_staking_reward_token_vault =
        amm.pda(&[b"staking_reward_token_vault", lp_staking.as_ref()]);
    let staking_reward_token_custody = amm.pda(&[
        b"custody",
        amm.pool_key.as_ref(),
        FEE_REDISTRIBUTION_MINT.as_ref(),
    ]);
    let staking_reward_token_custody_token_account = amm.pda(&[
        b"custody_token_account",
        amm.pool_key.as_ref(),
        FEE_REDISTRIBUTION_MINT.as_ref(),
    ]);

    Ok(RemoveLiquidity {
        owner: params.token_transfer_authority,
        lp_token_account: params.source_token_account,
        transfer_authority: params.token_transfer_authority,
        lm_staking: LM_STAKING,
        lp_staking,
        cortex,
        pool: amm.pool_key,
        staking_reward_token_custody,
        staking_reward_token_custody_oracle_account: REWARD_ORACLE_ACCOUNT,
        staking_reward_token_custody_token_account,
        custody: *receiving_custody,
        custody_oracle_account: receiving_custody_state.oracle.oracle_account,
        custody_token_account: params.destination_token_account,
        lm_staking_reward_token_vault,
        lp_staking_reward_token_vault,
        lp_token_mint,
        protocol_fee_recipient: PROTOCOL_FEE_RECIPIENT,
        token_program: SPL_TOKEN_ID,
        adrena_program: amm.program_id,
        receiving_account: params.destination_token_account,
    }
    .to_account_metas(None))
}
