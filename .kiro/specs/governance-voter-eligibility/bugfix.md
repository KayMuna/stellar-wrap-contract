# Bugfix Requirements Document

## Introduction

`vote_admin_proposal(e, voter, proposal_id, support)` accepts a vote from any arbitrary address
that can authenticate itself. There is no eligibility gate — any address, including freshly
generated ones, can cast one vote. All votes carry equal weight regardless of stake, wrap holdings,
or any other on-chain property. Because governance proposals gate admin rotation (a high-privilege
action), an open equal-weight vote over arbitrary addresses is not a governance mechanism: an
attacker can generate or control unlimited addresses and pass any proposal unilaterally.

This document captures what is broken, what the correct behavior must be, and what existing
behavior must be preserved unchanged.

## Bug Analysis

### Current Behavior (Defect)

1.1 WHEN an address that holds no stake, no wrap records, and has no special status calls
`vote_admin_proposal` THEN the system records the vote and counts it equally to any other vote

1.2 WHEN any caller-controlled address authenticates and calls `vote_admin_proposal` THEN the system
accepts the vote with no check against a defined voter-eligibility criterion

1.3 WHEN multiple addresses controlled by the same actor each call `vote_admin_proposal` on the
same proposal THEN the system counts each address as one independent vote, allowing a single actor
to accumulate an arbitrary vote tally

1.4 WHEN `vote_admin_proposal` is called THEN the system increments `votes_for` or `votes_against`
by exactly 1 regardless of any on-chain balance, stake, or record held by the voter

1.5 WHEN the eligibility rule for governance voters is requested from the contract's documentation
or code THEN no such rule is stated, enforced, or derivable

### Expected Behavior (Correct)

2.1 WHEN an address that does not satisfy the defined voter-eligibility criterion calls
`vote_admin_proposal` THEN the system SHALL reject the vote with `ContractError::VoterNotEligible`

2.2 WHEN the chosen eligibility criterion is staker-gated THEN the system SHALL only permit
addresses that hold an active (non-unstaking) `StakeRecord` to cast a vote

2.3 WHEN the chosen eligibility criterion is whitelist-gated THEN the system SHALL only permit
addresses that appear in the published Merkle whitelist to cast a vote

2.4 WHEN the chosen eligibility criterion is open (any address) THEN the system documentation
SHALL explicitly state that the vote is sybil-susceptible and that governance decisions are
therefore not sybil-resistant

2.5 WHEN a voter is eligible under whichever criterion is selected THEN the system SHALL record
the vote and count it as before, so that eligible voters are unaffected by the eligibility check

2.6 WHEN the eligibility and weighting model is deployed THEN the contract documentation SHALL
state the eligibility criterion, the vote weight per voter, and (if weighted by a balance) the
snapshot point at which the balance is read

### Unchanged Behavior (Regression Prevention)

3.1 WHEN an eligible voter calls `vote_admin_proposal` on an active proposal within the voting
window THEN the system SHALL CONTINUE TO record the vote and emit the `("gov", "vote")` event

3.2 WHEN the same eligible voter calls `vote_admin_proposal` on the same proposal more than once
THEN the system SHALL CONTINUE TO reject the second call with `ContractError::ProposalAlreadyVoted`

3.3 WHEN any caller calls `vote_admin_proposal` on a proposal whose voting window has closed
THEN the system SHALL CONTINUE TO reject the call with `ContractError::ProposalVotingPeriodEnded`

3.4 WHEN any caller calls `vote_admin_proposal` on a proposal that is not in `Active` status
THEN the system SHALL CONTINUE TO reject the call with `ContractError::ProposalNotActive`

3.5 WHEN any caller calls `vote_admin_proposal` with a `proposal_id` that does not exist
THEN the system SHALL CONTINUE TO reject the call with `ContractError::ProposalNotFound`

3.6 WHEN `create_admin_proposal` is called with a valid duration by any authenticated address
THEN the system SHALL CONTINUE TO create and store the proposal as before

3.7 WHEN `execute_admin_proposal` is called after the voting window ends on a proposal where
`votes_for > votes_against` THEN the system SHALL CONTINUE TO execute the admin rotation (or
enqueue it via the timelock) and set the proposal status to `Executed`

3.8 WHEN `execute_admin_proposal` is called after the voting window ends on a proposal where
`votes_for <= votes_against` THEN the system SHALL CONTINUE TO mark the proposal `Defeated`
without rotating the admin

3.9 WHEN the stale-proposal guard (issue #864) is active and the admin has changed since proposal
creation THEN the system SHALL CONTINUE TO reject execution with `ContractError::StaleProposal`
