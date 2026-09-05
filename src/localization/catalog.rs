use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};
use regex::{Captures, Regex};
use serde::{Deserialize, Serialize};

use crate::protocol::{BoosterType, GlobalEventType, ItemRarity, ItemType};

const RAID_OPEN_KEY: &str = "sao.msg.raid.gates_opened";
const RAID_CLOSE_KEY: &str = "sao.msg.raid.gates_closed";
const ITEM_DROP_KEY: &str = "sao.msg.chat.item_drop";
const BOOSTER_ACTIVATED_KEY: &str = "sao.msg.booster.global_activated";

const ITEM_TYPE_KEYS: &[(&str, ItemType)] = &[
    ("sao.enum.item_type.weapon.name", ItemType::Sword),
    ("sao.enum.item_type.pet.name", ItemType::Pet),
    ("sao.enum.item_type.aura.name", ItemType::Aura),
    ("sao.enum.item_type.relic.name", ItemType::Jewelry),
    ("sao.enum.item_type.book_active.name", ItemType::Book),
    ("sao.enum.item_type.book_passive.name", ItemType::Book),
];

const BOOSTER_KEYS: &[(&str, BoosterType)] = &[
    ("sao.enum.boost.luck.name", BoosterType::Luck),
    ("sao.enum.boost.coin.name", BoosterType::Money),
    ("sao.enum.boost.damage.name", BoosterType::Damage),
    ("sao.enum.boost.power.name", BoosterType::Power),
];

const GLOBAL_KEYS: &[(&str, GlobalEventType)] = &[
    ("sao.event.darkness.alert", GlobalEventType::Darkness),
    ("sao.event.blood_moon.alert", GlobalEventType::Moon),
    ("sao.event.solar_eclipse.alert", GlobalEventType::Eclipse),
    ("sao.event.sunburst.alert", GlobalEventType::Explosion),
    ("sao.event.comet_chaos.alert", GlobalEventType::CometChaos),
];

#[derive(Debug, Clone)]
pub struct LocalizedItemDrop {
    pub player_prefix: String,
    pub item_name: String,
    pub item_type: ItemType,
    pub item_rarity: ItemRarity,
}

#[derive(Debug, Clone)]
pub struct LocalizedBooster {
    pub player_prefix: String,
    pub booster_type: BoosterType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct LocalizationSnapshot {
    pub languages: HashMap<String, LocalizationLanguageSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct LocalizationLanguageSnapshot {
    pub hash: String,
    pub complete: bool,
    pub properties: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct SaoLocalizationStore {
    inner: Arc<RwLock<SaoLocalizationCatalog>>,
}

impl SaoLocalizationStore {
    pub(super) fn from_snapshot(snapshot: LocalizationSnapshot) -> Result<Self> {
        Ok(Self {
            inner: Arc::new(RwLock::new(SaoLocalizationCatalog::from_snapshot(
                snapshot,
            )?)),
        })
    }

    pub(super) fn snapshot(&self) -> LocalizationSnapshot {
        self.inner
            .read()
            .expect("SAO localization catalog lock poisoned")
            .snapshot
            .clone()
    }

    pub(super) fn replace(&self, snapshot: LocalizationSnapshot) -> Result<()> {
        let catalog = SaoLocalizationCatalog::from_snapshot(snapshot)?;
        *self
            .inner
            .write()
            .expect("SAO localization catalog lock poisoned") = catalog;

        Ok(())
    }

    pub fn raid_open_locations(&self, payload: &str) -> Option<BTreeSet<u16>> {
        self.inner
            .read()
            .expect("SAO localization catalog lock poisoned")
            .raid_open_locations(payload)
    }

    pub fn is_raid_close(&self, payload: &str) -> bool {
        self.inner
            .read()
            .expect("SAO localization catalog lock poisoned")
            .is_raid_close(payload)
    }

    pub fn parse_item_drop(&self, payload: &str) -> Option<LocalizedItemDrop> {
        self.inner
            .read()
            .expect("SAO localization catalog lock poisoned")
            .parse_item_drop(payload)
    }

    pub fn parse_booster(&self, payload: &str) -> Option<LocalizedBooster> {
        self.inner
            .read()
            .expect("SAO localization catalog lock poisoned")
            .parse_booster(payload)
    }

    pub fn parse_global(&self, payload: &str) -> Option<GlobalEventType> {
        self.inner
            .read()
            .expect("SAO localization catalog lock poisoned")
            .parse_global(payload)
    }
}

#[derive(Debug, Clone)]
struct SaoLocalizationCatalog {
    snapshot: LocalizationSnapshot,
    languages: Vec<CompiledLanguage>,
    canonical_ru_names: HashMap<String, String>,
}

impl SaoLocalizationCatalog {
    fn from_snapshot(snapshot: LocalizationSnapshot) -> Result<Self> {
        let canonical_ru_names = build_canonical_item_names(&snapshot, "ru_RU");
        let mut languages = snapshot
            .languages
            .iter()
            .map(|(locale, language)| {
                CompiledLanguage::new(locale.clone(), language.properties.clone())
            })
            .collect::<Result<Vec<_>>>()?;
        languages.sort_by(|left, right| left.locale.cmp(&right.locale));

        Ok(Self {
            snapshot,
            languages,
            canonical_ru_names,
        })
    }

    fn raid_open_locations(&self, payload: &str) -> Option<BTreeSet<u16>> {
        let payload = normalize_log_text(payload);
        let mut matched = false;
        let mut locations = BTreeSet::new();

        for language in &self.languages {
            let Some(template) = &language.raid_open else {
                continue;
            };

            for captures in template.regex.captures_iter(&payload) {
                matched = true;

                if let Some(location) = template_capture(&captures, 2)
                    .and_then(|value| value.trim().parse::<u16>().ok())
                {
                    locations.insert(location);
                }
            }
        }

        matched.then_some(locations)
    }

    fn is_raid_close(&self, payload: &str) -> bool {
        let payload = normalize_log_text(payload);

        self.languages.iter().any(|language| {
            language
                .raid_close
                .as_ref()
                .is_some_and(|template| template.regex.is_match(&payload))
        })
    }

    fn parse_item_drop(&self, payload: &str) -> Option<LocalizedItemDrop> {
        let payload = normalize_log_text(payload);

        for language in &self.languages {
            let Some(template) = &language.item_drop else {
                continue;
            };
            let Some(captures) = template.regex.captures(&payload) else {
                continue;
            };
            let Some(player_prefix) = template_capture(&captures, 1) else {
                continue;
            };
            let Some(rarity_label) = template_capture(&captures, 2) else {
                continue;
            };
            let Some(item_phrase) = template_capture(&captures, 3) else {
                continue;
            };
            let Some((item_type, raw_item_name)) = language.parse_item_phrase(item_phrase.trim())
            else {
                continue;
            };

            let item_key = language.resolve_item_key(raw_item_name, item_type);
            let Some(item_rarity) = item_key
                .as_deref()
                .and_then(|key| language.item_rarity_by_key.get(key).copied())
                .or_else(|| language.parse_rarity_label(rarity_label.trim()))
            else {
                continue;
            };
            let item_name = item_key
                .as_deref()
                .and_then(|key| self.canonical_ru_names.get(key))
                .cloned()
                .unwrap_or_else(|| raw_item_name.trim().to_owned());

            return Some(LocalizedItemDrop {
                player_prefix: player_prefix.trim().to_owned(),
                item_name,
                item_type,
                item_rarity,
            });
        }

        None
    }

    fn parse_booster(&self, payload: &str) -> Option<LocalizedBooster> {
        let payload = normalize_log_text(payload);

        for language in &self.languages {
            let Some(template) = &language.booster_activated else {
                continue;
            };
            let Some(captures) = template.regex.captures(&payload) else {
                continue;
            };
            let Some(player_prefix) = template_capture(&captures, 1) else {
                continue;
            };
            let Some(booster_label) = template_capture(&captures, 2) else {
                continue;
            };
            let booster_label = normalize_value(booster_label);
            let Some(booster_type) = language.booster_types.get(&booster_label).copied() else {
                continue;
            };

            return Some(LocalizedBooster {
                player_prefix: player_prefix.trim().to_owned(),
                booster_type,
            });
        }

        None
    }

    fn parse_global(&self, payload: &str) -> Option<GlobalEventType> {
        let payload = normalize_log_text(payload);

        self.languages.iter().find_map(|language| {
            language
                .global_alerts
                .iter()
                .find_map(|(alert, event_type)| payload.contains(alert).then_some(*event_type))
        })
    }
}

#[derive(Debug, Clone)]
struct CompiledLanguage {
    locale: String,
    raid_open: Option<CompiledTemplate>,
    raid_close: Option<CompiledTemplate>,
    item_drop: Option<CompiledTemplate>,
    booster_activated: Option<CompiledTemplate>,
    item_types: Vec<(String, ItemType)>,
    booster_types: HashMap<String, BoosterType>,
    rarity_labels: HashMap<String, ItemRarity>,
    item_keys_by_name: HashMap<String, Vec<String>>,
    item_rarity_by_key: HashMap<String, ItemRarity>,
    global_alerts: Vec<(String, GlobalEventType)>,
}

impl CompiledLanguage {
    fn new(locale: String, properties: HashMap<String, String>) -> Result<Self> {
        let raid_open = compile_property_template(&properties, RAID_OPEN_KEY)?;
        let raid_close = compile_property_template(&properties, RAID_CLOSE_KEY)?;
        let item_drop = compile_property_template(&properties, ITEM_DROP_KEY)?;
        let booster_activated = compile_property_template(&properties, BOOSTER_ACTIVATED_KEY)?;

        let mut item_types = ITEM_TYPE_KEYS
            .iter()
            .filter_map(|(key, item_type)| {
                properties
                    .get(*key)
                    .map(|value| (normalize_value(value), *item_type))
            })
            .collect::<Vec<_>>();
        item_types.sort_by_key(|item| std::cmp::Reverse(item.0.len()));
        item_types.dedup();

        let booster_types = BOOSTER_KEYS
            .iter()
            .filter_map(|(key, booster_type)| {
                properties
                    .get(*key)
                    .map(|value| (normalize_value(value), *booster_type))
            })
            .collect();
        let rarity_labels = build_rarity_labels(&properties);
        let (item_keys_by_name, item_rarity_by_key) = build_item_index(&properties);
        let global_alerts = GLOBAL_KEYS
            .iter()
            .filter_map(|(key, event_type)| {
                properties
                    .get(*key)
                    .map(|value| (normalize_log_text(value), *event_type))
            })
            .collect();

        Ok(Self {
            locale,
            raid_open,
            raid_close,
            item_drop,
            booster_activated,
            item_types,
            booster_types,
            rarity_labels,
            item_keys_by_name,
            item_rarity_by_key,
            global_alerts,
        })
    }

    fn parse_item_phrase<'a>(&self, phrase: &'a str) -> Option<(ItemType, &'a str)> {
        let phrase = phrase.trim();

        for (localized_type, item_type) in &self.item_types {
            if let Some(item_name) = phrase.strip_prefix(localized_type)
                && item_name.chars().next().is_some_and(char::is_whitespace)
            {
                return Some((*item_type, item_name.trim_start()));
            }
        }

        None
    }

    fn resolve_item_key(&self, item_name: &str, item_type: ItemType) -> Option<String> {
        let normalized = normalize_value(item_name);
        let candidates = self.item_keys_by_name.get(&normalized)?;
        let mut matching = candidates
            .iter()
            .filter(|key| item_key_matches_type(key, item_type));
        let first = matching.next()?.clone();

        if matching.next().is_some() {
            return None;
        }

        Some(first)
    }

    fn parse_rarity_label(&self, rarity_label: &str) -> Option<ItemRarity> {
        let normalized = normalize_value(rarity_label);

        if let Some(rarity) = self.rarity_labels.get(&normalized) {
            return Some(*rarity);
        }

        let lower = normalized.to_lowercase();
        let (mythical_stems, secret_stems): (&[&str], &[&str]) = match self.locale.as_str() {
            "en_US" => (&["mythic"], &["secret"]),
            "hy_AM" => (&["առասպելական"], &["գաղտնի"]),
            "ru_RU" => (&["мифическ"], &["секретн"]),
            "tr_TR" => (&["mitik"], &["gizli"]),
            "uk_UA" => (&["міфіч"], &["секретн"]),
            "uz_UZ" => (&["mifik"], &["maxfiy"]),
            _ => (&[], &[]),
        };

        if mythical_stems.iter().any(|stem| lower.starts_with(stem)) {
            return Some(ItemRarity::Mythical);
        }
        if secret_stems.iter().any(|stem| lower.starts_with(stem)) {
            return Some(ItemRarity::Secret);
        }

        None
    }
}

#[derive(Debug, Clone)]
struct CompiledTemplate {
    regex: Regex,
}

fn compile_property_template(
    properties: &HashMap<String, String>,
    key: &str,
) -> Result<Option<CompiledTemplate>> {
    properties
        .get(key)
        .map(|template| compile_template(template).map(|regex| CompiledTemplate { regex }))
        .transpose()
        .with_context(|| format!("failed to compile SAO localization template {key}"))
}

fn compile_template(template: &str) -> Result<Regex> {
    let template = normalize_log_text(template);
    let mut pattern = String::new();
    let mut remaining = template.as_str();

    loop {
        let Some(index) = remaining.find("%s") else {
            pattern.push_str(&escape_literal(remaining));
            break;
        };

        let (literal, tail) = remaining.split_at(index);
        pattern.push_str(&escape_literal(literal));
        pattern.push_str("(.*?)");
        remaining = &tail[2..];
    }

    if template.ends_with("%s") {
        pattern.push('$');
    }

    Regex::new(&pattern).context("invalid generated localization regex")
}

fn escape_literal(literal: &str) -> String {
    let mut output = String::new();
    let mut buffer = String::new();
    let mut in_whitespace = false;

    for character in literal.chars() {
        if character.is_whitespace() {
            if !buffer.is_empty() {
                output.push_str(&regex::escape(&buffer));
                buffer.clear();
            }

            if !in_whitespace {
                output.push_str(r"\s+");
                in_whitespace = true;
            }
        } else {
            in_whitespace = false;
            buffer.push(character);
        }
    }

    if !buffer.is_empty() {
        output.push_str(&regex::escape(&buffer));
    }

    output
}

fn template_capture<'a>(captures: &'a Captures<'a>, index: usize) -> Option<&'a str> {
    captures.get(index).map(|capture| capture.as_str())
}

fn build_canonical_item_names(
    snapshot: &LocalizationSnapshot,
    locale: &str,
) -> HashMap<String, String> {
    snapshot
        .languages
        .get(locale)
        .map(|language| {
            language
                .properties
                .iter()
                .filter(|(key, _)| is_item_name_key(key))
                .map(|(key, value)| (key.clone(), strip_style_prefix(value).trim().to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

fn build_item_index(
    properties: &HashMap<String, String>,
) -> (HashMap<String, Vec<String>>, HashMap<String, ItemRarity>) {
    let mut by_name = HashMap::<String, Vec<String>>::new();
    let mut rarity_by_key = HashMap::new();

    for (key, value) in properties {
        if !is_item_name_key(key) {
            continue;
        }

        by_name
            .entry(normalize_value(value))
            .or_default()
            .push(key.clone());

        if let Some(rarity) = rarity_from_style_prefix(value) {
            rarity_by_key.insert(key.clone(), rarity);
        }
    }

    (by_name, rarity_by_key)
}

fn build_rarity_labels(properties: &HashMap<String, String>) -> HashMap<String, ItemRarity> {
    properties
        .iter()
        .filter_map(|(key, value)| {
            let rarity = if key.starts_with("sao.enum.rarity.mythic.") {
                ItemRarity::Mythical
            } else if key.starts_with("sao.enum.rarity.secret.") {
                ItemRarity::Secret
            } else {
                return None;
            };

            Some((normalize_value(value), rarity))
        })
        .collect()
}

fn is_item_name_key(key: &str) -> bool {
    key.starts_with("sao.item.") && key.ends_with(".name")
}

fn item_key_matches_type(key: &str, item_type: ItemType) -> bool {
    let item_id = key
        .strip_prefix("sao.item.")
        .and_then(|value| value.strip_suffix(".name"))
        .unwrap_or(key);

    match item_type {
        ItemType::Sword => {
            item_id.starts_with('l')
                && item_id
                    .chars()
                    .nth(1)
                    .is_some_and(|character| character.is_ascii_digit())
        }
        ItemType::Pet => item_id.starts_with("pet"),
        ItemType::Aura => item_id.starts_with("aura"),
        ItemType::Book => item_id.starts_with("book"),
        ItemType::Jewelry => {
            item_id.starts_with('d')
                && item_id
                    .chars()
                    .nth(1)
                    .is_some_and(|character| character.is_ascii_digit())
        }
    }
}

fn rarity_from_style_prefix(value: &str) -> Option<ItemRarity> {
    let style = value.strip_prefix('¨')?.get(..6)?;

    if style.eq_ignore_ascii_case("a91925") {
        Some(ItemRarity::Mythical)
    } else if style.eq_ignore_ascii_case("37efef") {
        Some(ItemRarity::Secret)
    } else {
        None
    }
}

fn normalize_value(value: &str) -> String {
    normalize_log_text(strip_style_prefix(value))
        .trim()
        .to_owned()
}

fn strip_style_prefix(value: &str) -> &str {
    let Some(value) = value.strip_prefix('¨') else {
        return value;
    };

    if value.len() < 6 || !value[..6].bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return value;
    }

    &value[6..]
}

fn normalize_log_text(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars();

    while let Some(character) = chars.next() {
        if character == '§' {
            chars.next();
            continue;
        }

        output.push(character);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::{escape_literal, item_key_matches_type};
    use crate::protocol::ItemType;

    #[test]
    fn generated_template_literals_allow_flexible_whitespace() {
        assert_eq!(escape_literal("foo  bar"), r"foo\s+bar");
    }

    #[test]
    fn canonical_item_key_type_detection_distinguishes_drop_families() {
        assert!(item_key_matches_type(
            "sao.item.l28_7.name",
            ItemType::Sword
        ));
        assert!(item_key_matches_type(
            "sao.item.pet_l28_7.name",
            ItemType::Pet
        ));
        assert!(item_key_matches_type(
            "sao.item.auraFire.name",
            ItemType::Aura
        ));
        assert!(item_key_matches_type(
            "sao.item.book_l28_7.name",
            ItemType::Book
        ));
        assert!(item_key_matches_type(
            "sao.item.d1_41.name",
            ItemType::Jewelry
        ));
    }
}
