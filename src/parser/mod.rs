mod dedup;

pub use dedup::EventDeduplicator;

use std::collections::{BTreeSet, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::localization::{SaoLocalizationStore, sao_localizations};
use crate::protocol::CollectorEvent;

static NICKNAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\p{L}0-9_]{4,20}$").expect("valid regex"));
static PLAYER_CHAT_MESSAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[#(?:\d+|\?)\]\s*»").expect("valid regex"));
static MENTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"@[\p{L}0-9_]{4,20}").expect("valid regex"));

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GameMode {
    #[default]
    Unknown,
    MasterSwordLobby,
    MasterSword,
    Other,
}

impl GameMode {
    fn accepts_events(self) -> bool {
        matches!(self, Self::Unknown | Self::MasterSword)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChatPing {
    pub sender: String,
    pub message: String,
    pub text: String,
    pub mentions: Vec<String>,
}

#[derive(Debug)]
pub struct LogParser {
    mode: GameMode,
    pending_raid: Option<BTreeSet<u16>>,
    localizations: SaoLocalizationStore,
}

impl Default for LogParser {
    fn default() -> Self {
        Self::new(sao_localizations())
    }
}

impl LogParser {
    pub fn new(localizations: SaoLocalizationStore) -> Self {
        Self {
            mode: GameMode::Unknown,
            pending_raid: None,
            localizations,
        }
    }

    pub fn mode(&self) -> GameMode {
        self.mode
    }

    pub fn consume_context_line(&mut self, line: &str) {
        self.update_mode(line);

        let Some(payload) = extract_chat_payload(line) else {
            return;
        };

        if is_player_chat_message(payload) {
            return;
        }

        if is_master_sword_activity(payload, &self.localizations) {
            self.mode = GameMode::MasterSword;
        }
    }

    pub fn consume_line(&mut self, line: &str) -> Vec<CollectorEvent> {
        self.update_mode(line);

        let Some(payload) = extract_chat_payload(line) else {
            return Vec::new();
        };

        if is_player_chat_message(payload) {
            return Vec::new();
        }

        if !self.mode.accepts_events() {
            return Vec::new();
        }

        let localized_raid_open = self.localizations.raid_open_locations(payload);

        if let Some(locations) = localized_raid_open {
            let mut events = self.flush_pending_raid();

            if self.mode.accepts_events() {
                self.mode = GameMode::MasterSword;

                if locations.is_empty() {
                    self.pending_raid = Some(BTreeSet::new());
                } else {
                    events.push(CollectorEvent::Raid {
                        locations: locations.into_iter().collect(),
                    });
                }
            }

            return events;
        }

        if self.localizations.is_raid_close(payload) {
            let events = self.flush_pending_raid();
            self.pending_raid = None;

            return events;
        }

        let mut events = self.flush_pending_raid();

        let parsed = parse_drop(payload, &self.localizations)
            .or_else(|| parse_booster(payload, &self.localizations))
            .or_else(|| parse_global(payload, &self.localizations));

        if let Some(event) = parsed {
            self.mode = GameMode::MasterSword;
            events.push(event);
        }

        events
    }

    pub fn parse_chat_ping(&self, line: &str) -> Option<ChatPing> {
        if self.mode != GameMode::MasterSword {
            return None;
        }

        let payload = extract_chat_payload(line)?;
        parse_player_chat_ping(payload)
    }

    pub fn flush(&mut self) -> Vec<CollectorEvent> {
        self.flush_pending_raid()
    }

    fn update_mode(&mut self, line: &str) {
        if line.contains("Loading mod MasterSwordReborn") {
            self.mode = GameMode::MasterSword;
            return;
        }

        if line.contains("Loading mod MasterSword Lobby") {
            self.mode = GameMode::MasterSwordLobby;
            self.pending_raid = None;
            return;
        }

        if line.contains("Unloading mod MasterSword") {
            self.mode = GameMode::Other;
            self.pending_raid = None;
        }
    }

    fn flush_pending_raid(&mut self) -> Vec<CollectorEvent> {
        let Some(locations) = self.pending_raid.take() else {
            return Vec::new();
        };

        if locations.is_empty() {
            return Vec::new();
        }

        vec![CollectorEvent::Raid {
            locations: locations.into_iter().collect(),
        }]
    }
}

fn extract_chat_payload(line: &str) -> Option<&str> {
    let marker = "[CHAT]";
    let marker_index = line.find(marker)?;
    let payload = &line[marker_index + marker.len()..];

    Some(payload.trim_start_matches([':', ' ']).trim())
}

fn is_player_chat_message(payload: &str) -> bool {
    PLAYER_CHAT_MESSAGE.is_match(payload)
}

fn is_master_sword_activity(payload: &str, localizations: &SaoLocalizationStore) -> bool {
    localizations.raid_open_locations(payload).is_some()
        || localizations.is_raid_close(payload)
        || parse_drop(payload, localizations).is_some()
        || parse_booster(payload, localizations).is_some()
        || parse_global(payload, localizations).is_some()
}

fn parse_drop(payload: &str, localizations: &SaoLocalizationStore) -> Option<CollectorEvent> {
    let drop = localizations.parse_item_drop(payload)?;

    let dropped_for = extract_nickname(&drop.player_prefix)?;

    if !(1..=96).contains(&drop.item_name.chars().count()) {
        return None;
    }

    Some(CollectorEvent::ItemDrop {
        item_key: drop.item_key,
        item_name: drop.item_name,
        item_type: drop.item_type,
        item_rarity: drop.item_rarity,
        dropped_for,
    })
}

fn parse_booster(payload: &str, localizations: &SaoLocalizationStore) -> Option<CollectorEvent> {
    let booster = localizations.parse_booster(payload)?;
    let activated_by = extract_nickname(&booster.player_prefix)?;

    Some(CollectorEvent::Booster {
        booster_type: booster.booster_type,
        activated_by,
    })
}

fn parse_global(payload: &str, localizations: &SaoLocalizationStore) -> Option<CollectorEvent> {
    localizations
        .parse_global(payload)
        .map(|event_type| CollectorEvent::Global { event_type })
}

fn parse_player_chat_ping(payload: &str) -> Option<ChatPing> {
    let separator = PLAYER_CHAT_MESSAGE.find(payload)?;
    let player_prefix = payload[..separator.start()].trim_end();
    let text = payload[separator.end()..].trim();
    let sender = extract_nickname(player_prefix)?;
    let mentions = extract_mentions(text);

    if mentions.is_empty() {
        return None;
    }

    Some(ChatPing {
        sender,
        message: payload.trim().to_owned(),
        text: text.to_owned(),
        mentions,
    })
}

fn extract_mentions(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut mentions = Vec::new();

    for candidate in MENTION.find_iter(text) {
        let before = text[..candidate.start()].chars().next_back();
        let after = text[candidate.end()..].chars().next();

        if before.is_some_and(is_nickname_character) || after.is_some_and(is_nickname_character) {
            continue;
        }

        let nickname = &candidate.as_str()[1..];
        let normalized = nickname.to_lowercase();

        if seen.insert(normalized) {
            mentions.push(nickname.to_owned());
        }
    }

    mentions
}

fn is_nickname_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn extract_nickname(player_prefix: &str) -> Option<String> {
    let decorated_suffix = player_prefix
        .rsplit_once('┃')
        .map(|(_, suffix)| suffix)
        .or_else(|| player_prefix.rsplit_once('|').map(|(_, suffix)| suffix));

    let candidate = if let Some(suffix) = decorated_suffix {
        suffix
            .split_whitespace()
            .find(|token| NICKNAME.is_match(token))
    } else {
        player_prefix
            .split_whitespace()
            .rev()
            .find(|token| NICKNAME.is_match(token))
    }?;

    Some(candidate.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{GameMode, LogParser, extract_mentions};

    #[test]
    fn context_scan_recovers_master_sword_from_mod_lifecycle() {
        let mut parser = LogParser::default();

        parser.consume_context_line("[Client thread/INFO]: Loading mod MasterSwordReborn");

        assert_eq!(parser.mode(), GameMode::MasterSword);
        assert!(parser.flush().is_empty());
    }

    #[test]
    fn unknown_join_line_does_not_override_master_sword_without_an_unload_signal() {
        let mut parser = LogParser::default();

        parser.consume_context_line("[Client thread/INFO]: Loading mod MasterSwordReborn");
        parser.consume_context_line("Joining server Sword Masters #17");

        assert_eq!(parser.mode(), GameMode::MasterSword);

        parser.consume_context_line("[Client thread/INFO]: Unloading mod MasterSwordReborn");
        assert_eq!(parser.mode(), GameMode::Other);
    }

    #[test]
    fn context_scan_can_recover_master_sword_from_localized_activity() {
        let mut parser = LogParser::default();

        parser.consume_context_line("[CHAT] Darkness comes with the setting sun..");

        assert_eq!(parser.mode(), GameMode::MasterSword);
        assert!(parser.flush().is_empty());
    }

    #[test]
    fn mention_parser_respects_boundaries_and_deduplicates_case_insensitively() {
        assert_eq!(
            extract_mentions("@Knaliz, @other_1! @KNALIZ abc@ignored_user"),
            vec!["Knaliz".to_owned(), "other_1".to_owned()],
        );
    }
}
