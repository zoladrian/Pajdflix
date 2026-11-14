//! Prefix sniff for torrent payload. Denylist only: unknown containers play.
//! A missing or short header is not treated as malware.
use anyhow::Result;
use std::{
    collections::HashMap,
    future::Future,
    sync::{Mutex, OnceLock},
};

#[derive(Clone)]
enum Outcome {
    Allowed,
    Dangerous(String),
}

fn cache() -> &'static Mutex<HashMap<(String, usize), Outcome>> {
    static CACHE: OnceLock<Mutex<HashMap<(String, usize), Outcome>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Napisy to tekst; nie wymagamy nagłówka wideo i nie skanujemy ich jak programów.
pub fn is_subtitle_name(name: &str) -> bool {
    matches!(
        file_extension(name).as_str(),
        "srt" | "ass" | "ssa" | "vtt" | "sub" | "idx"
    )
}

fn file_extension(name: &str) -> String {
    name.rsplit(['/', '\\', '.'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// Read the start of a selected video file once. Flag only high-confidence
/// executables and archives. Anything else, including formats we do not
/// recognize, is allowed so legitimate movies are not blocked.
pub async fn ensure_safe_video_prefix(
    info_hash: &str,
    file_idx: usize,
    name: &str,
    read_prefix: impl Future<Output = Result<Vec<u8>>>,
) -> Result<()> {
    if is_subtitle_name(name) || crate::source_file_review::is_approved(info_hash) {
        return Ok(());
    }
    let key = (info_hash.to_owned(), file_idx);
    if let Some(outcome) = cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
        .cloned()
    {
        return apply(info_hash, name, outcome);
    }
    let prefix = read_prefix.await?;
    let outcome = match dangerous_prefix_reason(&prefix) {
        Some(reason) => Outcome::Dangerous(reason.to_owned()),
        None => Outcome::Allowed,
    };
    {
        let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= 256 {
            cache.retain(|_, outcome| matches!(outcome, Outcome::Dangerous(_)));
        }
        cache.insert(key, outcome.clone());
    }
    apply(info_hash, name, outcome)
}

fn apply(info_hash: &str, name: &str, outcome: Outcome) -> Result<()> {
    match outcome {
        Outcome::Allowed => Ok(()),
        Outcome::Dangerous(reason) => {
            let message = format!("Plik „{name}” {reason}");
            crate::source_file_review::record(info_hash, message.clone());
            anyhow::bail!(message)
        }
    }
}

pub fn dangerous_prefix_reason(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() < 2 {
        return None;
    }
    if bytes.starts_with(b"MZ") || bytes.starts_with(b"ZM") {
        return Some(
            "zaczyna się jak program Windows (PE), a nie jak film. Torrent nie został odtworzony.",
        );
    }
    if bytes.starts_with(b"\x7fELF") {
        return Some("zaczyna się jak program ELF, a nie jak film. Torrent nie został odtworzony.");
    }
    if bytes.len() >= 4
        && matches!(
            &bytes[..4],
            b"\xfe\xed\xfa\xce"
                | b"\xfe\xed\xfa\xcf"
                | b"\xce\xfa\xed\xfe"
                | b"\xcf\xfa\xed\xfe"
                | b"\xca\xfe\xba\xbe"
        )
    {
        return Some(
            "zaczyna się jak program (Mach-O/Java), a nie jak film. Torrent nie został odtworzony.",
        );
    }
    if bytes.starts_with(b"\0asm") {
        return Some("zaczyna się jak WebAssembly, a nie jak film. Torrent nie został odtworzony.");
    }
    if bytes.starts_with(b"#!") {
        return Some("zaczyna się jak skrypt, a nie jak film. Torrent nie został odtworzony.");
    }
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        return Some("zaczyna się jak archiwum ZIP, a nie jak film. Torrent nie został odtworzony.");
    }
    if bytes.starts_with(b"Rar!\x1a\x07") {
        return Some("zaczyna się jak archiwum RAR, a nie jak film. Torrent nie został odtworzony.");
    }
    if bytes.starts_with(b"7z\xbc\xaf'\x1c") {
        return Some("zaczyna się jak archiwum 7z, a nie jak film. Torrent nie został odtworzony.");
    }
    if bytes.starts_with(b"MSCF") {
        return Some("zaczyna się jak instalator CAB, a nie jak film. Torrent nie został odtworzony.");
    }
    if bytes.starts_with(b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1") {
        return Some(
            "zaczyna się jak dokument/instalator OLE (np. MSI), a nie jak film. Torrent nie został odtworzony.",
        );
    }
    if bytes.starts_with(b"\x4c\x00\x00\x00\x01\x14\x02\x00") {
        return Some(
            "zaczyna się jak skrót Windows (.lnk), a nie jak film. Torrent nie został odtworzony.",
        );
    }
    if starts_with_ignore_ascii_case(bytes, b"windows registry editor") {
        return Some(
            "zaczyna się jak plik rejestru Windows, a nie jak film. Torrent nie został odtworzony.",
        );
    }
    if html_or_hta_prefix(bytes) {
        return Some(
            "zaczyna się jak strona/skrypt HTML/HTA, a nie jak film. Torrent nie został odtworzony.",
        );
    }
    None
}

fn starts_with_ignore_ascii_case(bytes: &[u8], prefix: &[u8]) -> bool {
    bytes.len() >= prefix.len() && bytes[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn html_or_hta_prefix(bytes: &[u8]) -> bool {
    let trimmed = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .map(|i| &bytes[i..])
        .unwrap_or(&[]);
    const TAGS: [&[u8]; 5] = [b"<!doctype", b"<html", b"<hta:", b"<script", b"<iframe"];
    TAGS.iter()
        .any(|tag| starts_with_ignore_ascii_case(trimmed, tag))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executables_and_archives_are_dangerous() {
        for (bytes, needle) in [
            (&b"MZ\x90\x00"[..], "PE"),
            (&b"\x7fELF\x01\x01"[..], "ELF"),
            (&b"PK\x03\x04payload"[..], "ZIP"),
            (&b"Rar!\x1a\x07\x00"[..], "RAR"),
            (&b"7z\xbc\xaf'\x1c\x00"[..], "7z"),
            (&b"MSCF\x00\x00"[..], "CAB"),
            (&b"#! /bin/sh\n"[..], "skrypt"),
            (
                &b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1"[..],
                "OLE",
            ),
            (&b"<!DOCTYPE html>"[..], "HTML"),
            (&b"  <HTA:APPLICATION>"[..], "HTA"),
        ] {
            let reason = dangerous_prefix_reason(bytes).unwrap_or_else(|| panic!("missed {needle}"));
            assert!(
                reason.to_ascii_uppercase().contains(&needle.to_ascii_uppercase())
                    || reason.contains("strona")
                    || reason.contains("skrypt")
                    || reason.contains("HTA"),
                "{needle}: {reason}"
            );
        }
    }

    #[test]
    fn legitimate_media_and_unknown_bytes_are_allowed() {
        let mkv = [0x1a, 0x45, 0xdf, 0xa3, 0x01, 0x00, 0x00, 0x00];
        let mut mp4 = vec![0, 0, 0, 0x20];
        mp4.extend_from_slice(b"ftypisom");
        let mut avi = b"RIFF".to_vec();
        avi.extend_from_slice(&[0, 0, 0, 0]);
        avi.extend_from_slice(b"AVI ");
        let mut ts = vec![0x47];
        ts.extend_from_slice(&[0; 187]);
        let mpeg_ps = [0x00, 0x00, 0x01, 0xba, 0x00, 0x00, 0x00, 0x00];
        let flv = b"FLV\x01\x05\x00\x00\x00\x09".as_slice();
        let ogg = b"OggS\x00\x02".as_slice();
        let asf = [
            0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11, 0xa6, 0xd9, 0x00, 0xaa, 0x00, 0x62,
            0xce, 0x6c,
        ];
        let y4m = b"YUV4MPEG2 W1920 H1080\n".as_slice();
        let ivf = b"DKIF\x00\x00".as_slice();
        let unknown = [0x00, 0x01, 0x02, 0x03, 0xaa, 0xbb, 0xcc, 0xdd];
        let short = [b'M'];
        let empty: [u8; 0] = [];
        for bytes in [
            mkv.as_slice(),
            mp4.as_slice(),
            avi.as_slice(),
            ts.as_slice(),
            mpeg_ps.as_slice(),
            flv,
            ogg,
            asf.as_slice(),
            y4m,
            ivf,
            unknown.as_slice(),
            short.as_slice(),
            empty.as_slice(),
        ] {
            assert_eq!(
                dangerous_prefix_reason(bytes),
                None,
                "false-positive prefix: {:02x?}",
                &bytes[..bytes.len().min(8)]
            );
        }
    }

    #[test]
    fn subtitle_names_are_not_scanned_as_programs() {
        assert!(is_subtitle_name("Napisy/pol.srt"));
        assert!(is_subtitle_name(r"subs\en.ass"));
        assert!(!is_subtitle_name("Movie.2024.mkv"));
        assert!(!is_subtitle_name("Movie.mp4"));
    }

    #[tokio::test]
    async fn mz_payload_prompts_and_mkv_does_not() {
        let hash = "sniff-mz-prompt";
        let error = ensure_safe_video_prefix(hash, 0, "film.mkv", async {
            Ok(b"MZ\x90\x00\x03".to_vec())
        })
        .await
        .unwrap_err();
        assert!(error.to_string().contains("PE"));
        assert!(crate::source_file_review::pending_reason(hash).is_some());

        let hash = "sniff-mkv-ok";
        ensure_safe_video_prefix(hash, 0, "film.mkv", async {
            Ok(vec![0x1a, 0x45, 0xdf, 0xa3, 0x01, 0x00])
        })
        .await
        .unwrap();
        assert!(crate::source_file_review::pending_reason(hash).is_none());

        let hash = "sniff-unknown-ok";
        ensure_safe_video_prefix(hash, 0, "film.mkv", async {
            Ok(vec![0x00, 0x01, 0x02, 0x03, 0xaa, 0xbb])
        })
        .await
        .unwrap();
        assert!(crate::source_file_review::pending_reason(hash).is_none());
    }

    #[tokio::test]
    async fn subtitles_and_approval_skip_the_prefix_denylist() {
        let hash = "sniff-sub-skip";
        ensure_safe_video_prefix(hash, 0, "Napisy/pol.srt", async {
            Ok(b"MZ\x90\x00".to_vec())
        })
        .await
        .unwrap();
        assert!(crate::source_file_review::pending_reason(hash).is_none());

        crate::source_file_review::record("sniff-approved", "filename".into());
        let approval = crate::source_file_review::approve("sniff-approved").unwrap();
        ensure_safe_video_prefix("sniff-approved", 0, "film.mkv", async {
            Ok(b"MZ\x90\x00".to_vec())
        })
        .await
        .unwrap();
        drop(approval);
    }
}
