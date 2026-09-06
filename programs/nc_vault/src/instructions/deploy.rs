use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

use crate::{
    error::VaultError,
    state::{Position, Vault},
    POSITION_SEED, VALUE_SCALE, VAULT_SEED,
};

/// Route liquid asset into an approved market position, and route the position
/// tokens back, in a single transaction.
///
/// Both legs settle here or neither does, so there is no moment where the vault
/// has paid out and holds nothing to show for it. The market has to sign, which
/// is what makes the second leg possible without trusting the manager to follow
/// up afterwards.
#[derive(Accounts)]
pub struct Deploy<'info> {
    pub manager: Signer<'info>,

    /// The counterparty. Fixed at registration by the governor — the `has_one`
    /// below is the constraint that stops a manager from naming itself.
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

impl<'info> Deploy<'info> {
    pub fn deploy(&mut self, asset_amount: u64, position_amount: u64) -> Result<()> {
        require!(asset_amount > 0, VaultError::InvalidAmount);
        require!(position_amount > 0, VaultError::InvalidAmount);

        let token_program = self.token_program.key();

        let governor = self.vault.governor;
        let seed_bytes = self.vault.seed.to_le_bytes();
        let signer_seeds: [&[&[u8]]; 1] = [&[
            VAULT_SEED,
            governor.as_ref(),
            &seed_bytes[..],
            &[self.vault.bump],
        ]];

        // Leg one: asset out, vault PDA signing.
        let cpi_accounts = TransferChecked {
            from: self.vault_asset_ata.to_account_info(),
            mint: self.asset_mint.to_account_info(),
            to: self.market_asset_ata.to_account_info(),
            authority: self.vault.to_account_info(),
        };
        transfer_checked(
            CpiContext::new_with_signer(token_program, cpi_accounts, &signer_seeds),
            asset_amount,
            self.asset_mint.decimals,
        )?;

        // Leg two: position tokens in, market signing.
        let cpi_accounts = TransferChecked {
            from: self.market_position_ata.to_account_info(),
            mint: self.position_mint.to_account_info(),
            to: self.vault_position_ata.to_account_info(),
            authority: self.market.to_account_info(),
        };
        transfer_checked(
            CpiContext::new(token_program, cpi_accounts),
            position_amount,
            self.position_mint.decimals,
        )?;

        self.position.amount = self
            .position
            .amount
            .checked_add(position_amount)
            .ok_or(VaultError::MathOverflow)?;

        // The mark comes from the trade that just executed, not from something
        // the manager asserts. It is still only as good as the market it was
        // struck against — see the trust notes in the README.
        self.position.value_per_unit = executed_rate(asset_amount, position_amount)?;
        self.position.last_valued = Clock::get()?.unix_timestamp;

        Ok(())
    }
}

/// Asset units per position unit, scaled by `VALUE_SCALE`.
pub fn executed_rate(asset_amount: u64, position_amount: u64) -> Result<u64> {
    let rate = (asset_amount as u128)
        .checked_mul(VALUE_SCALE)
        .ok_or(VaultError::MathOverflow)?
        / (position_amount as u128);

    u64::try_from(rate).map_err(|_| error!(VaultError::MathOverflow))
}
