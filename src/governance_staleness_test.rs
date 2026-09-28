#![cfg(test)]
//! Bug condition exploration and fix-checking tests for issue #864.
//!
//! A governance proposal created under admin A must NOT be executable after the
//! contract admin has been changed to a different address by any route:
//!   - Route 1: direct `update_admin`
//!   - Route 2: concurrent proposal — a prior proposal execution changes admin
//!   - Route 3: timelocked `SetAdmin` (covered via direct test helper)
//!
//! ## Methodology
//!
//! Task 1  — Bug-condition exploration tests.  These tests assert the CORRECT
//!           (fixed) behaviour.  They FAIL on unfixed code (proving the bug)
//!           and PASS after the fix is applied.
//!
//! Task 2  — Preservation tests.  These assert that proposals whose
//!           `admin_at_creation` still matches the current admin continue to
//!           behave exactly as before.

extern crate std;

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, BytesN, Env,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn setup() -> (
    Env,
    StellarWrapContractClient<'static>,
    Address, // admin
    Address, // proposer
    Address, // proposed_admin
) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, StellarWrapContract);
    let client = StellarWrapContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let proposer = Address::generate(&env);
    let proposed_admin = Address::generate(&env);
    let pubkey = BytesN::from_array(&env, &[1u8; 32]);
    client.initialize(&admin, &pubkey);
    (env, client, admin, proposer, proposed_admin)
}

/// Create a proposal, cast one passing vote, and advance past the end time.
/// Returns the proposal id. The ledger is positioned just after `end_time`.
fn create_passing_proposal(
    env: &Env,
    client: &StellarWrapContractClient,
    proposer: &Address,
    proposed_admin: &Address,
) -> u64 {
    let duration: u64 = 3_600; // 1 hour (minimum)
    let id = client.create_admin_proposal(proposer, proposed_admin, &duration);
    // Cast a passing vote
    client.vote_admin_proposal(proposer, &id, &true);
    // Advance past end_time
    env.ledger().with_mut(|l| l.timestamp += duration + 1);
    id
}

// ═══════════════════════════════════════════════════════════════════════════════
// TASK 1 — Bug-condition exploration / fix-checking tests
// ═══════════════════════════════════════════════════════════════════════════════

/// Route 1: stale proposal after a direct `update_admin`.
///
/// Bug condition: proposal.admin_at_creation == A, current admin == B (B ≠ A).
/// Fixed behaviour: `execute_admin_proposal` must panic with #57 (StaleProposal).
/// Admin must remain B after the rejection.
#[test]
#[should_panic(expected = "Error(Contract, #57)")]
fn test_stale_proposal_after_direct_admin_change() {
    let (env, client, _admin, proposer, proposed_admin) = setup();

    // 1. Create and pass a proposal while admin is A.
    let proposal_id = create_passing_proposal(&env, &client, &proposer, &proposed_admin);

    // 2. Rotate admin to B via direct update_admin (A → B).
    let new_admin = Address::generate(&env);
    client.update_admin(&new_admin);
    assert_eq!(client.get_admin().unwrap(), new_admin, "admin should be new_admin");

    // 3. Attempt to execute the stale proposal — must panic with #57.
    client.execute_admin_proposal(&proposal_id);
    // ↑ should have panicked; anything below would be a test failure on fixed code.
}

/// Companion assertion: after the StaleProposal rejection, the admin is unchanged.
#[test]
fn test_stale_proposal_does_not_modify_admin() {
    let (env, client, _admin, proposer, proposed_admin) = setup();

    let proposal_id = create_passing_proposal(&env, &client, &proposer, &proposed_admin);

    let new_admin = Address::generate(&env);
    client.update_admin(&new_admin);

    // Catch the panic so we can inspect state afterwards.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.execute_admin_proposal(&proposal_id);
    }));
    assert!(result.is_err(), "execute should have panicked with StaleProposal");

    // Admin must still be new_admin, not proposed_admin.
    assert_eq!(
        client.get_admin().unwrap(),
        new_admin,
        "admin must not be overwritten by a stale proposal"
    );
}

/// Route 2: concurrent-proposal race.
///
/// Both P1 and P2 are created under admin A. P1 executes first (admin → X).
/// P2 must then be rejected with StaleProposal (#57); admin stays X.
#[test]
#[should_panic(expected = "Error(Contract, #57)")]
fn test_stale_proposal_concurrent_proposals_race() {
    let (env, client, _admin, proposer, _) = setup();

    let proposed_x = Address::generate(&env);
    let proposed_y = Address::generate(&env);
    let duration: u64 = 3_600;

    // Create P1 and P2 while admin is A.
    let p1 = client.create_admin_proposal(&proposer, &proposed_x, &duration);
    let p2 = client.create_admin_proposal(&proposer, &proposed_y, &duration);

    // Cast passing votes on both.
    client.vote_admin_proposal(&proposer, &p1, &true);
    let voter2 = Address::generate(&env);
    client.vote_admin_proposal(&voter2, &p2, &true);

    // Advance past both proposals' end_time.
    env.ledger().with_mut(|l| l.timestamp += duration + 1);

    // Execute P1 successfully — admin becomes X.
    client.execute_admin_proposal(&p1);
    assert_eq!(client.get_admin().unwrap(), proposed_x);

    // Executing P2 now must fail with StaleProposal (#57) because
    // P2.admin_at_creation == A but current admin == X.
    client.execute_admin_proposal(&p2);
}

/// Companion assertion: concurrent race — admin stays X after P2 is rejected.
#[test]
fn test_concurrent_proposals_admin_unchanged_after_stale_rejection() {
    let (env, client, _admin, proposer, _) = setup();

    let proposed_x = Address::generate(&env);
    let proposed_y = Address::generate(&env);
    let duration: u64 = 3_600;

    let p1 = client.create_admin_proposal(&proposer, &proposed_x, &duration);
    let p2 = client.create_admin_proposal(&proposer, &proposed_y, &duration);

    client.vote_admin_proposal(&proposer, &p1, &true);
    let voter2 = Address::generate(&env);
    client.vote_admin_proposal(&voter2, &p2, &true);

    env.ledger().with_mut(|l| l.timestamp += duration + 1);

    client.execute_admin_proposal(&p1);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.execute_admin_proposal(&p2);
    }));
    assert!(result.is_err(), "P2 should have been rejected as stale");

    assert_eq!(
        client.get_admin().unwrap(),
        proposed_x,
        "admin must remain X after P2 is rejected"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// TASK 2 — Preservation tests (no admin drift → identical behaviour)
// ═══════════════════════════════════════════════════════════════════════════════

/// Happy-path lifecycle: no admin drift → proposal executes, admin updates.
#[test]
fn test_preservation_passing_proposal_no_drift_executes() {
    let (env, client, _admin, proposer, proposed_admin) = setup();
    let proposal_id = create_passing_proposal(&env, &client, &proposer, &proposed_admin);
    client.execute_admin_proposal(&proposal_id);
    assert_eq!(
        client.get_admin().unwrap(),
        proposed_admin,
        "admin must be updated to proposed_admin on a clean execution"
    );
    let proposal = client.get_admin_proposal(&proposal_id).unwrap();
    assert_eq!(proposal.status, ProposalStatus::Executed);
}

/// Defeated proposal: no admin drift → status is Defeated, admin unchanged.
#[test]
fn test_preservation_defeated_proposal_no_drift() {
    let (env, client, admin, proposer, proposed_admin) = setup();
    let duration: u64 = 3_600;
    let id = client.create_admin_proposal(&proposer, &proposed_admin, &duration);
    // One against, zero for → defeated.
    let voter = Address::generate(&env);
    client.vote_admin_proposal(&voter, &id, &false);
    env.ledger().with_mut(|l| l.timestamp += duration + 1);
    client.execute_admin_proposal(&id);
    let proposal = client.get_admin_proposal(&id).unwrap();
    assert_eq!(proposal.status, ProposalStatus::Defeated);
    assert_eq!(client.get_admin().unwrap(), admin, "admin must not change on defeat");
}

/// Pre-end-time guard preserved: execution before end_time still fails #28.
#[test]
#[should_panic(expected = "Error(Contract, #28)")]
fn test_preservation_pre_end_time_guard_unchanged() {
    let (_env, client, _admin, proposer, proposed_admin) = setup();
    let id = client.create_admin_proposal(&proposer, &proposed_admin, &3_600u64);
    // Do NOT advance time — ledger is at 0, end_time is 3600.
    client.execute_admin_proposal(&id);
}

/// ProposalNotActive guard preserved: re-executing an executed proposal fails #26.
#[test]
#[should_panic(expected = "Error(Contract, #26)")]
fn test_preservation_not_active_guard_on_re_execute() {
    let (env, client, _admin, proposer, proposed_admin) = setup();
    let id = create_passing_proposal(&env, &client, &proposer, &proposed_admin);
    client.execute_admin_proposal(&id); // first execution — succeeds
    client.execute_admin_proposal(&id); // second execution — must fail #26
}

/// admin_at_creation is set correctly on a fresh proposal.
#[test]
fn test_admin_at_creation_matches_admin_at_proposal_creation_time() {
    let (_env, client, admin, proposer, proposed_admin) = setup();
    let id = client.create_admin_proposal(&proposer, &proposed_admin, &3_600u64);
    let proposal = client.get_admin_proposal(&id).unwrap();
    assert_eq!(
        proposal.admin_at_creation,
        admin,
        "admin_at_creation must equal the admin at proposal creation time"
    );
}

/// StaleProposal error code is 57.
#[test]
fn test_stale_proposal_error_code_is_57() {
    assert_eq!(ContractError::StaleProposal as u32, 57);
}
