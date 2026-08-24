mod dedup;

pub use dedup::EventDeduplicator;

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::protocol::{BoosterType, CollectorEvent, GlobalEventType, ItemRarity, ItemType};

static MASTER_SWORD_SERVER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Joining server Мастера Мечей #\d+").expect("valid regex"));
static DROP_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^(?P<player>.+?)\s+\[#(?:\d+|\?)\]\s+выбил\s+"(?P<rarity>[^"]+)"\s+(?P<item_type>оружие|питомца|реликвию|ауру|книгу)\s+(?P<item_name>.+?)\s*$"#,
    )
    .expect("valid regex")
});
static BOOSTER_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^(?P<player>.+?)\s+активировал\s+"Бустер\s+(?P<booster>удачи|денег|урона|силы)\s+x[^"]+"\s+на\s+\S+\s*$"#,
    )
    .expect("valid regex")
});
static RAID_LOCATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(локация\s+#(?P<location>\d+)\)").expect("valid regex"));
static NICKNAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\p{L}0-9_]{4,20}$").expect("valid regex"));

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

#[derive(Debug, Default)]
pub struct LogParser {
    mode: GameMode,
    pending_raid: Option<BTreeSet<u16>>,
}

impl LogParser {
    pub fn mode(&self) -> GameMode {
        self.mode
    }

    pub fn consume_context_line(&mut self, line: &str) {
        self.update_mode(line);

        let Some(payload) = extract_chat_payload(line) else {
            return;
        };

        if is_master_sword_activity(payload) {
            self.mode = GameMode::MasterSword;
        }
    }

    pub fn consume_line(&mut self, line: &str) -> Vec<CollectorEvent> {
        self.update_mode(line);

        let Some(payload) = extract_chat_payload(line) else {
            return Vec::new();
        };

        if is_raid_open(payload) {
            let mut events = self.flush_pending_raid();

            if self.mode.accepts_events() {
                self.mode = GameMode::MasterSword;

                let locations = parse_raid_locations(payload);

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

        if is_raid_close(payload) {
            let events = self.flush_pending_raid();
            self.pending_raid = None;

            return events;
        }

        let raid_locations = parse_raid_locations(payload);

        if !raid_locations.is_empty()
            && let Some(locations) = self.pending_raid.as_mut()
        {
            locations.extend(raid_locations);
            return Vec::new();
        }

        let mut events = self.flush_pending_raid();

        if !self.mode.accepts_events() {
            return events;
        }

        let parsed = parse_drop(payload)
            .or_else(|| parse_booster(payload))
            .or_else(|| parse_global(payload));

        if let Some(event) = parsed {
            self.mode = GameMode::MasterSword;
            events.push(event);
        }

        events
    }

    pub fn flush(&mut self) -> Vec<CollectorEvent> {
        self.flush_pending_raid()
    }

    fn update_mode(&mut self, line: &str) {
        if line.contains("Joining server Мастера Мечей Лобби") {
            self.mode = GameMode::MasterSwordLobby;
            self.pending_raid = None;
            return;
        }

        if MASTER_SWORD_SERVER.is_match(line) {
            self.mode = GameMode::MasterSword;
            return;
        }

        if line.contains("Joining server ") {
            self.mode = GameMode::Other;
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

fn is_master_sword_activity(payload: &str) -> bool {
    is_raid_open(payload)
        || is_raid_close(payload)
        || !parse_raid_locations(payload).is_empty()
        || parse_drop(payload).is_some()
        || parse_booster(payload).is_some()
        || parse_global(payload).is_some()
}

fn parse_drop(payload: &str) -> Option<CollectorEvent> {
    let captures = DROP_LINE.captures(payload)?;
    let player_prefix = captures.name("player")?.as_str();

    if player_prefix.contains('»') {
        return None;
    }

    let item_rarity = parse_rarity(captures.name("rarity")?.as_str())?;
    let item_type = parse_item_type(captures.name("item_type")?.as_str())?;
    let dropped_for = extract_nickname(player_prefix)?;
    let item_name = captures.name("item_name")?.as_str().trim();

    if !(4..=24).contains(&item_name.chars().count()) {
        return None;
    }

    Some(CollectorEvent::ItemDrop {
        item_name: item_name.to_owned(),
        item_type,
        item_rarity,
        dropped_for,
    })
}

fn parse_booster(payload: &str) -> Option<CollectorEvent> {
    let captures = BOOSTER_LINE.captures(payload)?;
    let activated_by = extract_nickname(captures.name("player")?.as_str())?;
    let booster_type = match captures.name("booster")?.as_str() {
        "удачи" => BoosterType::Luck,
        "денег" => BoosterType::Money,
        "урона" => BoosterType::Damage,
        "силы" => BoosterType::Power,
        _ => return None,
    };

    Some(CollectorEvent::Booster {
        booster_type,
        activated_by,
    })
}

fn parse_global(payload: &str) -> Option<CollectorEvent> {
    let event_type = if payload.contains("Тьма наступает с заходом солнца")
    {
        GlobalEventType::Darkness
    } else if payload.contains("кровавая луна") {
        GlobalEventType::Moon
    } else if payload.contains("Небо темнеет и окутывается глубокой тенью")
    {
        GlobalEventType::Eclipse
    } else if payload.contains("тепло солнца касается вашей кожи") {
        GlobalEventType::Explosion
    } else {
        return None;
    };

    Some(CollectorEvent::Global { event_type })
}

fn is_raid_open(payload: &str) -> bool {
    payload.contains("[Рейд]") && payload.contains("Открылись врата на рейды")
}

fn is_raid_close(payload: &str) -> bool {
    payload.contains("[Рейд]") && payload.contains("Закрылись врата")
}

fn parse_raid_locations(payload: &str) -> BTreeSet<u16> {
    RAID_LOCATION
        .captures_iter(payload)
        .filter_map(|captures| captures.name("location"))
        .filter_map(|location| location.as_str().parse().ok())
        .collect()
}

fn parse_rarity(raw: &str) -> Option<ItemRarity> {
    if raw.starts_with("Мифическ") {
        Some(ItemRarity::Mythical)
    } else if raw.starts_with("Секретн") {
        Some(ItemRarity::Secret)
    } else {
        None
    }
}

fn parse_item_type(raw: &str) -> Option<ItemType> {
    match raw {
        "оружие" => Some(ItemType::Sword),
        "ауру" => Some(ItemType::Aura),
        "питомца" => Some(ItemType::Pet),
        "книгу" => Some(ItemType::Book),
        "реликвию" => Some(ItemType::Jewelry),
        _ => None,
    }
}

fn extract_nickname(player_prefix: &str) -> Option<String> {
    let candidate = if let Some((_, suffix)) = player_prefix.rsplit_once('┃') {
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
    use super::{GameMode, LogParser};

    #[test]
    fn context_scan_recovers_master_sword_from_existing_join_line() {
        let mut parser = LogParser::default();

        parser.consume_context_line("[Client thread/INFO]: Joining server Мастера Мечей #17");

        assert_eq!(parser.mode(), GameMode::MasterSword);
        assert!(parser.flush().is_empty());
    }

    #[test]
    fn context_scan_keeps_the_latest_server_transition() {
        let mut parser = LogParser::default();

        parser.consume_context_line("Joining server Мастера Мечей #17");
        parser.consume_context_line("Joining server Лобби #4");

        assert_eq!(parser.mode(), GameMode::Other);
        assert!(parser.flush().is_empty());
    }

    #[test]
    fn context_scan_can_recover_master_sword_from_recent_activity() {
        let mut parser = LogParser::default();

        parser.consume_context_line("[CHAT] [Рейд] Открылись врата на рейды");

        assert_eq!(parser.mode(), GameMode::MasterSword);
        assert!(parser.flush().is_empty());
    }
}
