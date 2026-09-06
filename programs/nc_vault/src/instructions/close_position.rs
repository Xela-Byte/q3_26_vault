use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    close_account, CloseAccount, Mint, TokenAccount, TokenInterface,
};

use crate::{
    error::VaultError,
    state::{Position, Vault},
    POSITION_SEED, VAULT_SEED,
};

/// Retire a fully-unwound position and reclaim its rent.
///
/// This matters for more than tidiness: `position_count` is what `deposit` and
/// `exit` check their account lists against, so every dead position a vault
/// carries is one more account every redeemer has to pass. Governor-only,
/// because removing a venue is an approval decision, not an operational one.
#[derive(Accounts)]
pub struct ClosePosition<'info> {
    #[account(mut)]
    pub governor: Signer<'info>,

    #[account(
        mut,
        has_one = governor,
        seeds = [VAULT_SEED, governor.key().as_ref(), vault.seed.to_le_bytes().as_ref()],
        bump = vault.bump,
    )]
    pub vault: Account<'info, Vault>,

    pub position_mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        close = governor,
        has_one = vault,
        has_one = position_mint,
        seeds = [POSITION_SEED, vault.key().as_ref(), position_mint.key().as_ref()],
        bump = position.bump,
    )]
    pub position: Account<'info, Position>,

    #[account(
        mut,
        associated_token::mint = position_mint,
        associated_token::authority = vault,
        associated_token::token_program = token_program,
    )]
    pub vault_position_ata: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
}

impl<'info> ClosePosition<'info> {
    pub fn close_position(&mut self) -> Result<()> {
        // Closing a position with a balance still in it would strand tokens that
        // depositors have a claim on.
        require!(self.position.amount == 0, VaultError::PositionNotEmpty);
        require!(
            self.vault_position_ata.amount == 0,
            VaultError::PositionNotEmpty
        );

        let governor = self.vault.governor;
        let seed_bytes = self.vault.seed.to_le_bytes();
        let signer_seeds: [&[&[u8]]; 1] = [&[
            VAULT_SEED,
            governor.as_ref(),
            &seed_bytes[..],
            &[self.vault.bump],
        ]];

        let cpi_accounts = CloseAccount {
            account: self.vault_position_ata.to_account_info(),
            destination: self.governor.to_account_info(),
            authority: self.vault.to_account_info(),
        };
        close_account(CpiContext::new_with_signer(
            self.token_program.key(),
            cpi_accounts,
            &signer_seeds,
        ))?;

        self.vault.position_count = self
            .vault
            .position_count
            .checked_sub(1)
            .ok_or(VaultError::MathOverflow)?;

        Ok(())
    }
}
