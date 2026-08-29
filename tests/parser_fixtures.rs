use mnemos_collector::parser::LogParser;
use mnemos_collector::protocol::{
    BoosterType, CollectorEvent, GlobalEventType, ItemRarity, ItemType,
};

fn parse_fixture(contents: &str) -> Vec<CollectorEvent> {
    let mut parser = LogParser::default();
    let mut events = Vec::new();

    for line in contents.lines() {
        events.extend(parser.consume_line(line));
    }

    events.extend(parser.flush());
    events
}

#[test]
fn parses_item_drop_fixture_exactly() {
    let events = parse_fixture(include_str!("fixtures/cristalix/item-drop.log"));

    assert_eq!(
        events,
        vec![CollectorEvent::ItemDrop {
            item_name: "Клинок Бури".to_owned(),
            item_type: ItemType::Sword,
            item_rarity: ItemRarity::Mythical,
            dropped_for: "Knalis".to_owned(),
        }]
    );
}

#[test]
fn parses_booster_fixture_exactly() {
    let events = parse_fixture(include_str!("fixtures/cristalix/booster.log"));

    assert_eq!(
        events,
        vec![CollectorEvent::Booster {
            booster_type: BoosterType::Luck,
            activated_by: "Knalis".to_owned(),
        }]
    );
}

#[test]
fn parses_all_global_event_fixture_lines() {
    let events = parse_fixture(include_str!("fixtures/cristalix/global-events.log"));

    assert_eq!(
        events,
        vec![
            CollectorEvent::Global {
                event_type: GlobalEventType::Darkness,
            },
            CollectorEvent::Global {
                event_type: GlobalEventType::Moon,
            },
            CollectorEvent::Global {
                event_type: GlobalEventType::Eclipse,
            },
            CollectorEvent::Global {
                event_type: GlobalEventType::Explosion,
            },
            CollectorEvent::Global {
                event_type: GlobalEventType::CometChaos,
            },
        ]
    );
}

#[test]
fn aggregates_multiline_raid_fixture() {
    let events = parse_fixture(include_str!("fixtures/cristalix/raid.log"));

    assert_eq!(
        events,
        vec![CollectorEvent::Raid {
            locations: vec![1, 3],
        }]
    );
}

#[test]
fn ignores_master_sword_events_while_fixture_is_in_lobby() {
    let events = parse_fixture(include_str!("fixtures/cristalix/reconnect.log"));

    assert_eq!(
        events,
        vec![
            CollectorEvent::Global {
                event_type: GlobalEventType::Moon,
            },
            CollectorEvent::Booster {
                booster_type: BoosterType::Power,
                activated_by: "Knalis".to_owned(),
            },
        ]
    );
}
