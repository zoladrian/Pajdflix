use serde::{Deserialize, Serialize};
pub mod polish_versions;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RankingMode {
    #[default]
    Smart,
    Quality,
    Smallest,
    Seeders,
    Original,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DebridAvailability {
    Cached,
    Uncached,
    Unknown,
    ProviderUnavailable,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RankInput {
    pub id: String,
    pub name: String,
    pub description: String,
    pub addon: String,
    pub original_index: usize,
    pub size_bytes: Option<u64>,
    pub seeders: Option<u32>,
    pub debrid: Option<DebridAvailability>,
    /// Direct HTTP/YouTube/etc. can start without waiting on torrent pieces.
    #[serde(default)]
    pub http: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReasonKind {
    Positive,
    Negative,
    Neutral,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScoreReason {
    pub kind: ReasonKind,
    pub label: String,
    pub points: i32,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ParsedStream {
    pub quality_height: Option<u16>,
    pub source: Option<String>,
    pub codec: Option<String>,
    pub hdr: bool,
    pub dolby_vision: bool,
    pub audio: Option<String>,
    pub languages: Vec<String>,
    pub release_group: Option<String>,
    pub container: Option<String>,
    pub suspicious: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RankedStream {
    pub input: RankInput,
    pub parsed: ParsedStream,
    pub score: i32,
    pub reasons: Vec<ScoreReason>,
    pub filtered: bool,
}

/// Groups a parsed height into the source-picker tabs. Unlabeled sources land
/// on Auto, rather than advertising a resolution the provider did not report.
pub fn quality_tab(height: Option<u16>) -> &'static str {
    match height {
        Some(height) if height >= 4320 => "8K",
        Some(height) if height >= 2160 => "4K",
        Some(height) if height >= 1440 => "1440p",
        Some(height) if height >= 1080 => "1080p",
        Some(height) if height >= 720 => "720p",
        Some(height) if height >= 480 => "480p",
        Some(_) => "SD",
        None => "Auto",
    }
}

/// Only the qualities actually reported by the current sources.
pub fn ordered_quality_tabs<I, S>(tabs: I) -> Vec<&'static str>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    const ORDER: [&str; 8] = ["SD", "480p", "720p", "1080p", "1440p", "4K", "8K", "Auto"];
    let mut found = [false; 8];
    for tab in tabs {
        if let Some(index) = ORDER.iter().position(|q| *q == tab.as_ref()) { found[index] = true; }
    }
    ORDER.into_iter().zip(found).filter_map(|(q, present)| present.then_some(q)).collect()
}

/// Watch defaults to 1080p, then 720p, then 4K when the preferred rung is missing.
pub fn preferred_quality_tab(tabs: &[&str]) -> &'static str {
    ["1080p", "720p", "1440p", "480p", "4K", "SD", "8K", "Auto"]
        .into_iter().find(|q| tabs.contains(q)).unwrap_or("")
}

/// A provider can attach a clip to a movie's stream response. Keep it available
/// for explicit selection, but never use it as the movie's automatic source.
pub fn movie_autoplay_exclusion(youtube: bool, name: &str, description: &str) -> Option<&'static str> {
    if youtube {
        return Some("Źródło YouTube wymaga ręcznego wyboru; nie potwierdza pełnego filmu");
    }
    let text = format!("{name} {description}").to_lowercase()
        .split(|c: char| !c.is_alphanumeric()).filter(|word| !word.is_empty())
        .collect::<Vec<_>>().join(" ");
    let words = text.split_whitespace().collect::<Vec<_>>();
    let bounded = format!(" {text} ");
    if ["music video", "music videos", "lyric video", "official audio", "audio only"]
        .iter().any(|phrase| bounded.contains(&format!(" {phrase} ")))
        || words.contains(&"teledysk")
        || words.contains(&"videoclip")
    {
        return Some("Oznaczenie teledysku lub nagrania muzycznego; tylko wybór ręczny");
    }
    if text.contains("official trailer") || text.contains("official teaser")
        || matches!(name.trim().to_ascii_lowercase().as_str(), "trailer" | "teaser" | "zwiastun")
    {
        return Some("Oznaczenie zwiastuna; tylko wybór ręczny");
    }
    None
}

pub fn youtube_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    ["youtube.com", "youtu.be", "youtube-nocookie.com"].iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}

#[cfg(test)]
mod movie_autoplay_tests {
    use super::*;

    #[test]
    fn clips_cannot_be_the_automatic_movie_source() {
        assert!(movie_autoplay_exclusion(true, "Michael Jackson", "1080p").is_some());
        assert!(movie_autoplay_exclusion(false, "Michael Jackson", "Official Music Video 1080p").is_some());
        assert!(movie_autoplay_exclusion(false, "Michael", "Teledysk 2160p PL").is_some());
        assert!(movie_autoplay_exclusion(false, "Michael", "Official Trailer 1080p").is_some());
        // Musical subjects and titles containing 'Trailer' are still movies.
        for title in ["Michael.2026.1080p.WEB-DL", "Michael Jackson This Is It 2009",
            "Thriller", "Music (2021)", "Trailer Park Boys The Movie", "Music Videography Documentary"] {
            assert_eq!(movie_autoplay_exclusion(false, title, ""), None, "{title}");
        }
    }

    #[test]
    fn youtube_hosts_are_checked_at_domain_boundaries() {
        for host in ["youtu.be", "www.youtube.com", "M.YouTube.Com", "www.youtube-nocookie.com."] {
            assert!(youtube_host(host), "{host}");
        }
        assert!(!youtube_host("youtube.com.example.org"));
        assert!(!youtube_host("notyoutube.com"));
    }
}

pub fn rank_streams(
    inputs: impl IntoIterator<Item = RankInput>,
    mode: RankingMode,
    show_filtered: bool,
) -> Vec<RankedStream> {
    let mut streams = inputs
        .into_iter()
        .map(rank_one)
        .filter(|stream| show_filtered || !stream.filtered)
        .collect::<Vec<_>>();

    match mode {
        RankingMode::Original => streams.sort_by_key(|stream| stream.input.original_index),
        RankingMode::Quality => streams.sort_by(|left, right| {
            right
                .parsed
                .quality_height
                .cmp(&left.parsed.quality_height)
                .then_with(|| left.input.original_index.cmp(&right.input.original_index))
        }),
        RankingMode::Smallest => streams.sort_by(|left, right| {
            let left_size = left
                .input
                .size_bytes
                .or_else(|| parse_size_bytes_from_text(&left.input.description))
                .or_else(|| parse_size_bytes_from_text(&left.input.name));
            let right_size = right
                .input
                .size_bytes
                .or_else(|| parse_size_bytes_from_text(&right.input.description))
                .or_else(|| parse_size_bytes_from_text(&right.input.name));
            left_size
                .unwrap_or(u64::MAX)
                .cmp(&right_size.unwrap_or(u64::MAX))
                .then_with(|| left.input.original_index.cmp(&right.input.original_index))
        }),
        RankingMode::Seeders => streams.sort_by(|left, right| {
            let left_seeds = left
                .input
                .seeders
                .or_else(|| parse_seeders_from_text(&left.input.description))
                .or_else(|| parse_seeders_from_text(&left.input.name));
            let right_seeds = right
                .input
                .seeders
                .or_else(|| parse_seeders_from_text(&right.input.description))
                .or_else(|| parse_seeders_from_text(&right.input.name));
            right_seeds
                .unwrap_or_default()
                .cmp(&left_seeds.unwrap_or_default())
                .then_with(|| left.input.original_index.cmp(&right.input.original_index))
        }),
        RankingMode::Smart => streams.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| seeder_count(right).cmp(&seeder_count(left)))
                .then_with(|| stream_size(left).cmp(&stream_size(right)))
                .then_with(|| left.input.original_index.cmp(&right.input.original_index))
        }),
    }
    streams
}

/// Language tiers are stronger than quality/size preferences in children's mode.
/// Labels are provider hints; the player verifies the actual audio tracks later.
pub fn children_audio_priority(name: &str, description: &str) -> u8 {
    let raw = format!("{name} {description}").to_lowercase();
    let words = raw
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    let has = |word: &str| words.contains(&word);
    let polish = has("pl")
        || has("pol")
        || has("polish")
        || has("polski")
        || has("polskie")
        || raw.contains("🇵🇱");
    let dubbing = has("dubbing")
        || has("dub")
        || has("dubbed")
        || has("dubbingpl")
        || has("pldub")
        || has("dubpl");
    let subtitles = has("sub")
        || has("subs")
        || has("subtitles")
        || has("napisy")
        || has("subbed")
        || has("plsub")
        || has("subpl");
    let explicit_polish_dub = words.windows(2).any(|pair| {
        let is_pl = |word: &str| matches!(word, "pl" | "pol" | "polish" | "polski");
        let is_dub = |word: &str| matches!(word, "dubbing" | "dub" | "dubbed");
        (is_pl(pair[0]) && is_dub(pair[1])) || (is_dub(pair[0]) && is_pl(pair[1]))
    });
    if explicit_polish_dub
        || (polish && dubbing && !subtitles)
        || has("dubbingpl")
        || has("pldub")
        || has("dubpl")
    {
        0
    } else if has("lektor") {
        2
    } else if subtitles || has("plsub") || has("subpl") {
        4
    } else if polish {
        1
    } else if has("eng") || has("english") || has("french") || has("german") || has("spanish") {
        5
    } else {
        3
    }
}

pub fn prefer_children_audio(streams: &mut [RankedStream]) {
    for stream in streams.iter_mut() {
        let tier = children_audio_priority(&stream.input.name, &stream.input.description);
        let label = match tier {
            0 => "Polski dubbing — oznaczenie źródła",
            1 => "Polski dźwięk — rodzaj ścieżki do sprawdzenia",
            2 => "Lektor — dubbing ma pierwszeństwo",
            4 => "Napisy — wariant awaryjny",
            _ => "Niepotwierdzony polski dźwięk",
        };
        stream.reasons.push(ScoreReason {
            kind: if tier <= 1 {
                ReasonKind::Positive
            } else {
                ReasonKind::Neutral
            },
            label: label.into(),
            points: 0,
        });
    }
    streams.sort_by_key(|stream| {
        children_audio_priority(&stream.input.name, &stream.input.description)
    });
}

#[cfg(test)]
mod children_audio_tests {
    use super::*;
    #[test]
    fn polish_subtitles_are_never_evidence_of_polish_dubbing() {
        assert_eq!(children_audio_priority("Movie.1080p.PL.DUB", ""), 0);
        assert_eq!(
            children_audio_priority("Movie", "Polish subtitles, English dubbing"),
            4
        );
        assert_eq!(children_audio_priority("Movie.PLSUB", ""), 4);
        assert_eq!(children_audio_priority("Movie PL", "lektor"), 2);
        assert_eq!(children_audio_priority("Movie MULTI PL", ""), 1);
        assert_eq!(children_audio_priority("Movie", "🇵🇱 dubbing"), 0);
    }
    #[test]
    fn dubbing_beats_resolution_and_preserves_original_ranking_within_tiers() {
        let input = |id: &str, name: &str| RankInput {
            id: id.into(),
            name: name.into(),
            description: "".into(),
            addon: "test".into(),
            original_index: 0,
            size_bytes: None,
            seeders: None,
            debrid: None,
            http: true,
        };
        let mut ranked = rank_streams(
            [
                input("subs", "4K PLSUB"),
                input("dub", "720p DUBBING PL"),
                input("dub2", "1080p DUBBING PL"),
            ],
            RankingMode::Quality,
            false,
        );
        prefer_children_audio(&mut ranked);
        assert_eq!(
            ranked
                .iter()
                .map(|stream| stream.input.id.as_str())
                .collect::<Vec<_>>(),
            ["dub2", "dub", "subs"]
        );
    }
}

fn seeder_count(stream: &RankedStream) -> u32 {
    stream
        .input
        .seeders
        .or_else(|| parse_seeders_from_text(&stream.input.description))
        .or_else(|| parse_seeders_from_text(&stream.input.name))
        .unwrap_or_default()
}

fn stream_size(stream: &RankedStream) -> u64 {
    stream
        .input
        .size_bytes
        .or_else(|| parse_size_bytes_from_text(&stream.input.description))
        .or_else(|| parse_size_bytes_from_text(&stream.input.name))
        .unwrap_or(u64::MAX)
}

fn rank_one(input: RankInput) -> RankedStream {
    let parsed = parse_stream(&input);
    let mut score = 0;
    let mut reasons = Vec::new();
    let mut reason = |kind, label: String, points| {
        score += points;
        reasons.push(ScoreReason {
            kind,
            label,
            points,
        });
    };

    // Smart mode is the streaming default: a source that can start playback
    // quickly outranks a larger, higher-resolution file.
    if let Some(height) = parsed.quality_height {
        let points = match height {
            1080..=1439 => 36,
            720..=1079 => 28,
            1440..=2159 => 16,
            480..=719 => 8,
            2160.. => 6,
            _ => 0,
        };
        reason(ReasonKind::Positive, format!("{height}p video"), points);
    }
    if let Some(source) = parsed.source.as_deref() {
        let points = match source {
            "WEB-DL" => 10,
            "WEBRip" => 6,
            "HDTV" => 2,
            "BluRay" => -4,
            "REMUX" => -14,
            "CAM" | "TS" => -40,
            _ => 0,
        };
        reason(
            if points >= 0 {
                ReasonKind::Positive
            } else {
                ReasonKind::Negative
            },
            format!("{source} source"),
            points,
        );
    }
    if let Some(codec) = parsed.codec.as_deref() {
        let points = match codec {
            "H.264" => 4,
            "HEVC" => 2,
            "AV1" => 1,
            _ => 0,
        };
        reason(ReasonKind::Positive, format!("{codec} codec"), points);
    }
    if let Some(audio) = parsed.audio.as_deref() {
        reason(ReasonKind::Neutral, format!("{audio} audio"), 1);
    }
    let effective_seeders = input
        .seeders
        .or_else(|| parse_seeders_from_text(&input.description))
        .or_else(|| parse_seeders_from_text(&input.name));
    if let Some(seeders) = effective_seeders {
        let points = match seeders {
            200.. => 36,
            50..=199 => 24,
            10..=49 => 14,
            1..=9 => 4,
            0 => -30,
        };
        reason(
            if points >= 0 {
                ReasonKind::Positive
            } else {
                ReasonKind::Negative
            },
            format!("{seeders} seeders"),
            points,
        );
    }
    let effective_size = input
        .size_bytes
        .or_else(|| parse_size_bytes_from_text(&input.description))
        .or_else(|| parse_size_bytes_from_text(&input.name));
    if let Some(size) = effective_size {
        let gib = size as f64 / 1_073_741_824.0;
        let points = if gib <= 0.2 {
            -16
        } else if gib <= 3.0 {
            18
        } else if gib <= 8.0 {
            10
        } else if gib <= 15.0 {
            2
        } else if gib <= 30.0 {
            -8
        } else {
            -24
        };
        reason(
            if points >= 0 {
                ReasonKind::Positive
            } else {
                ReasonKind::Negative
            },
            format!("{gib:.1} GiB"),
            points,
        );
    }
    match input.debrid {
        Some(DebridAvailability::Cached) => reason(
            ReasonKind::Positive,
            "Instant on debrid provider".to_owned(),
            80,
        ),
        Some(DebridAvailability::Uncached) => reason(
            ReasonKind::Negative,
            "Not cached on debrid provider".to_owned(),
            -8,
        ),
        Some(DebridAvailability::Unknown | DebridAvailability::ProviderUnavailable) | None => {
            reason(
                ReasonKind::Neutral,
                "Debrid availability unknown".to_owned(),
                0,
            )
        }
    }
    if input.http {
        reason(
            ReasonKind::Positive,
            "Starts immediately without torrent pieces".to_owned(),
            20,
        );
    }
    if let Some(container) = parsed.container.as_deref() {
        let (points, label) = match container {
            "MKV" | "WebM" => (16, format!("{container} starts from the first pieces")),
            "TS" => (8, "MPEG-TS plays as it downloads".to_owned()),
            "MP4" => (8, "MP4 usually starts before the full download".to_owned()),
            "AVI" => (-18, "AVI index is usually at the end".to_owned()),
            "WMV" | "FLV" => (-12, format!("{container} is a poor streaming container")),
            "ISO" | "VOB" => (-24, "Disc image needs most of the file".to_owned()),
            "RAR" | "ZIP" => (-30, "Archive cannot play while downloading".to_owned()),
            _ => (0, format!("{container} container")),
        };
        if points != 0 {
            reason(
                if points >= 0 {
                    ReasonKind::Positive
                } else {
                    ReasonKind::Negative
                },
                label,
                points,
            );
        }
    }
    if parsed.suspicious {
        reason(
            ReasonKind::Negative,
            "Potential fake, password, or scam result".to_owned(),
            -1_000,
        );
    }

    RankedStream {
        input,
        parsed: parsed.clone(),
        score,
        reasons,
        filtered: parsed.suspicious,
    }
}

pub fn parse_stream(input: &RankInput) -> ParsedStream {
    let combined = format!("{} {}", input.name, input.description);
    let upper = combined.to_ascii_uppercase();
    let quality_height = [4320_u16, 2160, 1440, 1080, 720, 576, 540, 480, 360]
        .into_iter()
        .find(|height| {
            upper.contains(&format!("{height}P"))
                || (*height == 2160 && upper.contains("4K"))
                || (*height == 4320 && upper.contains("8K"))
        });
    let source = [
        ("REMUX", "REMUX"),
        ("BLURAY", "BluRay"),
        ("BLU-RAY", "BluRay"),
        ("WEB-DL", "WEB-DL"),
        ("WEBDL", "WEB-DL"),
        ("WEBRIP", "WEBRip"),
        ("HDTV", "HDTV"),
        ("CAM", "CAM"),
        ("TELESYNC", "TS"),
    ]
    .into_iter()
    .find_map(|(marker, normalized)| upper.contains(marker).then(|| normalized.to_owned()));
    let codec = [
        (&["AV1", "AV01"][..], "AV1"),
        (&["HEVC", "H265", "H.265", "X265"][..], "HEVC"),
        (&["H264", "H.264", "X264", "AVC"][..], "H.264"),
    ]
    .into_iter()
    .find_map(|(markers, normalized)| {
        markers
            .iter()
            .any(|marker| upper.contains(marker))
            .then(|| normalized.to_owned())
    });
    let dolby_vision = ["DOLBY VISION", "DOVI", " DV "]
        .iter()
        .any(|marker| upper.contains(marker));
    let hdr = dolby_vision
        || ["HDR10+", "HDR10", " HDR ", "HLG"]
            .iter()
            .any(|marker| upper.contains(marker));
    let audio = [
        "TRUEHD ATMOS",
        "ATMOS",
        "TRUEHD",
        "DTS-HD",
        "DTS",
        "EAC3",
        "DDP",
        "AAC",
    ]
    .into_iter()
    .find(|marker| upper.contains(marker))
    .map(ToOwned::to_owned);
    let languages = [
        ("ENGLISH", "English"),
        ("HINDI", "Hindi"),
        ("PORTUGUESE", "Portuguese"),
        ("ARABIC", "Arabic"),
        ("SPANISH", "Spanish"),
        ("FRENCH", "French"),
        ("JAPANESE", "Japanese"),
        ("MULTI", "Multi"),
    ]
    .into_iter()
    .filter(|(marker, _)| upper.contains(marker))
    .map(|(_, language)| language.to_owned())
    .collect();
    let release_group = combined
        .split_whitespace()
        .rev()
        .find_map(|token| token.strip_prefix('-'))
        .filter(|group| !group.is_empty() && group.len() <= 24)
        .map(ToOwned::to_owned);
    let suspicious = [
        "PASSWORD",
        "PASSW0RD",
        "CLICK HERE",
        "FREE BITCOIN",
        "INSTALL CODEC",
        "EXE INCLUDED",
        "SCAM",
        "FAKE",
    ]
    .iter()
    .any(|marker| upper.contains(marker));
    let container = parse_container(&upper);

    ParsedStream {
        quality_height,
        source,
        codec,
        hdr,
        dolby_vision,
        audio,
        languages,
        release_group,
        container,
        suspicious,
    }
}

fn parse_container(upper: &str) -> Option<String> {
    [
        (".MKV", "MKV"),
        (".MK3D", "MKV"),
        (".WEBM", "WebM"),
        (".M2TS", "TS"),
        (".MTS", "TS"),
        (".MP4", "MP4"),
        (".M4V", "MP4"),
        (".MOV", "MP4"),
        (".AVI", "AVI"),
        (".WMV", "WMV"),
        (".FLV", "FLV"),
        (".ISO", "ISO"),
        (".VOB", "VOB"),
        (".RAR", "RAR"),
        (".ZIP", "ZIP"),
        (".TS", "TS"),
    ]
    .into_iter()
    .find_map(|(marker, name)| upper.contains(marker).then(|| name.to_owned()))
}

pub fn parse_size_bytes_from_text(text: &str) -> Option<u64> {
    if let Some(idx) = text.find('💾') {
        let after = &text[idx + '💾'.len_utf8()..];
        if let Some(bytes) = parse_first_size(after) {
            return Some(bytes);
        }
    }
    parse_first_size(text)
}

fn parse_first_size(text: &str) -> Option<u64> {
    let words: Vec<&str> = text.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        let clean_word = word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.');
        if let Some(bytes) = parse_size_token(clean_word) {
            return Some(bytes);
        }
        if let Ok(num) = clean_word.parse::<f64>()
            && let Some(unit) = words.get(i + 1)
            && let Some(multiplier) =
                unit_multiplier(unit.trim_matches(|c: char| !c.is_ascii_alphabetic()))
        {
            return Some((num * multiplier) as u64);
        }
    }
    None
}

fn unit_multiplier(unit: &str) -> Option<f64> {
    match unit.to_ascii_uppercase().as_str() {
        "TB" | "TIB" => Some(1_099_511_627_776.0),
        "GB" | "GIB" => Some(1_073_741_824.0),
        "MB" | "MIB" => Some(1_048_576.0),
        "KB" | "KIB" => Some(1_024.0),
        "B" | "BYTES" => Some(1.0),
        _ => None,
    }
}

fn parse_size_token(token: &str) -> Option<u64> {
    let upper = token.to_ascii_uppercase();
    for (unit, mult) in [
        ("TIB", 1_099_511_627_776.0),
        ("TB", 1_099_511_627_776.0),
        ("GIB", 1_073_741_824.0),
        ("GB", 1_073_741_824.0),
        ("MIB", 1_048_576.0),
        ("MB", 1_048_576.0),
        ("KIB", 1_024.0),
        ("KB", 1_024.0),
    ] {
        if let Some(num_part) = upper.strip_suffix(unit)
            && let Ok(num) = num_part.parse::<f64>()
        {
            return Some((num * mult) as u64);
        }
    }
    None
}

pub fn parse_seeders_from_text(text: &str) -> Option<u32> {
    if let Some(idx) = text.find('👤') {
        let after = &text[idx + '👤'.len_utf8()..];
        let num_str: String = after
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(val) = num_str.parse::<u32>() {
            return Some(val);
        }
    }
    let upper = text.to_ascii_uppercase();
    for part in upper.split(['\n', '/', '|', '•', ',', ';']) {
        let tokens: Vec<&str> = part.split_whitespace().collect();
        for (i, token) in tokens.iter().enumerate() {
            let clean = token.trim_matches(|c: char| !c.is_ascii_alphanumeric());
            if matches!(clean, "S" | "SEEDS" | "SEEDERS")
                && let Some(next) = tokens.get(i + 1)
                && let Ok(val) = next
                    .trim_matches(|c: char| !c.is_ascii_digit())
                    .parse::<u32>()
            {
                return Some(val);
            }
            if clean.ends_with("SEEDS") || clean.ends_with("SEEDERS") {
                let num_part = clean.trim_end_matches(|c: char| c.is_ascii_alphabetic());
                if let Ok(val) = num_part.parse::<u32>() {
                    return Some(val);
                }
            }
            if let Ok(val) = clean.parse::<u32>()
                && tokens.get(i + 1).is_some_and(|next| {
                    matches!(
                        next.trim_matches(|c: char| !c.is_ascii_alphabetic()),
                        "SEEDS" | "SEEDERS" | "PEERS" | "S"
                    )
                })
            {
                return Some(val);
            }
        }
    }
    None
}

pub fn format_size(bytes: u64) -> String {
    let gib = bytes as f64 / 1_073_741_824.0;
    if gib >= 1.0 {
        format!("{:.2} GB", gib)
    } else {
        let mib = bytes as f64 / 1_048_576.0;
        format!("{:.1} MB", mib)
    }
}

pub fn format_stream_description(
    name: &str,
    description: &str,
    size_bytes: Option<u64>,
    seeders: Option<u32>,
) -> String {
    let seeders = seeders
        .or_else(|| parse_seeders_from_text(description))
        .or_else(|| parse_seeders_from_text(name));
    let size_bytes = size_bytes
        .or_else(|| parse_size_bytes_from_text(description))
        .or_else(|| parse_size_bytes_from_text(name));

    let raw_lines: Vec<&str> = description
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();

    if raw_lines.is_empty() {
        let mut stats = Vec::new();
        if let Some(s) = seeders {
            stats.push(format!("👤 {s}"));
        }
        if let Some(sz) = size_bytes {
            stats.push(format!("💾 {}", format_size(sz)));
        }
        return stats.join(" ");
    }

    let mut title_lines = Vec::new();
    let mut meta_lines = Vec::new();

    for line in &raw_lines {
        let is_meta = line.contains('👤')
            || line.contains('💾')
            || line.contains('⚙')
            || line.contains('≡')
            || line.contains('🗓')
            || line.contains('📺')
            || line.contains('⚡')
            || line.to_ascii_uppercase().contains("SEEDERS")
            || line.to_ascii_uppercase().contains("SEEDS");
        if is_meta {
            meta_lines.push((*line).to_string());
        } else {
            title_lines.push((*line).to_string());
        }
    }

    let title = if title_lines.is_empty() {
        raw_lines[0].to_string()
    } else {
        title_lines[0].clone()
    };

    let mut meta_line = meta_lines.join(" ");
    let meta_upper = meta_line.to_ascii_uppercase();
    if let Some(s) = seeders
        && !meta_line.contains('👤')
        && !meta_upper.contains("SEED")
    {
        if !meta_line.is_empty() {
            meta_line.push(' ');
        }
        meta_line.push_str(&format!("👤 {s}"));
    }
    if let Some(sz) = size_bytes
        && !meta_line.contains('💾')
        && !meta_upper.contains("GB")
        && !meta_upper.contains("MB")
        && !meta_upper.contains("GIB")
        && !meta_upper.contains("MIB")
    {
        if !meta_line.is_empty() {
            meta_line.push(' ');
        }
        meta_line.push_str(&format!("💾 {}", format_size(sz)));
    }

    if meta_line.is_empty() {
        title
    } else {
        format!("{title}\n{meta_line}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(index: usize, name: &str, size: u64, seeders: u32) -> RankInput {
        RankInput {
            id: index.to_string(),
            name: name.to_owned(),
            description: String::new(),
            addon: "Fixture".to_owned(),
            original_index: index,
            size_bytes: Some(size),
            seeders: Some(seeders),
            debrid: None,
            http: false,
        }
    }

    #[test]
    fn preferred_quality_falls_back_from_1080_to_720() {
        assert_eq!(preferred_quality_tab(&["1080p", "4K"]), "1080p");
        assert_eq!(preferred_quality_tab(&["720p", "4K"]), "720p");
        assert_eq!(preferred_quality_tab(&["4K"]), "4K");
        assert_eq!(
            ordered_quality_tabs(["4K", "720p", "1080p"]),
            vec!["720p", "1080p", "4K"]
        );
    }

    #[test]
    fn quality_tab_groups_720_1080_and_4k() {
        assert_eq!(quality_tab(Some(720)), "720p");
        assert_eq!(quality_tab(Some(480)), "480p");
        assert_eq!(quality_tab(Some(1080)), "1080p");
        assert_eq!(quality_tab(None), "Auto");
        assert_eq!(quality_tab(Some(1440)), "1440p");
        assert_eq!(quality_tab(Some(2160)), "4K");
        assert_eq!(
            quality_tab(parse_stream(&input(0, "Torrentio 4k", 1, 1)).quality_height),
            "4K"
        );
        assert_eq!(
            quality_tab(parse_stream(&input(0, "Torrentio 1080p", 1, 1)).quality_height),
            "1080p"
        );
        assert_eq!(
            quality_tab(parse_stream(&input(0, "Torrentio 720p", 1, 1)).quality_height),
            "720p"
        );
    }

    #[test]
    fn original_mode_exactly_preserves_addon_order() {
        let ranked = rank_streams(
            [
                input(2, "2160p REMUX", 50, 10),
                input(0, "720p WEBRip", 1, 100),
                input(1, "1080p WEB-DL", 2, 50),
            ],
            RankingMode::Original,
            false,
        );
        assert_eq!(
            ranked
                .iter()
                .map(|stream| stream.input.original_index)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn smart_ranking_is_deterministic_and_uses_original_order_as_final_tie_breaker() {
        let fixture = [
            input(1, "1080p WEB-DL x265", 2_000_000_000, 50),
            input(0, "1080p WEB-DL x265", 2_000_000_000, 50),
        ];
        let first = rank_streams(fixture.clone(), RankingMode::Smart, false);
        let second = rank_streams(fixture, RankingMode::Smart, false);
        assert_eq!(first, second);
        assert_eq!(first[0].input.original_index, 0);
    }

    #[test]
    fn smart_mode_ranks_sources_that_start_sooner_first() {
        let huge_remux = input(0, "2160p REMUX", 60 * 1_073_741_824, 8);
        let quick = input(1, "1080p WEB-DL", 2 * 1_073_741_824, 200);
        let mut cached = input(2, "1080p WEB-DL", 2 * 1_073_741_824, 5);
        cached.debrid = Some(DebridAvailability::Cached);
        let ranked = rank_streams([huge_remux, quick, cached], RankingMode::Smart, false);
        assert_eq!(
            ranked
                .iter()
                .map(|stream| stream.input.original_index)
                .collect::<Vec<_>>(),
            vec![2, 1, 0]
        );
    }

    #[test]
    fn smart_prefers_containers_that_start_before_the_full_download() {
        let avi = input(0, "1080p WEB-DL x264.avi", 2 * 1_073_741_824, 200);
        let mkv = input(1, "1080p WEB-DL x264.mkv", 2 * 1_073_741_824, 40);
        let ranked = rank_streams([avi, mkv], RankingMode::Smart, false);
        assert_eq!(ranked[0].input.original_index, 1);
        assert_eq!(
            parse_stream(&input(0, "file.mkv", 1, 1))
                .container
                .as_deref(),
            Some("MKV")
        );
        assert_eq!(
            parse_stream(&input(0, "file.avi", 1, 1))
                .container
                .as_deref(),
            Some("AVI")
        );
    }

    #[test]
    fn suspicious_streams_are_hidden_but_recoverable() {
        let fake = input(0, "2160p FAKE install codec", 1, 9999);
        assert!(rank_streams([fake.clone()], RankingMode::Smart, false).is_empty());
        let visible = rank_streams([fake], RankingMode::Smart, true);
        assert!(visible[0].filtered);
    }

    #[test]
    fn provider_outage_is_neutral() {
        let mut unavailable = input(0, "1080p", 2_000_000_000, 20);
        unavailable.debrid = Some(DebridAvailability::ProviderUnavailable);
        let mut unknown = unavailable.clone();
        unknown.debrid = Some(DebridAvailability::Unknown);
        assert_eq!(
            rank_streams([unavailable], RankingMode::Smart, false)[0].score,
            rank_streams([unknown], RankingMode::Smart, false)[0].score
        );
    }

    #[test]
    fn parses_seeders_and_size_correctly() {
        let desc = "Musafir.Cafe.2026.S01.1080p.NF.WEB-DL.Multi..DD+.5.1.Atmos.x264-KiN\nMusafir Cafe-S01E01-Arrival.1080p.Multi.WEB-DL.DD+.5.1.Atmos.x264-KiN..mkv\n👤 5 💾 1.24 GB ≡ EZTV";
        assert_eq!(parse_seeders_from_text(desc), Some(5));
        assert_eq!(
            parse_size_bytes_from_text(desc),
            Some((1.24 * 1_073_741_824.0) as u64)
        );

        let formatted = format_stream_description("Torrentio 1080p", desc, None, None);
        assert!(formatted.contains("👤 5"));
        assert!(formatted.contains("💾 1.24 GB"));
        assert_eq!(formatted.lines().count(), 2);
    }

    #[test]
    fn ensures_seeds_and_size_when_only_in_behavior_hints() {
        let formatted = format_stream_description(
            "HTTP Stream",
            "Big Buck Bunny 1080p",
            Some(1_331_691_520),
            Some(12),
        );
        assert!(formatted.contains("👤 12"));
        assert!(formatted.contains("💾 1.24 GB"));
        assert_eq!(formatted.lines().count(), 2);
    }
}
