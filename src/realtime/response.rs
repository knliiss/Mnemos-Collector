use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use crate::protocol::ObservationState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TransportErrorCode {
    InvalidMessage,
    MessageTooLarge,
    RateLimited,
    TooManyInFlight,
    NotObserving,
    DeliveryUnavailable,
    UnsupportedProtocol,
    UpgradeRequired,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    #[serde(rename = "WELCOME")]
    Welcome {
        #[serde(rename = "protocolVersion")]
        protocol_version: u16,
        #[serde(rename = "minimumCollectorVersion")]
        minimum_collector_version: Option<String>,
    },
    #[serde(rename = "UPGRADE_REQUIRED")]
    UpgradeRequired {
        #[serde(rename = "minimumVersion")]
        minimum_version: String,
        message: Option<String>,
    },
    #[serde(rename = "REPORT_QUEUED")]
    ReportQueued {
        #[serde(rename = "messageId")]
        message_id: Uuid,
        #[serde(rename = "queuedAt")]
        queued_at: DateTime<Utc>,
    },
    #[serde(rename = "COLLECTOR_STATE_UPDATED")]
    CollectorStateUpdated {
        state: ObservationState,
        #[serde(rename = "updatedAt")]
        updated_at: DateTime<Utc>,
    },
    #[serde(rename = "COLLECTOR_UPDATE_SLOT")]
    CollectorUpdateSlot {
        granted: bool,
        #[serde(rename = "retryAfterSeconds")]
        retry_after_seconds: Option<u64>,
    },
    #[serde(rename = "ERROR")]
    Error {
        code: TransportErrorCode,
        message: String,
    },
}
