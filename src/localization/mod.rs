mod catalog;
mod updater;

use std::sync::LazyLock;

pub use catalog::{LocalizedBooster, LocalizedItemDrop, SaoLocalizationStore};

use crate::diagnostics;
use catalog::LocalizationSnapshot;

static BUNDLED_FALLBACK: LazyLock<LocalizationSnapshot> = LazyLock::new(|| {
    serde_json::from_str::<LocalizationSnapshot>(include_str!(
        "../../resources/sao-localization-fallback.json"
    ))
    .expect("bundled SAO localization fallback must be valid")
});

static GLOBAL_STORE: LazyLock<SaoLocalizationStore> = LazyLock::new(|| {
    let fallback = (*BUNDLED_FALLBACK).clone();
    let store = SaoLocalizationStore::from_snapshot(fallback.clone())
        .expect("bundled SAO localization fallback must compile");

    if let Some(cached) = updater::load_cached_snapshot()
        && let Err(error) = store.replace(cached.merge_missing_from(&fallback))
    {
        diagnostics::warn(
            "localization",
            format!("Ignoring invalid cached SAO localization catalog: {error:#}"),
        );
    }

    store
});

pub fn sao_localizations() -> SaoLocalizationStore {
    let store = (*GLOBAL_STORE).clone();
    updater::ensure_refresh_started(store.clone(), (*BUNDLED_FALLBACK).clone());
    store
}
