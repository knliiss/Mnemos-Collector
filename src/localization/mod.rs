mod catalog;
mod updater;

use std::sync::LazyLock;

pub use catalog::{LocalizedBooster, LocalizedItemDrop, SaoLocalizationStore};

use catalog::LocalizationSnapshot;
use crate::diagnostics;

static GLOBAL_STORE: LazyLock<SaoLocalizationStore> = LazyLock::new(|| {
    let bundled = serde_json::from_str::<LocalizationSnapshot>(include_str!(
        "../../resources/sao-localization-fallback.json"
    ))
    .expect("bundled SAO localization fallback must be valid");
    let store = SaoLocalizationStore::from_snapshot(bundled)
        .expect("bundled SAO localization fallback must compile");

    if let Some(cached) = updater::load_cached_snapshot()
        && let Err(error) = store.replace(cached)
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
    updater::ensure_refresh_started(store.clone());
    store
}
