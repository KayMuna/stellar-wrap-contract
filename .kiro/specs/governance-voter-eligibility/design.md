# Governance Voter Eligibility — Bugfix Design

## Overview

`vote_admin_proposal` currently accepts a vote from any arbitrary authenticated address.
There is no eligibility gate and all votes weigh equally, making the governance mechanism
sybil-susceptible. This design establishes **staker-gated eligibility**: only addresses that
hold an active (non-unstaking) `StakeRecord` may vote. Votes remain flat weight (one eligible
address = one vote). The existing staking module (`src/stake.rs`) already provides the required
on-chain record; the fix is a single eligibility check inserted into `vote_admin_proposal`.

---

## Glossary

- **Bug condition C(X)**: `vote_admin_proposal` is called by `voter` where
  `DataKey::Stake(voter)` is absent or has `unstaking_at ≠ 0`.
- **Eligible voter**: an address whose `StakeRecord` exists and has `unstaking_at == 0`
  at the time `vote_admin_proposal` is called.
- **VoterNotEligible**: the new `ContractError` variant (discriminant 58) returned when
  the bug condition holds.
- **Flat weight**: every eligible voter contributes exactly +1 to `votes_for` or
  `votes_against`; no weighting by stake amount.
- **Snapshot point**: eligibility is checked at vote-cast time, not at proposal creation
  or execution time. An address that stakes after a proposal is created becomes eligible
  for that proposal; an address that initiates unstaking before voting loses eligibility
  for that proposal.

---

## Bug Details

### Bug Condition

```
FUNCTION isBugCondition(voter, env)
  INPUT:  voter of type Address
          env   of type contract execution environment
  OUTPUT: boolean

  record ← env.storage.persistent.get(DataKey::Stake(voter))
  IF record IS None THEN RETURN true
  IF record.unstaking_at ≠ 0 THEN RETURN true
  RETURN false
END FUNCTION
```

### Examples

- **No stake at all**: `voter` has never called `stake()`. `DataKey::Stake(voter)` is absent.
  `isBugCondition` returns `true`. Fixed: `VoterNotEligible` (#58).
- **Unstaking in progress**: `voter` called `unstake()` and `unstaking_at` is non-zero.
  `isBugCondition` returns `true`. Fixed: `VoterNotEligible` (#58).
- **Active staker**: `voter` has a `StakeRecord` with `unstaking_at == 0`.
  `isBugCondition` returns `false`. Vote proceeds as before.

---

## Design Decision: Eligibility Model

Three candidates from the requirements:

| Model | Sybil resistance | Implementation cost |
|---|---|---|
| Open (any address) | None — not a governance mechanism | Requires only documentation |
| Whitelist (Merkle) | High, but admin-controlled list | Requires Merkle proof per vote call |
| **Staker-gated** | **Medium — cost of capital** | **Reuses existing StakeRecord** |

**Decision: staker-gated.** Staking already exists in the contract and provides a meaningful
economic barrier. An attacker must lock real capital to accumulate votes, which is the standard
governance sybil-resistance property. Whitelist-gated is not chosen because it requires an admin
to maintain the whitelist off-chain, which introduces a centralisation risk comparable to the
admin-only governance the mechanism is meant to check.

**Decision: flat weight, not stake-weighted.** Weighted voting by current stake amount is
manipulable: an attacker could stake a large amount, vote, and immediately unstake, with no cost
beyond the cooldown period. Flat weight (one active staker = one vote) avoids this and is simpler
to reason about.

**Decision: eligibility checked at vote-cast time.** This is the earliest safe point. Checking
at proposal-creation time would require snapshotting all eligible voters (unbounded storage).
Checking at execution time would allow stake-then-vote-then-unstake manipulation. Vote-cast time
means the voter must hold an active stake throughout the voting window to influence the outcome.

---

## Expected Behavior

### Preservation Requirements

All inputs where `isBugCondition` returns `false` must produce identical outcomes to the
pre-fix code:

- An active staker voting within the window → vote recorded, `("gov", "vote")` event emitted.
- Double-vote → `ProposalAlreadyVoted` (#27) as before.
- Vote after window closes → `ProposalVotingPeriodEnded` (#29) as before.
- Vote on non-Active proposal → `ProposalNotActive` (#26) as before.
- Vote on non-existent proposal → `ProposalNotFound` (#25) as before.
- `create_admin_proposal` and `execute_admin_proposal` are unchanged.
- `StaleProposal` guard from issue #864 is unchanged.

---

## Correctness Properties

**Property 1 — Fix checking**: for any input where `isBugCondition(voter, env)` holds,
`vote_admin_proposal'` SHALL return `Err(ContractError::VoterNotEligible)` and SHALL NOT
modify `DataKey::AdminProposal(proposal_id)`.

```
FOR ALL voter WHERE isBugCondition(voter, env) DO
  proposal_before ← env.storage.persistent.get(DataKey::AdminProposal(id))
  result          ← vote_admin_proposal'(voter, id, support)
  ASSERT result = Err(ContractError::VoterNotEligible)
  ASSERT env.storage.persistent.get(DataKey::AdminProposal(id)) = proposal_before
END FOR
```

**Property 2 — Preservation**: for any input where `isBugCondition` does NOT hold,
`vote_admin_proposal'` SHALL produce the same outcome as the original function.

```
FOR ALL voter WHERE NOT isBugCondition(voter, env) DO
  ASSERT vote_admin_proposal_original(voter, id, support) =
         vote_admin_proposal_fixed(voter, id, support)
END FOR
```

---

## Hypothesized Root Cause

`vote_admin_proposal` in `src/governance.rs` only calls `voter.require_auth()` (proving the
caller controls the address) and checks for a duplicate vote. It does not read
`DataKey::Stake(voter)` at all. The `StakeRecord` type and the `Stake` key are available in
`storage_types.rs`, so the eligibility check is a one-liner insertion with no schema changes.

---

## Fix Implementation

### Changes Required

**File: `src/errors.rs`**

Add `VoterNotEligible = 58` after `StaleProposal = 57`. Bump `VARIANT_COUNT` to 58.
Append `ContractError::VoterNotEligible` to `ALL_VARIANTS`.

```rust
// After StaleProposal = 57:
/// Caller is not eligible to vote: no active stake record (issue #865).
VoterNotEligible = 58,
```

**File: `src/governance.rs` — `vote_admin_proposal`**

Insert the eligibility check immediately after `voter.require_auth()` and before the proposal
fetch. Placement before the proposal fetch is intentional: it avoids a persistent storage read
for ineligible callers.

```rust
// After voter.require_auth():

// Issue #865: only active stakers may vote on governance proposals.
// A staker whose unstake is in progress (unstaking_at != 0) is ineligible.
let stake_record: Option<crate::storage_types::StakeRecord> = e
    .storage()
    .persistent()
    .get(&DataKey::Stake(voter.clone()));
match stake_record {
    Some(r) if r.unstaking_at == 0 => {} // eligible — continue
    _ => panic_with_error!(e, ContractError::VoterNotEligible),
}
```

No other changes to `vote_admin_proposal` are needed. All existing guards remain in place and
in their existing order after the new eligibility check.

**File: `docs/authority-model.md`**

Add a section documenting:
- Voter eligibility: active staker (`StakeRecord` present, `unstaking_at == 0`).
- Vote weight: flat (1 vote per eligible address).
- Snapshot point: at vote-cast time.
- Sybil consequence: an attacker must hold locked capital equal to `min_stake` per vote.

---

## Testing Strategy

### Bug Condition Exploration (Task 1)

Write tests that assert the CORRECT (fixed) behavior. These tests FAIL on unfixed code,
proving the bug exists, and PASS after the fix.

- `test_non_staker_cannot_vote`: address with no stake calls `vote_admin_proposal` →
  assert `VoterNotEligible` (#58); proposal vote counts unchanged.
- `test_unstaking_address_cannot_vote`: address that called `unstake()` calls
  `vote_admin_proposal` → assert `VoterNotEligible` (#58).
- **PBT**: for arbitrary addresses with no stake, all calls return `VoterNotEligible`.

### Preservation Tests (Task 2)

Write tests that confirm existing behavior is preserved for eligible voters. These PASS on
both unfixed and fixed code.

- `test_active_staker_can_vote`: staked address votes → recorded, event emitted.
- `test_double_vote_still_rejected`: eligible voter votes twice → `ProposalAlreadyVoted` (#27).
- `test_vote_after_window_still_rejected`: eligible voter votes after end_time →
  `ProposalVotingPeriodEnded` (#29).
- `test_vote_on_cancelled_proposal_still_rejected`: eligible voter votes on cancelled proposal →
  `ProposalNotActive` (#26).
- **PBT**: for any active staker, vote outcome is identical to pre-fix behavior.
