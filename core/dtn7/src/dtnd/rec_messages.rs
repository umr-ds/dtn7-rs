use bp7::{
    CanonicalBlock, CanonicalBlockBuilder, CanonicalBlockType, CanonicalData,
    flags::BlockControlFlags,
};
use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

pub const REC_JOB_QUERY_BLOCK: CanonicalBlockType = 1001;
pub const REC_JOB_REPLY_BLOCK: CanonicalBlockType = 1002;

pub struct RecJobQuery {
    pub submitter: String,
}

impl RecJobQuery {
    pub fn to_canonical_block(&self, block_number: u64, bcf: BlockControlFlags) -> CanonicalBlock {
        CanonicalBlockBuilder::default()
            .block_type(REC_JOB_QUERY_BLOCK)
            .block_number(block_number)
            .block_control_flags(bcf.bits())
            .data(CanonicalData::Unknown(
                serde_cbor::to_vec(&self.submitter).unwrap(),
            ))
            .build()
            .unwrap()
    }

    pub fn from_canonical_block(cb: &CanonicalBlock) -> Option<Self> {
        if cb.block_type != REC_JOB_QUERY_BLOCK {
            return None;
        }
        match &cb.data() {
            CanonicalData::Unknown(data) => {
                let submitter: String = serde_cbor::from_slice(data).ok()?;
                Some(RecJobQuery { submitter })
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize_repr, Deserialize_repr, PartialEq, Eq)]
#[repr(u8)]
pub enum RecNodeType {
    NTypeBroker = 1,
    NTypeExecutor = 2,
    NTypeDataStore = 3,
    NTypeClient = 4,
}

#[derive(Debug, Clone, Copy, Serialize_repr, Deserialize_repr, PartialEq, Eq)]
#[repr(u8)]
pub enum BundleType {
    JobsQuery = 1,
    JobsReply = 2,
}

#[derive(Debug, Clone, Copy, Serialize_repr, Deserialize_repr, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    Reply = 1,
    Register = 2,
    Fetch = 3,
    FetchReply = 4,
    BundleCreate = 5,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    #[serde(rename = "type")]
    pub message_type: MessageType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reply {
    #[serde(flatten)]
    pub message: Message,
    pub success: bool,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Register {
    #[serde(flatten)]
    pub message: Message,
    pub endpoint_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fetch {
    #[serde(flatten)]
    pub message: Message,
    pub endpoint_id: String,
    pub node_type: RecNodeType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchReply {
    #[serde(flatten)]
    pub reply: Reply,
    pub bundles: Vec<BundleData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleCreate {
    #[serde(flatten)]
    pub message: Message,
    pub bundle: BundleData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleData {
    #[serde(rename = "type")]
    pub bundle_type: BundleType,
    pub source: String,
    pub destination: String,
    #[serde(with = "serde_bytes")]
    pub payload: Vec<u8>,
    #[serde(default)]
    pub submitter: Option<String>,
}
