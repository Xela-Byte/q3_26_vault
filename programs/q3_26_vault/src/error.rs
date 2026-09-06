use anchor_lang::prelude::*;

#[error_code]
pub enum ErrorCode {
    #[msg("Deposit amount must be greater than zero")]
    InvalidAmount,
    #[msg("Withdrawal would take the vault below its rent-exempt minimum")]
    InsufficientFunds,
}
