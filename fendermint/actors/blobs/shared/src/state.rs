// Copyright 2024 Hoku Contributors
// Copyright 2021-2023 Protocol Labs
// SPDX-License-Identifier: Apache-2.0, MIT

use std::collections::HashMap;
use std::fmt;

use fil_actors_runtime::ActorError;
use fvm_ipld_encoding::tuple::*;
use fvm_shared::address::Address;
use fvm_shared::bigint::BigInt;
use fvm_shared::clock::ChainEpoch;
use serde::{Deserialize, Serialize};

/// The stored representation of a credit account.
#[derive(Clone, Debug, PartialEq, Serialize_tuple, Deserialize_tuple)]
pub struct Account {
    /// Total size of all blobs managed by the account.
    pub capacity_used: BigInt,
    /// Current free credit in byte-blocks that can be used for new commitments.
    pub credit_free: BigInt,
    /// Current committed credit in byte-blocks that will be used for debits.
    pub credit_committed: BigInt,
    /// The chain epoch of the last debit.
    pub last_debit_epoch: ChainEpoch,
    /// Credit approvals to other accounts, keyed by receiver, keyed by caller,
    /// which could be the receiver or a specific contract, like a bucket.
    /// This allows for limiting approvals to interactions from a specific contract.
    /// For example, an approval for Alice might be valid for any contract caller, so long as
    /// the origin is Alice.
    /// An approval for Bob might be valid from only one contract caller, so long as
    /// the origin is Bob.
    pub approvals: HashMap<Address, HashMap<Address, CreditApproval>>,
}

impl Account {
    pub fn new(credit_free: BigInt, current_epoch: ChainEpoch) -> Self {
        Self {
            capacity_used: Default::default(),
            credit_free,
            credit_committed: Default::default(),
            last_debit_epoch: current_epoch,
            approvals: Default::default(),
        }
    }
}

/// A credit approval from one account to another.
#[derive(Debug, Clone, PartialEq, Serialize_tuple, Deserialize_tuple)]
pub struct CreditApproval {
    /// Optional credit approval limit.
    pub limit: Option<BigInt>,
    /// Optional credit approval expiry epoch.
    pub expiry: Option<ChainEpoch>,
    /// Counter for how much credit has been used via this approval.
    pub used: BigInt,
}

/// Blob blake3 hash.
#[derive(
    Clone, Copy, Debug, Default, Hash, PartialEq, Eq, Ord, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Hash(pub [u8; 32]);

/// Source https://github.com/n0-computer/iroh/blob/main/iroh-base/src/hash.rs
impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // the result will be 52 bytes
        let mut res = [b'b'; 52];
        // write the encoded bytes
        data_encoding::BASE32_NOPAD.encode_mut(self.0.as_slice(), &mut res);
        // convert to string, this is guaranteed to succeed
        let t = std::str::from_utf8_mut(res.as_mut()).unwrap();
        // hack since data_encoding doesn't have BASE32LOWER_NOPAD as a const
        t.make_ascii_lowercase();
        // write the str, no allocations
        f.write_str(t)
    }
}

impl TryFrom<&str> for Hash {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let mut res = [0u8; 32];
        data_encoding::BASE32_NOPAD
            .decode_mut(value.as_bytes(), &mut res)
            .map_err(|_| anyhow::anyhow!("invalid hash"))?;
        Ok(Self(res))
    }
}

/// Iroh node public key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PublicKey(pub [u8; 32]);

/// The stored representation of a blob.
#[derive(Clone, Debug, Default, Serialize_tuple, Deserialize_tuple)]
pub struct Blob {
    /// The size of the content.
    pub size: u64,
    /// Blob metadata that contains information for block recovery.
    pub metadata_hash: Hash,
    /// Active subscribers (accounts) that are paying for the blob.
    pub subscribers: HashMap<Address, SubscriptionGroup>,
    /// Blob status.
    pub status: BlobStatus,
}

/// An object used to determine what [`Account`](s) are accountable for a blob, and for how long.
/// Subscriptions allow us to distribute the cost of a blob across multiple accounts that
/// have added the same blob.   
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize_tuple, Deserialize_tuple)]
pub struct Subscription {
    /// Added block.
    pub added: ChainEpoch,
    /// Expiry block.
    pub expiry: ChainEpoch,
    /// Whether to automatically renew the subscription.
    pub auto_renew: bool,
    /// Source Iroh node ID used for ingestion.
    /// This might be unique to each instance of the same blob.
    /// It's included here for record keeping.
    pub source: PublicKey,
    /// The delegate origin and caller that may have created the subscription via a credit approval.
    pub delegate: Option<(Address, Address)>,
    /// Whether the subsciption failed due to an issue resolving the target blob.
    pub failed: bool,
}

/// User-defined identifier used to differentiate blob subscriptions for the same subscriber.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubscriptionId {
    /// Default (empty) ID.
    Default,
    /// Key-based ID.
    Key(Vec<u8>),
}

impl From<Vec<u8>> for SubscriptionId {
    fn from(value: Vec<u8>) -> Self {
        if value.is_empty() {
            SubscriptionId::Default
        } else {
            let key = blake3::hash(&value).as_bytes().to_vec();
            SubscriptionId::Key(key)
        }
    }
}

impl fmt::Display for SubscriptionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SubscriptionId::Default => write!(f, "default"),
            SubscriptionId::Key(key) => write!(f, "{:?}", key),
        }
    }
}

/// A group of subscriptions for the same subscriber.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubscriptionGroup {
    /// Subscription group keys.
    pub subscriptions: HashMap<SubscriptionId, Subscription>,
}

impl SubscriptionGroup {
    /// Returns the current group max expiry and the group max expiry after adding the provided ID
    /// and new value.
    pub fn max_expiries(
        &self,
        target_id: &SubscriptionId,
        new_value: Option<ChainEpoch>,
    ) -> (Option<ChainEpoch>, Option<ChainEpoch>) {
        let mut max = None;
        let mut new_max = None;
        for (id, sub) in self.subscriptions.iter() {
            if sub.failed {
                continue;
            }
            if sub.expiry > max.unwrap_or(0) {
                max = Some(sub.expiry);
            }
            let new_value = if id == target_id {
                new_value.unwrap_or_default()
            } else {
                sub.expiry
            };
            if new_value > new_max.unwrap_or(0) {
                new_max = Some(new_value);
            }
        }
        // Target ID may not be in the current group
        if let Some(new_value) = new_value {
            if new_value > new_max.unwrap_or(0) {
                new_max = Some(new_value);
            }
        }
        (max, new_max)
    }

    /// Returns whether the provided ID corresponds to a subscription that has the minimum
    /// added epoch and the next minimum added epoch in the group.
    pub fn is_min_added(
        &self,
        trim_id: &SubscriptionId,
    ) -> anyhow::Result<(bool, Option<ChainEpoch>), ActorError> {
        let trim = self
            .subscriptions
            .get(trim_id)
            .ok_or(ActorError::not_found(format!(
                "subscription id {} not found",
                trim_id
            )))?;
        let mut next_min = None;
        for (id, sub) in self.subscriptions.iter() {
            if sub.failed || id == trim_id {
                continue;
            }
            if sub.added < trim.added {
                return Ok((false, None));
            }
            if sub.added < next_min.unwrap_or(ChainEpoch::MAX) {
                next_min = Some(sub.added);
            }
        }
        Ok((true, next_min))
    }
}

/// The status of a blob.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum BlobStatus {
    /// Blob is added but not resolving.
    #[default]
    Added,
    /// Blob is pending resolve.
    Pending,
    /// Blob was successfully resolved.
    Resolved,
    /// Blob resolution failed.
    Failed,
}

impl fmt::Display for BlobStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlobStatus::Added => write!(f, "added"),
            BlobStatus::Pending => write!(f, "pending"),
            BlobStatus::Resolved => write!(f, "resolved"),
            BlobStatus::Failed => write!(f, "failed"),
        }
    }
}

/// A request to read a blob data from the Iroh node.
#[derive(Clone, Debug, Serialize_tuple, Deserialize_tuple)]
pub struct ReadRequest {
    pub blob_hash: Hash,
    pub offset: u32,
    pub callback_addr: Address,
    pub callback_method: u64,
}
