use anchor_lang::prelude::*;

#[error_code]
pub enum VaultError {
    #[msg("Amount must be greater than zero")]
    InvalidAmount,
    #[msg("The number of position accounts passed does not match the vault's position count")]
    PositionCountMismatch,
    #[msg("Position accounts must be passed in strictly ascending key order")]
    PositionsOutOfOrder,
    #[msg("A position account does not belong to this vault")]
    ForeignPosition,
    #[msg("A position valuation is older than the vault's maximum age")]
    StaleValuation,
    #[msg("Vault holds shares but reports zero value; deposits are frozen")]
    VaultInsolvent,
    #[msg("Deposit is too small to mint a whole share at the current price")]
    DepositTooSmall,
    #[msg("Not enough shares")]
    InsufficientShares,
    #[msg("The vault does not hold enough of this position")]
    InsufficientPosition,
    #[msg("Token account has the wrong mint")]
    WrongMint,
    #[msg("Token account has the wrong owner")]
    WrongOwner,
    #[msg("Position must be fully unwound before it can be closed")]
    PositionNotEmpty,
    #[msg("Arithmetic overflow")]
    MathOverflow,
}
