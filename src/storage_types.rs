//! Shared on-chain storage types for the Stellar Wrap Registry contract.
//!
//! All `#[contracttype]` structs and enums that are persisted to Soroban
//! storage are defined here so every module imports from a single canonical
//! location. The `DataKey` enum lists every distinct storage key used by the
//! contract, which keeps key-space collisions impossible and makes storage
//! layout auditing straightforward.
//!
//! ## Storage layout (instance vs persistent vs temporary)
//!
//! | Key variant | Storage tier | Notes |
//! |---|---|---|
//! | `Admin`, `AdminPubKey`, `AdminProposalCount` | instance | contract-wide config |
//! | `TimelockDelay`, `TimelockOps`, `TimelockOp` | instance / persistent | controller state |
//! | `Paused`, `WhitelistRoot`, `TransferFee` | instance | flags / config |
//! | `Wrap`, `WrapCount`, `WrapPeriods`, `UserPeriods`, `LatestPeriod` | persistent | per-user wrap state |
//! | `AdminProposal`, `AdminProposalVote` | persistent | governance |
//! | `Stake`, `StakeConfig`, `TotalStaked` | persistent / instance | staking |
//! | `TransferGuard` | temporary | re-entrancy guard |

use soroban_sdk::{contracttype, Address, BytesN, Bytes, Env, String, Symbol, Vec};

// ── Storage keys ─────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    // Admin / ownership
    Admin,
    AdminPubKey,
    PendingAdmin,
    ContractVersion,
    SchemaVersion,
    MigrationVersion,

    // Pause
    Paused,

    // Wrap records
    Wrap(Address, u64),
    WrapCount(Address),
    WrapPeriods(Address),
    UserPeriods(Address),
    LatestPeriod(Address),
    TotalWrapCount,
    TotalRevoked,
    LastUpdated(Address),

    // Expiration
    ExpirationDuration,

    // Opt-out
    OptOut(Address),

    // Token metadata
    Name,
    Symbol,

    // Transfer
    TransferFee,
    TransferGuard,

    // Storage accounting
    StorageBytes,
    FeeParams,

    // Governance proposals
    AdminProposalCount,
    AdminProposal(u64),
    AdminProposalVote(u64, Address),

    // Timelock controller
    TimelockDelay,
    TimelockOp(BytesN<32>),
    TimelockOps,

    // Whitelist (merkle)
    WhitelistRoot,

    // Bridge
    BridgeRelayer,
    BridgeRelayerSet(u32),
    BridgeChainStatus(u32),
    OutboundBridgeNonce,
    OutboundBridgeRequest(u64),
    InboundBridgeRecord(u32, u64),
    InboundBridgeProcessed(u32, u64),

    // Staking
    Stake(Address),
    StakeConfig,
    TotalStaked,
}

// ── Wrap record ───────────────────────────────────────────────────────────────

/// All possible lifecycle states of a wrap record.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WrapState {
    /// Initial state: wrap minted but not yet verified.
    Draft,
    /// Verification in progress.
    Pending,
    /// Wrap is verified and active.
    Active,
    /// Wrap has been bridged to another chain.
    Bridged,
    /// Wrap was cancelled before verification.
    Cancelled,
    /// Wrap expired without verification.
    Expired,
    /// Wrap was archived / permanently deactivated.
    Archived,
}

/// Finite-state machine tracking a wrap record's lifecycle.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WrapLifecycleFSM {
    pub state: WrapState,
    /// Ledger timestamp of the last state transition.
    pub updated_at: u64,
}

impl WrapLifecycleFSM {
    /// Create a new FSM in `initial_state` at ledger time `now`.
    pub fn new(initial_state: WrapState, now: u64) -> Self {
        Self { state: initial_state, updated_at: now }
    }

    /// Attempt a transition to `next`. Returns `true` on success, `false` if
    /// the transition is invalid from the current state.
    pub fn transition_to(&mut self, next: WrapState, now: u64) -> bool {
        let valid = match self.state {
            WrapState::Draft    => matches!(next, WrapState::Pending | WrapState::Cancelled | WrapState::Expired),
            WrapState::Pending  => matches!(next, WrapState::Active | WrapState::Cancelled | WrapState::Expired),
            WrapState::Active   => matches!(next, WrapState::Bridged | WrapState::Archived | WrapState::Expired),
            WrapState::Bridged  => matches!(next, WrapState::Active),
            WrapState::Cancelled | WrapState::Expired | WrapState::Archived => false,
        };
        if valid {
            self.state = next;
            self.updated_at = now;
        }
        valid
    }

    /// Restore a bridged wrap back to `Active` (refund path).
    pub fn restore_from_bridge(&mut self, now: u64) -> bool {
        if self.state == WrapState::Bridged {
            self.state = WrapState::Active;
            self.updated_at = now;
            true
        } else {
            false
        }
    }
}

/// An immutable on-chain record of a wrap commitment.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WrapRecord {
    /// Ledger timestamp when the wrap was minted.
    pub timestamp: u64,
    /// SHA-256 hash of the off-chain data being committed.
    pub data_hash: BytesN<32>,
    /// Archetype label (e.g. `"gold"`, `"silver"`).
    pub archetype: Symbol,
    /// Period in `YYYYMM` format.
    pub period: u64,
    /// Lifecycle state machine.
    pub fsm: WrapLifecycleFSM,
    /// Optional human-readable description (max 256 bytes).
    pub description: Option<String>,
    /// Optional image URL (max 2048 bytes).
    pub image_url: Option<String>,
}

/// Aggregate summary of a user's wraps.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WrapSummary {
    pub total_wraps: u32,
    pub periods: Vec<u64>,
    pub archetypes: Vec<Symbol>,
    pub first_period: u64,
    pub latest_period: u64,
}

// ── Governance proposals ──────────────────────────────────────────────────────

/// Status of an admin governance proposal.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProposalStatus {
    Active,
    Executed,
    Defeated,
    Cancelled,
}

/// An on-chain admin governance proposal.
///
/// `admin_at_creation` is an immutable snapshot of `DataKey::Admin` taken
/// when the proposal was created. `execute_admin_proposal` compares the
/// current admin against this snapshot to detect admin drift (issue #864).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminProposal {
    pub id: u64,
    pub proposer: Address,
    pub proposed_admin: Address,
    /// Snapshot of the contract admin at proposal-creation time (issue #864).
    pub admin_at_creation: Address,
    pub votes_for: u64,
    pub votes_against: u64,
    pub start_time: u64,
    pub end_time: u64,
    pub status: ProposalStatus,
}

// ── Timelock controller ───────────────────────────────────────────────────────

/// An action that can be queued and executed via the timelock controller.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TimelockAction {
    SetAdmin(Address),
    SetAdminPubKey(BytesN<32>),
    Upgrade(BytesN<32>),
    SetWhitelistRoot(BytesN<32>),
    SetTimelockDelay(u64),
    SetBridgeRelayers(u32, BridgeRelayerSet),
}

/// A scheduled timelock operation.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimelockOperation {
    pub action: TimelockAction,
    /// Earliest ledger timestamp at which the operation may execute.
    pub eta: u64,
    pub scheduled_at: u64,
}

// ── Transfer fee ──────────────────────────────────────────────────────────────

/// Configuration for the token-denominated wrap-transfer fee.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferFeeConfig {
    pub token: Address,
    pub recipient: Address,
    pub amount: i128,
}

// ── Batch minting ─────────────────────────────────────────────────────────────

/// A single item in a batch mint call.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchWrapItem {
    pub user: Address,
    pub period: u64,
    pub archetype: Symbol,
    pub data_hash: BytesN<32>,
    pub payload_version: u32,
    pub signature: BytesN<64>,
}

// ── Bridge types ──────────────────────────────────────────────────────────────

/// A set of authorized bridge relayer public keys for a given chain.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BridgeRelayerSet {
    pub relayers: Vec<BytesN<32>>,
    pub threshold: u32,
}

/// A record of an outbound bridge transfer initiated by a user.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutboundBridgeRequest {
    pub nonce: u64,
    pub sender: Address,
    pub destination_chain: u32,
    pub recipient_address: Bytes,
    pub period: u64,
    pub archetype: Symbol,
    pub data_hash: BytesN<32>,
    pub timestamp: u64,
}

/// A record of an inbound bridge transfer fulfilled by a relayer.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InboundBridgeRecord {
    pub source_chain: u32,
    pub source_nonce: u64,
    pub recipient: Address,
    pub period: u64,
    pub archetype: Symbol,
    pub data_hash: BytesN<32>,
    pub timestamp: u64,
}

// ── Staking ───────────────────────────────────────────────────────────────────

/// Configuration for the staking mechanism.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StakeConfig {
    pub min_stake: i128,
    pub cooldown_seconds: u64,
    pub priority_multiplier_bps: u32,
    pub max_priority_bps: u32,
}

/// A user's staking record.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StakeRecord {
    pub amount: i128,
    pub staked_at: u64,
    /// Non-zero while an unstake is in progress; zero otherwise.
    pub unstaking_at: u64,
}

// ── Contract health / invariant reports ──────────────────────────────────────

/// High-level contract health status returned by `health()`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractHealth {
    pub initialized: bool,
    pub has_admin: bool,
    pub has_signing_key: bool,
}

/// Detailed invariant report returned by `check_user_invariants()`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvariantReport {
    pub wrap_count_matches_periods: bool,
    pub latest_period_valid: bool,
    pub observed_count: u32,
    pub observed_periods_len: u32,
    pub observed_latest_period: u64,
}

// ── Storage-fee accounting ────────────────────────────────────────────────────

/// Parameters for the on-chain storage-fee model.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FeeParams {
    pub base_fee: i128,
    pub per_kib_fee: i128,
    pub scale_step_kib: u64,
    pub max_fee: i128,
}
