//! Fetch addon subtitles with the application's TLS stack before handing them
//! to libmpv, whose bundled FFmpeg may not share the Windows certificate store.
use anyhow::{Context, Result, ensure};
use std::{
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

const MAX_BYTES: usize = 8 * 1024 * 1024;
static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
static DOWNLOADS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

pub async fn local_source(remote: &str, directory: &Path) -> Result<String> {
    let parsed = url::Url::parse(remote).context("invalid subtitle URL")?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Ok(remote.to_owned());
    }
    let _permit = DOWNLOADS.acquire().await?;
    let extension = Path::new(parsed.path())
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("srt")
        .to_ascii_lowercase();
    let extension = match extension.as_str() {
        "ass" | "ssa" | "vtt" | "sub" | "srt" | "txt" => extension.as_str(),
        _ => "srt",
    };
    let key = blake3::hash(remote.as_bytes()).to_hex();
    for cached_extension in ["srt", "ass", "ssa", "vtt", "sub", "txt"] {
        let path = directory.join(format!("{key}.{cached_extension}"));
        if let Ok(metadata) = tokio::fs::metadata(&path).await {
            if metadata.is_file() && metadata.len() > 0 && metadata.len() <= MAX_BYTES as u64 {
                // Files cached before UTF-8 normalization are re-downloaded below.
                if let Ok(bytes) = tokio::fs::read(&path).await
                    && std::str::from_utf8(&bytes).is_ok()
                    && validate_subtitle_body(&bytes).is_ok()
                {
                    ensure_letters_intact(&bytes)?;
                    return file_url(&path);
                }
            }
        }
    }
    let client = CLIENT.get_or_init(|| {
        let builder = reqwest::Client::builder().timeout(Duration::from_secs(20));
        #[cfg(windows)]
        let builder = builder.use_native_tls();
        builder.build().expect("subtitle HTTP client configuration")
    });
    let mut response = client
        .get(parsed)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!(e.without_url().to_string()))?
        .error_for_status()
        .map_err(|e| anyhow::anyhow!(e.without_url().to_string()))?;
    ensure!(
        response
            .content_length()
            .is_none_or(|size| size <= MAX_BYTES as u64),
        "subtitle exceeds size limit"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| anyhow::anyhow!(e.without_url().to_string()))?
    {
        ensure!(
            bytes.len().saturating_add(chunk.len()) <= MAX_BYTES,
            "subtitle exceeds size limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    ensure!(!bytes.is_empty(), "empty subtitle response");
    let (bytes, extension) = unpack_subtitle(bytes, extension)?;
    let bytes = normalize_to_utf8(bytes);
    validate_subtitle_body(&bytes)?;
    ensure_letters_intact(&bytes)?;
    let path = directory.join(format!("{key}.{extension}"));
    tokio::fs::create_dir_all(directory).await?;
    let temporary: PathBuf = directory.join(format!(
        ".{key}-{}-{}.tmp",
        std::process::id(),
        TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    tokio::fs::write(&temporary, bytes).await?;
    if let Err(error) = tokio::fs::rename(&temporary, &path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        // Another request may have cached the same subtitle concurrently.
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Err(error.into());
        }
    }
    file_url(&path)
}

fn validate_subtitle_body(bytes: &[u8]) -> Result<()> {
    let sample = String::from_utf8_lossy(&bytes[..bytes.len().min(32 * 1024)]);
    let sample = sample.trim_start_matches('\u{feff}').trim_start();
    let recognized = sample.starts_with("WEBVTT")
        || sample.contains("[Script Info]")
        || sample.lines().any(|line| {
            let line = line.trim();
            (line.contains("-->") && line.contains(':'))
                || line.starts_with("Dialogue:")
                || (line.starts_with('{') && line.contains("}{"))
                || (line.len() >= 22
                    && line.as_bytes()[0].is_ascii_digit()
                    && line.matches(':').count() >= 4
                    && line.contains(','))
        });
    ensure!(
        recognized,
        "provider returned no recognizable subtitle cues"
    );
    Ok(())
}

/// Polish subtitles are commonly Windows-1250 or ISO-8859-2, which libmpv
/// cannot reliably tell apart from UTF-8 garbage, so store everything as UTF-8.
fn normalize_to_utf8(bytes: Vec<u8>) -> Vec<u8> {
    if let Some((encoding, bom_length)) = encoding_rs::Encoding::for_bom(&bytes) {
        if encoding == encoding_rs::UTF_8 {
            return bytes;
        }
        let (text, _) = encoding.decode_without_bom_handling(&bytes[bom_length..]);
        return text.into_owned().into_bytes();
    }
    if std::str::from_utf8(&bytes).is_ok() {
        return bytes;
    }
    // On a tie `max_by_key` keeps the last candidate: Windows-1250 is the more common one.
    [encoding_rs::ISO_8859_2, encoding_rs::WINDOWS_1250]
        .into_iter()
        .map(|encoding| encoding.decode_without_bom_handling(&bytes).0)
        .max_by_key(|text| polish_text_score(text))
        .map(|text| text.into_owned().into_bytes())
        .unwrap_or(bytes)
}

fn polish_text_score(text: &str) -> i64 {
    text.chars()
        .map(|character| match character {
            'ą' | 'ć' | 'ę' | 'ł' | 'ń' | 'ó' | 'ś' | 'ź' | 'ż' | 'Ą' | 'Ć' | 'Ę' | 'Ł' | 'Ń'
            | 'Ó' | 'Ś' | 'Ź' | 'Ż' => 2,
            '\u{80}'..='\u{9f}' => -10,
            _ => 0,
        })
        .sum()
}

/// Some providers convert legacy-encoded files to UTF-8 lossily, replacing every
/// national letter with U+FFFD. Such a track is
/// unreadable; rejecting it lets the player pick an intact version instead.
/// A few replacement characters (music notes, curly quotes) are tolerated.
fn ensure_letters_intact(bytes: &[u8]) -> Result<()> {
    let text = String::from_utf8_lossy(bytes);
    let replaced = text.matches('\u{fffd}').count();
    let letters = text.chars().filter(|character| character.is_alphabetic()).count();
    ensure!(
        replaced < 8 || replaced.saturating_mul(200) < letters,
        "subtitle provider replaced {replaced} letters with U+FFFD"
    );
    Ok(())
}

/// Read a single subtitle from a ZIP in memory, without extracting archive paths.
fn unpack_subtitle(bytes: Vec<u8>, extension: &str) -> Result<(Vec<u8>, String)> {
    ensure!(
        !bytes.starts_with(b"Rar!\x1a\x07"),
        "RAR subtitle archives are unsupported"
    );
    if !bytes.starts_with(b"PK\x03\x04") && !bytes.starts_with(b"PK\x05\x06") {
        return Ok((bytes, extension.to_owned()));
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).context("invalid subtitle ZIP")?;
    ensure!(archive.len() <= 64, "subtitle ZIP has too many entries");
    let mut selected = None;
    for index in 0..archive.len() {
        let file = archive.by_index(index)?;
        if !file.is_file()
            || file.enclosed_name().is_none()
            || file
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            continue;
        }
        let ext = Path::new(file.name())
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(ext.as_str(), "srt" | "ass" | "ssa" | "vtt" | "sub" | "txt") {
            continue;
        }
        ensure!(
            selected.is_none(),
            "subtitle ZIP contains multiple subtitle files; cannot choose a matching version"
        );
        ensure!(
            file.size() <= MAX_BYTES as u64,
            "unpacked subtitle exceeds size limit"
        );
        selected = Some((index, ext));
    }
    let (index, extension) =
        selected.context("subtitle ZIP contains no supported subtitle file")?;
    let mut subtitle = Vec::new();
    archive
        .by_index(index)?
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut subtitle)?;
    ensure!(
        !subtitle.is_empty() && subtitle.len() <= MAX_BYTES,
        "invalid unpacked subtitle size"
    );
    Ok((subtitle, extension))
}

fn file_url(path: &Path) -> Result<String> {
    url::Url::from_file_path(path)
        .map(|url| url.to_string())
        .map_err(|_| anyhow::anyhow!("invalid local subtitle path"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLISH: &str = "1\r\n00:00:01,000 --> 00:00:02,000\r\nZażółć gęślą jaźń. Śródźle Ąę.\r\n";

    #[test]
    fn utf8_subtitles_are_left_untouched() {
        let bytes = POLISH.as_bytes().to_vec();
        assert_eq!(normalize_to_utf8(bytes.clone()), bytes);
    }

    #[test]
    fn windows_1250_subtitles_become_utf8() {
        let (encoded, _, _) = encoding_rs::WINDOWS_1250.encode(POLISH);
        assert_eq!(normalize_to_utf8(encoded.into_owned()), POLISH.as_bytes());
    }

    #[test]
    fn iso_8859_2_subtitles_become_utf8() {
        let (encoded, _, _) = encoding_rs::ISO_8859_2.encode(POLISH);
        assert_eq!(normalize_to_utf8(encoded.into_owned()), POLISH.as_bytes());
    }

    #[test]
    fn utf16_subtitles_with_bom_become_utf8() {
        let mut encoded = vec![0xff, 0xfe];
        encoded.extend(POLISH.encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(normalize_to_utf8(encoded), POLISH.as_bytes());
    }

    #[test]
    fn lossily_converted_subtitles_are_rejected() {
        let mangled = POLISH.repeat(20).replace(['ż', 'ó', 'ł', 'ć', 'ę', 'ś', 'ą', 'ź', 'ń'], "\u{fffd}");
        assert!(ensure_letters_intact(mangled.as_bytes()).is_err());
        assert!(ensure_letters_intact(POLISH.repeat(20).as_bytes()).is_ok());
    }

    #[test]
    fn occasional_replacement_characters_are_tolerated() {
        let text = format!("{}\u{fffd} La la la \u{fffd}\r\n", "I am Jack's smirking revenge. ".repeat(300));
        assert!(ensure_letters_intact(text.as_bytes()).is_ok());
    }
}
