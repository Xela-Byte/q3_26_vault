use anchor_lang::prelude::*;

#[constant]
pub const VAULT_SEED: &[u8] = b"nc_vault";

#[constant]
pub const SHARE_SEED: &[u8] = b"share";

#[constant]
pub const POSITION_SEED: &[u8] = b"position";

/// Fixed-point scale for `Position::value_per_unit`.
///
/// A mark of `1_500_000` with `VALUE_SCALE == 1_000_000` means one unit of the
/// position token is currently worth 1.5 units of the vault's asset. Integer
/// math only — there are no floats on-chain.
#[constant]
pub const VALUE_SCALE: u128 = 1_000_000;
