//! Available language/quality combinations, without assuming that plain PL means dubbing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Version {
    Subtitles,
    VoiceOver,
    Dubbing,
}

impl Version {
    pub fn label(self) -> &'static str {
        match self {
            Self::Subtitles => "Napisy",
            Self::VoiceOver => "Lektor",
            Self::Dubbing => "Dubbing",
        }
    }
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "Napisy" => Some(Self::Subtitles),
            "Lektor" => Some(Self::VoiceOver),
            "Dubbing" => Some(Self::Dubbing),
            _ => None,
        }
    }
}

pub fn classify(name: &str, description: &str) -> Vec<Version> {
    let raw = format!("{name} {description}").to_lowercase();
    let words: Vec<_> = raw
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let has = |w| words.contains(&w);
    let polish = ["pl", "pol", "polish", "polski", "polskie"]
        .into_iter()
        .any(has)
        || raw.contains("🇵🇱");
    let paired = |tokens: &[&str]| {
        raw.split([',', ';', '\n', '|']).any(|clause| {
            let words: Vec<_> = clause
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .collect();
            words.windows(2).any(|p| {
                let pl = |s| matches!(s, "pl" | "pol" | "polish" | "polski");
                (pl(p[0]) && tokens.contains(&p[1])) || (pl(p[1]) && tokens.contains(&p[0]))
            })
        })
    };
    let subs = has("plsub")
        || has("plsubs")
        || has("subpl")
        || paired(&["sub", "subs", "subtitles", "napisy"]);
    let any_subs = [
        "sub",
        "subs",
        "subtitles",
        "napisy",
        "plsub",
        "plsubs",
        "subpl",
    ]
    .into_iter()
    .any(has);
    let dub = has("dubbingpl")
        || has("dubpl")
        || has("pldub")
        || paired(&["dub", "dubbing", "dubbed"])
        || (polish && !any_subs && ["dub", "dubbing", "dubbed"].into_iter().any(has));
    let lektor = has("lektorpl")
        || has("pllektor")
        || paired(&["lektor", "voiceover"])
        || (polish && !any_subs && has("lektor"));
    let is_sub = |word: &str| {
        matches!(
            word,
            "sub" | "subs" | "subtitles" | "napisy" | "plsub" | "plsubs"
        )
    };
    let foreign = words.iter().enumerate().any(|(index, word)| {
        [
            "eng", "english", "en", "japanese", "jpn", "french", "german", "spanish", "original",
        ]
        .contains(word)
            && !index.checked_sub(1).is_some_and(|i| is_sub(words[i]))
            && !words.get(index + 1).is_some_and(|w| is_sub(w))
    }) || ["🇬🇧", "🇺🇸", "🇯🇵", "🇫🇷", "🇩🇪", "🇪🇸"]
        .iter()
        .any(|flag| raw.contains(flag));
    let mut result = Vec::new();
    // An original/foreign soundtrack can use independently supplied Polish subtitles.
    // Ambiguous PL-only and unlabelled sources remain unclassified.
    if subs || foreign {
        result.push(Version::Subtitles);
    }
    if lektor {
        result.push(Version::VoiceOver);
    }
    if dub {
        result.push(Version::Dubbing);
    }
    result
}

pub struct Source<'a> {
    pub quality: &'a str,
    pub versions: &'a [Version],
}

#[derive(Debug, Default)]
pub struct Picker {
    pub versions: Vec<String>,
    pub qualities: Vec<String>,
    pub version: String,
    pub quality: String,
    pub available: bool,
}

pub fn picker(
    sources: &[Source<'_>],
    requested_version: Option<Version>,
    requested_quality: Option<&str>,
    preferred: Version,
) -> Picker {
    let all = [Version::Subtitles, Version::VoiceOver, Version::Dubbing];
    let available: Vec<_> = all
        .into_iter()
        .filter(|v| sources.iter().any(|s| s.versions.contains(v)))
        .collect();
    let fallback = match preferred {
        Version::Dubbing => [Version::Dubbing, Version::VoiceOver, Version::Subtitles],
        Version::VoiceOver => [Version::VoiceOver, Version::Subtitles, Version::Dubbing],
        Version::Subtitles => [Version::Subtitles, Version::VoiceOver, Version::Dubbing],
    };
    let version = requested_version
        .filter(|v| available.contains(v))
        .or_else(|| fallback.into_iter().find(|v| available.contains(v)));
    let qualities = super::ordered_quality_tabs(
        sources
            .iter()
            .filter(|s| version.is_none_or(|v| s.versions.contains(&v)))
            .map(|s| s.quality),
    );
    let quality = requested_quality
        .filter(|q| qualities.contains(q))
        .unwrap_or_else(|| super::preferred_quality_tab(&qualities))
        .to_owned();
    Picker {
        versions: available.iter().map(|v| v.label().to_owned()).collect(),
        qualities: qualities.iter().map(|q| (*q).to_owned()).collect(),
        version: version.map(|v| v.label().to_owned()).unwrap_or_default(),
        available: !qualities.is_empty(),
        quality,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognizes_only_explicit_polish_dub_and_voiceover() {
        assert_eq!(classify("Movie PL DUB", ""), vec![Version::Dubbing]);
        assert_eq!(classify("Movie", "LEKTOR PL"), vec![Version::VoiceOver]);
        assert_eq!(classify("Movie", "PLSUB"), vec![Version::Subtitles]);
        assert_eq!(
            classify("Movie", "English dubbing, Polish subtitles"),
            vec![Version::Subtitles]
        );
        assert_eq!(
            classify("Movie", "English subtitles, Polish dubbing"),
            vec![Version::Dubbing]
        );
        assert!(classify("Movie MULTI PL", "").is_empty());
        assert!(classify("Movie German dubbing", "").contains(&Version::Dubbing) == false);
        assert_eq!(
            classify("PL DUB ENG", ""),
            vec![Version::Subtitles, Version::Dubbing]
        );
    }
    #[test]
    fn only_voiceover_720_is_selected_even_for_a_cartoon() {
        let sources = [Source {
            quality: "720p",
            versions: &[Version::VoiceOver],
        }];
        let p = picker(&sources, None, None, Version::Dubbing);
        assert_eq!(p.version, "Lektor");
        assert_eq!(p.quality, "720p");
        assert_eq!(p.versions, ["Lektor"]);
        assert_eq!(p.qualities, ["720p"]);
    }
    #[test]
    fn changing_version_adapts_quality_without_inventing_combinations() {
        let sources = [
            Source {
                quality: "720p",
                versions: &[Version::VoiceOver],
            },
            Source {
                quality: "1080p",
                versions: &[Version::Subtitles],
            },
            Source {
                quality: "4K",
                versions: &[Version::Dubbing],
            },
        ];
        let p = picker(&sources, None, None, Version::Dubbing);
        assert_eq!((p.version.as_str(), p.quality.as_str()), ("Dubbing", "4K"));
        let p = picker(
            &sources,
            Some(Version::VoiceOver),
            Some("4K"),
            Version::Subtitles,
        );
        assert_eq!((p.version.as_str(), p.quality.as_str()), ("Lektor", "720p"));
        assert_eq!(p.qualities, ["720p"]);
        assert_eq!(p.versions.len(), 3);
        let p = picker(&sources, None, None, Version::Subtitles);
        assert_eq!(
            (p.version.as_str(), p.quality.as_str()),
            ("Napisy", "1080p")
        );
    }
    #[test]
    fn empty_unknown_and_disappearing_results_are_handled() {
        assert!(!picker(&[], None, None, Version::Subtitles).available);
        let sources = [Source {
            quality: "Auto",
            versions: &[],
        }];
        let p = picker(
            &sources,
            Some(Version::Dubbing),
            Some("1080p"),
            Version::Subtitles,
        );
        assert!(p.available);
        assert!(p.versions.is_empty());
        assert_eq!(p.quality, "Auto");
        let sources = [Source {
            quality: "720p",
            versions: &[Version::VoiceOver],
        }];
        let p = picker(
            &sources,
            Some(Version::Dubbing),
            Some("4K"),
            Version::Subtitles,
        );
        assert_eq!((p.version.as_str(), p.quality.as_str()), ("Lektor", "720p"));
    }
}
