use mnemos_collector::protocol::ObservationState;
use mnemos_collector::realtime::{ServerMessage, TransportErrorCode};

#[test]
fn parses_report_queued_response() {
    let message: ServerMessage = serde_json::from_str(
        r#"{
            "type": "REPORT_QUEUED",
            "messageId": "019c1129-ef54-7000-8000-000000000221",
            "queuedAt": "2026-08-23T17:00:00Z"
        }"#,
    )
    .unwrap();

    match message {
        ServerMessage::ReportQueued { message_id, .. } => {
            assert_eq!(
                message_id.to_string(),
                "019c1129-ef54-7000-8000-000000000221"
            );
        }
        other => panic!("unexpected server message: {other:?}"),
    }
}

#[test]
fn parses_collector_state_updated_response() {
    let message: ServerMessage = serde_json::from_str(
        r#"{
            "type": "COLLECTOR_STATE_UPDATED",
            "state": "PAUSED",
            "updatedAt": "2026-08-23T17:00:00Z"
        }"#,
    )
    .unwrap();

    match message {
        ServerMessage::CollectorStateUpdated { state, .. } => {
            assert_eq!(state, ObservationState::Paused);
        }
        other => panic!("unexpected server message: {other:?}"),
    }
}

#[test]
fn parses_transport_error_response() {
    let message: ServerMessage = serde_json::from_str(
        r#"{
            "type": "ERROR",
            "code": "NOT_OBSERVING",
            "message": "Collector must enter OBSERVING state before reporting events"
        }"#,
    )
    .unwrap();

    match message {
        ServerMessage::Error { code, .. } => {
            assert_eq!(code, TransportErrorCode::NotObserving);
        }
        other => panic!("unexpected server message: {other:?}"),
    }
}
