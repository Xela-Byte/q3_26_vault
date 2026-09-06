use anchor_lang::prelude::*;
use anchor_spl::token_interface::Mint;

use crate::{
    error::VaultError,
    state::{Position, Vault},
    POSITION_SEED, VAULT_SEED,
};

/// Refresh a position's mark between trades.
///
/// Marks drift; `deploy` only sets one when capital actually moves. Without this
/// the vault's last executed price would go stale and deposits would stop, which
/// is safe but useless. What a bad mark can do is misprice a *deposit* — it can
/// never touch a redemption, because `exit` does not read this field.
#[derive(Accounts)]
pub struct Revalue<'info> {
    pub manager: Signer<'info>,

    #[account(
        has_one = manager,
        seeds = [VAULT_SEED, vault.governor.as_ref(), vault.seed.to_le_bytes().as_ref()],
        bump = vault.bump,
    )]
    pub vault: Account<'info, Vault>,

    pub position_mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        has_one = vault,
        has_one = position_mint,
        seeds = [POSITION_SEED, vault.key().as_ref(), position_mint.key().as_ref()],
        bump = position.bump,
    )]
    pub position: Account<'info, Position>,
}

impl<'info> Revalue<'info> {
    pub fn revalue(&mut self, value_per_unit: u64) -> Result<()> {
        require!(value_per_unit > 0, VaultError::InvalidAmount);

        self.position.value_per_unit = value_per_unit;
        self.position.last_valued = Clock::get()?.unix_timestamp;

        Ok(())
    }
}
