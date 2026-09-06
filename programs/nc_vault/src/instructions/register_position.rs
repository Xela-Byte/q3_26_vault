use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{
    error::VaultError,
    state::{Position, Vault},
    POSITION_SEED, VAULT_SEED,
};

/// Governor-only. Approves one position token and the single market the manager
/// may trade it against.
///
/// This is the whole reason the manager cannot simply route the vault's assets
/// to an address it controls: `deploy` and `unwind` check `position.market`, and
/// only the governor writes that field.
#[derive(Accounts)]
pub struct RegisterPosition<'info> {
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

    /// CHECK: an opaque counterparty address. It is recorded, not read — the only
    /// thing that matters is that the manager cannot later substitute another one.
    pub market: UncheckedAccount<'info>,

    #[account(
        init,
        payer = governor,
        seeds = [POSITION_SEED, vault.key().as_ref(), position_mint.key().as_ref()],
        bump,
        space = Position::DISCRIMINATOR.len() + Position::INIT_SPACE
    )]
    pub position: Account<'info, Position>,

    /// Created here so that `deploy` never has to create it, and so the vault's
    /// custody of the position token exists before any capital moves.
    #[account(
        init,
        payer = governor,
        associated_token::mint = position_mint,
        associated_token::authority = vault,
        associated_token::token_program = token_program,
    )]
    pub vault_position_ata: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

impl<'info> RegisterPosition<'info> {
    pub fn register_position(&mut self, bumps: &RegisterPositionBumps) -> Result<()> {
        self.position.set_inner(Position {
            vault: self.vault.key(),
            position_mint: self.position_mint.key(),
            market: self.market.key(),
            amount: 0,
            // An empty position is worth nothing, and `value()` multiplies by
            // `amount`, so a zero mark here is both true and harmless.
            value_per_unit: 0,
            last_valued: Clock::get()?.unix_timestamp,
            bump: bumps.position,
        });

        self.vault.position_count = self
            .vault
            .position_count
            .checked_add(1)
            .ok_or(VaultError::MathOverflow)?;

        Ok(())
    }
}
