//! Subtitle preference is independent of addon response order.
pub fn language_key(language: &str) -> String {
    let language = language.trim().to_ascii_lowercase();
    let base = language.split(['-', '_']).next().unwrap_or_default();
    match base {
        "pl" | "pol" | "polish" | "polski" => "pol".to_owned(),
        "en" | "eng" | "english" => "eng".to_owned(),
        _ => base.to_owned(),
    }
}

pub fn preferred_track<'a>(
    tracks: impl Iterator<Item = (usize, Option<&'a str>, bool)>,
    preferred: &str,
) -> Option<usize> {
    let preferred = language_key(preferred);
    tracks
        .filter_map(|(index, language, selected)| {
            let language = language_key(language?);
            let priority = if language == preferred {
                0
            } else if language == "eng" {
                1
            } else {
                return None;
            };
            Some((priority, !selected, index))
        })
        .min()
        .map(|(_, _, index)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_polish_track_replaces_english_fallback() {
        let tracks = [(0, Some("eng"), true), (1, Some("pl-PL"), false)];
        assert_eq!(preferred_track(tracks.into_iter(), "pol"), Some(1));
    }

    #[test]
    fn english_is_used_when_polish_is_missing() {
        let tracks = [(0, Some("spa"), true), (1, Some("en_US"), false)];
        assert_eq!(preferred_track(tracks.into_iter(), "pol"), Some(1));
        assert_eq!(
            preferred_track([(0, Some("spa"), true)].into_iter(), "pol"),
            None
        );
    }

    #[test]
    fn selected_variant_of_the_preferred_language_is_kept() {
        let tracks = [(0, Some("pol"), false), (1, Some("POL"), true)];
        assert_eq!(preferred_track(tracks.into_iter(), "pl"), Some(1));
    }
}
