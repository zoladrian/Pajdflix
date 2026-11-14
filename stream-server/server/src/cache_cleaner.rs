use crate::state::AppState;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Serializes the GUI cleaner with the optional headless maintenance process.
fn cleanup_lock(cache_root: &std::path::Path) -> anyhow::Result<Option<fslock::LockFile>> {
    std::fs::create_dir_all(cache_root)?;
    let mut lock = fslock::LockFile::open(&cache_root.join("cleanup.lock"))?;
    Ok(lock.try_lock()?.then_some(lock))
}

pub fn start(state: Arc<AppState>) -> JoinHandle<()> {
    tokio::spawn(async move {
        info!(retention_secs = 0, poll_secs = 60, "cache_cleanup_started");
        if let Err(error) = clean_cache(&state).await {
            error!(%error, "cache_cleanup_startup_failed");
        }

        // Channel for file system events
        let (tx, mut rx) = mpsc::channel::<()>(100);

        // Setup Watcher
        // We use a sync watcher bridge to async channel
        let tx_clone = tx.clone();
        let mut watcher = match RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                match res {
                    Ok(event) => {
                        // Filter interesting events
                        if matches!(
                            event.kind,
                            notify::EventKind::Create(_)
                                | notify::EventKind::Modify(_)
                                | notify::EventKind::Remove(_)
                        ) {
                            let _ = tx_clone.blocking_send(());
                        }
                    }
                    Err(e) => error!("Watch error: {:?}", e),
                }
            },
            notify::Config::default(),
        ) {
            Ok(w) => Some(w),
            Err(e) => {
                error!("Failed to create watcher: {}", e);
                None // Periodic cleanup must still work without filesystem notifications.
            }
        };

        // Initial watch
        // We might need to retry if directory doesn't exist yet
        let mut download_dirs = vec![
            state.engine.download_dir.clone(),
            state.download_engine.download_dir.clone(),
        ];
        download_dirs.sort();
        download_dirs.dedup();
        for download_dir in &download_dirs {
            if let Some(watcher) = watcher.as_mut()
                && let Err(e) = watcher.watch(download_dir, RecursiveMode::Recursive)
            {
                warn!("Failed to watch download dir {:?}: {}", download_dir, e);
                // We will try to re-watch inside the loop if needed (omitted for brevity, relying on fallback poll)
            }
        }

        // Scan at least once a minute, including while downloads keep changing.
        let mut poll_interval = tokio::time::interval(Duration::from_secs(60));

        // State for debouncing
        let debounce_duration = Duration::from_secs(60); // Wait 60s after last activity to clean
        let mut active_cleaning_timer = Box::pin(tokio::time::sleep(Duration::MAX)); // Inactive initially

        loop {
            tokio::select! {
                // 1. Fallback / Periodic Poll
                _ = poll_interval.tick() => {
                    debug!("Periodic cache clean trigger");
                    if let Err(e) = clean_cache(&state).await {
                        error!("Cache cleaner error: {}", e);
                    }
                    // Re-ensure watch if needed
                    for download_dir in [&state.engine.download_dir, &state.download_engine.download_dir] {
                        if let Some(watcher) = watcher.as_mut()
                            && let Err(e) = watcher.watch(download_dir, RecursiveMode::Recursive) {
                            debug!("Retry watch {:?}: {}", download_dir, e);
                        }
                    }
                }

                // 2. File System Event
                Some(_) = rx.recv() => {
                    // Reset debounce timer
                    active_cleaning_timer = Box::pin(tokio::time::sleep(debounce_duration));
                }

                // 3. Debounce Timer Fired
                _ = &mut active_cleaning_timer => {
                    debug!("Debounced cache clean trigger");
                    if let Err(e) = clean_cache(&state).await {
                        error!("Cache cleaner error: {}", e);
                    }
                    // Reset timer to infinite
                    active_cleaning_timer = Box::pin(tokio::time::sleep(Duration::MAX));
                }
            }
        }
    })
}

async fn clean_cache(state: &Arc<AppState>) -> anyhow::Result<()> {
    let Ok(_access) = crate::playback_cache::access_gate().try_write_owned() else {
        info!("cache_cleanup_deferred_open_request");
        return Ok(());
    };
    let Some(root) = state.engine.download_dir.parent() else {
        return Ok(());
    };
    let Some(_guard) = cleanup_lock(root)? else {
        return Ok(());
    };
    let settings = state.settings.read().await;
    let limit = settings.cache_size as u64;
    let delete_after_playback = true; // Always enabled for managed torrent payloads.
    drop(settings); // Release lock

    let mut download_dirs = vec![
        state.engine.download_dir.clone(),
        state.download_engine.download_dir.clone(),
    ];
    download_dirs.sort();
    download_dirs.dedup();
    if download_dirs
        .iter()
        .all(|download_dir| !download_dir.exists())
    {
        return Ok(());
    }

    // Release only explicitly closed torrents with no reader/probe owners.
    // The HTTP write gate prevents a new request racing retirement/deletion.
    for engine_fs in [&state.engine, &state.download_engine] {
        let Some(cache_root) = engine_fs.download_dir.parent() else {
            continue;
        };
        for hash in engine_fs.list_engines().await {
            if crate::playback_cache::is_playing(&hash) {
                continue;
            }
            let Ok(text) = tokio::fs::read_to_string(cache_root.join("watched").join(&hash)).await
            else {
                continue;
            };
            let records = parse_watched_records(&normalize_watched_text(cache_root, &hash, &text));
            if records.is_empty() || records.iter().any(|record| record.closed_at.is_none()) {
                continue;
            }
            match engine_fs.retire_unreferenced_engine(&hash).await {
                Ok(true) => info!(info_hash = %hash, "cache_cleanup_closed_engine_released"),
                Ok(false) => info!(info_hash = %hash, "cache_cleanup_deferred_active_reader"),
                Err(error) => {
                    warn!(info_hash = %hash, %error, "cache_cleanup_engine_release_failed")
                }
            }
        }
    }

    // 1. Identify protected files matching current active engines
    let mut protected_paths = HashSet::new();
    for root in &download_dirs {
        for hash in crate::playback_cache::playing_hashes() {
            protected_paths.insert(root.join(hash));
        }
    }

    for (root, engines) in [
        (
            state.engine.download_dir.clone(),
            state.engine.list_engines().await,
        ),
        (
            state.download_engine.download_dir.clone(),
            state.download_engine.list_engines().await,
        ),
    ] {
        for hash in engines {
            // Payload lives under root/<info-hash>, including during initialization.
            // Protect the entire torrent (subtitles and partially created files too).
            if hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                protected_paths.insert(root.join(hash.to_ascii_lowercase()));
            }
        }
    }

    expire_watched_downloads(&download_dirs, &protected_paths, delete_after_playback).await;

    // 2. Scan and Evict immediately based on age (30 days)
    let thirty_days = Duration::from_secs(30 * 24 * 60 * 60);
    let now = std::time::SystemTime::now();

    let mut files = Vec::new();
    let mut total_size = 0u64;

    for download_dir in &download_dirs {
        if !download_dir.exists() {
            continue;
        }

        let mut entries = walkdir::WalkDir::new(download_dir)
            .follow_links(false)
            .into_iter();

        loop {
            match entries.next() {
                Some(Ok(entry)) => {
                    if entry.file_type().is_file() {
                        let path = entry.path().to_path_buf();
                        if path
                            .components()
                            .any(|component| component.as_os_str() == ".metadata")
                        {
                            continue;
                        }
                        // Only payloads in hash-named torrent folders are disposable.
                        let Some(relative) = path.strip_prefix(download_dir).ok() else {
                            continue;
                        };
                        let mut parts = relative.components();
                        let Some(hash) = parts.next().and_then(|part| part.as_os_str().to_str())
                        else {
                            continue;
                        };
                        if !crate::playback_cache::valid_hash(hash)
                            || !crate::playback_cache::unlinked_path(&path)
                        {
                            continue;
                        }
                        // Is protected?
                        let is_protected = protected_paths.contains(&path)
                            || protected_paths.iter().any(|p| path.starts_with(p));

                        if let Ok(metadata) = entry.metadata() {
                            let size = metadata.len();

                            if is_protected {
                                total_size += size;
                                continue;
                            }

                            if let Ok(modified) = metadata.modified() {
                                // Check AGE
                                let age = now
                                    .duration_since(modified)
                                    .unwrap_or(Duration::from_secs(0));
                                if age > thirty_days {
                                    info!("File older than 30 days, deleting: {:?}", path);
                                    if let Err(e) = tokio::fs::remove_file(&path).await {
                                        error!("Failed to delete file {:?}: {}", path, e);
                                        // Count it in total size since we failed to delete?
                                        // Or ignore? Let's count it to be safe for cache limit.
                                        total_size += size;
                                    } else {
                                        // Successfully deleted, do not add to total_size
                                        // Try to clean empty parent dir
                                        if let Some(parent) = path.parent() {
                                            remove_empty_parents(parent, download_dir).await;
                                        }
                                    }
                                } else {
                                    // Keep for potentially size-based eviction
                                    total_size += size;
                                    files.push((path, size, modified));
                                }
                            } else {
                                // Could not read time, keep it but count size
                                total_size += size;
                                files.push((path, size, std::time::SystemTime::UNIX_EPOCH));
                            }
                        }
                    }
                }
                Some(Err(e)) => {
                    debug!("Error walking directory: {}", e);
                }
                None => break,
            }
        }
    }

    // 3. Size-based Eviction
    if limit > 0 && total_size > limit {
        info!(
            "Cache size {} exceeds limit {}. Cleaning up...",
            total_size, limit
        );

        // Sort by modification time (oldest first)
        files.sort_by_key(|a| a.2);

        let mut deleted_count = 0;
        let mut freed_space = 0;

        for (path, size, _) in files {
            if total_size <= limit {
                break;
            }

            debug!("Deleting old file (size limit): {:?}", path);
            if let Err(e) = tokio::fs::remove_file(&path).await {
                error!("Failed to delete file {:?}: {}", path, e);
            } else {
                total_size = total_size.saturating_sub(size);
                freed_space += size;
                deleted_count += 1;

                if let Some(parent) = path.parent()
                    && let Some(root) = download_dirs.iter().find(|root| path.starts_with(root))
                {
                    remove_empty_parents(parent, root).await;
                }
            }
        }

        info!(
            "Cleaned up {} files, freed {} bytes. New size: {}",
            deleted_count, freed_space, total_size
        );
    }

    Ok(())
}

/// Retry promptly while MPV releases its response body after an explicit close.
pub async fn after_playback_closed(state: Arc<AppState>) {
    for _ in 0..20 {
        if let Err(error) = clean_cache(&state).await {
            error!(%error, "cache_cleanup_close_scan_failed");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

const WATCHED_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// Run before the GUI/profile picker starts. The single-instance guard ensures
/// no other Pajdflix is playing these files. Interrupted playback is marked
/// closed only after the previous GUI process is known to have stopped.
pub async fn startup_cleanup(cache_root: &std::path::Path) {
    while !offline_cleanup(cache_root, true).await {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub async fn offline_cleanup(cache_root: &std::path::Path, recover_interrupted: bool) -> bool {
    let _guard = match cleanup_lock(cache_root) {
        Ok(Some(guard)) => guard,
        Ok(None) => return false,
        Err(error) => {
            error!(%error, "cache_cleanup_lock_failed");
            return true;
        }
    };
    let dirs = vec![
        cache_root.join("rqbit-downloads"),
        cache_root.join("torrent-cache"),
    ];
    let watched = cache_root.join("watched");
    if watched.exists() && !crate::playback_cache::unlinked_path(&watched) {
        warn!("cache_cleanup_skipped_linked_marker_directory");
        return true;
    }
    if let Ok(mut entries) = tokio::fs::read_dir(&watched).await {
        let now = now_secs();
        while let Ok(Some(entry)) = entries.next_entry().await {
            if !crate::playback_cache::valid_hash(&entry.file_name().to_string_lossy()) {
                continue;
            }
            let Ok(text) = tokio::fs::read_to_string(entry.path()).await else {
                continue;
            };
            let mut records = parse_watched_records(&text);
            let mut changed = false;
            for record in &mut records {
                if recover_interrupted && record.closed_at.is_none() {
                    record.closed_at = Some(now);
                    changed = true;
                }
            }
            if changed {
                match tokio::fs::write(entry.path(), render_watched_records(&records)).await {
                    Ok(()) => {
                        info!(info_hash = %entry.file_name().to_string_lossy(), delete_after = now, "cache_cleanup_recovered_interrupted_playback")
                    }
                    Err(error) => warn!(%error, "cache_cleanup_marker_write_failed"),
                }
            }
        }
    }
    info!("cache_cleanup_startup_scan");
    expire_watched_downloads(&dirs, &HashSet::new(), true).await;
    true
}

/// Merge old URL identities with real filenames before applying a deadline.
/// A stale record must never override a later replay of the same episode.
pub fn normalize_watched_text(cache_root: &std::path::Path, hash: &str, text: &str) -> String {
    let mut merged: Vec<WatchedRecord> = Vec::new();
    for mut record in parse_watched_records(text) {
        if let Some(name) =
            crate::playback_cache::resolve_filename(cache_root, hash, &record.filename)
        {
            record.filename = name;
        }
        if let Some(existing) = merged
            .iter_mut()
            .find(|old| old.filename == record.filename)
        {
            let completed = existing.completed || record.completed;
            if record.last_played > existing.last_played {
                *existing = record;
            } else if record.last_played == existing.last_played {
                existing.closed_at = existing.closed_at.max(record.closed_at);
            }
            existing.completed = completed;
        } else {
            merged.push(record);
        }
    }
    render_watched_records(&merged)
}

fn expiry_reason(record: &WatchedRecord, now: u64, enabled: bool) -> Option<&'static str> {
    if enabled && let Some(closed) = record.closed_at {
        (now >= closed).then_some("playback_closed")
    } else if record.completed
        && now.saturating_sub(record.last_played) >= WATCHED_RETENTION.as_secs()
    {
        Some("completed_24_hours")
    } else {
        None
    }
}

async fn expire_watched_downloads(
    download_dirs: &[std::path::PathBuf],
    protected_paths: &HashSet<std::path::PathBuf>,
    delete_after_playback: bool,
) {
    let now = now_secs();
    let roots: HashSet<_> = download_dirs
        .iter()
        .filter_map(|dir| dir.parent())
        .collect();
    let (mut removed, mut pending, mut protected, mut failed) = (0u64, 0u64, 0u64, 0u64);
    for cache_root in roots {
        let watched_dir = cache_root.join("watched");
        if !crate::playback_cache::unlinked_path(&watched_dir) {
            continue;
        }
        let Ok(mut entries) = tokio::fs::read_dir(&watched_dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let hash = entry.file_name().to_string_lossy().into_owned();
            if !crate::playback_cache::valid_hash(&hash)
                || !crate::playback_cache::unlinked_path(&entry.path())
            {
                continue;
            }
            let Ok(text) = tokio::fs::read_to_string(entry.path()).await else {
                continue;
            };
            let records = parse_watched_records(&normalize_watched_text(cache_root, &hash, &text));
            let mut remaining = Vec::new();
            for mut record in records {
                let Some(reason) = expiry_reason(&record, now, delete_after_playback) else {
                    pending += 1;
                    remaining.push(record);
                    continue;
                };
                let Some(filename) =
                    crate::playback_cache::resolve_filename(cache_root, &hash, &record.filename)
                else {
                    failed += 1;
                    warn!(info_hash = %hash, reason, "cache_cleanup_skipped_unresolved_identity");
                    remaining.push(record);
                    continue;
                };
                record.filename = filename.clone();
                let candidates: Vec<_> = download_dirs
                    .iter()
                    .filter(|dir| dir.parent() == Some(cache_root))
                    .collect();
                if crate::playback_cache::is_playing(&hash)
                    || candidates
                        .iter()
                        .any(|dir| protected_paths.contains(&dir.join(&hash)))
                {
                    protected += 1;
                    info!(info_hash = %hash, file = %filename, reason, "cache_cleanup_deferred_active_torrent");
                    remaining.push(record);
                    continue;
                }
                let mut file_failed = false;
                for dir in candidates {
                    let path = dir.join(&hash).join(&filename);
                    match tokio::fs::symlink_metadata(&path).await {
                        Ok(_) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(error) => {
                            file_failed = true;
                            warn!(info_hash = %hash, file = %filename, %error, "cache_cleanup_stat_failed");
                            continue;
                        }
                    }
                    let Some(path) = crate::playback_cache::payload_path(dir, &hash, &filename)
                    else {
                        file_failed = true;
                        warn!(info_hash = %hash, "cache_cleanup_skipped_unsafe_path");
                        continue;
                    };
                    let bytes = tokio::fs::metadata(&path)
                        .await
                        .map(|m| m.len())
                        .unwrap_or(0);
                    match crate::playback_cache::remove_inactive_file(&hash, &path) {
                        Ok(true) => {
                            removed += 1;
                            info!(info_hash = %hash, file = %filename, path = %path.display(), bytes, reason, closed_at = record.closed_at, "cache_cleanup_deleted");
                            if let Some(parent) = path.parent() {
                                remove_empty_parents(parent, dir).await;
                            }
                        }
                        Ok(false) => {
                            file_failed = true;
                            info!(info_hash = %hash, "cache_cleanup_deferred_logical_playback");
                        }
                        Err(error) => {
                            file_failed = true;
                            error!(info_hash = %hash, file = %filename, %error, reason, "cache_cleanup_delete_failed");
                        }
                    }
                }
                if file_failed {
                    failed += 1;
                    remaining.push(record);
                }
            }
            // Playback callbacks can refresh a record while the scan is running.
            if tokio::fs::read_to_string(entry.path())
                .await
                .ok()
                .as_deref()
                != Some(&text)
            {
                info!(info_hash = %hash, "cache_cleanup_marker_changed_during_scan");
                continue;
            }
            let result = if remaining.is_empty() {
                tokio::fs::remove_file(entry.path()).await
            } else {
                tokio::fs::write(entry.path(), render_watched_records(&remaining)).await
            };
            if let Err(error) = result {
                warn!(info_hash = %hash, %error, "cache_cleanup_marker_write_failed");
            }
        }
    }
    info!(
        removed,
        pending, protected, failed, "cache_cleanup_scan_finished"
    );
}

struct WatchedRecord {
    last_played: u64,
    completed: bool,
    filename: String,
    closed_at: Option<u64>,
}

fn parse_watched_records(text: &str) -> Vec<WatchedRecord> {
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
                Some(WatchedRecord {
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
    vec![WatchedRecord {
        last_played,
        completed: true,
        filename,
        closed_at: None,
    }]
}

fn render_watched_records(records: &[WatchedRecord]) -> String {
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

async fn remove_empty_parents(mut dir: &std::path::Path, root: &std::path::Path) {
    while dir != root {
        if tokio::fs::remove_dir(dir).await.is_err() {
            break;
        }
        let Some(parent) = dir.parent() else {
            break;
        };
        dir = parent;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct LogBuffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for LogBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn record(closed_at: u64) -> WatchedRecord {
        WatchedRecord {
            last_played: closed_at,
            completed: false,
            filename: "Season/ep1.mkv".into(),
            closed_at: Some(closed_at),
        }
    }

    #[test]
    fn closes_immediately_but_never_while_open_or_with_future_timestamp() {
        let mut watched = record(100);
        assert_eq!(expiry_reason(&watched, 99, true), None);
        assert_eq!(expiry_reason(&watched, 100, true), Some("playback_closed"));
        assert_eq!(expiry_reason(&watched, 1900, false), None);
        watched.closed_at = None;
        assert_eq!(expiry_reason(&watched, 1900, true), None);
        assert_eq!(expiry_reason(&record(2000), 1900, true), None);
    }

    #[tokio::test]
    async fn old_signed_url_records_cannot_expire_a_more_recent_replay() {
        let root = tempfile::tempdir().unwrap();
        let hash = "d".repeat(40);
        crate::playback_cache::remember_files(
            root.path(),
            &hash,
            &["Season/ep1.mkv".into()],
            "0",
            0,
        )
        .await;
        let text = "100\t0\t0?token=old\t120\n200\t0\tSeason/ep1.mkv\t";
        let normalized = normalize_watched_text(root.path(), &hash, text);
        let records = parse_watched_records(&normalized);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].last_played, 200);
        assert_eq!(records[0].closed_at, None);
        assert!(!normalized.contains("token"));
    }

    #[tokio::test]
    async fn removes_only_expired_episode_and_preserves_personal_media_and_active_torrent() {
        let output = LogBuffer(Default::default());
        let writer = output.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(move || writer.clone())
            .finish();
        // Cleaner callsites are shared with parallel startup tests. A global
        // capture also covers their first registration without a thread-local
        // subscriber, which otherwise makes this log assertion intermittent.
        tracing::subscriber::set_global_default(subscriber).expect("test log capture");
        let root = tempfile::tempdir().unwrap();
        let hash = "b".repeat(40);
        let dir = root.path().join("rqbit-downloads");
        let folder = dir.join(&hash).join("Season");
        tokio::fs::create_dir_all(&folder).await.unwrap();
        tokio::fs::create_dir_all(root.path().join("watched"))
            .await
            .unwrap();
        for name in ["ep1.mkv", "ep2.mkv"] {
            tokio::fs::write(folder.join(name), b"fixture")
                .await
                .unwrap();
        }
        let personal = root.path().join("my-own-movie.mp4");
        tokio::fs::write(&personal, b"personal").await.unwrap();
        let marker = root.path().join("watched").join(&hash);
        tokio::fs::write(
            &marker,
            render_watched_records(&[record(now_secs() - 1801)]),
        )
        .await
        .unwrap();
        let dirs = vec![root.path().join("torrent-cache"), dir.clone()];
        expire_watched_downloads(&dirs, &HashSet::from([dir.join(&hash)]), true).await;
        assert!(folder.join("ep1.mkv").exists());
        assert!(
            marker.exists(),
            "a missing alternate cache directory must not lose the marker"
        );
        // A paused/minimized/fully buffered player is still logically active,
        // even with a stale closed marker and no open HTTP reader.
        crate::playback_cache::set_playing(&hash, true);
        expire_watched_downloads(&dirs, &HashSet::new(), true).await;
        assert!(folder.join("ep1.mkv").exists());
        crate::playback_cache::set_playing(&hash, false);
        expire_watched_downloads(&dirs, &HashSet::new(), true).await;
        assert!(!folder.join("ep1.mkv").exists());
        assert!(folder.join("ep2.mkv").exists());
        assert_eq!(tokio::fs::read(&personal).await.unwrap(), b"personal");
        assert!(!marker.exists());
        let messages = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        assert!(messages.contains("cache_cleanup_deferred_active_torrent"));
        assert!(
            messages.lines().any(|line| line.contains("cache_cleanup_deleted") && line.contains(&hash)),
            "captured logs: {messages}"
        );
        assert!(messages.contains("playback_closed"));
        assert!(messages.contains("cache_cleanup_scan_finished"));
    }

    #[tokio::test]
    async fn startup_removes_interrupted_playback_when_no_gui_is_running() {
        let root = tempfile::tempdir().unwrap();
        let hash = "c".repeat(40);
        let folder = root
            .path()
            .join("rqbit-downloads")
            .join(&hash)
            .join("Season");
        tokio::fs::create_dir_all(&folder).await.unwrap();
        tokio::fs::create_dir_all(root.path().join("watched"))
            .await
            .unwrap();
        tokio::fs::write(folder.join("ep1.mkv"), b"fixture")
            .await
            .unwrap();
        let marker = root.path().join("watched").join(hash);
        let mut watched = record(now_secs() - 3600);
        watched.closed_at = None;
        tokio::fs::write(&marker, render_watched_records(&[watched]))
            .await
            .unwrap();
        startup_cleanup(root.path()).await;
        assert!(!folder.join("ep1.mkv").exists());
        assert!(!marker.exists());
    }
}
