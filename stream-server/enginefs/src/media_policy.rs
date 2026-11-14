//! Validate torrent paths and select playable files before downloading payload.
use anyhow::{Result, bail};
use std::path::Path;

fn validate_path(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("empty torrent filename");
    }
    for part in name.split(['/', '\\']) {
        if part.is_empty()
            || part.starts_with('.')
            || part.ends_with(['.', ' '])
            || part
                .chars()
                .any(|c| c.is_control() || "<>:\"|?*".contains(c))
        {
            bail!(
                "Plik „{name}” ma niedozwoloną ścieżkę w Windows. Torrent nie został uruchomiony."
            );
        }
        let lower = part.to_lowercase();
        let stem = lower.split('.').next().unwrap_or_default();
        if [
            "con", "prn", "aux", "nul", "com¹", "com²", "com³", "lpt¹", "lpt²", "lpt³",
        ]
        .contains(&stem)
            || (stem.len() == 4
                && (stem.starts_with("com") || stem.starts_with("lpt"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            bail!(
                "Plik „{name}” używa zastrzeżonej nazwy Windows (np. CON albo LPT). Torrent nie został uruchomiony."
            );
        }
    }
    Ok(())
}

/// Windows-runnable extra extensions hidden in front of a media suffix,
/// e.g. `film.cmd.mkv`. The last suffix is ignored here: a trailing `.exe`
/// is already skipped because it is not playable media.
///
/// `com` and `scr` are omitted on purpose. In torrent names they are almost
/// always a website TLD (`YTS.COM.mkv`, folder `www.example.com`) or a
/// screener tag (`Movie.2024.SCR.mkv`). Bare `*.com` / `*.scr` attachments
/// are still not selected for download.
const DISGUISED_EXECUTABLE_EXTENSIONS: &[&str] = &[
    "exe", "dll", "msi", "msp", "bat", "cmd", "ps1", "psm1", "pif", "lnk", "url", "hta", "vbs",
    "vbe", "js", "jse", "wsf", "reg", "cpl", "jar", "chm", "scf",
];

fn reject_disguised_executable(name: &str) -> Result<()> {
    for part in name.split(['/', '\\']) {
        let lower = part.to_lowercase();
        let mut suffixes = lower.split('.');
        let Some(_stem) = suffixes.next() else {
            continue;
        };
        let extra: Vec<&str> = suffixes.collect();
        if extra.len() < 2 {
            continue;
        }
        if extra[..extra.len() - 1]
            .iter()
            .any(|extension| DISGUISED_EXECUTABLE_EXTENSIONS.contains(extension))
        {
            bail!(
                "Plik „{name}” ma rozszerzenie programu ukryte przed rozszerzeniem filmu. Torrent nie został uruchomiony."
            );
        }
    }
    Ok(())
}

fn is_playable_media(name: &str) -> bool {
    let ext = name
        .rsplit(['/', '\\', '.'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    [
        "mkv", "mk3d", "mp4", "m4v", "mov", "qt", "webm", "avi", "mpeg", "mpg", "mpe", "m1v",
        "m2v", "mpv", "ts", "m2ts", "mts", "ogv", "ogm", "wmv", "flv", "f4v", "3gp", "3g2", "asf",
        "rm", "rmvb", "vob", "mxf", "nsv", "nuv", "y4m", "ivf", "roq", "amv", "srt", "ass", "ssa",
        "vtt", "sub", "idx",
    ]
    .contains(&ext.as_str())
}

pub fn validate_name(name: &str) -> Result<()> {
    validate_path(name)?;
    reject_disguised_executable(name)?;
    if !is_playable_media(name) {
        bail!(
            "Plik „{name}” nie jest plikiem wideo ani napisami. Opisy .nfo, obrazy i archiwa są dodatkami, których nie da się odtworzyć."
        );
    }
    Ok(())
}

/// Skip unsupported attachments, but validate every path before ignoring its
/// extension so unsafe torrent paths cannot bypass the Windows path policy.
pub fn is_selected_for_playback(name: &str) -> Result<bool> {
    validate_path(name)?;
    if !is_playable_media(name) {
        return Ok(false);
    }
    reject_disguised_executable(name)?;
    Ok(true)
}

pub fn validate_existing_path(root: &Path, relative: &str) -> Result<()> {
    validate_name(relative)?;
    validate_storage_path(root, relative)
}

fn validate_storage_path(root: &Path, relative: &str) -> Result<()> {
    validate_path(relative)?;
    let mut path = root.to_path_buf();
    for part in std::iter::once(None).chain(relative.split(['/', '\\']).map(Some)) {
        if let Some(part) = part {
            path.push(part);
        }
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                let mut link = metadata.file_type().is_symlink();
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    link |= metadata.file_attributes() & 0x400 != 0;
                }
                if link {
                    bail!("torrent path crosses a symbolic link or reparse point");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// Validate every storage path before offering an exception for media names.
/// Attachments remain unselected, including executable files and archives.
pub fn select_source_files(root: &Path, info_hash: &str, names: &[String]) -> Result<Vec<usize>> {
    for name in names {
        validate_storage_path(root, name)?;
    }
    let mut selected = Vec::new();
    for (index, name) in names.iter().enumerate() {
        if !is_playable_media(name) {
            continue;
        }
        validate_source_path(root, info_hash, name)?;
        selected.push(index);
    }
    anyhow::ensure!(
        !selected.is_empty(),
        "W tym torrencie nie ma pliku wideo ani napisów do odtworzenia."
    );
    Ok(selected)
}

pub fn validate_source_path(root: &Path, info_hash: &str, relative: &str) -> Result<()> {
    validate_storage_path(root, relative)?;
    anyhow::ensure!(
        is_playable_media(relative),
        "Wybrany plik nie jest wideo ani napisami."
    );
    if !crate::source_file_review::is_approved(info_hash) {
        if let Err(error) = reject_disguised_executable(relative) {
            crate::source_file_review::record(info_hash, error.to_string());
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_exception_keeps_attachments_unselected_and_expires_with_guard() {
        let root = tempfile::tempdir().unwrap();
        let hash = "policy-source-exception";
        let names =
            ["film.cmd.mkv", "opis.nfo", "installer.exe", "subs/pol.srt"].map(str::to_owned);
        assert!(select_source_files(root.path(), hash, &names).is_err());
        assert!(
            crate::source_file_review::pending_reason(hash)
                .unwrap()
                .contains("film.cmd.mkv")
        );
        let approval = crate::source_file_review::approve(hash).unwrap();
        assert_eq!(
            select_source_files(root.path(), hash, &names).unwrap(),
            [0, 3]
        );
        assert!(validate_source_path(root.path(), hash, "film.cmd.mkv").is_ok());
        for forbidden in [
            "../film.mkv",
            "CON.mkv",
            "film.mkv:evil.exe",
            "installer.exe",
            "film.mkv.exe",
        ] {
            assert!(validate_source_path(root.path(), hash, forbidden).is_err());
        }
        assert!(select_source_files(root.path(), "another-policy-source", &names).is_err());
        drop(approval);
        assert!(select_source_files(root.path(), hash, &names).is_err());
    }

    #[test]
    fn hard_path_errors_and_missing_media_do_not_offer_an_exception() {
        let root = tempfile::tempdir().unwrap();
        let hash = "policy-hard-path";
        let names = ["film.cmd.mkv".into(), "../README.txt".into()];
        assert!(select_source_files(root.path(), hash, &names).is_err());
        assert!(crate::source_file_review::pending_reason(hash).is_none());
        assert!(crate::source_file_review::approve(hash).is_none());
        let hash = "policy-no-media";
        assert!(
            select_source_files(root.path(), hash, &["setup.exe".into(), "opis.nfo".into()])
                .is_err()
        );
        assert!(crate::source_file_review::pending_reason(hash).is_none());
        assert!(crate::source_file_review::approve(hash).is_none());
    }

    #[test]
    fn source_file_selection_skips_ordinary_extras_without_prompting() {
        let root = tempfile::tempdir().unwrap();
        let hash = "policy-ordinary-extras";
        let names = [
            "movie.mkv",
            "README.nfo",
            "poster.jpg",
            "archive.zip",
            "setup.exe",
        ]
        .map(str::to_owned);
        assert_eq!(select_source_files(root.path(), hash, &names).unwrap(), [0]);
        assert!(crate::source_file_review::pending_reason(hash).is_none());
    }

    #[test]
    fn media_and_subtitles_are_allowed() {
        for name in [
            "Film.PL.2026.mkv",
            "Season 1/Episode 1.MP4",
            "Napisy/Polskie.srt",
            "subs\\pol.ass",
            "Movie.2024.1080p.BluRay.x264-YTS.COM.mkv",
            "YTS.COM/Movie.mkv",
            "www.example.com/Movie.2024.mp4",
            "Movie.2024.SCR.XviD.mkv",
            "Napisy/YTS.COM.srt",
        ] {
            validate_name(name).unwrap();
            assert!(
                is_selected_for_playback(name).unwrap(),
                "false-positive unsafe name: {name:?}"
            );
        }
    }

    #[test]
    fn unsupported_attachments_are_skipped_without_blocking_media() {
        assert!(is_selected_for_playback("Film.mkv").unwrap());
        assert!(!is_selected_for_playback("opis.nfo").unwrap());
        assert!(!is_selected_for_playback("poster.jpg").unwrap());
        assert!(!is_selected_for_playback("README.txt").unwrap());
        assert!(!is_selected_for_playback("crack.exe").unwrap());
        assert!(!is_selected_for_playback("archive.zip").unwrap());
        assert!(is_selected_for_playback("../README.txt").is_err());
        assert!(is_selected_for_playback("CON.nfo").is_err());
        assert!(is_selected_for_playback("film.cmd.mkv").is_err());
        assert!(is_selected_for_playback("../film.mkv").is_err());
    }

    #[test]
    fn invalid_extensions_and_windows_path_tricks_are_rejected_for_playback() {
        for name in [
            "evil.exe",
            "evil.MSI",
            "film.cmd.mkv",
            "x.ps1.mp4",
            "film.exe.mkv",
            "film.mkv.exe",
            "a.zip",
            "a.rar",
            "a.7z",
            "README.txt",
            "../film.mkv",
            "folder/../film.mkv",
            "/film.mkv",
            "\\\\host\\film.mkv",
            "C:\\film.mkv",
            "film.mkv:evil.exe",
            "CON.mkv",
            "lpt9.srt",
            "COM¹.mkv",
            "film.mkv.",
            "film.mkv ",
            "x\0.mkv",
            "file.url",
            "x.hta",
            "",
        ] {
            assert!(
                validate_name(name).is_err(),
                "accepted unsafe name: {name:?}"
            );
        }
    }
}
