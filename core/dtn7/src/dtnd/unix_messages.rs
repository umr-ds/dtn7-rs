use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

#[derive(Debug, Clone, Copy, Serialize_repr, Deserialize_repr, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum MessageType {
    #[default]
    GeneralResponse = 1,
    RegisterEID = 2,
    UnregisterEID = 3,
    BundleCreate = 4,
    List = 5,
    ListResponse = 6,
    GetBundle = 7,
    GetBundleResponse = 8,
    GetAllBundles = 9,
    GetAllBundlesResponse = 10,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Message {
    #[serde(rename = "type")]
    pub message_type: MessageType,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GeneralResponse {
    #[serde(flatten)]
    pub message: Message,
    pub success: bool,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RegisterUnregisterMessage {
    #[serde(flatten)]
    pub message: Message,
    pub endpoint_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BundleCreateMessage {
    #[serde(flatten)]
    pub message: Message,
    pub destination_id: String,
    #[serde(with = "serde_bytes")]
    pub payload: Vec<u8>,
    #[serde(default)]
    pub source_id: Option<String>,
    #[serde(default)]
    pub creation_timestamp: Option<String>,
    #[serde(default)]
    pub lifetime: Option<String>,
    #[serde(default)]
    pub report_to: Option<String>,
    #[serde(default)]
    pub bundle_flags: Option<u64>,
    #[serde(default)]
    pub block_flags: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MailboxListMessage {
    #[serde(flatten)]
    pub message: Message,
    pub mailbox: String,
    pub new: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MailboxListResponse {
    #[serde(flatten)]
    pub general_response: GeneralResponse,
    pub bundles: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GetBundleMessage {
    #[serde(flatten)]
    pub message: Message,
    pub mailbox: String,
    pub bundle_id: String,
    pub remove: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BundleContent {
    pub bundle_id: String,
    pub source_id: String,
    pub destination_id: String,
    #[serde(with = "serde_bytes")]
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GetBundleResponse {
    #[serde(flatten)]
    pub general_response: GeneralResponse,
    #[serde(flatten)]
    pub content: BundleContent,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GetAllBundlesMessage {
    #[serde(flatten)]
    pub message: Message,
    pub mailbox: String,
    pub new: bool,
    pub remove: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GetAllBundlesResponse {
    #[serde(flatten)]
    pub general_response: GeneralResponse,
    pub bundles: Vec<BundleContent>,
}
