use anchor_lang::prelude::*;

/// One vault: a single asset, a share mint, and a set of market positions the
/// deposited asset has been routed into.
///
/// Two roles, deliberately split:
///
/// * `governor` decides *where* capital is allowed to go — it registers the
///   position mints and the market each one may trade against.
/// * `manager` decides *when and how much* to move, but only through venues the
///   governor already approved, and never to an address of its own choosing.
///
/// Neither role can touch a depositor's exit. `exit` reads no marks, needs no
/// signature but the holder's own, and cannot be paused.
#[account]
#[derive(InitSpace)]
pub struct Vault {
    pub seed: u64,
    pub governor: Pubkey,
    pub manager: Pubkey,
    pub asset_mint: Pubkey,
    pub share_mint: Pubkey,
    /// How many `Position` records exist. `deposit` and `exit` both require the
    /// caller to pass exactly this many, so neither can be run against a partial
    /// view of what the vault holds.
    pub position_count: u32,
    /// Deposits are refused if any mark is older than this many seconds. A silent
    /// manager therefore freezes *deposits* — never exits.
    pub max_valuation_age: i64,
    pub bump: u8,
    pub share_mint_bump: u8,
}

/// One market position held by the vault, denominated in a position token: an LP
/// token, a liquid-staking receipt, a vault share from another protocol —
/// anything transferable that represents the deployed capital.
#[account]
#[derive(InitSpace)]
pub struct Position {
    pub vault: Pubkey,
    pub position_mint: Pubkey,
    /// The only counterparty `deploy` and `unwind` may trade with. Set by the
    /// governor at registration and never editable by the manager.
    pub market: Pubkey,
    /// Position-token units the vault currently holds. This is the number `exit`
    /// pays out against, and it is the only field redemption reads.
    pub amount: u64,
    /// Asset units per position unit, scaled by `VALUE_SCALE`. Used for pricing
    /// *deposits* only.
    pub value_per_unit: u64,
    pub last_valued: i64,
    pub bump: u8,
}

impl Position {
    /// Mark-to-market value of this position in asset units.
    pub fn value(&self) -> u128 {
        (self.amount as u128)
            .saturating_mul(self.value_per_unit as u128)
            / crate::VALUE_SCALE
    }
}
