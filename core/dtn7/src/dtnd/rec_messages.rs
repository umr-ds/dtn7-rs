use bp7::{
    CanonicalBlock, CanonicalBlockBuilder, CanonicalBlockType, CanonicalData,
    flags::BlockControlFlags,
};
use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

/// REC Bundle Type Block Type
pub const REC_BUNDLE_TYPE_BLOCK_TYPE: CanonicalBlockType = 1000;

/// REC Node Types.
#[derive(Debug, Clone, Copy, Serialize_repr, Deserialize_repr, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum NodeType {
    #[default]
    None = 0,
    Broker = 1,
    Executor = 2,
    DataStore = 3,
    Client = 4,
}

/// REC Bundle Types.
#[derive(Debug, Clone, Copy, Serialize_repr, Deserialize_repr, PartialEq, Eq)]
#[repr(u8)]
pub enum BundleType {
    // 1-10: Broker discovery
    BrokerAnnounce = 1,
    BrokerRequest = 2,
    BrokerAck = 3,

    // 11-20: Jobs
    JobSubmit = 11,
    JobResult = 12,
    JobQuery = 13,
    JobList = 14,

    // 21-30: Named Data
    NDataPut = 21,
    NDataGet = 22,
    NDataDel = 23,
}

/// REC Message Types.
#[derive(Debug, Clone, Copy, Serialize_repr, Deserialize_repr, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    Reply = 1,
    Register = 2,
    Fetch = 3,
    FetchReply = 4,
    BundleCreate = 5,
}

/// Base message.
///
/// Used to identify the message type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    #[serde(rename = "type")]
    pub message_type: MessageType,
}

/// `Reply` message (`MessageType::Reply`).
///
/// Used to acknowledge other messages with success or error information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reply {
    #[serde(flatten)]
    pub header: Message,
    pub success: bool,
    #[serde(default)]
    pub error: String,
}

/// `Register` message (`MessageType::Register`).
///
/// Used to register an endpoint ID with the REC daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Register {
    #[serde(flatten)]
    pub header: Message,
    pub endpoint_id: String,
}

/// `Fetch` message (`MessageType::Fetch`).
///
/// Used to request REC bundles for a specific endpoint ID and node type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fetch {
    #[serde(flatten)]
    pub header: Message,
    pub endpoint_id: String,
    pub node_type: NodeType,
}

/// `FetchReply` message (`MessageType::FetchReply`).
///
/// Used to reply to `Fetch` messages with a list of REC bundles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchReply {
    #[serde(flatten)]
    pub header: Message,
    pub success: bool,
    #[serde(default)]
    pub error: String,
    pub bundles: Vec<BundleData>,
}

/// `BundleCreate` message (`MessageType::BundleCreate`).
///
/// Used to create and send REC bundles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleCreate {
    #[serde(flatten)]
    pub header: Message,
    pub bundle: BundleData,
}

/// The payload of a REC message bundle.
///
/// Holds the actual data for various REC bundle types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleData {
    #[serde(rename = "type")]
    pub bundle_type: BundleType,
    pub source: String,
    pub destination: String,
    #[serde(default, with = "serde_bytes", skip_serializing_if = "Vec::is_empty")]
    pub payload: Vec<u8>,
    #[serde(default)]
    pub success: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    // Used by broker discovery
    #[serde(default, skip_serializing_if = "is_node_type_none")]
    pub node_type: NodeType,
    // Used by job query/list
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub submitter: String,
    // Used by named data
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub named_data: String,
}

fn is_node_type_none(node_type: &NodeType) -> bool {
    *node_type == NodeType::None
}

impl Default for BundleData {
    fn default() -> Self {
        BundleData {
            bundle_type: BundleType::BrokerAnnounce,
            source: String::new(),
            destination: String::new(),
            payload: Vec::new(),
            success: true,
            error: String::new(),
            node_type: NodeType::None,
            submitter: String::new(),
            named_data: String::new(),
        }
    }
}

/// REC Bundle Type extension block.
///
/// Holds the `BundleType` as a single byte in a CBOR array.
pub struct RecBundleTypeBlock(pub u8);

impl RecBundleTypeBlock {
    pub fn new(bundle_type: BundleType) -> Self {
        RecBundleTypeBlock(bundle_type as u8)
    }

    /// Convert the `RecBundleTypeBlock` into a `CanonicalBlock`.
    ///
    /// * `block_number` - block number to use.
    /// * `bcf` - `BlockControlFlags` to set.
    pub fn to_canonical_block(&self, block_number: u64, bcf: BlockControlFlags) -> CanonicalBlock {
        // Encode as CBOR array with single element
        let cbor_data = serde_cbor::to_vec(&vec![self.0 as u64]).unwrap();
        CanonicalBlockBuilder::default()
            .block_type(REC_BUNDLE_TYPE_BLOCK_TYPE)
            .block_number(block_number)
            .block_control_flags(bcf.bits())
            .data(CanonicalData::Unknown(cbor_data))
            .build()
            .unwrap()
    }

    /// Create a `RecBundleTypeBlock` from a `CanonicalBlock`.
    ///
    /// Returns `None` if the block is not of the correct type or cannot be parsed.
    pub fn from_canonical_block(cb: &CanonicalBlock) -> Option<Self> {
        if cb.block_type != REC_BUNDLE_TYPE_BLOCK_TYPE {
            return None;
        }
        match &cb.data() {
            CanonicalData::Unknown(data) => {
                // Decode CBOR array with single element
                let arr: Vec<u64> = serde_cbor::from_slice(data).ok()?;
                if arr.len() == 1 {
                    Some(RecBundleTypeBlock(arr[0] as u8))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Get the `BundleType` represented by this block.
    ///
    /// Returns `None` if the value does not correspond to a valid `BundleType`.
    pub fn bundle_type(&self) -> Option<BundleType> {
        match self.0 {
            1 => Some(BundleType::BrokerAnnounce),
            2 => Some(BundleType::BrokerRequest),
            3 => Some(BundleType::BrokerAck),
            11 => Some(BundleType::JobSubmit),
            12 => Some(BundleType::JobResult),
            13 => Some(BundleType::JobQuery),
            14 => Some(BundleType::JobList),
            21 => Some(BundleType::NDataPut),
            22 => Some(BundleType::NDataGet),
            23 => Some(BundleType::NDataDel),
            _ => None,
        }
    }
}
