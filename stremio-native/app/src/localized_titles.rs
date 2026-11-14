//! Catalog rows ship original/English names. Details pages often get a localized
//! title from a configured meta addon (typically TMDB). Overlay that name onto
//! home/discover/search/library cards so the two screens stay in sync.
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use core_env::DesktopEnv;
use futures::{StreamExt, stream};
use slint::{ComponentHandle, Model, ModelRc};
use stremio_core::{
    runtime::Env,
    types::addon::{Descriptor, ResourcePath, ResourceResponse},
};

use crate::{AppModel, MainWindow, MediaCardItem, SearchSuggestion};
use std::sync::Arc;
use stremio_core::runtime::Runtime;

const CACHE_FILE: &str = "title-cache-v1.json";
const MAX_IN_FLIGHT: usize = 4;
const BATCH_CAP: usize = 40;

struct TitleCache {
    titles: HashMap<String, String>,
    in_flight: HashSet<String>,
    attempted: HashSet<String>,
}

static CACHE: OnceLock<Mutex<TitleCache>> = OnceLock::new();
static DISK_LOADED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn cache() -> &'static Mutex<TitleCache> {
    CACHE.get_or_init(|| {
        Mutex::new(TitleCache {
            titles: HashMap::new(),
            in_flight: HashSet::new(),
            attempted: HashSet::new(),
        })
    })
}

/// Disk access must not happen on the UI thread's first title lookup.
pub fn initialize(ui: &MainWindow) {
    let weak = ui.as_weak();
    tokio::spawn(async move {
        let Ok(titles) = tokio::task::spawn_blocking(load_from_disk).await else { return; };
        let changed = {
            let mut guard = cache().lock().unwrap_or_else(|e| e.into_inner());
            let mut changed = HashMap::new();
            for (id, title) in titles {
                if !guard.titles.contains_key(&id) {
                    changed.insert(id.clone(), title.clone());
                    guard.titles.insert(id, title);
                }
            }
            changed
        };
        // Do not let early metadata updates overwrite the cache before its
        // previous contents have been merged, and flush any such updates now.
        DISK_LOADED.store(true, std::sync::atomic::Ordering::Release);
        persist();
        if !changed.is_empty() {
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() { apply_to_ui(&ui, &changed); }
            });
        }
    });
}

pub fn display(id: &str, fallback: &str) -> String {
    if id.is_empty() {
        return fallback.to_owned();
    }
    let guard = cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard
        .titles
        .get(id)
        .cloned()
        .unwrap_or_else(|| fallback.to_owned())
}

pub fn remember(id: &str, title: &str) -> bool {
    let title = title.trim();
    if id.is_empty() || title.is_empty() {
        return false;
    }
    let changed = {
        let mut guard = cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.attempted.remove(id);
        guard.in_flight.remove(id);
        match guard.titles.get(id) {
            Some(existing) if existing == title => false,
            _ => {
                guard.titles.insert(id.to_owned(), title.to_owned());
                true
            }
        }
    };
    if changed {
        persist();
    }
    changed
}

pub fn remember_and_apply(ui: &MainWindow, id: &str, title: &str) {
    if remember(id, title) {
        let mut changed = HashMap::new();
        changed.insert(id.to_owned(), title.trim().to_owned());
        apply_to_ui(ui, &changed);
    }
}

pub fn hydrate_visible(ui: &MainWindow, addons: &[Descriptor]) {
    let items = collect_visible(ui);
    hydrate(ui.as_weak(), addons, items);
}

pub fn hydrate_from_runtime(ui: &MainWindow, runtime: &Arc<Runtime<DesktopEnv, AppModel>>) {
    let items = collect_visible(ui);
    if !has_uncached(&items) {
        return;
    }
    let addons = runtime
        .model()
        .ok()
        .map(|model| model.ctx.profile.addons.clone())
        .unwrap_or_default();
    hydrate(ui.as_weak(), &addons, items);
}

fn hydrate(
    ui: slint::Weak<MainWindow>,
    addons: &[Descriptor],
    items: Vec<(String, String, String)>,
) {
    let pending = pending_items(&items);
    if pending.is_empty() {
        return;
    }
    let addons = addons.to_vec();
    tokio::spawn(async move {
        let providers = ranked_meta_addons(&addons);
        let fetched = stream::iter(pending)
            .map(|(id, kind, catalog_name)| {
                let providers = providers.clone();
                async move {
                    let title = fetch_title(&providers, &id, &kind, &catalog_name).await;
                    (id, catalog_name, title)
                }
            })
            .buffer_unordered(MAX_IN_FLIGHT)
            .collect::<Vec<_>>()
            .await;

        let mut changed = HashMap::new();
        {
            let mut guard = cache()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for (id, catalog_name, title) in fetched {
                guard.in_flight.remove(&id);
                let Some(title) = title else {
                    guard.attempted.insert(id);
                    continue;
                };
                if guard.titles.get(&id) != Some(&title) {
                    if title != catalog_name {
                        changed.insert(id.clone(), title.clone());
                    }
                    guard.titles.insert(id, title);
                }
            }
        }
        persist();
        let addons_for_next = addons;
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui.upgrade() {
                if !changed.is_empty() {
                    apply_to_ui(&ui, &changed);
                }
                hydrate_visible(&ui, &addons_for_next);
            }
        });
    });
}

fn has_uncached(items: &[(String, String, String)]) -> bool {
    let guard = cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    items.iter().any(|(id, kind, _)| {
        !id.is_empty()
            && !kind.is_empty()
            && !guard.titles.contains_key(id)
            && !guard.attempted.contains(id)
            && !guard.in_flight.contains(id)
    })
}

fn pending_items(items: &[(String, String, String)]) -> Vec<(String, String, String)> {
    let mut pending = Vec::new();
    let mut guard = cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (id, kind, catalog_name) in items {
        if pending.len() >= BATCH_CAP {
            break;
        }
        if id.is_empty() || kind.is_empty() {
            continue;
        }
        if guard.titles.contains_key(id) || guard.attempted.contains(id) {
            continue;
        }
        if !guard.in_flight.insert(id.clone()) {
            continue;
        }
        pending.push((id.clone(), kind.clone(), catalog_name.clone()));
    }
    pending
}

fn collect_visible(ui: &MainWindow) -> Vec<(String, String, String)> {
    let mut items = Vec::new();
    collect_cards(&ui.get_board_continue_watching(), &mut items);
    let sections = ui.get_board_sections();
    for index in 0..sections.row_count() {
        if let Some(section) = sections.row_data(index) {
            collect_cards(&section.items, &mut items);
        }
    }
    let discover = ui.get_discover_rows();
    for index in 0..discover.row_count() {
        if let Some(row) = discover.row_data(index) {
            collect_cards(&row.cols, &mut items);
        }
    }
    let library = ui.get_library_rows();
    for index in 0..library.row_count() {
        if let Some(row) = library.row_data(index) {
            collect_cards(&row.cols, &mut items);
        }
    }
    let search = ui.get_search_sections();
    for index in 0..search.row_count() {
        if let Some(section) = search.row_data(index) {
            collect_cards(&section.items, &mut items);
        }
    }
    collect_suggestions(&ui.get_search_suggestions(), &mut items);
    items
}

fn collect_cards(model: &ModelRc<MediaCardItem>, items: &mut Vec<(String, String, String)>) {
    for index in 0..model.row_count() {
        let Some(card) = model.row_data(index) else {
            continue;
        };
        items.push((
            card.id.to_string(),
            card.media_type.to_string(),
            card.title.to_string(),
        ));
    }
}

fn collect_suggestions(
    model: &ModelRc<SearchSuggestion>,
    items: &mut Vec<(String, String, String)>,
) {
    for index in 0..model.row_count() {
        let Some(suggestion) = model.row_data(index) else {
            continue;
        };
        items.push((
            suggestion.id.to_string(),
            suggestion.media_type.to_string(),
            suggestion.title.to_string(),
        ));
    }
}

fn apply_to_ui(ui: &MainWindow, changed: &HashMap<String, String>) {
    if changed.is_empty() {
        return;
    }
    patch_card_titles(&ui.get_board_continue_watching(), changed);
    let sections = ui.get_board_sections();
    for index in 0..sections.row_count() {
        if let Some(section) = sections.row_data(index) {
            patch_card_titles(&section.items, changed);
        }
    }
    let discover = ui.get_discover_rows();
    for index in 0..discover.row_count() {
        if let Some(row) = discover.row_data(index) {
            patch_card_titles(&row.cols, changed);
        }
    }
    let library = ui.get_library_rows();
    for index in 0..library.row_count() {
        if let Some(row) = library.row_data(index) {
            patch_card_titles(&row.cols, changed);
        }
    }
    let search = ui.get_search_sections();
    for index in 0..search.row_count() {
        if let Some(section) = search.row_data(index) {
            patch_card_titles(&section.items, changed);
        }
    }
    patch_suggestion_titles(&ui.get_search_suggestions(), changed);
}

fn patch_card_titles(model: &ModelRc<MediaCardItem>, changed: &HashMap<String, String>) {
    for index in 0..model.row_count() {
        let Some(mut card) = model.row_data(index) else {
            continue;
        };
        let Some(title) = changed.get(card.id.as_str()) else {
            continue;
        };
        if card.title.as_str() == title {
            continue;
        }
        card.title = title.as_str().into();
        model.set_row_data(index, card);
    }
}

fn patch_suggestion_titles(model: &ModelRc<SearchSuggestion>, changed: &HashMap<String, String>) {
    for index in 0..model.row_count() {
        let Some(mut suggestion) = model.row_data(index) else {
            continue;
        };
        let Some(title) = changed.get(suggestion.id.as_str()) else {
            continue;
        };
        if suggestion.title.as_str() == title {
            continue;
        }
        suggestion.title = title.as_str().into();
        model.set_row_data(index, suggestion);
    }
}

fn ranked_meta_addons(addons: &[Descriptor]) -> Vec<Descriptor> {
    let mut providers = addons.to_vec();
    providers.sort_by_key(|addon| provider_rank(addon));
    providers
}

fn provider_rank(addon: &Descriptor) -> u8 {
    let id = addon.manifest.id.to_ascii_lowercase();
    if id.contains("tmdb") {
        0
    } else if id == "com.linvo.cinemeta" || id == "org.debridin.cinema" {
        2
    } else {
        1
    }
}

async fn fetch_title(
    providers: &[Descriptor],
    id: &str,
    kind: &str,
    catalog_name: &str,
) -> Option<String> {
    let mut candidates = Vec::new();
    for addon in providers {
        let path = ResourcePath {
            resource: "meta".to_owned(),
            r#type: kind.to_owned(),
            id: id.to_owned(),
            extra: vec![],
        };
        if !addon.manifest.is_resource_supported(&path) {
            continue;
        }
        let pending = DesktopEnv::addon_transport(&addon.transport_url).resource(&path);
        let Ok(response) = pending.await else {
            continue;
        };
        let Some(name) = title_from_response(response, id) else {
            continue;
        };
        let rank = provider_rank(addon);
        if looks_polish(&name) || (rank == 0 && name != catalog_name) {
            return Some(name);
        }
        candidates.push((rank, name));
    }
    choose_localized_title(catalog_name, &candidates)
}

fn title_from_response(response: ResourceResponse, id: &str) -> Option<String> {
    let name = match response {
        ResourceResponse::Meta { meta } => meta.preview.name,
        ResourceResponse::MetasDetailed { metas_detailed } => metas_detailed
            .into_iter()
            .find(|item| item.preview.id == id)?
            .preview
            .name,
        ResourceResponse::Metas { metas } => metas.into_iter().find(|item| item.id == id)?.name,
        _ => return None,
    };
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

fn looks_polish(name: &str) -> bool {
    name.chars().any(|character| {
        matches!(
            character,
            'ą' | 'ć'
                | 'ę'
                | 'ł'
                | 'ń'
                | 'ó'
                | 'ś'
                | 'ź'
                | 'ż'
                | 'Ą'
                | 'Ć'
                | 'Ę'
                | 'Ł'
                | 'Ń'
                | 'Ó'
                | 'Ś'
                | 'Ź'
                | 'Ż'
        )
    })
}

pub(crate) fn choose_localized_title(
    catalog_name: &str,
    candidates: &[(u8, String)],
) -> Option<String> {
    let catalog_name = catalog_name.trim();
    let mut fallback = None;
    let mut translated = None;
    for (rank, name) in candidates {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        if looks_polish(name) {
            return Some(name.to_owned());
        }
        if name != catalog_name && translated.is_none() {
            translated = Some(name.to_owned());
        }
        if fallback.is_none() {
            fallback = Some(name.to_owned());
        }
        if *rank == 0 && name != catalog_name {
            return Some(name.to_owned());
        }
    }
    translated.or(fallback)
}

fn cache_path() -> Option<std::path::PathBuf> {
    crate::paths::try_get().map(|paths| paths.root().join("cache").join(CACHE_FILE))
}

fn load_from_disk() -> HashMap<String, String> {
    let Some(path) = cache_path() else {
        return HashMap::new();
    };
    let Ok(bytes) = std::fs::read(path) else {
        return HashMap::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn persist() {
    if cache_path().is_none() { return; }
    static WRITER: OnceLock<tokio::sync::mpsc::Sender<()>> = OnceLock::new();
    let writer = WRITER.get_or_init(|| {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        tokio::spawn(async move {
            while receiver.recv().await.is_some() {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                while receiver.try_recv().is_ok() {}
                if let Err(error) = tokio::task::spawn_blocking(persist_to_disk).await {
                    tracing::warn!(%error, "localized_title_cache_write_task_failed");
                }
            }
        });
        sender
    });
    let _ = writer.try_send(());
}

fn persist_to_disk() {
    if !DISK_LOADED.load(std::sync::atomic::Ordering::Acquire) { return; }
    let Some(path) = cache_path() else {
        return;
    };
    let titles = {
        let guard = cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.titles.clone()
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_vec(&titles) {
        let _ = std::fs::write(path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::{choose_localized_title, looks_polish};

    #[test]
    fn tmdb_translation_wins_over_the_english_catalog_name() {
        let title = choose_localized_title(
            "The Godfather",
            &[
                (0, "Ojciec chrzestny".to_owned()),
                (2, "The Godfather".to_owned()),
            ],
        );
        assert_eq!(title.as_deref(), Some("Ojciec chrzestny"));
    }

    #[test]
    fn a_later_translation_is_used_when_tmdb_repeats_the_catalog_name() {
        let title = choose_localized_title(
            "The Dark Knight",
            &[
                (0, "The Dark Knight".to_owned()),
                (1, "Mroczny Rycerz".to_owned()),
            ],
        );
        assert_eq!(title.as_deref(), Some("Mroczny Rycerz"));
    }

    #[test]
    fn polish_diacritics_win_even_without_a_tmdb_hit() {
        let title = choose_localized_title(
            "Blade Runner",
            &[
                (2, "Blade Runner".to_owned()),
                (1, "Łowca androidów".to_owned()),
            ],
        );
        assert_eq!(title.as_deref(), Some("Łowca androidów"));
        assert!(looks_polish("Łowca androidów"));
    }

    #[test]
    fn identical_english_names_stay_english() {
        let title = choose_localized_title(
            "Avatar",
            &[(0, "Avatar".to_owned()), (2, "Avatar".to_owned())],
        );
        assert_eq!(title.as_deref(), Some("Avatar"));
    }

    #[test]
    fn remembered_details_title_overlays_the_catalog_name() {
        assert_eq!(
            super::display("tt-test-overlay", "The Godfather"),
            "The Godfather"
        );
        assert!(super::remember("tt-test-overlay", "Ojciec chrzestny"));
        assert_eq!(
            super::display("tt-test-overlay", "The Godfather"),
            "Ojciec chrzestny"
        );
    }
}
