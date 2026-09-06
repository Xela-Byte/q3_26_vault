use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token_interface::{
        mint_to, transfer_checked, Mint, MintTo, TokenAccount, TokenInterface, TransferChecked,
    },
};

use crate::{
    error::VaultError,
    helpers::{load_position, AscendingKeys},
    state::Vault,
    VAULT_SEED,
};

/// Deposit the vault's asset and receive shares.
///
/// Pricing a deposit is the one operation that has to know what the deployed
/// capital is worth, so this is the only instruction that reads marks — and the
/// only one that can be blocked by a stale one. That asymmetry is deliberate:
/// entry can wait for a fresh price, exit never should.
#[derive(Accounts)]
pub struct Deposit<'info> {
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
        associated_token::mint = asset_mint,
        associated_token::authority = user,
        associated_token::token_program = token_program,
    )]
    pub user_asset_ata: InterfaceAccount<'info, TokenAccount>,

    #[account(
        init_if_needed,
        payer = user,
        associated_token::mint = share_mint,
        associated_token::authority = user,
        associated_token::token_program = token_program,
    )]
    pub user_share_ata: InterfaceAccount<'info, TokenAccount>,

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

/// `remaining_accounts`: every `Position` of this vault, in ascending key order.
pub fn deposit_handler<'info>(ctx: Context<'info, Deposit<'info>>, amount: u64) -> Result<()> {
    require!(amount > 0, VaultError::InvalidAmount);

    let vault_key = ctx.accounts.vault.key();
    let position_count = ctx.accounts.vault.position_count as usize;
    let max_age = ctx.accounts.vault.max_valuation_age;
    let now = Clock::get()?.unix_timestamp;

    // Requiring *all* positions means a depositor can never be priced against a
    // partial view of the book, which would systematically undervalue the vault
    // and over-issue shares.
    require_eq!(
        ctx.remaining_accounts.len(),
        position_count,
        VaultError::PositionCountMismatch
    );

    let mut order = AscendingKeys::new();
    let mut deployed: u128 = 0;

    for info in ctx.remaining_accounts {
        order.push(info.key)?;

        let position = load_position(info, &vault_key)?;

        require!(
            now.saturating_sub(position.last_valued) <= max_age,
            VaultError::StaleValuation
        );

        deployed = deployed
            .checked_add(position.value())
            .ok_or(VaultError::MathOverflow)?;
    }

    // Balances are read before the transfer below, so this is the pre-deposit NAV
    // — which is what the incoming money has to be priced against.
    let liquid = ctx.accounts.vault_asset_ata.amount as u128;
    let nav = liquid
        .checked_add(deployed)
        .ok_or(VaultError::MathOverflow)?;

    let supply = ctx.accounts.share_mint.supply;

    let shares: u64 = if supply == 0 {
        // First deposit sets the scale: one share per asset unit.
        amount
    } else {
        require!(nav > 0, VaultError::VaultInsolvent);

        let value = (amount as u128)
            .checked_mul(supply as u128)
            .ok_or(VaultError::MathOverflow)?
            / nav;

        u64::try_from(value).map_err(|_| error!(VaultError::MathOverflow))?
    };

    // Rounding down means a dust deposit can round to nothing. Failing is better
    // than silently taking the money and issuing zero shares.
    require!(shares > 0, VaultError::DepositTooSmall);

    let cpi_accounts = TransferChecked {
        from: ctx.accounts.user_asset_ata.to_account_info(),
        mint: ctx.accounts.asset_mint.to_account_info(),
        to: ctx.accounts.vault_asset_ata.to_account_info(),
        authority: ctx.accounts.user.to_account_info(),
    };
    transfer_checked(
        CpiContext::new(ctx.accounts.token_program.key(), cpi_accounts),
        amount,
        ctx.accounts.asset_mint.decimals,
    )?;

    let governor = ctx.accounts.vault.governor;
    let seed_bytes = ctx.accounts.vault.seed.to_le_bytes();
    let signer_seeds: [&[&[u8]]; 1] = [&[
        VAULT_SEED,
        governor.as_ref(),
        &seed_bytes[..],
        &[ctx.accounts.vault.bump],
    ]];

    let cpi_accounts = MintTo {
        mint: ctx.accounts.share_mint.to_account_info(),
        to: ctx.accounts.user_share_ata.to_account_info(),
        authority: ctx.accounts.vault.to_account_info(),
    };
    mint_to(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            cpi_accounts,
            &signer_seeds,
        ),
        shares,
    )
}
