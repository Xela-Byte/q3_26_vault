//! # Non-custodial share vault with in-kind redemption
//!
//! A vault that takes deposits in one asset, issues transferable shares, and
//! lets a manager route the pooled capital into market positions — without ever
//! being able to hold a depositor's exit hostage.
//!
//! The property that makes it non-custodial is narrow and worth stating exactly:
//!
//! > A share holder can burn shares and receive their pro-rata slice of
//! > everything the vault holds, at any time, with no signature but their own,
//! > and regardless of how much liquid asset the vault has on hand.
//!
//! When the vault is fully deployed there is nothing to pay a cash redemption
//! with. A custodial vault answers that with a queue, a notice period, or a
//! manager who "processes" withdrawals. This one answers it by handing over the
//! position tokens themselves — the depositor leaves with the same basket the
//! vault holds, in the same proportions. Illiquidity becomes the redeemer's
//! problem to solve on the open market, not a lever the manager holds.
//!
//! ## Instruction map
//!
//! | Instruction | Signer | What it does |
//! |---|---|---|
//! | `initialize_vault` | governor | Creates the vault, its share mint, and its asset account |
//! | `register_position` | governor | Approves a position token and the one market it may trade against |
//! | `deploy` | manager + market | Asset out, position tokens in — both legs, one transaction |
//! | `unwind` | manager + market | The reverse |
//! | `revalue` | manager | Refreshes a mark between trades (affects deposits only) |
//! | `deposit` | anyone | Asset in, shares out, priced against current NAV |
//! | `exit` | share holder | Burn shares, take a pro-rata slice of cash **and** every position |
//! | `close_position` | governor | Retires a fully-unwound position |

pub mod constants;
pub mod error;
pub mod helpers;
pub mod instructions;
pub mod state;

use anchor_lang::prelude::*;

pub use constants::*;
pub use instructions::*;
pub use state::*;

declare_id!("HVEPaNfpQksnVs9ZYYenMdAtzvJ9cshQF3reabBfv68L");

#[program]
pub mod nc_vault {
    use super::*;

    pub fn initialize_vault(
        ctx: Context<InitializeVault>,
        seed: u64,
        manager: Pubkey,
        max_valuation_age: i64,
    ) -> Result<()> {
        ctx.accounts
            .initialize_vault(seed, manager, max_valuation_age, &ctx.bumps)
    }

    pub fn register_position(ctx: Context<RegisterPosition>) -> Result<()> {
        ctx.accounts.register_position(&ctx.bumps)
    }

    pub fn deploy(ctx: Context<Deploy>, asset_amount: u64, position_amount: u64) -> Result<()> {
        ctx.accounts.deploy(asset_amount, position_amount)
    }

    pub fn unwind(ctx: Context<Unwind>, position_amount: u64, asset_amount: u64) -> Result<()> {
        ctx.accounts.unwind(position_amount, asset_amount)
    }

    pub fn revalue(ctx: Context<Revalue>, value_per_unit: u64) -> Result<()> {
        ctx.accounts.revalue(value_per_unit)
    }

    /// Remaining accounts: every `Position` of this vault, ascending by key.
    pub fn deposit<'info>(ctx: Context<'info, Deposit<'info>>, amount: u64) -> Result<()> {
        deposit_handler(ctx, amount)
    }

    /// Remaining accounts: four per position — the `Position`, its mint, the
    /// vault's token account, the redeemer's token account — positions ascending
    /// by key.
    pub fn exit<'info>(ctx: Context<'info, Exit<'info>>, shares: u64) -> Result<()> {
        exit_handler(ctx, shares)
    }

    pub fn close_position(ctx: Context<ClosePosition>) -> Result<()> {
        ctx.accounts.close_position()
    }
}
