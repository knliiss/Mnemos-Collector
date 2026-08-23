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
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
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
    #[serde(rename = "ERROR")]
    Error {
        code: TransportErrorCode,
        message: String,
    },
}
