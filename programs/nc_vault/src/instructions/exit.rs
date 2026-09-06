use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token_interface::{
        burn, transfer_checked, Burn, Mint, TokenAccount, TokenInterface, TransferChecked,
    },
};

use crate::{
    error::VaultError,
    helpers::{load_position, load_token_account, pro_rata, AscendingKeys},
    state::Vault,
    VAULT_SEED,
};

/// Burn shares and walk away with a pro-rata slice of everything the vault
/// holds — the liquid asset *and* every deployed position, in kind.
///
/// This is the instruction the whole design exists for, so note what it does
/// **not** do:
///
/// * it reads no marks, so a stale, missing, or lying valuation cannot affect it;
/// * it requires no signature but the holder's own — no manager, no governor,
///   no queue, no notice period;
/// * it has no liquidity precondition. If the vault's cash balance is zero, the
///   asset leg simply pays zero and the position legs pay everything. A vault
///   that is fully deployed is still fully exitable.
///
/// The cost of that is the account budget: a redeemer must pass all of the
/// vault's positions in one transaction, which caps a vault at roughly a dozen
/// positions. See the README for the paginated-receipt design that lifts the cap
/// and what it gives up.
#[derive(Accounts)]
pub struct Exit<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        has_one = asset_mint,
        has_one = share_mint,
        seeds = [VAULT_SEED, vault.governor.as_ref(), vault.seed.to_le_bytes().as_ref()],
        bump = vault.bump,
    )]
    pub vault: Account<'info, Vault>,

    pub asset_mint: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub share_mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        associated_token::mint = share_mint,
        associated_token::authority = user,
        associated_token::token_program = token_program,
    )]
    pub user_share_ata: InterfaceAccount<'info, TokenAccount>,

    #[account(
        init_if_needed,
        payer = user,
        associated_token::mint = asset_mint,
        associated_token::authority = user,
        associated_token::token_program = token_program,
    )]
    pub user_asset_ata: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        associated_token::mint = asset_mint,
        associated_token::authority = vault,
        associated_token::token_program = token_program,
    )]
    pub vault_asset_ata: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

/// `remaining_accounts`: four per position, positions in ascending key order.
///
/// 0. the `Position` record (writable)
/// 1. the position mint
/// 2. the vault's token account for that mint (writable)
/// 3. the redeemer's token account for that mint (writable)
pub fn exit_handler<'info>(ctx: Context<'info, Exit<'info>>, shares: u64) -> Result<()> {
    require!(shares > 0, VaultError::InvalidAmount);

    let vault_key = ctx.accounts.vault.key();
    let user_key = ctx.accounts.user.key();
    let position_count = ctx.accounts.vault.position_count as usize;
    let token_program_key = ctx.accounts.token_program.key();

    require_eq!(
        ctx.remaining_accounts.len(),
        position_count
            .checked_mul(4)
            .ok_or(VaultError::MathOverflow)?,
        VaultError::PositionCountMismatch
    );

    // Captured before the burn. Every payout below is a fraction of this number,
    // so it has to be the supply the redeemer's shares were a slice of.
    let supply = ctx.accounts.share_mint.supply;
    require!(
        shares <= supply && shares <= ctx.accounts.user_share_ata.amount,
        VaultError::InsufficientShares
    );

    let asset_out = pro_rata(ctx.accounts.vault_asset_ata.amount, shares, supply)?;

    let governor = ctx.accounts.vault.governor;
    let seed_bytes = ctx.accounts.vault.seed.to_le_bytes();
    let vault_bump = ctx.accounts.vault.bump;
    let signer_seeds: [&[&[u8]]; 1] = [&[
        VAULT_SEED,
        governor.as_ref(),
        &seed_bytes[..],
        &[vault_bump],
    ]];

    // Burn first. The shares are gone whatever happens next, and if any transfer
    // below fails the whole transaction unwinds anyway.
    let cpi_accounts = Burn {
        mint: ctx.accounts.share_mint.to_account_info(),
        from: ctx.accounts.user_share_ata.to_account_info(),
        authority: ctx.accounts.user.to_account_info(),
    };
    burn(
        CpiContext::new(token_program_key, cpi_accounts),
        shares,
    )?;

    // ---- the liquid leg (may legitimately be zero) ----
    if asset_out > 0 {
        let cpi_accounts = TransferChecked {
            from: ctx.accounts.vault_asset_ata.to_account_info(),
            mint: ctx.accounts.asset_mint.to_account_info(),
            to: ctx.accounts.user_asset_ata.to_account_info(),
            authority: ctx.accounts.vault.to_account_info(),
        };
        transfer_checked(
            CpiContext::new_with_signer(token_program_key, cpi_accounts, &signer_seeds),
            asset_out,
            ctx.accounts.asset_mint.decimals,
        )?;
    }

    // ---- the in-kind legs ----
    let mut order = AscendingKeys::new();

    for chunk in ctx.remaining_accounts.chunks(4) {
        let position_info = &chunk[0];
        let mint_info = &chunk[1];
        let vault_ata_info = &chunk[2];
        let user_ata_info = &chunk[3];

        order.push(position_info.key)?;

        let mut position = load_position(position_info, &vault_key)?;

        require_keys_eq!(
            position.position_mint,
            *mint_info.key,
            VaultError::WrongMint
        );

        let mint: InterfaceAccount<Mint> = InterfaceAccount::try_from(mint_info)?;

        // A position token could belong to a different token program than the
        // vault's asset. Checking here turns a confusing CPI failure into a clear
        // one.
        require_keys_eq!(*mint_info.owner, token_program_key, VaultError::WrongMint);

        load_token_account(vault_ata_info, mint_info.key, &vault_key)?;
        load_token_account(user_ata_info, mint_info.key, &user_key)?;

        let position_out = pro_rata(position.amount, shares, supply)?;

        if position_out > 0 {
            let cpi_accounts = TransferChecked {
                from: vault_ata_info.clone(),
                mint: mint_info.clone(),
                to: user_ata_info.clone(),
                authority: ctx.accounts.vault.to_account_info(),
            };
            transfer_checked(
                CpiContext::new_with_signer(token_program_key, cpi_accounts, &signer_seeds),
                position_out,
                mint.decimals,
            )?;

            position.amount = position
                .amount
                .checked_sub(position_out)
                .ok_or(VaultError::InsufficientPosition)?;

            // `remaining_accounts` are not part of the Accounts struct, so Anchor
            // will not serialize this one back for us at the end of the
            // instruction. Write it explicitly.
            position.exit(&crate::ID)?;
        }
    }

    Ok(())
}
