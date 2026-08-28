use chrono::{TimeZone, Utc};
use mnemos_collector::parser::{GameMode, LogParser};
use mnemos_collector::protocol::{CollectorEvent, EventReport, GlobalEventType};
use pretty_assertions::assert_eq;

const COMET_CHAOS_LOG_LINE: &str = "[17:59:59] [Client thread/INFO] [CHAT] i [Событие] » По мере того, как комета проносится по небу, вас охватывает чувство надвигающегося хаоса...";

#[test]
fn parses_real_comet_chaos_announcement() {
    let mut parser = LogParser::default();

    let events = parser.consume_line(COMET_CHAOS_LOG_LINE);

    assert_eq!(
        events,
        vec![CollectorEvent::Global {
            event_type: GlobalEventType::CometChaos,
        }],
    );
    assert_eq!(parser.mode(), GameMode::MasterSword);
}

#[test]
fn context_scan_recovers_master_sword_from_comet_chaos_activity() {
    let mut parser = LogParser::default();

    parser.consume_context_line(COMET_CHAOS_LOG_LINE);

    assert_eq!(parser.mode(), GameMode::MasterSword);
}

#[test]
fn serializes_comet_chaos_with_backend_contract_name() {
    let observed_at = Utc.with_ymd_and_hms(2026, 8, 28, 14, 59, 59).unwrap();
    let report = EventReport::new(
        CollectorEvent::Global {
            event_type: GlobalEventType::CometChaos,
        },
        observed_at,
    );

    let value = serde_json::to_value(report).unwrap();

    assert_eq!(value["event"]["kind"], "GLOBAL");
    assert_eq!(value["event"]["eventType"], "COMET_CHAOS");
}
