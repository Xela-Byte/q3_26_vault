use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{state::Vault, SHARE_SEED, VAULT_SEED};

#[derive(Accounts)]
#[instruction(seed: u64)]
pub struct InitializeVault<'info> {
    #[account(mut)]
    pub governor: Signer<'info>,

    pub asset_mint: InterfaceAccount<'info, Mint>,

    #[account(
        init,
        payer = governor,
        seeds = [VAULT_SEED, governor.key().as_ref(), seed.to_le_bytes().as_ref()],
        bump,
        space = Vault::DISCRIMINATOR.len() + Vault::INIT_SPACE
    )]
    pub vault: Account<'info, Vault>,

    /// The share mint's authority is the vault PDA, so shares can only come into
    /// existence through `deposit` and can only leave through `exit`. No key
    /// anywhere can mint one.
    #[account(
        init,
        payer = governor,
        seeds = [SHARE_SEED, vault.key().as_ref()],
        bump,
        mint::decimals = asset_mint.decimals,
        mint::authority = vault,
        mint::token_program = token_program,
    )]
    pub share_mint: InterfaceAccount<'info, Mint>,

    #[account(
        init,
        payer = governor,
        associated_token::mint = asset_mint,
        associated_token::authority = vault,
        associated_token::token_program = token_program,
    )]
    pub vault_asset_ata: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

impl<'info> InitializeVault<'info> {
    pub fn initialize_vault(
        &mut self,
        seed: u64,
        manager: Pubkey,
        max_valuation_age: i64,
        bumps: &InitializeVaultBumps,
    ) -> Result<()> {
        self.vault.set_inner(Vault {
            seed,
            governor: self.governor.key(),
            manager,
            asset_mint: self.asset_mint.key(),
            share_mint: self.share_mint.key(),
            position_count: 0,
            max_valuation_age,
            bump: bumps.vault,
            share_mint_bump: bumps.share_mint,
        });
        Ok(())
    }
}
