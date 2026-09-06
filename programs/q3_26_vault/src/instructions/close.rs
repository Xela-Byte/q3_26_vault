use crate::{
    constants::{STATE, VAULT_SEED},
    state::VaultState,
};
use anchor_lang::{
    prelude::*,
    system_program::{transfer, Transfer},
};

#[derive(Accounts)]
pub struct Close<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    /// `close = user` returns the state account's rent to the owner and zeroes the
    /// account so it cannot be replayed. Anchor runs this *after* the instruction
    /// body, so the body can still read `vault_state.vault_bump`.
    #[account(
        mut,
        close = user,
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

impl<'info> Close<'info> {
    pub fn close(&mut self) -> Result<()> {
        // Sweep the whole balance, rent-exempt reserve included. Once the vault
        // hits zero lamports the runtime reaps it, which is what we want here —
        // the state account is going away in the same transaction.
        let amount = self.vault.lamports();

        if amount == 0 {
            return Ok(());
        }

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
