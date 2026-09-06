use anchor_lang::prelude::*;
use anchor_spl::token_interface::TokenAccount;

use crate::{error::VaultError, state::Position, POSITION_SEED};

/// Load and validate a `Position` handed in through `remaining_accounts`.
///
/// `Account::try_from` already checks the owning program and the account
/// discriminator. What it cannot check is that this particular position belongs
/// to *this* vault, so both the stored `vault` field and the PDA derivation are
/// verified here — the field alone would be enough only if nothing else could
/// ever write it, and re-deriving the address costs one hash.
pub fn load_position<'info>(
    info: &'info AccountInfo<'info>,
    vault: &Pubkey,
) -> Result<Account<'info, Position>> {
    let position: Account<'info, Position> = Account::try_from(info)?;

    require_keys_eq!(position.vault, *vault, VaultError::ForeignPosition);

    let expected = Pubkey::create_program_address(
        &[
            POSITION_SEED,
            vault.as_ref(),
            position.position_mint.as_ref(),
            &[position.bump],
        ],
        &crate::ID,
    )
    .map_err(|_| error!(VaultError::ForeignPosition))?;

    require_keys_eq!(*info.key, expected, VaultError::ForeignPosition);

    Ok(position)
}

/// Load a token account from `remaining_accounts` and pin down both its mint and
/// its owner. Without the owner check a redeemer could name someone else's token
/// account as the destination; without the mint check they could be paid in the
/// wrong asset.
pub fn load_token_account<'info>(
    info: &'info AccountInfo<'info>,
    mint: &Pubkey,
    owner: &Pubkey,
) -> Result<InterfaceAccount<'info, TokenAccount>> {
    let account: InterfaceAccount<'info, TokenAccount> = InterfaceAccount::try_from(info)?;

    require_keys_eq!(account.mint, *mint, VaultError::WrongMint);
    require_keys_eq!(account.owner, *owner, VaultError::WrongOwner);

    Ok(account)
}

/// Enforces strictly ascending key order across a sequence of accounts.
///
/// This is how duplicates are ruled out. If a redeemer could pass the same
/// position twice, the pro-rata transfer would run twice and they would be paid
/// double. Sorting turns that into a single comparison per item instead of an
/// O(n²) scan.
pub struct AscendingKeys {
    previous: Option<Pubkey>,
}

impl AscendingKeys {
    pub fn new() -> Self {
        Self { previous: None }
    }

    pub fn push(&mut self, key: &Pubkey) -> Result<()> {
        if let Some(previous) = self.previous {
            require!(*key > previous, VaultError::PositionsOutOfOrder);
        }
        self.previous = Some(*key);
        Ok(())
    }
}

impl Default for AscendingKeys {
    fn default() -> Self {
        Self::new()
    }
}

/// `floor(amount * numerator / denominator)` in u128, back down to u64.
///
/// Every payout in this program rounds down. The remainder — at most one base
/// unit per position — stays in the vault, so rounding can only ever favour the
/// holders who have not exited yet, never the one leaving. Rounding the other
/// way would let a stream of tiny redemptions bleed the vault.
pub fn pro_rata(amount: u64, numerator: u64, denominator: u64) -> Result<u64> {
    if denominator == 0 {
        return Ok(0);
    }

    let value = (amount as u128)
        .checked_mul(numerator as u128)
        .ok_or(VaultError::MathOverflow)?
        / (denominator as u128);

    u64::try_from(value).map_err(|_| error!(VaultError::MathOverflow))
}
