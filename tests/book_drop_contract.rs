use chrono::{TimeZone, Utc};
use mnemos_collector::parser::{GameMode, LogParser};
use mnemos_collector::protocol::{CollectorEvent, EventReport, ItemRarity, ItemType};
use pretty_assertions::assert_eq;

const BOOK_DROP_LOG_LINE: &str = "[18:43:12] [Client thread/INFO]: [CHAT]  [GOD] Барлок НЕ | zetopikk_nyan_ [#1] выбил \"Мифическая\" книгу Темная жатва";

#[test]
fn parses_real_book_drop_with_ascii_player_separator() {
    let mut parser = LogParser::default();

    let events = parser.consume_line(BOOK_DROP_LOG_LINE);

    assert_eq!(
        events,
        vec![CollectorEvent::ItemDrop {
            item_key: None,
            item_name: "Темная жатва".to_owned(),
            item_type: ItemType::Book,
            item_rarity: ItemRarity::Mythical,
            dropped_for: "zetopikk_nyan_".to_owned(),
        }],
    );
    assert_eq!(parser.mode(), GameMode::MasterSword);
}

#[test]
fn serializes_book_drop_with_backend_contract_name() {
    let observed_at = Utc.with_ymd_and_hms(2026, 8, 28, 15, 43, 12).unwrap();
    let report = EventReport::new(
        CollectorEvent::ItemDrop {
            item_key: None,
            item_name: "Темная жатва".to_owned(),
            item_type: ItemType::Book,
            item_rarity: ItemRarity::Mythical,
            dropped_for: "zetopikk_nyan_".to_owned(),
        },
        observed_at,
    );

    let value = serde_json::to_value(report).unwrap();

    assert_eq!(value["event"]["kind"], "ITEM_DROP");
    assert_eq!(value["event"]["itemType"], "BOOK");
    assert_eq!(value["event"]["itemRarity"], "MYTHICAL");
    assert_eq!(value["event"]["droppedFor"], "zetopikk_nyan_");
    assert_eq!(value["event"]["itemName"], "Темная жатва");
}
