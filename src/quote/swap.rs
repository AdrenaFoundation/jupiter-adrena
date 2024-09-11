use adrena::accounts::Swap;
use anchor_lang::{prelude::AccountMeta, ToAccountMetas};
use anyhow::anyhow;
use jupiter_amm_interface::{QuoteParams, SwapParams};

use crate::{
    CalculateFeesParams, PoolAmm, FEE_REDISTRIBUTION_MINT, LM_STAKING, PROTOCOL_FEE_RECIPIENT,
    REWARD_ORACLE_ACCOUNT, SPL_TOKEN_ID,
};

use super::ComputeResult;

pub fn calculate_swap(amm: &PoolAmm, params: &QuoteParams) -> anyhow::Result<ComputeResult> {
    let in_amount = params.amount;

    let (custody_in_pubkey, custody_in, token_price_in) =
        amm.get_custody_and_oracle(params.input_mint)?;
    let (custody_out_pubkey, custody_out, token_price_out) =
        amm.get_custody_and_oracle(params.output_mint)?;

    let token_id_in = amm.pool.get_token_id(&custody_in_pubkey)?;
    let token_id_out = amm.pool.get_token_id(&custody_out_pubkey)?;

    let out_amount = amm.pool.get_swap_amount(
        token_price_in,
        token_price_out,
        custody_in,
        custody_out,
        in_amount,
    )?;

    let fees = {
        let swap_fees_in = amm.pool.get_swap_in_fees(
            token_id_in,
            in_amount,
            custody_in,
            token_price_in,
            custody_out,
        )?;

        let swap_fees_out = amm.pool.get_swap_out_fees(
            token_id_out,
            out_amount,
            custody_in,
            custody_out,
            token_price_out,
        )?;

        (swap_fees_in, swap_fees_out)
    };

    let real_out_amount = out_amount - fees.1;

    let (fee_amount, fee_pct) = amm.calculate_swap_fees(CalculateFeesParams {
        fees,
        in_decimals: custody_in.decimals,
        in_oracle: token_price_in,
        in_amount,
        out_amount,
        out_decimals: custody_out.decimals,
        out_oracle: token_price_out,
    })?;

    Ok(ComputeResult {
        in_amount,
        fee_amount,
        fee_pct,
        out_amount: real_out_amount,
    })
}

pub fn get_swap_metas(amm: &PoolAmm, params: &SwapParams) -> anyhow::Result<Vec<AccountMeta>> {
    let (dispensing_custody, dispensing_custody_state) = amm
        .custodies
        .iter()
        .find(|c| c.1.mint == params.source_mint)
        .ok_or(anyhow!(
            "Can't find the custody for the mint {}",
            params.source_mint
        ))?;
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
    let user_profile = amm.pda(&[b"user_profile", params.token_transfer_authority.as_ref()]);
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

    Ok(Swap {
        owner: params.token_transfer_authority,
        funding_account: params.source_token_account,
        receiving_account: params.destination_token_account,
        transfer_authority: params.token_transfer_authority,
        cortex,
        lm_staking: LM_STAKING,
        lp_staking,
        pool: amm.pool_key,
        staking_reward_token_custody,
        staking_reward_token_custody_oracle_account: REWARD_ORACLE_ACCOUNT,
        staking_reward_token_custody_token_account,
        receiving_custody: *receiving_custody,
        receiving_custody_oracle_account: receiving_custody_state.oracle.oracle_account,
        receiving_custody_token_account: receiving_custody_state.token_account,
        dispensing_custody: *dispensing_custody,
        dispensing_custody_oracle_account: dispensing_custody_state.oracle.oracle_account,
        dispensing_custody_token_account: dispensing_custody_state.token_account,
        lm_staking_reward_token_vault,
        lp_staking_reward_token_vault,
        lp_token_mint,
        protocol_fee_recipient: PROTOCOL_FEE_RECIPIENT,
        user_profile: Some(user_profile),
        token_program: SPL_TOKEN_ID,
        adrena_program: amm.program_id,
    }
    .to_account_metas(None))
}
