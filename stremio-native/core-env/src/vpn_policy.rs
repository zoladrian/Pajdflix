//! Production-country policy. Unknown origin requires VPN or an explicit user declaration.
//! Audio, subtitles, translated titles and filenames are never evidence of origin.
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::{Mutex, OnceLock},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Production {
    Polish,
    Foreign,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VpnGate {
    /// Confirmed foreign production: no VPN prompt.
    None,
    /// Polish production: play without VPN, but tell the user to use one, and
    /// refuse if this process is seeding.
    Recommended,
    /// Unknown origin or unmapped torrent without an explicit user declaration.
    Required,
}

#[derive(Default)]
struct Registry {
    countries: HashMap<(String, String), Production>,
    torrents: HashMap<String, Vec<(String, String)>>,
    user_confirmed_foreign: HashSet<String>,
}
static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(Default::default)
}

fn country(value: &str) -> Production {
    match value.trim().to_lowercase().as_str() {
        "pl" | "pol" | "poland" | "polska" | "pologne" | "polen" | "polonia" => Production::Polish,
        "us"
        | "usa"
        | "united states"
        | "united states of america"
        | "gb"
        | "uk"
        | "united kingdom"
        | "ca"
        | "canada"
        | "fr"
        | "france"
        | "de"
        | "germany"
        | "es"
        | "spain"
        | "it"
        | "italy"
        | "jp"
        | "japan"
        | "kr"
        | "south korea"
        | "cn"
        | "china"
        | "hk"
        | "hong kong"
        | "au"
        | "australia"
        | "nz"
        | "new zealand"
        | "in"
        | "india"
        | "se"
        | "sweden"
        | "no"
        | "norway"
        | "dk"
        | "denmark"
        | "fi"
        | "finland"
        | "nl"
        | "netherlands"
        | "be"
        | "belgium"
        | "at"
        | "austria"
        | "ch"
        | "switzerland"
        | "cz"
        | "czech republic"
        | "czechia"
        | "sk"
        | "slovakia"
        | "hu"
        | "hungary"
        | "ie"
        | "ireland"
        | "pt"
        | "portugal"
        | "ro"
        | "romania"
        | "bg"
        | "bulgaria"
        | "ua"
        | "ukraine"
        | "by"
        | "belarus"
        | "ru"
        | "russia"
        | "russian federation"
        | "mx"
        | "mexico"
        | "br"
        | "brazil"
        | "ar"
        | "argentina"
        | "za"
        | "south africa"
        | "tr"
        | "turkey"
        | "türkiye"
        | "gr"
        | "greece"
        | "is"
        | "iceland"
        | "il"
        | "israel"
        | "ir"
        | "iran"
        | "tw"
        | "taiwan"
        | "th"
        | "thailand" => Production::Foreign,
        _ => Production::Unknown,
    }
}

pub fn classify(meta: &Value) -> Production {
    let mut values = Vec::new();
    if let Some(countries) = meta.get("production_countries").and_then(Value::as_array) {
        values.extend(
            countries
                .iter()
                .map(|c| country(c.get("iso_3166_1").and_then(Value::as_str).unwrap_or(""))),
        );
    }
    if let Some(countries) = meta.get("origin_country").and_then(Value::as_array) {
        values.extend(countries.iter().map(|c| country(c.as_str().unwrap_or(""))));
    }
    if let Some(countries) = meta.get("country").and_then(Value::as_str) {
        values.extend(countries.split([',', '/', ';', '|']).map(country));
    }
    if values.contains(&Production::Polish) {
        Production::Polish
    } else if !values.is_empty() && values.iter().all(|v| *v == Production::Foreign) {
        Production::Foreign
    } else {
        Production::Unknown
    }
}

/// Called on both cached and fresh addon metadata before Core drops extra fields.
pub(crate) fn observe_response(body: &[u8]) {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return;
    };
    let Some(meta) = value.get("meta") else {
        return;
    };
    let (Some(id), Some(kind)) = (
        meta.get("id").and_then(Value::as_str),
        meta.get("type").and_then(Value::as_str),
    ) else {
        return;
    };
    let production = classify(meta);
    let mut state = registry().lock().unwrap_or_else(|e| e.into_inner());
    let key = (kind.to_owned(), id.to_owned());
    // At capacity new identities remain Unknown; never erase Polish evidence.
    if state.countries.len() >= 4096 && !state.countries.contains_key(&key) {
        return;
    }
    let previous = state.countries.entry(key).or_insert(Production::Unknown);
    // A later sparse response cannot erase known Polish origin.
    if *previous != Production::Polish && production != Production::Unknown {
        *previous = production;
    }
}

pub fn register_torrent(hash: &str, media_type: &str, media_id: &str) {
    let mut state = registry().lock().unwrap_or_else(|e| e.into_inner());
    let hash = hash.to_ascii_lowercase();
    if state.torrents.len() >= 4096 && !state.torrents.contains_key(&hash) {
        return;
    }
    let keys = state.torrents.entry(hash).or_default();
    let key = (media_type.to_owned(), media_id.to_owned());
    if !keys.contains(&key) {
        keys.push(key);
    }
}

pub fn production_of(media_type: &str, media_id: &str) -> Production {
    let state = registry().lock().unwrap_or_else(|e| e.into_inner());
    state
        .countries
        .get(&(media_type.to_owned(), media_id.to_owned()))
        .copied()
        .unwrap_or(Production::Unknown)
}

pub fn gate(hash: Option<&str>) -> VpnGate {
    let Some(hash) = hash else {
        return VpnGate::Required;
    };
    let state = registry().lock().unwrap_or_else(|e| e.into_inner());
    let confirmed = state.user_confirmed_foreign.contains(&hash.to_ascii_lowercase());
    let Some(keys) = state.torrents.get(&hash.to_ascii_lowercase()) else {
        return if confirmed { VpnGate::None } else { VpnGate::Required };
    };
    if keys.is_empty() {
        return VpnGate::Required;
    }
    let mut saw_polish = false;
    for key in keys {
        match state.countries.get(key).copied().unwrap_or(Production::Unknown) {
            Production::Unknown if !confirmed => return VpnGate::Required,
            Production::Unknown => {},
            Production::Polish => saw_polish = true,
            Production::Foreign => {}
        }
    }
    if saw_polish {
        VpnGate::Recommended
    } else {
        VpnGate::None
    }
}

/// Explicit user declaration, scoped to this torrent for the current app session.
/// It does not replace production metadata or erase evidence of Polish origin.
pub fn confirm_foreign_source(hash: &str) {
    if hash.len() != 40 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return;
    }
    let mut state = registry().lock().unwrap_or_else(|e| e.into_inner());
    if state.user_confirmed_foreign.len() < 4096 {
        state.user_confirmed_foreign.insert(hash.to_ascii_lowercase());
    }
}

pub fn requires_vpn(hash: Option<&str>) -> bool {
    gate(hash) == VpnGate::Required
}

pub fn recommends_vpn(hash: Option<&str>) -> bool {
    gate(hash) == VpnGate::Recommended
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn production_country_handles_coproductions_and_ambiguous_values() {
        assert_eq!(
            classify(&json!({"country":"United Kingdom, Poland"})),
            Production::Polish
        );
        assert_eq!(
            classify(&json!({"production_countries":[{"iso_3166_1":"DE"},{"iso_3166_1":"PL"}]})),
            Production::Polish
        );
        assert_eq!(
            classify(&json!({"origin_country":["US","GB"]})),
            Production::Foreign
        );
        for value in [
            json!({}),
            json!({"country":"unknown"}),
            json!({"country":"US, XX"}),
            json!({"original_language":"pl"}),
        ] {
            assert_eq!(classify(&value), Production::Unknown);
        }
        assert_eq!(
            classify(
                &json!({"country":"United States of America","audioLanguage":"pol","name":"Polski tytuł"})
            ),
            Production::Foreign
        );
    }
    #[test]
    fn missing_mapping_and_conflicting_sources_cannot_bypass_vpn() {
        let hash = "policy-test-foreign";
        assert!(requires_vpn(Some(hash)));
        observe_response(br#"{"meta":{"id":"policy-id","type":"movie","country":"US"}}"#);
        register_torrent(hash, "movie", "policy-id");
        assert_eq!(gate(Some(hash)), VpnGate::None);
        assert!(!requires_vpn(Some(hash)));
        observe_response(br#"{"meta":{"id":"policy-id","type":"movie","country":"Poland"}}"#);
        assert_eq!(gate(Some(hash)), VpnGate::Recommended);
        assert!(!requires_vpn(Some(hash)));
        assert!(recommends_vpn(Some(hash)));
        observe_response(br#"{"meta":{"id":"policy-id","type":"movie","country":"US"}}"#);
        assert_eq!(gate(Some(hash)), VpnGate::Recommended);
        assert!(requires_vpn(None));
        assert_eq!(production_of("movie", "policy-id"), Production::Polish);
    }
}
