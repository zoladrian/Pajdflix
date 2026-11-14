//! Children's playback prefers understandable spoken audio, with Polish-only subtitles as fallback.
pub fn polish(language: Option<&str>, title: Option<&str>) -> bool {
    let language = language.unwrap_or("").trim().to_ascii_lowercase();
    let base = language.split(['-', '_']).next().unwrap_or_default();
    if matches!(base, "pl" | "pol" | "polish" | "polski") {
        return true;
    }
    if !matches!(base, "" | "und" | "unknown" | "mul") {
        return false;
    }
    // Track titles can identify otherwise untagged Polish audio.
    title
        .unwrap_or("")
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| matches!(word, "pl" | "pol" | "polski" | "polish"))
}

pub fn preferred_audio<'a>(
    tracks: impl Iterator<Item = (usize, Option<&'a str>, Option<&'a str>, bool)>,
) -> Option<usize> {
    preferred_polish_audio(tracks, None)
}

/// Some(true) requests dubbing, Some(false) voiceover; None is the children's fallback.
pub fn preferred_polish_audio<'a>(
    tracks: impl Iterator<Item = (usize, Option<&'a str>, Option<&'a str>, bool)>,
    dubbing: Option<bool>,
) -> Option<usize> {
    tracks
        .filter_map(|(index, language, title, selected)| {
            if !polish(language, title) {
                return None;
            }
            let title = title.unwrap_or("").to_lowercase();
            let dub = title.contains("dubbing")
                || title
                    .split(|c: char| !c.is_alphanumeric())
                    .any(|w| w == "dub");
            let lektor = title.contains("lektor") || title.contains("voiceover");
            if (dubbing == Some(true) && lektor && !dub)
                || (dubbing == Some(false) && dub && !lektor)
            {
                return None;
            }
            let tier = if title.contains("commentary")
                || title.contains("komentarz")
                || title.contains("audiodes")
            {
                3
            } else if (dubbing == Some(false) && lektor) || (dubbing != Some(false) && dub) {
                0
            } else if title.contains("lektor") {
                2
            } else {
                1
            };
            (tier < 3).then_some((tier, !selected, index))
        })
        .min()
        .map(|(_, _, index)| index)
}

pub fn preferred_original_audio<'a>(
    tracks: impl Iterator<Item = (usize, Option<&'a str>, Option<&'a str>, bool)>,
) -> Option<usize> {
    tracks
        .filter_map(|(index, language, title, selected)| {
            let language = language?.to_ascii_lowercase();
            let title = title.unwrap_or_default().to_lowercase();
            if polish(Some(&language), None)
                || matches!(language.as_str(), "und" | "unknown" | "mul" | "")
                || title.contains("commentary")
                || title.contains("audiodes")
            {
                return None;
            }
            Some((!title.contains("original"), !selected, index))
        })
        .min()
        .map(|(_, _, index)| index)
}

pub fn preferred_subtitle<'a>(
    polish_audio_available: bool,
    tracks: impl Iterator<Item = (usize, Option<&'a str>, bool)>,
) -> Option<usize> {
    if polish_audio_available {
        return None;
    }
    tracks
        .filter_map(|(index, language, selected)| {
            polish(language, None).then_some((!selected, index))
        })
        .min()
        .map(|(_, index)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_voiceover_and_dubbing_select_different_polish_tracks() {
        let tracks = [
            (0, Some("pol"), Some("Dubbing"), true),
            (1, Some("pl"), Some("Lektor PL"), false),
            (2, Some("eng"), Some("Original audio"), false),
        ];
        assert_eq!(
            preferred_polish_audio(tracks.into_iter(), Some(true)),
            Some(0)
        );
        assert_eq!(
            preferred_polish_audio(tracks.into_iter(), Some(false)),
            Some(1)
        );
        assert_eq!(preferred_original_audio(tracks.into_iter()), Some(2));
        assert_eq!(
            preferred_polish_audio(tracks[..1].iter().copied(), Some(false)),
            None
        );
    }
    #[test]
    fn dubbing_beats_lektor_and_commentary_and_handles_late_tracks() {
        assert!(!polish(
            Some("eng"),
            Some("English audio; Polish subtitles")
        ));
        assert_eq!(
            preferred_audio(
                [
                    (0, Some("pl"), Some("Lektor"), true),
                    (1, Some("pol"), Some("Dubbing"), false)
                ]
                .into_iter()
            ),
            Some(1)
        );
        assert_eq!(
            preferred_audio([(0, Some("pol"), Some("Komentarz reżysera"), true)].into_iter()),
            None
        );
        assert_eq!(
            preferred_audio(
                [
                    (0, Some("en"), None, true),
                    (1, None, Some("Polski dubbing"), false)
                ]
                .into_iter()
            ),
            Some(1)
        );
    }
    #[test]
    fn subtitles_stay_off_with_polish_audio_and_never_fall_back_to_english() {
        assert_eq!(
            preferred_subtitle(true, [(0, Some("pol"), true)].into_iter()),
            None
        );
        assert_eq!(
            preferred_subtitle(
                false,
                [(0, Some("eng"), true), (1, Some("pl-PL"), false)].into_iter()
            ),
            Some(1)
        );
        assert_eq!(
            preferred_subtitle(false, [(0, Some("eng"), true)].into_iter()),
            None
        );
    }
}
