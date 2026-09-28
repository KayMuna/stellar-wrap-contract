# Bugfix Requirements Document

## Introduction

A governance proposal carries the address of a `proposed_admin` chosen at proposal-creation time and
records that address permanently in the `AdminProposal` struct. Nothing binds the proposal to the
admin identity that was in effect when it was created. If the contract admin changes between
proposal creation and execution — through `update_admin`, through a timelocked `SetAdmin` operation,
or through the execution of a different governance proposal — the original proposal can still be
executed and will silently revert the contract to a superseded admin address.

This creates two distinct failure modes:

1. **Stale-admin execution**: a proposal created under admin A is executed after admin B is already
   in place, overwriting B with the stale address from the proposal.
2. **Concurrent-proposal race**: two proposals are open simultaneously, both pass, and the second
   one to execute overwrites the effect of the first.

Both modes leave the contract in an admin state that no currently-authorised party voted on.
This bug was filed as issue #864 and shares the stale-action problem with issue #672 (timelock
grace-period expiry).

---

## Bug Analysis

### Current Behavior (Defect)

1.1 WHEN a governance proposal is created under admin A and the contract admin is changed to admin B
    before the proposal is executed THEN the system executes the stale proposal and replaces admin B
    with the originally proposed address, reverting to a superseded admin state.

1.2 WHEN two governance proposals are open simultaneously and both accumulate enough votes to pass
    THEN the system executes whichever proposal is called second, overwriting the admin change
    already applied by the first execution.

1.3 WHEN a governance proposal is created and a `timelock_execute` call for a `SetAdmin` action
    completes before the proposal's voting period ends THEN the system allows the governance
    proposal to execute later and overrides the timelocked admin rotation.

1.4 WHEN `execute_admin_proposal` is called on a passing proposal THEN the system does not compare
    the proposal's `proposed_admin` against the current admin snapshot; it writes the stored
    `proposed_admin` unconditionally without verifying it is still consistent with the admin state
    at creation time.

### Expected Behavior (Correct)

2.1 WHEN a governance proposal is created under admin A and the contract admin is changed to a
    different address before the proposal is executed THEN the system SHALL reject the execution
    with a `StaleProposal` error and leave the current admin unchanged.

2.2 WHEN two governance proposals are open simultaneously and the first one executes successfully
    THEN the system SHALL mark any remaining open proposals as stale, and subsequent calls to
    `execute_admin_proposal` on those proposals SHALL fail with a `StaleProposal` error.

2.3 WHEN a `timelock_execute` for `SetAdmin` completes while a governance proposal is still open
    THEN the system SHALL invalidate any open proposals whose creation-time admin snapshot no
    longer matches the current admin, and those proposals SHALL NOT be executable.

2.4 WHEN `create_admin_proposal` is called THEN the system SHALL record the current admin address
    as an immutable snapshot on the proposal, so that `execute_admin_proposal` can detect admin
    drift before applying any state change.

### Unchanged Behavior (Regression Prevention)

3.1 WHEN a governance proposal is created and no admin change occurs before execution THEN the
    system SHALL CONTINUE TO execute the proposal successfully and update the admin to the
    proposed address.

3.2 WHEN a governance proposal is defeated (more votes against than for) THEN the system SHALL
    CONTINUE TO mark the proposal as `Defeated` and leave the admin unchanged.

3.3 WHEN a governance proposal is cancelled by the proposer or current admin before voting ends
    THEN the system SHALL CONTINUE TO mark the proposal as `Cancelled` and prevent further
    execution.

3.4 WHEN a voter attempts to vote on a proposal after the voting period has ended THEN the system
    SHALL CONTINUE TO reject the vote with `ProposalVotingPeriodEnded`.

3.5 WHEN `execute_admin_proposal` is called before the voting period has ended THEN the system
    SHALL CONTINUE TO reject the call with `ProposalVotingPeriodNotEnded`.

3.6 WHEN `execute_admin_proposal` is called on a proposal whose status is not `Active` THEN the
    system SHALL CONTINUE TO reject the call with `ProposalNotActive`.

3.7 WHEN the timelock is enabled and a governance proposal passes THEN the system SHALL CONTINUE TO
    route the admin change through `timelock_schedule(SetAdmin(...))` rather than applying it
    immediately.

3.8 WHEN a single governance proposal is open and no concurrent proposals or out-of-band admin
    changes exist THEN the system SHALL CONTINUE TO behave exactly as before this fix for the full
    create → vote → execute lifecycle.

---

## Bug Condition

**Bug Condition Function** — identifies inputs that trigger the stale-proposal bug:

```pascal
FUNCTION isBugCondition(proposal, env)
  INPUT: proposal of type AdminProposal, env of type contract state
  OUTPUT: boolean

  current_admin ← env.storage.instance.get(DataKey::Admin)
  creation_admin ← proposal.admin_at_creation   // field introduced by this fix

  RETURN current_admin ≠ creation_admin
END FUNCTION
```

**Property: Fix Checking** — stale proposals must not execute:

```pascal
FOR ALL proposal WHERE isBugCondition(proposal, env) DO
  result ← execute_admin_proposal'(proposal.id)
  ASSERT result = Err(ContractError::StaleProposal)
  ASSERT env.storage.instance.get(DataKey::Admin) IS UNCHANGED
END FOR
```

**Property: Preservation Checking** — non-stale proposals must behave identically to the original:

```pascal
FOR ALL proposal WHERE NOT isBugCondition(proposal, env) DO
  ASSERT execute_admin_proposal'(proposal) = execute_admin_proposal(proposal)
END FOR
```
