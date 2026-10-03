use mnemos_collector::parser::LogParser;
use mnemos_collector::protocol::ChatPingReport;

#[test]
fn parses_multiple_mentions_from_real_player_chat() {
    let mut parser = LogParser::default();
    parser.consume_context_line("[Client thread/INFO] Loading mod MasterSwordReborn v1.0.4");

    let line = "[07:02:10] [Client thread/INFO] [CHAT] MVP ┃ TranZorXx [#401] » @knaliz привет, @Other_1! @KNALIZ";
    let ping = parser.parse_chat_ping(line).expect("expected chat ping");

    assert_eq!(ping.sender, "TranZorXx");
    assert_eq!(
        ping.message,
        "MVP ┃ TranZorXx [#401] » @knaliz привет, @Other_1! @KNALIZ"
    );
    assert_eq!(ping.text, "@knaliz привет, @Other_1! @KNALIZ");
    assert_eq!(ping.mentions, vec!["knaliz", "Other_1"]);
}

#[test]
fn ignores_email_like_or_embedded_at_signs() {
    let mut parser = LogParser::default();
    parser.consume_context_line("[Client thread/INFO] Loading mod MasterSwordReborn v1.0.4");

    let line = "[07:02:10] [Client thread/INFO] [CHAT] MVP ┃ TranZorXx [#401] » abc@knaliz test@example_user";
    assert!(parser.parse_chat_ping(line).is_none());
}

#[test]
fn serializes_ephemeral_ping_report_contract() {
    let report = ChatPingReport::new(
        "TranZorXx".to_owned(),
        "MVP ┃ TranZorXx [#401] » @knaliz привет".to_owned(),
        "@knaliz привет".to_owned(),
        vec!["knaliz".to_owned()],
        chrono::Utc::now(),
    );
    let json = serde_json::to_value(report).unwrap();

    assert_eq!(json["type"], "CHAT_PING_REPORT");
    assert_eq!(json["sender"], "TranZorXx");
    assert_eq!(json["mentions"][0], "knaliz");
    assert!(json.get("messageId").is_some());
    assert!(json.get("observedAt").is_some());
}
