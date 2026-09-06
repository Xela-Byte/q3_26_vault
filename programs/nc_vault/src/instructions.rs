pub mod close_position;
pub mod deploy;
pub mod deposit;
pub mod exit;
pub mod initialize_vault;
pub mod register_position;
pub mod revalue;
pub mod unwind;

pub use close_position::*;
pub use deploy::*;
pub use initialize_vault::*;
pub use register_position::*;
pub use revalue::*;
pub use unwind::*;

// `deposit` and `exit` each expose a free handler function rather than an `impl`
// block, because both read `ctx.remaining_accounts`, which lives on the
// `Context` and not on the accounts struct. The handlers carry distinct names so
// these globs — which `#[program]` relies on to find the generated client
// account modules — cannot collide.
pub use deposit::*;
pub use exit::*;
