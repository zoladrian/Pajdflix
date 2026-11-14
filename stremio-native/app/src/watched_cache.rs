use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MARKER_DIR: &str = "watched";
static MARKER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Clone, Debug, PartialEq, Eq)]
struct Record {
    last_played: u64,
    completed: bool,
    filename: String,
    closed_at: Option<u64>,
}

/// Update last-played time when this torrent file is opened.
/// A replay cancels a previous close. Completion is recorded separately.
pub fn note_played(info_hash: &str, filename: Option<&str>) {
    update(info_hash, filename, false);
}

/// Remember that this file reached 95% of its duration.
/// Completion itself does not close a paused or buffered player.
pub fn note_completed(info_hash: &str, filename: Option<&str>) {
    update(info_hash, filename, true);
}

pub fn note_closed(info_hash: &str, filename: Option<&str>) {
    let _guard = MARKER_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let hash = info_hash.to_ascii_lowercase();
    if hash.len() != 40 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return;
    }
    let Ok(stamp) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return;
    };
    stream_server::playback_cache::set_playing(&hash, false);
    let Some(filename) = resolve_filename(&hash, filename) else {
        return;
    };
    let filename = filename.as_str();
    if filename.is_empty() {
        return;
    }
    let directory = crate::paths::get()
        .streaming_server_cache()
        .join(MARKER_DIR);
    if std::fs::create_dir_all(&directory).is_err() {
        return;
    }
    let marker = marker_path(&hash);
    let mut records = std::fs::read_to_string(&marker)
        .ok()
        .map(|text| {
            parse_records(&stream_server::cache_cleaner::normalize_watched_text(
                crate::paths::get().streaming_server_cache(),
                &hash,
                &text,
            ))
        })
        .unwrap_or_default();
    let only_one = records.len() == 1;
    if let Some(record) = records
        .iter_mut()
        .find(|record| record.filename == filename || (filename.is_empty() && only_one))
    {
        record.closed_at = Some(stamp.as_secs());
        if !filename.is_empty() {
            record.filename = filename.to_owned();
        }
    } else {
        records.push(Record {
            last_played: stamp.as_secs(),
            completed: false,
            filename: filename.to_owned(),
            closed_at: Some(stamp.as_secs()),
        });
    }
    match std::fs::write(marker, render_records(&records)) {
        Ok(()) => {
            tracing::info!(info_hash = %hash, file = filename, closed_at = stamp.as_secs(), delete_after = stamp.as_secs(), "cache_cleanup_scheduled")
        }
        Err(error) => tracing::warn!(info_hash = %hash, %error, "cache_cleanup_schedule_failed"),
    }
    if let Some(state) = stream_server::GLOBAL_STATE
        .read()
        .ok()
        .and_then(|state| state.clone())
    {
        tokio::spawn(stream_server::cache_cleaner::after_playback_closed(
            std::sync::Arc::new(state),
        ));
    }
}

/// Stop the torrent and delete this title or episode from the streaming cache.
pub fn delete_cached_media(info_hash: &str, filename: Option<&str>, whole_title: bool) {
    let hash = info_hash.to_ascii_lowercase();
    if hash.len() != 40 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return;
    }
    let filename = if whole_title {
        None
    } else {
        resolve_filename(&hash, filename)
    };
    if !whole_title && filename.is_none() {
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let Ok(_access) = stream_server::playback_cache::access_gate().try_write_owned() else {
            return;
        };
        if stream_server::playback_cache::is_playing(&hash) {
            return;
        }
        let Some(state) = stream_server::GLOBAL_STATE
            .read()
            .ok()
            .and_then(|guard| guard.clone())
        else {
            return;
        };
        let engine = state.stream_engine();
        let folder = engine.download_dir.join(&hash);
        if !engine
            .retire_unreferenced_engine(&hash)
            .await
            .unwrap_or(false)
        {
            return;
        }
        if !stream_server::playback_cache::unlinked_path(&folder) {
            tracing::warn!(info_hash = %hash, "cache_cleanup_manual_unsafe_path");
            return;
        }
        if whole_title {
            match stream_server::playback_cache::remove_inactive_directory(&hash, &folder) {
                Ok(true) => {
                    let _ = tokio::fs::remove_file(marker_path(&hash)).await;
                    tracing::info!(info_hash = %hash, "cache_cleanup_manual_title_deleted");
                }
                Ok(false) => return,
                Err(error) => {
                    tracing::error!(info_hash = %hash, %error, "cache_cleanup_manual_delete_failed")
                }
            }
            return;
        }
        let Some(filename) = filename else {
            return;
        };
        let Some(path) =
            stream_server::playback_cache::payload_path(&engine.download_dir, &hash, &filename)
        else {
            tracing::warn!(info_hash = %hash, "cache_cleanup_manual_file_not_found");
            return;
        };
        match stream_server::playback_cache::remove_inactive_file(&hash, &path) {
            Ok(true) => {
                drop_record(&hash, &filename);
                tracing::info!(info_hash = %hash, file = %filename, "cache_cleanup_manual_episode_deleted");
            }
            Ok(false) => return,
            Err(error) => {
                tracing::error!(info_hash = %hash, file = %filename, %error, "cache_cleanup_manual_delete_failed")
            }
        }
    });
}

fn marker_path(hash: &str) -> PathBuf {
    crate::paths::get()
        .streaming_server_cache()
        .join(MARKER_DIR)
        .join(hash)
}

fn drop_record(hash: &str, filename: &str) {
    let _guard = MARKER_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let marker = marker_path(hash);
    let Ok(text) = std::fs::read_to_string(&marker) else {
        return;
    };
    let records: Vec<_> = parse_records(&text)
        .into_iter()
        .filter(|record| record.filename != filename)
        .collect();
    if records.is_empty() {
        let _ = std::fs::remove_file(marker);
    } else {
        let _ = std::fs::write(marker, render_records(&records));
    }
}

pub fn torrent_hash(player: &stremio_core::models::player::Player) -> Option<String> {
    let source = &player.selected.as_ref()?.stream.source;
    let stremio_core::types::resource::StreamSource::Torrent { info_hash, .. } = source else {
        return None;
    };
    Some(hex::encode(info_hash))
}

fn resolve_filename(hash: &str, raw: Option<&str>) -> Option<String> {
    let resolved = stream_server::playback_cache::resolve_filename(
        crate::paths::get().streaming_server_cache(),
        hash,
        raw.unwrap_or("").trim(),
    );
    if resolved.is_none() {
        // Do not log the raw MPV filename: it may contain a signed URL token.
        tracing::warn!(info_hash = hash, "cache_cleanup_identity_unresolved");
    }
    resolved
}

fn update(info_hash: &str, filename: Option<&str>, completed: bool) {
    let _guard = MARKER_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let hash = info_hash.to_ascii_lowercase();
    if hash.len() != 40 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return;
    }
    let Ok(stamp) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return;
    };
    let directory = crate::paths::get()
        .streaming_server_cache()
        .join(MARKER_DIR);
    if std::fs::create_dir_all(&directory).is_err() {
        return;
    }
    let marker = marker_path(&hash);
    let mut records = std::fs::read_to_string(&marker)
        .ok()
        .map(|text| {
            parse_records(&stream_server::cache_cleaner::normalize_watched_text(
                crate::paths::get().streaming_server_cache(),
                &hash,
                &text,
            ))
        })
        .unwrap_or_default();
    let Some(filename) = resolve_filename(&hash, filename) else {
        return;
    };
    if completed
        && records
            .iter()
            .any(|record| record.filename == filename && record.completed)
    {
        return;
    }
    upsert(&mut records, &filename, completed, stamp.as_secs());
    match std::fs::write(marker, render_records(&records)) {
        Ok(()) => {
            tracing::info!(info_hash = %hash, file = %filename, completed, "cache_cleanup_playback_recorded")
        }
        Err(error) => {
            tracing::warn!(info_hash = %hash, %error, "cache_cleanup_marker_write_failed")
        }
    }
}

fn parse_records(text: &str) -> Vec<Record> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.iter().any(|line| line.contains('\t')) {
        return lines
            .into_iter()
            .filter_map(|line| {
                let mut parts = line.splitn(4, '\t');
                let last_played = parts.next()?.parse().ok()?;
                let completed = parts.next()? == "1";
                let filename = parts.next().unwrap_or("").trim().to_owned();
                let closed_at = parts.next().and_then(|value| value.parse().ok());
                Some(Record {
                    last_played,
                    completed,
                    filename,
                    closed_at,
                })
            })
            .collect();
    }
    let mut lines = text.lines();
    let Some(last_played) = lines.next().and_then(|line| line.parse().ok()) else {
        return Vec::new();
    };
    let filename = lines.next().unwrap_or("").trim().to_owned();
    vec![Record {
        last_played,
        completed: true,
        filename,
        closed_at: None,
    }]
}

fn render_records(records: &[Record]) -> String {
    records
        .iter()
        .map(|record| {
            format!(
                "{}\t{}\t{}\t{}",
                record.last_played,
                if record.completed { "1" } else { "0" },
                record.filename,
                record
                    .closed_at
                    .map(|time| time.to_string())
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn upsert(records: &mut Vec<Record>, filename: &str, completed: bool, now: u64) {
    let only_one = records.len() == 1;
    if let Some(record) = records
        .iter_mut()
        .find(|record| record.filename == filename || (filename.is_empty() && only_one))
    {
        record.last_played = now;
        record.closed_at = None;
        if completed {
            record.completed = true;
        }
        if !filename.is_empty() {
            record.filename = filename.to_owned();
        }
        return;
    }
    records.push(Record {
        last_played: now,
        completed,
        filename: filename.to_owned(),
        closed_at: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_marker_is_treated_as_completed() {
        let records = parse_records("1700000000\nShow.S01E01.mkv\n");
        assert_eq!(
            records,
            vec![Record {
                last_played: 1_700_000_000,
                completed: true,
                filename: "Show.S01E01.mkv".into(),
                closed_at: None,
            }]
        );
    }

    #[test]
    fn replay_moves_the_clock_without_clearing_95_percent() {
        let mut records = parse_records("100\t1\tep1.mkv\n100\t0\tep2.mkv\n");
        upsert(&mut records, "ep1.mkv", false, 200);
        upsert(&mut records, "ep2.mkv", true, 200);
        assert_eq!(records[0].last_played, 200);
        assert!(records[0].completed);
        assert_eq!(records[1].last_played, 200);
        assert!(records[1].completed);
        assert_eq!(
            parse_records(&render_records(&records)).len(),
            2,
            "{}",
            render_records(&records)
        );
    }

    #[test]
    fn season_pack_tracks_episodes_separately() {
        let mut records = Vec::new();
        upsert(&mut records, "ep1.mkv", true, 10);
        upsert(&mut records, "ep2.mkv", false, 20);
        assert_eq!(records.len(), 2);
        assert!(records[0].completed);
        assert!(!records[1].completed);
    }
}
