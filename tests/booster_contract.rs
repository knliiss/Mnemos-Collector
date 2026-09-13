use mnemos_collector::parser::LogParser;
use mnemos_collector::protocol::{BoosterType, CollectorEvent};

#[test]
fn parses_current_russian_global_booster_contract() {
    let mut parser = LogParser::default();
    let line = r#"[12:00:00] [Client thread/INFO]: [CHAT] MVP+ ┃ Booster_User активировал "Бустер силы x1.5" на 30м"#;

    assert_eq!(
        parser.consume_line(line),
        vec![CollectorEvent::Booster {
            booster_type: BoosterType::Power,
            activated_by: "Booster_User".to_owned(),
        }],
    );
}
