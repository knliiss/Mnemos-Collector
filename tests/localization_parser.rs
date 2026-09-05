use mnemos_collector::parser::LogParser;
use mnemos_collector::protocol::{
    BoosterType, CollectorEvent, GlobalEventType, ItemRarity, ItemType,
};

fn chat(payload: &str) -> String {
    format!("[12:00:00] [Client thread/INFO]: [CHAT] {payload}")
}

struct LocaleCase {
    raid_open: &'static str,
    raid_close: &'static str,
    darkness: &'static str,
    item_drop: &'static str,
    booster: &'static str,
}

const LOCALE_CASES: &[LocaleCase] = &[
    LocaleCase {
        raid_open: "The gates to the raid \"Forest\" are open (location #17)",
        raid_close: "The gates to the raid \"Forest\" are closed (location #17)",
        darkness: "Darkness comes with the setting sun..",
        item_drop: "ExamplePlayer [#1] dropped \"Secret\" weapon Forest Shadow",
        booster: "ExamplePlayer activated \"Power Booster x2\" for 10m",
    },
    LocaleCase {
        raid_open: "Բացվեցին «Forest» ռեյդի դարպասները (լոկացիա #17)",
        raid_close: "Փակվեցին «Forest» ռեյդի դարպասները (լոկացիա #17)",
        darkness: "Խավարը գալիս է արևի մայրամուտի հետ..",
        item_drop: "ExamplePlayer [#1] խփեց «Գաղտնի» զենք Անտառի ստվեր",
        booster: "ExamplePlayer ակտիվացրեց «Ուժի բուստեր x2» 10m-ով",
    },
    LocaleCase {
        raid_open: "Открылись врата на рейд \"Forest\" (локация #17)",
        raid_close: "Закрылись врата на рейд \"Forest\" (локация #17)",
        darkness: "Тьма наступает с заходом солнца..",
        item_drop: "ExamplePlayer [#1] выбил \"Секретное\" оружие Тень леса",
        booster: "ExamplePlayer активировал \"Бустер силы x2\" на 10m",
    },
    LocaleCase {
        raid_open: "\"Forest\" baskınının kapıları açıldı (lokasyon #17)",
        raid_close: "\"Forest\" baskınının kapıları kapandı (lokasyon #17)",
        darkness: "Karanlık, güneşin batışıyla geliyor..",
        item_drop: "ExamplePlayer [#1] \"Gizli\" silah Orman gölgesi düşürdü",
        booster: "ExamplePlayer \"Güç booster'ı x2\" boosterını 10m süreyle etkinleştirdi",
    },
    LocaleCase {
        raid_open: "Відкрилися врата на рейд \"Forest\" (локація #17)",
        raid_close: "Зачинилися врата на рейд \"Forest\" (локація #17)",
        darkness: "Пітьма настає із заходом сонця..",
        item_drop: "ExamplePlayer [#1] вибив \"Секретне\" зброю Тінь лісу",
        booster: "ExamplePlayer активував \"Бустер сили x2\" на 10m",
    },
    LocaleCase {
        raid_open: "\"Forest\" reydiga darvozalar ochildi (lokatsiya #17)",
        raid_close: "\"Forest\" reydiga darvozalar yopildi (lokatsiya #17)",
        darkness: "Quyosh botishi bilan zulmat keladi..",
        item_drop: "ExamplePlayer [#1] \"Maxfiy\" qurol O'rmon soyasi tushirdi",
        booster: "ExamplePlayer \"Kuch busteri x2\" ni 10m ga faollashtirdi",
    },
];

#[test]
fn parses_supported_sao_locales_from_bundled_fallback() {
    for case in LOCALE_CASES {
        let mut parser = LogParser::default();

        assert_eq!(
            parser.consume_line(&chat(case.raid_open)),
            vec![CollectorEvent::Raid {
                locations: vec![17],
            }],
        );
        assert!(parser.consume_line(&chat(case.raid_close)).is_empty());

        assert_eq!(
            parser.consume_line(&chat(case.darkness)),
            vec![CollectorEvent::Global {
                event_type: GlobalEventType::Darkness,
            }],
        );

        let drop_events = parser.consume_line(&chat(case.item_drop));
        let [
            CollectorEvent::ItemDrop {
                item_name,
                item_type,
                item_rarity,
                dropped_for,
            },
        ] = drop_events.as_slice()
        else {
            panic!("expected one item drop for {}", case.item_drop);
        };

        assert!(!item_name.is_empty());
        assert_eq!(*item_type, ItemType::Sword);
        assert_eq!(*item_rarity, ItemRarity::Secret);
        assert_eq!(dropped_for, "ExamplePlayer");

        assert_eq!(
            parser.consume_line(&chat(case.booster)),
            vec![CollectorEvent::Booster {
                booster_type: BoosterType::Power,
                activated_by: "ExamplePlayer".to_owned(),
            }],
        );
    }
}

#[test]
fn keeps_player_chat_drop_lookalikes_ignored() {
    let mut parser = LogParser::default();

    assert!(
        parser
            .consume_line(&chat(
                "[Master] PlayerOne [#20] » PlayerTwo [#21] dropped \"Secret\" weapon Fake Sword"
            ))
            .is_empty()
    );
}
