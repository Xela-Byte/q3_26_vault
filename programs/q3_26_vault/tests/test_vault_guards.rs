//! Negative tests: every guard in the vault program should actually bite.

use {
    anchor_lang::{
        prelude::Pubkey,
        solana_program::{instruction::Instruction, system_program},
        InstructionData, ToAccountMetas,
    },
    litesvm::{types::FailedTransactionMetadata, LiteSVM},
    q3_26_vault::{STATE, VAULT_SEED},
    solana_keypair::Keypair,
    solana_message::{Message, VersionedMessage},
    solana_signer::Signer,
    solana_transaction::versioned::VersionedTransaction,
};

const DEPOSIT_LAMPORTS: u64 = 500_000_000;

struct Ctx {
    svm: LiteSVM,
    program_id: Pubkey,
    user: Keypair,
    vault_state: Pubkey,
    vault: Pubkey,
}

fn submit(
    svm: &mut LiteSVM,
    payer: &Keypair,
    ix: Instruction,
) -> Result<(), FailedTransactionMetadata> {
    let blockhash = svm.latest_blockhash();
    let msg = Message::new_with_blockhash(&[ix], Some(&payer.pubkey()), &blockhash);
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[payer]).unwrap();
    svm.send_transaction(tx).map(|_| ())
}

fn expect_ok(svm: &mut LiteSVM, payer: &Keypair, ix: Instruction) {
    submit(svm, payer, ix).unwrap_or_else(|err| {
        panic!("tx should have succeeded: {err:?}\nlogs: {:#?}", err.meta.logs);
    });
}

/// Asserts the transaction failed and that `needle` shows up in the program logs.
/// Matching on the log text keeps the assertion readable and survives Anchor
/// renumbering its custom error codes.
fn expect_failure(svm: &mut LiteSVM, payer: &Keypair, ix: Instruction, needle: &str) {
    match submit(svm, payer, ix) {
        Ok(()) => panic!("expected failure containing {needle:?}, but the tx succeeded"),
        Err(err) => {
            let logs = err.meta.logs.join("\n");
            assert!(
                logs.contains(needle),
                "expected logs to contain {needle:?}\nerror: {:?}\nlogs:\n{logs}",
                err.err
            );
        }
    }
}

/// Fresh SVM with the program loaded and one initialized, funded vault.
fn setup() -> Ctx {
    let program_id = q3_26_vault::id();
    let user = Keypair::new();
    let (vault_state, _) =
        Pubkey::find_program_address(&[STATE, user.pubkey().as_ref()], &program_id);
    let (vault, _) =
        Pubkey::find_program_address(&[VAULT_SEED, user.pubkey().as_ref()], &program_id);

    let mut svm = LiteSVM::new();
    let bytes = include_bytes!(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/../deploy/q3_26_vault.so"
    ));
    svm.add_program(program_id, bytes).unwrap();
    svm.airdrop(&user.pubkey(), 2_000_000_000).unwrap();

    expect_ok(
        &mut svm,
        &user,
        Instruction::new_with_bytes(
            program_id,
            &q3_26_vault::instruction::Initialize {}.data(),
            q3_26_vault::accounts::Initialize {
                user: user.pubkey(),
                vault_state,
                vault,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
        ),
    );

    expect_ok(
        &mut svm,
        &user,
        Instruction::new_with_bytes(
            program_id,
            &q3_26_vault::instruction::Deposit {
                amount: DEPOSIT_LAMPORTS,
            }
            .data(),
            q3_26_vault::accounts::Deposit {
                user: user.pubkey(),
                vault_state,
                vault,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
        ),
    );

    Ctx {
        svm,
        program_id,
        user,
        vault_state,
        vault,
    }
}

fn withdraw_ix(ctx: &Ctx, signer: &Pubkey, amount: u64) -> Instruction {
    Instruction::new_with_bytes(
        ctx.program_id,
        &q3_26_vault::instruction::Withdraw { amount }.data(),
        q3_26_vault::accounts::Withdraw {
            user: *signer,
            vault_state: ctx.vault_state,
            vault: ctx.vault,
            system_program: system_program::ID,
        }
        .to_account_metas(None),
    )
}

#[test]
fn deposit_of_zero_is_rejected() {
    let mut ctx = setup();
    let ix = Instruction::new_with_bytes(
        ctx.program_id,
        &q3_26_vault::instruction::Deposit { amount: 0 }.data(),
        q3_26_vault::accounts::Deposit {
            user: ctx.user.pubkey(),
            vault_state: ctx.vault_state,
            vault: ctx.vault,
            system_program: system_program::ID,
        }
        .to_account_metas(None),
    );
    let user = ctx.user.insecure_clone();
    expect_failure(&mut ctx.svm, &user, ix, "InvalidAmount");
}

#[test]
fn withdraw_of_zero_is_rejected() {
    let mut ctx = setup();
    let ix = withdraw_ix(&ctx, &ctx.user.pubkey(), 0);
    let user = ctx.user.insecure_clone();
    expect_failure(&mut ctx.svm, &user, ix, "InvalidAmount");
}

/// The vault must stay rent-exempt. Asking for the full balance, reserve
/// included, has to fail — otherwise the runtime reaps the vault while
/// `vault_state` still points at it.
#[test]
fn withdraw_that_breaks_rent_exemption_is_rejected() {
    let mut ctx = setup();
    let balance = ctx.svm.get_balance(&ctx.vault).unwrap();
    let ix = withdraw_ix(&ctx, &ctx.user.pubkey(), balance);
    let user = ctx.user.insecure_clone();
    expect_failure(&mut ctx.svm, &user, ix, "InsufficientFunds");
}

/// Withdrawing exactly down to the rent-exempt floor is the boundary and must
/// still succeed.
#[test]
fn withdraw_down_to_the_rent_floor_succeeds() {
    let mut ctx = setup();
    let rent_exempt = ctx.svm.minimum_balance_for_rent_exemption(0);
    let available = ctx.svm.get_balance(&ctx.vault).unwrap() - rent_exempt;
    let ix = withdraw_ix(&ctx, &ctx.user.pubkey(), available);
    let user = ctx.user.insecure_clone();
    expect_ok(&mut ctx.svm, &user, ix);
    assert_eq!(ctx.svm.get_balance(&ctx.vault).unwrap(), rent_exempt);
}

/// A stranger signing over someone else's vault PDAs. The seeds constraint
/// derives from `user.key()`, so the addresses simply do not match.
#[test]
fn a_stranger_cannot_withdraw_from_someone_elses_vault() {
    let mut ctx = setup();
    let attacker = Keypair::new();
    ctx.svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();

    let ix = withdraw_ix(&ctx, &attacker.pubkey(), 1_000);
    expect_failure(&mut ctx.svm, &attacker, ix, "ConstraintSeeds");
}

/// Same idea for close: nobody but the owner gets to sweep the vault.
#[test]
fn a_stranger_cannot_close_someone_elses_vault() {
    let mut ctx = setup();
    let attacker = Keypair::new();
    ctx.svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();

    let ix = Instruction::new_with_bytes(
        ctx.program_id,
        &q3_26_vault::instruction::Close {}.data(),
        q3_26_vault::accounts::Close {
            user: attacker.pubkey(),
            vault_state: ctx.vault_state,
            vault: ctx.vault,
            system_program: system_program::ID,
        }
        .to_account_metas(None),
    );
    expect_failure(&mut ctx.svm, &attacker, ix, "ConstraintSeeds");
}

/// Close returns everything: the vault balance and the state account's rent.
#[test]
fn close_returns_vault_balance_and_state_rent() {
    let mut ctx = setup();
    let user = ctx.user.insecure_clone();

    let vault_balance = ctx.svm.get_balance(&ctx.vault).unwrap();
    let state_rent = ctx.svm.get_balance(&ctx.vault_state).unwrap();
    let user_before = ctx.svm.get_balance(&user.pubkey()).unwrap();

    let ix = Instruction::new_with_bytes(
        ctx.program_id,
        &q3_26_vault::instruction::Close {}.data(),
        q3_26_vault::accounts::Close {
            user: user.pubkey(),
            vault_state: ctx.vault_state,
            vault: ctx.vault,
            system_program: system_program::ID,
        }
        .to_account_metas(None),
    );
    expect_ok(&mut ctx.svm, &user, ix);

    let user_after = ctx.svm.get_balance(&user.pubkey()).unwrap();
    let returned = user_after - user_before;
    let expected = vault_balance + state_rent;

    // The only thing standing between `returned` and `expected` is the tx fee.
    assert!(
        returned <= expected && expected - returned < 100_000,
        "expected roughly {expected} lamports back, got {returned}"
    );
    assert_eq!(ctx.svm.get_balance(&ctx.vault).unwrap_or(0), 0);
}
