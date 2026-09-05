use chrono::{TimeZone, Utc};
use mnemos_collector::parser::{GameMode, LogParser};
use mnemos_collector::protocol::{
    BoosterType, CollectorEvent, EventReport, GlobalEventType, ItemRarity, ItemType,
};
use pretty_assertions::assert_eq;

fn chat(payload: &str) -> String {
    format!("[12:00:00] [Client thread/INFO]: [CHAT] {payload}")
}

#[test]
fn parses_full_player_prefix_and_ignores_clan_title_rank_and_top_position() {
    let mut parser = LogParser::default();
    let line = chat(
        " [FOX] [Потусторонний] GOD ┃ ExamplePlayer  [#61] выбил \"Мифическое\" оружие Лесной меч",
    );

    let events = parser.consume_line(&line);

    assert_eq!(
        events,
        vec![CollectorEvent::ItemDrop {
            item_key: None,
            item_name: "Лесной меч".to_owned(),
            item_type: ItemType::Sword,
            item_rarity: ItemRarity::Mythical,
            dropped_for: "ExamplePlayer".to_owned(),
        }],
    );
    assert_eq!(parser.mode(), GameMode::MasterSword);
}

#[test]
fn parses_supported_drop_types_and_grammatical_rarity_forms() {
    let cases = [
        (
            "[Начинающий] PlayerSword [#?] выбил \"Секретное\" оружие Шепот мертвеца",
            ItemType::Sword,
            ItemRarity::Secret,
        ),
        (
            "[Нежить] PlayerPet [#355] выбил \"Мифического\" питомца Дух бездны",
            ItemType::Pet,
            ItemRarity::Mythical,
        ),
        (
            "[Магистр] PlayerRing [#11] выбил \"Мифическую\" реликвию Кольцо света II",
            ItemType::Jewelry,
            ItemRarity::Mythical,
        ),
        (
            "[Магистр] PlayerAura [#12] выбил \"Секретную\" ауру Аура смирения",
            ItemType::Aura,
            ItemRarity::Secret,
        ),
        (
            "[Магистр] PlayerBook [#13] выбил \"Мифическая\" книгу Книга бездны",
            ItemType::Book,
            ItemRarity::Mythical,
        ),
    ];

    for (payload, expected_type, expected_rarity) in cases {
        let mut parser = LogParser::default();
        let events = parser.consume_line(&chat(payload));
        let [
            CollectorEvent::ItemDrop {
                item_type,
                item_rarity,
                ..
            },
        ] = events.as_slice()
        else {
            panic!("expected one item drop for {payload}");
        };

        assert_eq!(*item_type, expected_type);
        assert_eq!(*item_rarity, expected_rarity);
    }
}

#[test]
fn ignores_legendary_drops_and_chat_messages_that_only_look_like_drops() {
    let mut parser = LogParser::default();

    assert!(
        parser
            .consume_line(&chat(
                "[Магистр] PlayerOne [#20] выбил \"Легендарное\" оружие Старый меч"
            ))
            .is_empty()
    );
    assert!(
        parser
            .consume_line(&chat(
                "[Магистр] PlayerOne [#20] » PlayerTwo [#21] выбил \"Секретное\" оружие Ложный меч"
            ))
            .is_empty()
    );
}

#[test]
fn parses_all_supported_booster_types() {
    let cases = [
        ("удачи", BoosterType::Luck),
        ("денег", BoosterType::Money),
        ("урона", BoosterType::Damage),
        ("силы", BoosterType::Power),
    ];

    for (name, expected) in cases {
        let mut parser = LogParser::default();
        let payload = format!("MVP+ ┃ Booster_User активировал \"Бустер {name} x1.25\" на 30м");

        assert_eq!(
            parser.consume_line(&chat(&payload)),
            vec![CollectorEvent::Booster {
                booster_type: expected,
                activated_by: "Booster_User".to_owned(),
            }],
        );
    }
}

#[test]
fn parses_global_event_signatures() {
    let cases = [
        (
            "i [Эвент] » Тьма наступает с заходом солнца..",
            GlobalEventType::Darkness,
        ),
        (
            "i [Эвент] » Дрожь пробегает по вашей спине, когда восходит кровавая луна..",
            GlobalEventType::Moon,
        ),
        (
            "i [Эвент] » Небо темнеет и окутывается глубокой тенью..",
            GlobalEventType::Eclipse,
        ),
        (
            "i [Эвент] » Вы чувствуете, как Ваше сердце бьется быстрее, когда тепло солнца касается вашей кожи..",
            GlobalEventType::Explosion,
        ),
    ];

    for (payload, expected) in cases {
        let mut parser = LogParser::default();

        assert_eq!(
            parser.consume_line(&chat(payload)),
            vec![CollectorEvent::Global {
                event_type: expected,
            }],
        );
    }
}

#[test]
fn collects_only_raid_location_numbers() {
    let mut parser = LogParser::default();

    assert!(
        parser
            .consume_line(&chat("i [Рейд] » Открылись врата на рейды"))
            .is_empty()
    );
    assert!(
        parser
            .consume_line(&chat("\"Темный лес\" (локация #1),"))
            .is_empty()
    );
    assert!(
        parser
            .consume_line(&chat("\"Заброшенная Тюрьма\" (локация #13),"))
            .is_empty()
    );

    assert_eq!(
        parser.flush(),
        vec![CollectorEvent::Raid {
            locations: vec![1, 13],
        }],
    );
}

#[test]
fn parses_real_single_line_raid_announcement() {
    let mut parser = LogParser::default();
    let opening = chat(
        "i [Рейд] » Открылись врата на рейды \"Шиноби\" (локация #3), \"Каньон\" (локация #15), \"Тайпинская башня\" (локация #21), \"Космодрайвер\" (локация #25)",
    );

    assert_eq!(
        parser.consume_line(&opening),
        vec![CollectorEvent::Raid {
            locations: vec![3, 15, 21, 25],
        }],
    );
    assert_eq!(parser.mode(), GameMode::MasterSword);

    let closing = chat(
        "i [Рейд] » Закрылись врата на рейды \"Шиноби\" (локация #3), \"Каньон\" (локация #15), \"Тайпинская башня\" (локация #21), \"Космодрайвер\" (локация #25)",
    );

    assert!(parser.consume_line(&closing).is_empty());
}

#[test]
fn parses_singular_raid_open_and_ignores_singular_raid_close_with_escaped_newlines() {
    let mut parser = LogParser::default();
    let opening = chat(
        "[Рейд] » Открылись врата на рейд \"Темный лес\" (локация #1)\\nОткрылись врата на рейд \"Луга с обелисками\" (локация #16)\\nОткрылись врата на рейд \"Подводный храм\" (локация #17)",
    );

    assert_eq!(
        parser.consume_line(&opening),
        vec![CollectorEvent::Raid {
            locations: vec![1, 16, 17],
        }],
    );
    assert_eq!(parser.mode(), GameMode::MasterSword);

    let closing = chat(
        "[Рейд] » Закрылись врата на рейд \"Темный лес\" (локация #1)\\nЗакрылись врата на рейд \"Луга с обелисками\" (локация #16)\\nЗакрылись врата на рейд \"Подводный храм\" (локация #17)",
    );

    assert!(parser.consume_line(&closing).is_empty());
}

#[test]
fn blocks_events_in_other_modes_and_accepts_them_after_master_sword_join() {
    let mut parser = LogParser::default();
    let drop = chat("[Магистр] PlayerOne [#20] выбил \"Мифическое\" оружие Лесной меч");

    parser.consume_line("[INFO] Joining server Хаб");
    assert_eq!(parser.mode(), GameMode::Other);
    assert!(parser.consume_line(&drop).is_empty());

    parser.consume_line("[INFO] Joining server Мастера Мечей #2");
    assert_eq!(parser.mode(), GameMode::MasterSword);
    assert_eq!(parser.consume_line(&drop).len(), 1);

    parser.consume_line("[INFO] Joining server Мастера Мечей Лобби");
    assert_eq!(parser.mode(), GameMode::MasterSwordLobby);
    assert!(parser.consume_line(&drop).is_empty());
}

#[test]
fn serializes_event_report_with_the_existing_mnemos_contract_names() {
    let observed_at = Utc.with_ymd_and_hms(2026, 8, 23, 17, 0, 0).unwrap();
    let report = EventReport::new(
        CollectorEvent::Booster {
            booster_type: BoosterType::Money,
            activated_by: "ExamplePlayer".to_owned(),
        },
        observed_at,
    );

    let value = serde_json::to_value(report).unwrap();

    assert_eq!(value["type"], "EVENT_REPORT");
    assert_eq!(value["observedAt"], "2026-08-23T17:00:00Z");
    assert_eq!(value["event"]["kind"], "BOOSTER");
    assert_eq!(value["event"]["boosterType"], "MONEY");
    assert_eq!(value["event"]["activatedBy"], "ExamplePlayer");
    assert!(value["messageId"].is_string());
}
