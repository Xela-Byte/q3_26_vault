# Process and AI Use

This repository was completed with AI assistance. It should not be treated as proof that the author understands the implementation by default. Before submission, the author is responsible for reviewing, testing, and being able to explain every important account constraint, PDA seed, signer requirement, and security trade-off.

## What I Need To Be Able To Explain

- Why `vault_state` and `vault` are separate PDAs in `q3_26_vault`.
- Why `withdraw` refuses to take the vault below rent exemption.
- Why `close` can sweep the full vault balance when `withdraw` cannot.
- How PDA seeds stop another signer from withdrawing from someone else's vault.
- How the non-custodial `nc_vault` share model works.
- Why `exit` does not read price marks and can work with zero liquid asset.
- How `remaining_accounts` are checked to prevent missing or duplicated positions.
- What the governor can do, what the manager can do, and why those roles are separate.
- Which risks the design does not solve.

## Validation Performed

- The workspace includes Rust LiteSVM tests for the basic vault and the advanced non-custodial vault.
- The README documents the account model, instruction flow, PDA seeds, security reasoning, and known limitations.
- A screenshot of passing tests is included at `docs/tests-passing.png`.

## Study Notes Before Submission

For each instruction, I should be able to answer:

1. Which accounts are passed in?
2. Which account signs?
3. Which PDA seeds are used?
4. What state changes?
5. What attack or mistake does each constraint prevent?
6. Which test proves the behavior?

If I cannot answer those questions without AI assistance, I should continue studying before submitting.
