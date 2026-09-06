use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

use crate::{
    error::VaultError,
    instructions::deploy::executed_rate,
    state::{Position, Vault},
    POSITION_SEED, VAULT_SEED,
};

/// The reverse of `deploy`: hand position tokens back to the market and take the
/// asset. Same two-legged, single-transaction shape, same approved counterparty.
///
/// Note that this is a convenience for the manager, not a precondition for
/// anyone's exit. Depositors never have to wait for capital to be unwound —
/// that is what in-kind redemption is for.
#[derive(Accounts)]
pub struct Unwind<'info> {
    pub manager: Signer<'info>,

    pub market: Signer<'info>,

    #[account(
        has_one = manager,
        has_one = asset_mint,
        seeds = [VAULT_SEED, vault.governor.as_ref(), vault.seed.to_le_bytes().as_ref()],
        bump = vault.bump,
    )]
    pub vault: Account<'info, Vault>,

    #[account(
        mut,
        has_one = vault,
        has_one = market,
        has_one = position_mint,
        seeds = [POSITION_SEED, vault.key().as_ref(), position_mint.key().as_ref()],
        bump = position.bump,
    )]
    pub position: Account<'info, Position>,

    pub asset_mint: InterfaceAccount<'info, Mint>,
    pub position_mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        associated_token::mint = asset_mint,
        associated_token::authority = vault,
        associated_token::token_program = token_program,
    )]
    pub vault_asset_ata: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        associated_token::mint = position_mint,
        associated_token::authority = vault,
        associated_token::token_program = token_program,
    )]
    pub vault_position_ata: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        associated_token::mint = asset_mint,
        associated_token::authority = market,
        associated_token::token_program = token_program,
    )]
    pub market_asset_ata: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        associated_token::mint = position_mint,
        associated_token::authority = market,
        associated_token::token_program = token_program,
    )]
    pub market_position_ata: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
}

impl<'info> Unwind<'info> {
    pub fn unwind(&mut self, position_amount: u64, asset_amount: u64) -> Result<()> {
        require!(position_amount > 0, VaultError::InvalidAmount);
        require!(asset_amount > 0, VaultError::InvalidAmount);
        require!(
            position_amount <= self.position.amount,
            VaultError::InsufficientPosition
        );

        let token_program = self.token_program.key();

        let governor = self.vault.governor;
        let seed_bytes = self.vault.seed.to_le_bytes();
        let signer_seeds: [&[&[u8]]; 1] = [&[
            VAULT_SEED,
            governor.as_ref(),
            &seed_bytes[..],
            &[self.vault.bump],
        ]];

        // Leg one: position tokens out, vault PDA signing.
        let cpi_accounts = TransferChecked {
            from: self.vault_position_ata.to_account_info(),
            mint: self.position_mint.to_account_info(),
            to: self.market_position_ata.to_account_info(),
            authority: self.vault.to_account_info(),
        };
        transfer_checked(
            CpiContext::new_with_signer(token_program, cpi_accounts, &signer_seeds),
            position_amount,
            self.position_mint.decimals,
        )?;

        // Leg two: asset back in, market signing.
        let cpi_accounts = TransferChecked {
            from: self.market_asset_ata.to_account_info(),
            mint: self.asset_mint.to_account_info(),
            to: self.vault_asset_ata.to_account_info(),
            authority: self.market.to_account_info(),
        };
        transfer_checked(
            CpiContext::new(token_program, cpi_accounts),
            asset_amount,
            self.asset_mint.decimals,
        )?;

        self.position.amount = self
            .position
            .amount
            .checked_sub(position_amount)
            .ok_or(VaultError::InsufficientPosition)?;

        self.position.value_per_unit = executed_rate(asset_amount, position_amount)?;
        self.position.last_valued = Clock::get()?.unix_timestamp;

        Ok(())
    }
}
