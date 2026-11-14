//! The home page has an explicit movie/series plan, independent of addon order.
use futures::StreamExt;
use serde::Serialize;
use std::{collections::HashSet, ops::Deref};
use stremio_core::{
    constants::{CINEMETA_URL, GENRES_LINK_CATEGORY, IMDB_LINK_CATEGORY},
    models::{
        catalogs_with_extra::{CatalogsWithExtra, Selected},
        common::{Loadable, ResourceLoadable},
        ctx::Ctx,
    },
    runtime::{
        EffectFuture, Effects, Env, EnvError, EnvFutureExt, UpdateWithCtx,
        msg::{Action, ActionCatalogsWithExtra, ActionLoad, Internal, Msg},
    },
    types::{
        addon::{Descriptor, ExtraValue, ResourcePath, ResourceRequest, ResourceResponse},
        resource::MetaItemPreview,
    },
};

pub const MOVIE_GENRES: [(&str, &str); 4] = [
    ("Horror", "Horrory — najwyżej oceniane"),
    ("Thriller", "Thrillery — najwyżej oceniane"),
    ("Comedy", "Komedie — najwyżej oceniane"),
    ("Crime", "Filmy kryminalne — najwyżej oceniane"),
];
pub const CHILDREN_GENRES: [(&str, &str); 4] = [
    ("Family", "Dziecięce i rodzinne"),
    ("Animation", "Animacje dla dzieci"),
    ("Adventure", "Rodzinne przygody"),
    ("Comedy", "Rodzinne komedie"),
];

pub fn movie_genres() -> &'static [(&'static str, &'static str); 4] {
    if crate::config::with_config(|c| c.children_recommendations) {
        &CHILDREN_GENRES
    } else {
        &MOVIE_GENRES
    }
}

const MOVIE_PAGES: usize = 5;

#[derive(Default, Clone, Debug, Serialize)]
#[serde(transparent)]
pub struct HomeCatalogs(CatalogsWithExtra);

impl Deref for HomeCatalogs {
    type Target = CatalogsWithExtra;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

fn request(base: url::Url, kind: &str, id: &str) -> ResourceRequest {
    ResourceRequest {
        base,
        path: ResourcePath {
            resource: "catalog".into(),
            r#type: kind.into(),
            id: id.into(),
            extra: vec![],
        },
    }
}

pub fn fallback_request(kind: &str, genre: Option<&str>, addons: &[Descriptor]) -> ResourceRequest {
    let base = addons
        .iter()
        .find(|a| a.manifest.id == "com.linvo.cinemeta")
        .map(|a| a.transport_url.clone())
        .unwrap_or_else(|| CINEMETA_URL.clone());
    let mut request = request(
        base,
        kind,
        if kind == "movie" { "imdbRating" } else { "top" },
    );
    if let Some(genre) = genre {
        request.path.extra.push(ExtraValue {
            name: "genre".into(),
            value: genre.into(),
        });
    }
    request
}

fn planned_requests(addons: &[Descriptor]) -> Vec<ResourceRequest> {
    planned_requests_for(
        addons,
        crate::config::with_config(|c| c.children_recommendations),
    )
}

fn planned_requests_for(addons: &[Descriptor], children: bool) -> Vec<ResourceRequest> {
    if children {
        return ["movie", "series"]
            .into_iter()
            .map(|kind| fallback_request(kind, Some("Family"), addons))
            .collect();
    }
    let cinema = addons
        .iter()
        .find(|a| a.manifest.id == "org.debridin.cinema");
    [
        ("movie", "tmdb_discover_top_movie"),
        ("series", "tmdb_discover_trending_series"),
    ]
    .into_iter()
    .map(|(kind, id)| {
        cinema
            .filter(|a| {
                a.manifest
                    .catalogs
                    .iter()
                    .any(|c| c.r#type == kind && c.id == id)
            })
            .map(|a| request(a.transport_url.clone(), kind, id))
            .unwrap_or_else(|| fallback_request(kind, None, addons))
    })
    .collect()
}

impl HomeCatalogs {
    fn select(&mut self, addons: &[Descriptor]) -> Effects {
        let requests = planned_requests(addons);
        if self.0.selected.is_some()
            && self
                .0
                .catalogs
                .iter()
                .filter_map(|c| c.first())
                .map(|p| &p.request)
                .eq(requests.iter())
        {
            return Effects::none().unchanged();
        }
        self.0.selected = Some(Selected {
            r#type: None,
            extra: vec![],
        });
        self.0.catalogs = requests
            .into_iter()
            .map(|request| {
                vec![ResourceLoadable {
                    request,
                    content: None,
                }]
            })
            .collect();
        Effects::none()
    }

    fn load_range<E: Env + 'static>(
        &mut self,
        range: &std::ops::Range<usize>,
        addons: &[Descriptor],
    ) -> Effects {
        let mut effects = Vec::new();
        for (index, catalog) in self.0.catalogs.iter_mut().enumerate() {
            let Some(page) = catalog.first_mut() else {
                continue;
            };
            if index < range.start || index > range.end || page.content.is_some() {
                continue;
            }
            page.content = Some(Loadable::Loading);
            let request = page.request.clone();
            let genre = request
                .path
                .extra
                .iter()
                .find(|extra| extra.name == "genre")
                .map(|extra| extra.value.as_str());
            let fallback = fallback_request(&request.path.r#type, genre, addons);
            effects.push(
                EffectFuture::Concurrent(
                    async move {
                        let result = match fetch_pool::<E>(&request).await {
                            Ok(items) => Ok(items),
                            Err(_) if request != fallback => fetch_pool::<E>(&fallback).await,
                            Err(error) => Err(error),
                        }
                        .map(|metas| ResourceResponse::Metas { metas });
                        Msg::Internal(Internal::ResourceRequestResult(request, Box::new(result)))
                    }
                    .boxed_env(),
                )
                .into(),
            );
        }
        if effects.is_empty() {
            Effects::none().unchanged()
        } else {
            Effects::many(effects)
        }
    }
}

impl<E: Env + 'static> UpdateWithCtx<E> for HomeCatalogs {
    fn update(&mut self, msg: &Msg, ctx: &Ctx) -> Effects {
        match msg {
            Msg::Action(Action::Load(ActionLoad::CatalogsWithExtra(_))) => {
                self.select(&ctx.profile.addons)
            }
            Msg::Internal(Internal::ProfileChanged) if self.0.selected.is_some() => {
                self.select(&ctx.profile.addons)
            }
            Msg::Action(Action::CatalogsWithExtra(ActionCatalogsWithExtra::LoadRange(range))) => {
                self.load_range::<E>(range, &ctx.profile.addons)
            }
            // The movie pool is already bounded and paginated. "See all" opens Discover.
            Msg::Action(Action::CatalogsWithExtra(ActionCatalogsWithExtra::LoadNextPage(_))) => {
                Effects::none().unchanged()
            }
            _ => <CatalogsWithExtra as UpdateWithCtx<E>>::update(&mut self.0, msg, ctx),
        }
    }
}

async fn fetch_page<E: Env + 'static>(
    request: &ResourceRequest,
) -> Result<Vec<MetaItemPreview>, EnvError> {
    let pending = E::addon_transport(&request.base).resource(&request.path);
    let response = pending.await?;
    Vec::<MetaItemPreview>::try_from(response)
        .map_err(|_| EnvError::Other("Katalog nie zwrócił listy filmów lub seriali.".into()))
}

async fn fetch_pool<E: Env + 'static>(
    request: &ResourceRequest,
) -> Result<Vec<MetaItemPreview>, EnvError> {
    let mut items = fetch_page::<E>(request).await?;
    if items.is_empty() {
        return Err(EnvError::Other(
            "Katalog jest pusty. Spróbuj ponownie później.".into(),
        ));
    }
    if request.path.r#type == "movie" {
        // Known providers page in batches of 100 (Cinema) or 50 (Cinemeta).
        // Fetch a broad top-rated pool once, shared by all four genre rows.
        let page_size = if request.path.id.starts_with("tmdb_") {
            100
        } else {
            50
        };
        let pages = futures::stream::iter((1..MOVIE_PAGES).map(|index| {
            let mut page = request.clone();
            page.path.extra.push(ExtraValue {
                name: "skip".into(),
                value: (index * page_size).to_string(),
            });
            async move { fetch_page::<E>(&page).await }
        }))
        .buffered(2)
        .collect::<Vec<_>>()
        .await;
        for page in pages {
            match page {
                Ok(page) => items.extend(page.into_iter().take(page_size)),
                Err(_) => tracing::warn!(
                    "A supplementary home movie page was unavailable; keeping loaded results"
                ),
            }
        }
        items.truncate(page_size * MOVIE_PAGES);
    }
    let mut seen = HashSet::new();
    items.retain(|item| recommendable(item, &request.path.r#type) && seen.insert(item.id.clone()));
    Ok(items)
}

fn has_genre(item: &MetaItemPreview, genre: &str) -> bool {
    item.links.iter().any(|link| {
        link.category.eq_ignore_ascii_case(GENRES_LINK_CATEGORY)
            && link.name.eq_ignore_ascii_case(genre)
    })
}

pub fn recommendable(item: &MetaItemPreview, kind: &str) -> bool {
    recommendable_for(
        item,
        kind,
        crate::config::with_config(|c| c.children_recommendations),
    )
}

fn recommendable_for(item: &MetaItemPreview, kind: &str, children: bool) -> bool {
    if children {
        return item.r#type == kind && children_content(item);
    }
    item.r#type == kind
        && matches!(kind, "movie" | "series")
        && !has_genre(item, "Animation")
        && !has_genre(item, "Anime")
        && !["kitsu:", "mal:", "anilist:"]
            .iter()
            .any(|prefix| item.id.starts_with(prefix))
        && item
            .released
            .is_none_or(|released| released <= chrono::Utc::now())
}

pub fn children_content(item: &MetaItemPreview) -> bool {
    let kind = item.r#type.as_str();
    matches!(kind, "movie" | "series")
        && ["Family", "Kids", "Children"]
            .iter()
            .any(|genre| has_genre(item, genre))
        && !["Horror", "Thriller", "Crime", "War", "Adult", "Erotic"]
            .iter()
            .any(|genre| has_genre(item, genre))
        && item
            .released
            .is_none_or(|released| released <= chrono::Utc::now())
}

/// Library/history records lack genres. Only show them when a loaded family
/// catalog supplies matching metadata; never infer suitability from a title.
pub fn known_children_ids(catalogs: &CatalogsWithExtra) -> HashSet<String> {
    catalogs
        .catalogs
        .iter()
        .flatten()
        .filter_map(|page| page.content.as_ref().and_then(Loadable::ready))
        .flatten()
        .filter(|item| children_content(item))
        .map(|item| item.id.clone())
        .collect()
}

pub fn rating(item: &MetaItemPreview) -> Option<f64> {
    item.links
        .iter()
        .find(|link| link.category.eq_ignore_ascii_case(IMDB_LINK_CATEGORY))
        .and_then(|link| link.name.replace(',', ".").parse::<f64>().ok())
        .filter(|rating| rating.is_finite() && *rating > 0.0 && *rating <= 10.0)
}

pub fn ranked_genre(items: &[MetaItemPreview], genre: &str) -> Vec<MetaItemPreview> {
    let mut seen = HashSet::new();
    let mut items = items
        .iter()
        .filter(|item| {
            recommendable(item, "movie")
                && has_genre(item, genre)
                && rating(item).is_some()
                && seen.insert(item.id.clone())
        })
        .cloned()
        .collect::<Vec<_>>();
    items.sort_by(|a, b| rating(b).unwrap().total_cmp(&rating(a).unwrap()));
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item(id: &str, kind: &str, score: &str, genres: &[&str]) -> MetaItemPreview {
        serde_json::from_value(json!({
            "id": id, "type": kind, "name": id, "imdbRating": score, "genres": genres
        }))
        .unwrap()
    }

    #[test]
    fn children_catalog_requests_keep_family_filter_for_movies_and_series() {
        let requests = planned_requests_for(&[], true);
        assert_eq!(requests.len(), 2);
        for request in requests {
            assert!(
                request
                    .path
                    .extra
                    .iter()
                    .any(|extra| extra.name == "genre" && extra.value == "Family")
            );
        }
    }

    #[test]
    fn history_and_library_require_known_family_metadata() {
        let mut home = HomeCatalogs::default();
        home.select(&[]);
        home.0.catalogs[0][0].content = Some(Loadable::Ready(vec![
            item("family", "movie", "8.0", &["Family", "Animation"]),
            item("adult", "movie", "8.0", &["Animation", "Comedy"]),
            item("scary", "movie", "8.0", &["Family", "Horror"]),
        ]));
        assert_eq!(
            known_children_ids(&home.0),
            HashSet::from(["family".into()])
        );
    }

    #[test]
    fn children_recommendations_require_family_metadata_and_exclude_adult_animation() {
        assert!(recommendable_for(
            &item("family", "movie", "8.1", &["Family", "Animation"]),
            "movie",
            true
        ));
        assert!(!recommendable_for(
            &item("adult-animation", "movie", "8.1", &["Animation", "Comedy"]),
            "movie",
            true
        ));
        assert!(!recommendable_for(
            &item("scary", "movie", "8.1", &["Family", "Horror"]),
            "movie",
            true
        ));
        assert!(!recommendable_for(
            &item("unknown", "movie", "8.1", &[]),
            "movie",
            true
        ));
        assert!(recommendable_for(
            &item("thriller", "movie", "8.1", &["Thriller"]),
            "movie",
            false
        ));
    }

    #[test]
    fn genre_ranking_excludes_unrated_animation_wrong_types_and_duplicates() {
        let items = vec![
            item("low", "movie", "6.1", &["Horror"]),
            item("best", "movie", "8.5", &["Horror", "Thriller"]),
            item("best", "movie", "8.5", &["Horror"]),
            item("anime", "movie", "9.9", &["Horror", "Animation"]),
            item("channel", "tv", "9.8", &["Horror"]),
            item("series", "series", "9.8", &["Horror"]),
            item("other-genre", "movie", "9.7", &["Comedy"]),
            item("unrated", "movie", "", &["Horror"]),
            item("invalid-score", "movie", "NaN", &["Horror"]),
        ];
        let result = ranked_genre(&items, "Horror");
        assert_eq!(
            result.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["best", "low"]
        );
    }

    #[test]
    fn equal_ratings_keep_provider_order_and_unreleased_titles_are_hidden() {
        let mut future = item("future", "movie", "9.9", &["Crime"]);
        future.released = Some(chrono::Utc::now() + chrono::Duration::days(365));
        let items = vec![
            item("first", "movie", "8.2", &["Crime"]),
            future,
            item("second", "movie", "8.2", &["Crime"]),
        ];
        assert_eq!(
            ranked_genre(&items, "Crime")
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
    }

    #[test]
    fn unrelated_addons_cannot_add_home_sections() {
        let addon: Descriptor = serde_json::from_value(json!({
            "transportUrl": "https://example.test/manifest.json",
            "manifest": { "id": "unrelated", "version": "1.0.0", "name": "Anime and TV",
                "resources": ["catalog"], "types": ["anime", "tv"],
                "catalogs": [{"id":"anime", "type":"anime"}, {"id":"live", "type":"tv"}] }
        }))
        .unwrap();
        let mut home = HomeCatalogs::default();
        home.select(&[addon]);
        assert_eq!(home.catalogs.len(), 2);
        assert_eq!(home.catalogs[0][0].request.path.r#type, "movie");
        assert_eq!(home.catalogs[1][0].request.path.r#type, "series");
        assert_eq!(home.catalogs[1][0].request.path.id, "top");
        assert!(!home.select(&[]).has_changed);
    }

    #[test]
    fn installed_trending_catalog_is_preferred_and_fallback_keeps_genre() {
        let addon: Descriptor = serde_json::from_value(json!({
            "transportUrl": "https://example.test/manifest.json",
            "manifest": { "id": "org.debridin.cinema", "version": "1.0.0", "name": "Cinema",
                "resources": ["catalog"], "types": ["movie", "series"],
                "catalogs": [{"id":"tmdb_discover_top_movie", "type":"movie"},
                    {"id":"tmdb_discover_trending_series", "type":"series"}] }
        }))
        .unwrap();
        let plan = planned_requests(&[addon]);
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].path.id, "tmdb_discover_top_movie");
        assert_eq!(plan[1].path.id, "tmdb_discover_trending_series");
        assert_eq!(
            fallback_request("movie", Some("Thriller"), &[]).path.extra[0].value,
            "Thriller"
        );
    }
}
