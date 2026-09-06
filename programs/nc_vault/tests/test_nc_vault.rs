//! Non-custodial vault: share accounting, market deployment, and — the point of
//! the whole exercise — in-kind exit from a vault holding zero liquid asset.

use {
    anchor_lang::{
        prelude::Clock,
        solana_program::{instruction::AccountMeta, instruction::Instruction},
        system_program::ID as SYSTEM_PROGRAM_ID,
        AccountDeserialize, InstructionData, ToAccountMetas,
    },
    anchor_spl::{
        associated_token::{self, ID as ASSOCIATED_TOKEN_PROGRAM_ID},
        token::spl_token,
    },
    litesvm::{types::FailedTransactionMetadata, LiteSVM},
    litesvm_token::{
        spl_token::ID as TOKEN_PROGRAM_ID, CreateAssociatedTokenAccount, CreateMint, MintTo,
    },
    nc_vault::{POSITION_SEED, SHARE_SEED, VAULT_SEED},
    solana_keypair::Keypair,
    solana_message::Message,
    solana_pubkey::Pubkey,
    solana_signer::Signer,
    solana_transaction::Transaction,
};

const DECIMALS: u8 = 6;
const ONE: u64 = 1_000_000;
const SEED: u64 = 7;
const MAX_AGE: i64 = 3_600;

struct World {
    svm: LiteSVM,
    payer: Keypair,
    governor: Keypair,
    manager: Keypair,
    market: Keypair,
    alice: Keypair,
    bob: Keypair,

    asset_mint: Pubkey,
    position_mint: Pubkey,

    vault: Pubkey,
    share_mint: Pubkey,
    vault_asset_ata: Pubkey,
    vault_position_ata: Pubkey,
    position: Pubkey,

    market_asset_ata: Pubkey,
    market_position_ata: Pubkey,
}

// ------------------------------------------------------------------ plumbing

fn submit(
    svm: &mut LiteSVM,
    payer: &Keypair,
    signers: &[&Keypair],
    ix: Instruction,
) -> Result<(), FailedTransactionMetadata> {
    // A fresh blockhash for every transaction. Several tests send byte-identical
    // instructions twice (deposit the same amount, retry after a warp), and those
    // would otherwise collide on signature and come back as `AlreadyProcessed`
    // rather than exercising the program at all.
    svm.expire_blockhash();

    let message = Message::new(&[ix], Some(&payer.pubkey()));
    let blockhash = svm.latest_blockhash();
    let tx = Transaction::new(signers, message, blockhash);
    svm.send_transaction(tx).map(|_| ())
}

fn expect_ok(svm: &mut LiteSVM, payer: &Keypair, signers: &[&Keypair], ix: Instruction) {
    submit(svm, payer, signers, ix).unwrap_or_else(|err| {
        panic!(
            "tx should have succeeded: {:?}\nlogs:\n{}",
            err.err,
            err.meta.logs.join("\n")
        );
    });
}

fn expect_failure(
    svm: &mut LiteSVM,
    payer: &Keypair,
    signers: &[&Keypair],
    ix: Instruction,
    needle: &str,
) {
    match submit(svm, payer, signers, ix) {
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

fn token_balance(svm: &LiteSVM, ata: &Pubkey) -> u64 {
    svm.get_account(ata)
        .map(|acc| spl_token::state::Account::unpack_from_slice(&acc.data).unwrap().amount)
        .unwrap_or(0)
}

fn mint_supply(svm: &LiteSVM, mint: &Pubkey) -> u64 {
    let acc = svm.get_account(mint).unwrap();
    spl_token::state::Mint::unpack_from_slice(&acc.data)
        .unwrap()
        .supply
}

fn read_position(svm: &LiteSVM, position: &Pubkey) -> nc_vault::state::Position {
    let acc = svm.get_account(position).unwrap();
    nc_vault::state::Position::try_deserialize(&mut acc.data.as_ref()).unwrap()
}

fn now(svm: &LiteSVM) -> i64 {
    svm.get_sysvar::<Clock>().unix_timestamp
}

fn warp_by(svm: &mut LiteSVM, seconds: i64) {
    let mut clock = svm.get_sysvar::<Clock>();
    clock.unix_timestamp += seconds;
    svm.set_sysvar::<Clock>(&clock);
}

use anchor_lang::solana_program::program_pack::Pack as _;

// ------------------------------------------------------------------ setup

fn setup() -> World {
    let program_id = nc_vault::id();

    let payer = Keypair::new();
    let governor = Keypair::new();
    let manager = Keypair::new();
    let market = Keypair::new();
    let alice = Keypair::new();
    let bob = Keypair::new();

    let mut svm = LiteSVM::new();
    let bytes = include_bytes!(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/../deploy/nc_vault.so"
    ));
    svm.add_program(program_id, bytes).unwrap();

    for key in [&payer, &governor, &manager, &market, &alice, &bob] {
        svm.airdrop(&key.pubkey(), 100_000_000_000).unwrap();
    }

    let asset_mint = CreateMint::new(&mut svm, &payer)
        .decimals(DECIMALS)
        .authority(&payer.pubkey())
        .send()
        .unwrap();
    let position_mint = CreateMint::new(&mut svm, &payer)
        .decimals(DECIMALS)
        .authority(&payer.pubkey())
        .send()
        .unwrap();

    let (vault, _) = Pubkey::find_program_address(
        &[VAULT_SEED, governor.pubkey().as_ref(), &SEED.to_le_bytes()],
        &program_id,
    );
    let (share_mint, _) =
        Pubkey::find_program_address(&[SHARE_SEED, vault.as_ref()], &program_id);
    let (position, _) = Pubkey::find_program_address(
        &[POSITION_SEED, vault.as_ref(), position_mint.as_ref()],
        &program_id,
    );

    let vault_asset_ata = associated_token::get_associated_token_address(&vault, &asset_mint);
    let vault_position_ata =
        associated_token::get_associated_token_address(&vault, &position_mint);

    // The market is a plain counterparty: it holds position tokens to sell and an
    // account to be paid into.
    let market_asset_ata = CreateAssociatedTokenAccount::new(&mut svm, &payer, &asset_mint)
        .owner(&market.pubkey())
        .send()
        .unwrap();
    let market_position_ata = CreateAssociatedTokenAccount::new(&mut svm, &payer, &position_mint)
        .owner(&market.pubkey())
        .send()
        .unwrap();

    // Deep inventory on both sides: the market has to be able to sell position
    // tokens and to buy them back for asset later.
    MintTo::new(
        &mut svm,
        &payer,
        &position_mint,
        &market_position_ata,
        100_000 * ONE,
    )
    .send()
    .unwrap();
    MintTo::new(&mut svm, &payer, &asset_mint, &market_asset_ata, 100_000 * ONE)
        .send()
        .unwrap();

    for who in [&alice, &bob] {
        let ata = CreateAssociatedTokenAccount::new(&mut svm, &payer, &asset_mint)
            .owner(&who.pubkey())
            .send()
            .unwrap();
        MintTo::new(&mut svm, &payer, &asset_mint, &ata, 10_000 * ONE)
            .send()
            .unwrap();
    }

    World {
        svm,
        payer,
        governor,
        manager,
        market,
        alice,
        bob,
        asset_mint,
        position_mint,
        vault,
        share_mint,
        vault_asset_ata,
        vault_position_ata,
        position,
        market_asset_ata,
        market_position_ata,
    }
}

impl World {
    fn asset_ata(&self, owner: &Pubkey) -> Pubkey {
        associated_token::get_associated_token_address(owner, &self.asset_mint)
    }

    fn share_ata(&self, owner: &Pubkey) -> Pubkey {
        associated_token::get_associated_token_address(owner, &self.share_mint)
    }

    fn position_ata(&self, owner: &Pubkey) -> Pubkey {
        associated_token::get_associated_token_address(owner, &self.position_mint)
    }

    fn init_vault(&mut self) {
        let ix = Instruction {
            program_id: nc_vault::id(),
            accounts: nc_vault::accounts::InitializeVault {
                governor: self.governor.pubkey(),
                asset_mint: self.asset_mint,
                vault: self.vault,
                share_mint: self.share_mint,
                vault_asset_ata: self.vault_asset_ata,
                token_program: TOKEN_PROGRAM_ID,
                associated_token_program: ASSOCIATED_TOKEN_PROGRAM_ID,
                system_program: SYSTEM_PROGRAM_ID,
            }
            .to_account_metas(None),
            data: nc_vault::instruction::InitializeVault {
                seed: SEED,
                manager: self.manager.pubkey(),
                max_valuation_age: MAX_AGE,
            }
            .data(),
        };
        let governor = self.governor.insecure_clone();
        expect_ok(&mut self.svm, &governor, &[&governor], ix);
    }

    fn register_position_ix(&self, market: Pubkey) -> Instruction {
        Instruction {
            program_id: nc_vault::id(),
            accounts: nc_vault::accounts::RegisterPosition {
                governor: self.governor.pubkey(),
                vault: self.vault,
                position_mint: self.position_mint,
                market,
                position: self.position,
                vault_position_ata: self.vault_position_ata,
                token_program: TOKEN_PROGRAM_ID,
                associated_token_program: ASSOCIATED_TOKEN_PROGRAM_ID,
                system_program: SYSTEM_PROGRAM_ID,
            }
            .to_account_metas(None),
            data: nc_vault::instruction::RegisterPosition {}.data(),
        }
    }

    fn register_position(&mut self) {
        let ix = self.register_position_ix(self.market.pubkey());
        let governor = self.governor.insecure_clone();
        expect_ok(&mut self.svm, &governor, &[&governor], ix);
    }

    /// `deposit` needs every position of the vault appended as a read-only
    /// remaining account, sorted by key.
    fn deposit_ix(&self, who: &Pubkey, amount: u64, positions: &[Pubkey]) -> Instruction {
        let mut accounts = nc_vault::accounts::Deposit {
            user: *who,
            vault: self.vault,
            asset_mint: self.asset_mint,
            share_mint: self.share_mint,
            user_asset_ata: self.asset_ata(who),
            user_share_ata: self.share_ata(who),
            vault_asset_ata: self.vault_asset_ata,
            token_program: TOKEN_PROGRAM_ID,
            associated_token_program: ASSOCIATED_TOKEN_PROGRAM_ID,
            system_program: SYSTEM_PROGRAM_ID,
        }
        .to_account_metas(None);

        let mut sorted = positions.to_vec();
        sorted.sort();
        for position in sorted {
            accounts.push(AccountMeta::new_readonly(position, false));
        }

        Instruction {
            program_id: nc_vault::id(),
            accounts,
            data: nc_vault::instruction::Deposit { amount }.data(),
        }
    }

    fn deposit(&mut self, who: &Keypair, amount: u64, positions: &[Pubkey]) {
        let ix = self.deposit_ix(&who.pubkey(), amount, positions);
        expect_ok(&mut self.svm, who, &[who], ix);
    }

    fn deploy_ix_for(
        &self,
        position: Pubkey,
        position_mint: Pubkey,
        asset_amount: u64,
        position_amount: u64,
    ) -> Instruction {
        Instruction {
            program_id: nc_vault::id(),
            accounts: nc_vault::accounts::Deploy {
                manager: self.manager.pubkey(),
                market: self.market.pubkey(),
                vault: self.vault,
                position,
                asset_mint: self.asset_mint,
                position_mint,
                vault_asset_ata: self.vault_asset_ata,
                vault_position_ata: associated_token::get_associated_token_address(
                    &self.vault,
                    &position_mint,
                ),
                market_asset_ata: self.market_asset_ata,
                market_position_ata: associated_token::get_associated_token_address(
                    &self.market.pubkey(),
                    &position_mint,
                ),
                token_program: TOKEN_PROGRAM_ID,
            }
            .to_account_metas(None),
            data: nc_vault::instruction::Deploy {
                asset_amount,
                position_amount,
            }
            .data(),
        }
    }

    fn deploy_ix(&self, asset_amount: u64, position_amount: u64) -> Instruction {
        self.deploy_ix_for(self.position, self.position_mint, asset_amount, position_amount)
    }

    fn deploy(&mut self, asset_amount: u64, position_amount: u64) {
        let ix = self.deploy_ix(asset_amount, position_amount);
        let manager = self.manager.insecure_clone();
        let market = self.market.insecure_clone();
        expect_ok(&mut self.svm, &manager, &[&manager, &market], ix);
    }

    fn deploy_into(
        &mut self,
        position: Pubkey,
        position_mint: Pubkey,
        asset_amount: u64,
        position_amount: u64,
    ) {
        let ix = self.deploy_ix_for(position, position_mint, asset_amount, position_amount);
        let manager = self.manager.insecure_clone();
        let market = self.market.insecure_clone();
        expect_ok(&mut self.svm, &manager, &[&manager, &market], ix);
    }

    /// Mint a brand-new position token, stock the market with it, and have the
    /// governor approve it. Returns the position record and its mint.
    fn add_position(&mut self) -> (Pubkey, Pubkey) {
        let payer = self.payer.insecure_clone();
        let governor = self.governor.insecure_clone();

        let mint = CreateMint::new(&mut self.svm, &payer)
            .decimals(DECIMALS)
            .authority(&payer.pubkey())
            .send()
            .unwrap();

        let market_ata = CreateAssociatedTokenAccount::new(&mut self.svm, &payer, &mint)
            .owner(&self.market.pubkey())
            .send()
            .unwrap();
        MintTo::new(&mut self.svm, &payer, &mint, &market_ata, 100_000 * ONE)
            .send()
            .unwrap();

        let (position, _) = Pubkey::find_program_address(
            &[POSITION_SEED, self.vault.as_ref(), mint.as_ref()],
            &nc_vault::id(),
        );

        let ix = Instruction {
            program_id: nc_vault::id(),
            accounts: nc_vault::accounts::RegisterPosition {
                governor: governor.pubkey(),
                vault: self.vault,
                position_mint: mint,
                market: self.market.pubkey(),
                position,
                vault_position_ata: associated_token::get_associated_token_address(
                    &self.vault,
                    &mint,
                ),
                token_program: TOKEN_PROGRAM_ID,
                associated_token_program: ASSOCIATED_TOKEN_PROGRAM_ID,
                system_program: SYSTEM_PROGRAM_ID,
            }
            .to_account_metas(None),
            data: nc_vault::instruction::RegisterPosition {}.data(),
        };
        expect_ok(&mut self.svm, &governor, &[&governor], ix);

        (position, mint)
    }

    fn ensure_ata_for(&mut self, who: &Pubkey, mint: &Pubkey) {
        let payer = self.payer.insecure_clone();
        let ata = associated_token::get_associated_token_address(who, mint);
        if self.svm.get_account(&ata).is_none() {
            CreateAssociatedTokenAccount::new(&mut self.svm, &payer, mint)
                .owner(who)
                .send()
                .unwrap();
        }
    }

    fn unwind_ix(&self, position_amount: u64, asset_amount: u64) -> Instruction {
        Instruction {
            program_id: nc_vault::id(),
            accounts: nc_vault::accounts::Unwind {
                manager: self.manager.pubkey(),
                market: self.market.pubkey(),
                vault: self.vault,
                position: self.position,
                asset_mint: self.asset_mint,
                position_mint: self.position_mint,
                vault_asset_ata: self.vault_asset_ata,
                vault_position_ata: self.vault_position_ata,
                market_asset_ata: self.market_asset_ata,
                market_position_ata: self.market_position_ata,
                token_program: TOKEN_PROGRAM_ID,
            }
            .to_account_metas(None),
            data: nc_vault::instruction::Unwind {
                position_amount,
                asset_amount,
            }
            .data(),
        }
    }

    fn revalue_ix(&self, value_per_unit: u64) -> Instruction {
        Instruction {
            program_id: nc_vault::id(),
            accounts: nc_vault::accounts::Revalue {
                manager: self.manager.pubkey(),
                vault: self.vault,
                position_mint: self.position_mint,
                position: self.position,
            }
            .to_account_metas(None),
            data: nc_vault::instruction::Revalue { value_per_unit }.data(),
        }
    }

    fn revalue(&mut self, value_per_unit: u64) {
        let ix = self.revalue_ix(value_per_unit);
        let manager = self.manager.insecure_clone();
        expect_ok(&mut self.svm, &manager, &[&manager], ix);
    }

    /// `exit` needs four accounts per position: the record, its mint, the vault's
    /// token account and the redeemer's.
    fn exit_ix(&self, who: &Pubkey, shares: u64, positions: &[(Pubkey, Pubkey)]) -> Instruction {
        let mut accounts = nc_vault::accounts::Exit {
            user: *who,
            vault: self.vault,
            asset_mint: self.asset_mint,
            share_mint: self.share_mint,
            user_share_ata: self.share_ata(who),
            user_asset_ata: self.asset_ata(who),
            vault_asset_ata: self.vault_asset_ata,
            token_program: TOKEN_PROGRAM_ID,
            associated_token_program: ASSOCIATED_TOKEN_PROGRAM_ID,
            system_program: SYSTEM_PROGRAM_ID,
        }
        .to_account_metas(None);

        let mut sorted = positions.to_vec();
        sorted.sort_by_key(|(position, _)| *position);

        for (position, mint) in sorted {
            accounts.push(AccountMeta::new(position, false));
            accounts.push(AccountMeta::new_readonly(mint, false));
            accounts.push(AccountMeta::new(
                associated_token::get_associated_token_address(&self.vault, &mint),
                false,
            ));
            accounts.push(AccountMeta::new(
                associated_token::get_associated_token_address(who, &mint),
                false,
            ));
        }

        Instruction {
            program_id: nc_vault::id(),
            accounts,
            data: nc_vault::instruction::Exit { shares }.data(),
        }
    }

    /// The redeemer must already own a token account for each position mint —
    /// `exit` will not create them, because remaining accounts cannot carry
    /// Anchor's `init_if_needed`.
    fn ensure_position_ata(&mut self, who: &Pubkey) {
        let payer = self.payer.insecure_clone();
        let mint = self.position_mint;
        if self.svm.get_account(&self.position_ata(who)).is_none() {
            CreateAssociatedTokenAccount::new(&mut self.svm, &payer, &mint)
                .owner(who)
                .send()
                .unwrap();
        }
    }

    fn exit(&mut self, who: &Keypair, shares: u64) {
        self.ensure_position_ata(&who.pubkey());
        let ix = self.exit_ix(
            &who.pubkey(),
            shares,
            &[(self.position, self.position_mint)],
        );
        expect_ok(&mut self.svm, who, &[who], ix);
    }
}

// ------------------------------------------------------------------ tests

#[test]
fn first_deposit_sets_one_share_per_asset_unit() {
    let mut w = setup();
    w.init_vault();

    let alice = w.alice.insecure_clone();
    w.deposit(&alice, 1_000 * ONE, &[]);

    assert_eq!(token_balance(&w.svm, &w.share_ata(&alice.pubkey())), 1_000 * ONE);
    assert_eq!(token_balance(&w.svm, &w.vault_asset_ata), 1_000 * ONE);
    assert_eq!(mint_supply(&w.svm, &w.share_mint), 1_000 * ONE);
}

#[test]
fn a_second_depositor_gets_shares_at_the_current_price() {
    let mut w = setup();
    w.init_vault();
    w.register_position();

    let alice = w.alice.insecure_clone();
    let bob = w.bob.insecure_clone();
    let positions = [w.position];

    w.deposit(&alice, 1_000 * ONE, &positions);

    // Deploy 800 asset for 400 position units: an executed rate of 2.0.
    w.deploy(800 * ONE, 400 * ONE);
    assert_eq!(read_position(&w.svm, &w.position).value_per_unit, 2 * ONE);

    // NAV is unchanged by the trade itself: 200 liquid + 400 units at 2.0 = 1000.
    // So Bob's 500 buys exactly 500 shares.
    w.deposit(&bob, 500 * ONE, &positions);
    assert_eq!(token_balance(&w.svm, &w.share_ata(&bob.pubkey())), 500 * ONE);

    // Now the position doubles in value. NAV becomes 700 liquid + 1600 = 2300
    // against 1500 shares, so a share is worth about 1.533 and the same 500 asset
    // buys fewer of them.
    w.revalue(4 * ONE);
    let alice_before = token_balance(&w.svm, &w.share_ata(&alice.pubkey()));
    w.deposit(&alice, 500 * ONE, &positions);
    let minted = token_balance(&w.svm, &w.share_ata(&alice.pubkey())) - alice_before;

    // 500 * 1500 / 2300 = 326.08...
    assert_eq!(minted, 326_086_956);
    assert!(minted < 500 * ONE, "a richer vault must issue fewer shares");
}

/// The headline case: the vault holds **no** liquid asset at all, and a holder
/// still exits in full, in kind, with nobody's permission.
#[test]
fn exit_works_with_zero_liquidity_and_pays_in_kind() {
    let mut w = setup();
    w.init_vault();
    w.register_position();

    let alice = w.alice.insecure_clone();
    let positions = [w.position];

    w.deposit(&alice, 1_000 * ONE, &positions);

    // Every last unit of asset goes into the position.
    w.deploy(1_000 * ONE, 500 * ONE);
    assert_eq!(
        token_balance(&w.svm, &w.vault_asset_ata),
        0,
        "vault should hold no liquid asset"
    );

    let asset_before = token_balance(&w.svm, &w.asset_ata(&alice.pubkey()));

    // Half her shares.
    w.exit(&alice, 500 * ONE);

    // The cash leg pays nothing — there is nothing to pay with — and that is not
    // an error. The position leg pays her half of what the vault holds.
    assert_eq!(
        token_balance(&w.svm, &w.asset_ata(&alice.pubkey())),
        asset_before,
        "no cash was available, so none was paid"
    );
    assert_eq!(
        token_balance(&w.svm, &w.position_ata(&alice.pubkey())),
        250 * ONE,
        "paid in kind instead"
    );

    // The vault's own books agree.
    assert_eq!(read_position(&w.svm, &w.position).amount, 250 * ONE);
    assert_eq!(token_balance(&w.svm, &w.vault_position_ata), 250 * ONE);
    assert_eq!(mint_supply(&w.svm, &w.share_mint), 500 * ONE);
}

#[test]
fn a_partly_deployed_exit_pays_both_legs_pro_rata() {
    let mut w = setup();
    w.init_vault();
    w.register_position();

    let alice = w.alice.insecure_clone();
    let bob = w.bob.insecure_clone();
    let positions = [w.position];

    w.deposit(&alice, 1_000 * ONE, &positions);
    w.deposit(&bob, 500 * ONE, &positions);

    // 1200 of the 1500 goes to work; 300 stays liquid.
    w.deploy(1_200 * ONE, 600 * ONE);

    let alice_asset_before = token_balance(&w.svm, &w.asset_ata(&alice.pubkey()));

    // Alice holds 1000 of 1500 shares — two thirds of everything.
    w.exit(&alice, 1_000 * ONE);

    assert_eq!(
        token_balance(&w.svm, &w.asset_ata(&alice.pubkey())) - alice_asset_before,
        200 * ONE,
        "two thirds of the 300 liquid"
    );
    assert_eq!(
        token_balance(&w.svm, &w.position_ata(&alice.pubkey())),
        400 * ONE,
        "two thirds of the 600 position units"
    );

    // What is left is exactly Bob's third.
    assert_eq!(token_balance(&w.svm, &w.vault_asset_ata), 100 * ONE);
    assert_eq!(read_position(&w.svm, &w.position).amount, 200 * ONE);
    assert_eq!(mint_supply(&w.svm, &w.share_mint), 500 * ONE);

    // And Bob can take it whenever he likes.
    w.exit(&bob, 500 * ONE);
    assert_eq!(token_balance(&w.svm, &w.position_ata(&bob.pubkey())), 200 * ONE);
    assert_eq!(token_balance(&w.svm, &w.vault_asset_ata), 0);
    assert_eq!(mint_supply(&w.svm, &w.share_mint), 0);
}

#[test]
fn exit_needs_no_valuation_at_all() {
    let mut w = setup();
    w.init_vault();
    w.register_position();

    let alice = w.alice.insecure_clone();
    let positions = [w.position];

    w.deposit(&alice, 1_000 * ONE, &positions);
    w.deploy(1_000 * ONE, 500 * ONE);

    // The manager goes quiet for a year. Deposits freeze...
    warp_by(&mut w.svm, 365 * 24 * 3_600);

    let ix = w.deposit_ix(&alice.pubkey(), 100 * ONE, &positions);
    expect_failure(&mut w.svm, &alice, &[&alice], ix, "StaleValuation");

    // ...but the exit does not care in the slightest.
    w.exit(&alice, 1_000 * ONE);
    assert_eq!(token_balance(&w.svm, &w.position_ata(&alice.pubkey())), 500 * ONE);
    assert_eq!(mint_supply(&w.svm, &w.share_mint), 0);
}

#[test]
fn unwinding_returns_asset_to_the_vault() {
    let mut w = setup();
    w.init_vault();
    w.register_position();

    let alice = w.alice.insecure_clone();
    w.deposit(&alice, 1_000 * ONE, &[w.position]);
    w.deploy(1_000 * ONE, 500 * ONE);

    // The market buys the position back at 2.4, a profit for the vault.
    let ix = w.unwind_ix(500 * ONE, 1_200 * ONE);
    let manager = w.manager.insecure_clone();
    let market = w.market.insecure_clone();
    expect_ok(&mut w.svm, &manager, &[&manager, &market], ix);

    assert_eq!(token_balance(&w.svm, &w.vault_asset_ata), 1_200 * ONE);
    let position = read_position(&w.svm, &w.position);
    assert_eq!(position.amount, 0);
    assert_eq!(position.value_per_unit, 2_400_000);

    // Alice's single share of the vault is now worth 1.2 asset.
    let before = token_balance(&w.svm, &w.asset_ata(&alice.pubkey()));
    w.exit(&alice, 1_000 * ONE);
    assert_eq!(
        token_balance(&w.svm, &w.asset_ata(&alice.pubkey())) - before,
        1_200 * ONE
    );
}

// ---------------------------------------------------------------- guards

#[test]
fn the_manager_cannot_route_capital_to_an_unapproved_market() {
    let mut w = setup();
    w.init_vault();
    w.register_position();

    let alice = w.alice.insecure_clone();
    w.deposit(&alice, 1_000 * ONE, &[w.position]);

    // A market of the manager's own choosing, with real token accounts and a real
    // signature — and it still fails, because the governor never approved it.
    let rogue = Keypair::new();
    w.svm.airdrop(&rogue.pubkey(), 10_000_000_000).unwrap();
    let payer = w.payer.insecure_clone();
    let rogue_asset_ata = CreateAssociatedTokenAccount::new(&mut w.svm, &payer, &w.asset_mint)
        .owner(&rogue.pubkey())
        .send()
        .unwrap();
    let rogue_position_ata =
        CreateAssociatedTokenAccount::new(&mut w.svm, &payer, &w.position_mint)
            .owner(&rogue.pubkey())
            .send()
            .unwrap();
    MintTo::new(&mut w.svm, &payer, &w.position_mint, &rogue_position_ata, 500 * ONE)
        .send()
        .unwrap();

    let mut ix = w.deploy_ix(1_000 * ONE, 1);
    // Swap the market and its two token accounts for the rogue's.
    for meta in ix.accounts.iter_mut() {
        if meta.pubkey == w.market.pubkey() {
            meta.pubkey = rogue.pubkey();
        } else if meta.pubkey == w.market_asset_ata {
            meta.pubkey = rogue_asset_ata;
        } else if meta.pubkey == w.market_position_ata {
            meta.pubkey = rogue_position_ata;
        }
    }

    let manager = w.manager.insecure_clone();
    expect_failure(&mut w.svm, &manager, &[&manager, &rogue], ix, "ConstraintHasOne");

    // Not a lamport moved.
    assert_eq!(token_balance(&w.svm, &w.vault_asset_ata), 1_000 * ONE);
}

#[test]
fn only_the_manager_can_deploy() {
    let mut w = setup();
    w.init_vault();
    w.register_position();

    let alice = w.alice.insecure_clone();
    w.deposit(&alice, 1_000 * ONE, &[w.position]);

    let mut ix = w.deploy_ix(100 * ONE, 50 * ONE);
    ix.accounts[0].pubkey = alice.pubkey();

    let market = w.market.insecure_clone();
    expect_failure(&mut w.svm, &alice, &[&alice, &market], ix, "ConstraintHasOne");
}

#[test]
fn exit_rejects_a_short_position_list() {
    let mut w = setup();
    w.init_vault();
    w.register_position();

    let alice = w.alice.insecure_clone();
    w.deposit(&alice, 1_000 * ONE, &[w.position]);
    w.deploy(1_000 * ONE, 500 * ONE);
    w.ensure_position_ata(&alice.pubkey());

    // Omitting the position would hand back the cash leg while leaving the
    // in-kind leg unpaid — and the vault's books wrong.
    let ix = w.exit_ix(&alice.pubkey(), 500 * ONE, &[]);
    expect_failure(&mut w.svm, &alice, &[&alice], ix, "PositionCountMismatch");
}

#[test]
fn exit_rejects_a_duplicated_position() {
    let mut w = setup();
    w.init_vault();
    w.register_position();
    let (second, second_mint) = w.add_position();

    let alice = w.alice.insecure_clone();
    w.deposit(&alice, 1_000 * ONE, &[w.position, second]);
    w.deploy(500 * ONE, 250 * ONE);
    w.deploy_into(second, second_mint, 500 * ONE, 100 * ONE);

    w.ensure_position_ata(&alice.pubkey());
    w.ensure_ata_for(&alice.pubkey(), &second_mint);

    // Two positions exist, so a list of length two passes the count check. Naming
    // the same one twice would pay Alice her slice of it twice over — the
    // strictly-ascending rule is the only thing standing in the way.
    let ix = w.exit_ix(
        &alice.pubkey(),
        500 * ONE,
        &[(w.position, w.position_mint), (w.position, w.position_mint)],
    );
    expect_failure(&mut w.svm, &alice, &[&alice], ix, "PositionsOutOfOrder");

    // The honest list works, and pays out of both.
    let ix = w.exit_ix(
        &alice.pubkey(),
        500 * ONE,
        &[(w.position, w.position_mint), (second, second_mint)],
    );
    expect_ok(&mut w.svm, &alice, &[&alice], ix);
    assert_eq!(token_balance(&w.svm, &w.position_ata(&alice.pubkey())), 125 * ONE);
    assert_eq!(
        token_balance(
            &w.svm,
            &associated_token::get_associated_token_address(&alice.pubkey(), &second_mint)
        ),
        50 * ONE
    );
}

/// A basket of two positions, redeemed in one shot. This is what in-kind exit
/// looks like once a vault holds more than one thing.
#[test]
fn exit_pays_a_slice_of_every_position_in_the_basket() {
    let mut w = setup();
    w.init_vault();
    w.register_position();
    let (second, second_mint) = w.add_position();

    let alice = w.alice.insecure_clone();
    let bob = w.bob.insecure_clone();
    let positions = [w.position, second];

    w.deposit(&alice, 900 * ONE, &positions);
    w.deposit(&bob, 300 * ONE, &positions);

    // Everything deployed, across both venues. No liquid asset left over.
    w.deploy(800 * ONE, 400 * ONE);
    w.deploy_into(second, second_mint, 400 * ONE, 100 * ONE);
    assert_eq!(token_balance(&w.svm, &w.vault_asset_ata), 0);

    w.ensure_position_ata(&bob.pubkey());
    w.ensure_ata_for(&bob.pubkey(), &second_mint);

    // Bob holds 300 of 1200 shares: a quarter of each position, and no cash.
    let ix = w.exit_ix(
        &bob.pubkey(),
        300 * ONE,
        &[(w.position, w.position_mint), (second, second_mint)],
    );
    expect_ok(&mut w.svm, &bob, &[&bob], ix);

    assert_eq!(token_balance(&w.svm, &w.position_ata(&bob.pubkey())), 100 * ONE);
    assert_eq!(
        token_balance(
            &w.svm,
            &associated_token::get_associated_token_address(&bob.pubkey(), &second_mint)
        ),
        25 * ONE
    );
    assert_eq!(read_position(&w.svm, &w.position).amount, 300 * ONE);
    assert_eq!(read_position(&w.svm, &second).amount, 75 * ONE);
    assert_eq!(mint_supply(&w.svm, &w.share_mint), 900 * ONE);
}

#[test]
fn exit_will_not_pay_into_someone_elses_account() {
    let mut w = setup();
    w.init_vault();
    w.register_position();

    let alice = w.alice.insecure_clone();
    let bob = w.bob.insecure_clone();
    w.deposit(&alice, 1_000 * ONE, &[w.position]);
    w.deploy(1_000 * ONE, 500 * ONE);
    w.ensure_position_ata(&alice.pubkey());
    w.ensure_position_ata(&bob.pubkey());

    let mut ix = w.exit_ix(&alice.pubkey(), 500 * ONE, &[(w.position, w.position_mint)]);
    let last = ix.accounts.len() - 1;
    ix.accounts[last].pubkey = w.position_ata(&bob.pubkey());

    expect_failure(&mut w.svm, &alice, &[&alice], ix, "WrongOwner");
}

#[test]
fn nobody_can_exit_more_shares_than_they_hold() {
    let mut w = setup();
    w.init_vault();
    w.register_position();

    let alice = w.alice.insecure_clone();
    let bob = w.bob.insecure_clone();
    w.deposit(&alice, 1_000 * ONE, &[w.position]);
    w.deposit(&bob, 500 * ONE, &[w.position]);
    w.deploy(1_500 * ONE, 750 * ONE);
    w.ensure_position_ata(&bob.pubkey());

    // Bob owns 500 shares but asks for the whole 1500 supply.
    let ix = w.exit_ix(&bob.pubkey(), 1_500 * ONE, &[(w.position, w.position_mint)]);
    expect_failure(&mut w.svm, &bob, &[&bob], ix, "InsufficientShares");
}

#[test]
fn only_the_governor_can_approve_a_new_venue() {
    let mut w = setup();
    w.init_vault();

    let mut ix = w.register_position_ix(w.market.pubkey());
    ix.accounts[0].pubkey = w.manager.pubkey();

    let manager = w.manager.insecure_clone();
    expect_failure(&mut w.svm, &manager, &[&manager], ix, "ConstraintSeeds");
}

#[test]
fn a_position_holding_tokens_cannot_be_retired() {
    let mut w = setup();
    w.init_vault();
    w.register_position();

    let alice = w.alice.insecure_clone();
    w.deposit(&alice, 1_000 * ONE, &[w.position]);
    w.deploy(1_000 * ONE, 500 * ONE);

    let ix = Instruction {
        program_id: nc_vault::id(),
        accounts: nc_vault::accounts::ClosePosition {
            governor: w.governor.pubkey(),
            vault: w.vault,
            position_mint: w.position_mint,
            position: w.position,
            vault_position_ata: w.vault_position_ata,
            token_program: TOKEN_PROGRAM_ID,
        }
        .to_account_metas(None),
        data: nc_vault::instruction::ClosePosition {}.data(),
    };

    let governor = w.governor.insecure_clone();
    expect_failure(&mut w.svm, &governor, &[&governor], ix, "PositionNotEmpty");
}

#[test]
fn retiring_an_empty_position_frees_redeemers_from_carrying_it() {
    let mut w = setup();
    w.init_vault();
    w.register_position();

    let alice = w.alice.insecure_clone();
    w.deposit(&alice, 1_000 * ONE, &[w.position]);
    w.deploy(1_000 * ONE, 500 * ONE);

    let ix = w.unwind_ix(500 * ONE, 1_000 * ONE);
    let manager = w.manager.insecure_clone();
    let market = w.market.insecure_clone();
    expect_ok(&mut w.svm, &manager, &[&manager, &market], ix);

    let ix = Instruction {
        program_id: nc_vault::id(),
        accounts: nc_vault::accounts::ClosePosition {
            governor: w.governor.pubkey(),
            vault: w.vault,
            position_mint: w.position_mint,
            position: w.position,
            vault_position_ata: w.vault_position_ata,
            token_program: TOKEN_PROGRAM_ID,
        }
        .to_account_metas(None),
        data: nc_vault::instruction::ClosePosition {}.data(),
    };
    let governor = w.governor.insecure_clone();
    expect_ok(&mut w.svm, &governor, &[&governor], ix);

    assert!(w.svm.get_account(&w.position).is_none());

    // With the position retired, an exit carries no remaining accounts at all.
    let before = token_balance(&w.svm, &w.asset_ata(&alice.pubkey()));
    let ix = w.exit_ix(&alice.pubkey(), 1_000 * ONE, &[]);
    expect_ok(&mut w.svm, &alice, &[&alice], ix);
    assert_eq!(
        token_balance(&w.svm, &w.asset_ata(&alice.pubkey())) - before,
        1_000 * ONE
    );
}

#[test]
fn a_dust_deposit_that_would_mint_no_shares_is_rejected() {
    let mut w = setup();
    w.init_vault();
    w.register_position();

    let alice = w.alice.insecure_clone();
    let positions = [w.position];
    w.deposit(&alice, 1_000 * ONE, &positions);
    w.deploy(1_000 * ONE, 500 * ONE);

    // Mark the position up hard: NAV per share becomes enormous, so one base unit
    // of asset rounds down to zero shares.
    w.revalue(2_000_000 * ONE);

    let ix = w.deposit_ix(&alice.pubkey(), 1, &positions);
    expect_failure(&mut w.svm, &alice, &[&alice], ix, "DepositTooSmall");
}

#[test]
fn clock_moves_with_the_valuation_window() {
    let mut w = setup();
    w.init_vault();
    w.register_position();

    let alice = w.alice.insecure_clone();
    let positions = [w.position];
    w.deposit(&alice, 1_000 * ONE, &positions);
    w.deploy(1_000 * ONE, 500 * ONE);

    // Just inside the window.
    warp_by(&mut w.svm, MAX_AGE - 1);
    w.deposit(&alice, 10 * ONE, &positions);

    // Just outside it.
    warp_by(&mut w.svm, 2);
    let ix = w.deposit_ix(&alice.pubkey(), 10 * ONE, &positions);
    expect_failure(&mut w.svm, &alice, &[&alice], ix, "StaleValuation");

    // A fresh mark reopens deposits immediately.
    let stamp = now(&w.svm);
    w.revalue(2 * ONE);
    assert_eq!(read_position(&w.svm, &w.position).last_valued, stamp);
    w.deposit(&alice, 10 * ONE, &positions);
}
