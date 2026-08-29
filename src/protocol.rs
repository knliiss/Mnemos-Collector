use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const COLLECTOR_PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ItemType {
    Sword,
    Aura,
    Pet,
    Book,
    Jewelry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ItemRarity {
    Mythical,
    Secret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BoosterType {
    Luck,
    Money,
    Damage,
    Power,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GlobalEventType {
    Darkness,
    Moon,
    Eclipse,
    Explosion,
    CometChaos,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CollectorEvent {
    ItemDrop {
        #[serde(rename = "itemName")]
        item_name: String,
        #[serde(rename = "itemType")]
        item_type: ItemType,
        #[serde(rename = "itemRarity")]
        item_rarity: ItemRarity,
        #[serde(rename = "droppedFor")]
        dropped_for: String,
    },
    Booster {
        #[serde(rename = "boosterType")]
        booster_type: BoosterType,
        #[serde(rename = "activatedBy")]
        activated_by: String,
    },
    Global {
        #[serde(rename = "eventType")]
        event_type: GlobalEventType,
    },
    Raid {
        locations: Vec<u16>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ObservationState {
    Observing,
    Paused,
}

#[derive(Debug, Clone, Serialize)]
pub struct EventReport {
    #[serde(rename = "type")]
    pub message_type: &'static str,
    #[serde(rename = "messageId")]
    pub message_id: Uuid,
    #[serde(rename = "observedAt")]
    pub observed_at: DateTime<Utc>,
    pub event: CollectorEvent,
}

impl EventReport {
    pub fn new(event: CollectorEvent, observed_at: DateTime<Utc>) -> Self {
        Self::with_message_id(Uuid::now_v7(), event, observed_at)
    }

    pub fn with_message_id(
        message_id: Uuid,
        event: CollectorEvent,
        observed_at: DateTime<Utc>,
    ) -> Self {
        Self {
            message_type: "EVENT_REPORT",
            message_id,
            observed_at,
            event,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CollectorStateMessage {
    #[serde(rename = "type")]
    pub message_type: &'static str,
    pub state: ObservationState,
}

impl CollectorStateMessage {
    pub fn observing() -> Self {
        Self {
            message_type: "COLLECTOR_STATE",
            state: ObservationState::Observing,
        }
    }

    pub fn paused() -> Self {
        Self {
            message_type: "COLLECTOR_STATE",
            state: ObservationState::Paused,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CollectorUpdateReadyMessage {
    #[serde(rename = "type")]
    pub message_type: &'static str,
    pub version: String,
}

impl CollectorUpdateReadyMessage {
    pub fn new(version: &str) -> Self {
        Self {
            message_type: "COLLECTOR_UPDATE_READY",
            version: version.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_report_preserves_camel_case_wire_fields() {
        let message_id = Uuid::now_v7();
        let observed_at = Utc::now();
        let report = EventReport::with_message_id(
            message_id,
            CollectorEvent::Global {
                event_type: GlobalEventType::Moon,
            },
            observed_at,
        );
        let json = serde_json::to_value(report).unwrap();

        assert_eq!(json["type"], "EVENT_REPORT");
        assert_eq!(json["messageId"], message_id.to_string());
        assert!(json.get("observedAt").is_some());
        assert!(json.get("observed_at").is_none());
        assert_eq!(json["event"]["kind"], "GLOBAL");
        assert_eq!(json["event"]["eventType"], "MOON");
    }
}
