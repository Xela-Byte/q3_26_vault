use crate::{
    constants::{STATE, VAULT_SEED},
    error::ErrorCode,
    state::VaultState,
};
use anchor_lang::{
    prelude::*,
    system_program::{transfer, Transfer},
};

#[derive(Accounts)]
pub struct Withdraw<'info> {
    /// The vault owner. Only this signer can pull lamports back out, because both
    /// PDAs below are derived from their key — a different signer derives a
    /// different pair of addresses and the seeds constraint fails.
    #[account(mut)]
    pub user: Signer<'info>,

    /// Bumps are read from state rather than recomputed, so the runtime does a
    /// single `create_program_address` check instead of a full bump search.
    #[account(
        seeds = [STATE, user.key().as_ref()],
        bump = vault_state.state_bump
    )]
    pub vault_state: Account<'info, VaultState>,

    #[account(
        mut,
        seeds = [VAULT_SEED, user.key().as_ref()],
        bump = vault_state.vault_bump
    )]
    pub vault: SystemAccount<'info>,

    pub system_program: Program<'info, System>,
}

impl<'info> Withdraw<'info> {
    pub fn withdraw(&mut self, amount: u64) -> Result<()> {
        require!(amount > 0, ErrorCode::InvalidAmount);

        // The vault is a system account with no data, so its rent-exempt floor is
        // the minimum balance for a zero-length account. Dropping below it would
        // let the runtime reap the account, which would strand the vault_state
        // record pointing at an address that no longer exists.
        let rent_exempt = Rent::get()?.minimum_balance(self.vault.data_len());
        let available = self.vault.lamports().saturating_sub(rent_exempt);

        require!(amount <= available, ErrorCode::InsufficientFunds);

        // The vault is a PDA, so the program signs on its behalf. Seeds must match
        // the derivation in the accounts struct exactly, bump included.
        let user_key = self.user.key();
        let signer_seeds: [&[&[u8]]; 1] =
            [&[VAULT_SEED, user_key.as_ref(), &[self.vault_state.vault_bump]]];

        let cpi_program = self.system_program.key();

        let cpi_accounts = Transfer {
            from: self.vault.to_account_info(),
            to: self.user.to_account_info(),
        };

        let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, &signer_seeds);

        transfer(cpi_ctx, amount)
    }
}
