use mnemos_collector::parser::LogParser;
use mnemos_collector::protocol::CollectorEvent;
use pretty_assertions::assert_eq;

fn chat(payload: &str) -> String {
    format!("[12:00:00] [Client thread/INFO]: [CHAT] {payload}")
}

#[test]
fn parses_singular_raid_open_announcement_with_escaped_newlines() {
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
}

#[test]
fn ignores_singular_raid_close_announcement_with_escaped_newlines() {
    let mut parser = LogParser::default();
    let closing = chat(
        "[Рейд] » Закрылись врата на рейд \"Темный лес\" (локация #1)\\nЗакрылись врата на рейд \"Луга с обелисками\" (локация #16)\\nЗакрылись врата на рейд \"Подводный храм\" (локация #17)",
    );

    assert!(parser.consume_line(&closing).is_empty());
}
