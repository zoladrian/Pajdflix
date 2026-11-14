//! Identities come from torrent metadata, never from MPV's URL-shaped filename.
use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

static ACTIVE_PLAYBACK: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static ACCESS_GATE: OnceLock<Arc<tokio::sync::RwLock<()>>> = OnceLock::new();

pub fn access_gate() -> Arc<tokio::sync::RwLock<()>> {
    ACCESS_GATE
        .get_or_init(|| Arc::new(tokio::sync::RwLock::new(())))
        .clone()
}

/// Logical playback remains active while paused, minimized or fully buffered.
pub fn set_playing(hash: &str, playing: bool) {
    if !valid_hash(hash) {
        return;
    }
    let mut active = ACTIVE_PLAYBACK.lock().unwrap_or_else(|e| e.into_inner());
    let active = active.get_or_insert_with(HashSet::new);
    if playing {
        active.insert(hash.to_ascii_lowercase());
    } else {
        active.remove(&hash.to_ascii_lowercase());
    }
}

pub fn is_playing(hash: &str) -> bool {
    ACTIVE_PLAYBACK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|active| active.contains(&hash.to_ascii_lowercase()))
}

pub fn playing_hashes() -> Vec<String> {
    ACTIVE_PLAYBACK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|active| active.iter().cloned().collect())
        .unwrap_or_default()
}

/// Call only after both the player and embedded server have stopped.
pub fn clear_playing_after_shutdown() {
    *ACTIVE_PLAYBACK.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub fn remove_inactive_file(hash: &str, path: &Path) -> std::io::Result<bool> {
    let active = ACTIVE_PLAYBACK.lock().unwrap_or_else(|e| e.into_inner());
    if active.as_ref().is_some_and(|active| active.contains(hash)) {
        return Ok(false);
    }
    std::fs::remove_file(path)?;
    Ok(true)
}

pub fn remove_inactive_directory(hash: &str, path: &Path) -> std::io::Result<bool> {
    let active = ACTIVE_PLAYBACK.lock().unwrap_or_else(|e| e.into_inner());
    if active.as_ref().is_some_and(|active| active.contains(hash)) {
        return Ok(false);
    }
    std::fs::remove_dir_all(path)?;
    Ok(true)
}

/// Hold a read lease for the entire HTTP response body, not just its headers.
pub async fn guard_request(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use futures_util::StreamExt;
    let guard = access_gate().read_owned().await;
    let response = next.run(request).await;
    let (parts, body) = response.into_parts();
    let stream = futures_util::stream::unfold(
        (body.into_data_stream(), guard),
        |(mut body, guard)| async move { body.next().await.map(|item| (item, (body, guard))) },
    );
    axum::response::Response::from_parts(parts, axum::body::Body::from_stream(stream))
}

pub fn valid_hash(hash: &str) -> bool {
    hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn safe_relative(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['\t', '\n', '\r', ':', '?'])
        && Path::new(name)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        && !name
            .split(['/', '\\'])
            .any(|part| part == ".." || part.is_empty())
}

fn linked(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

/// Refuse symlinks/junctions anywhere in the path, including the cache root.
pub fn unlinked_path(path: &Path) -> bool {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if let Ok(metadata) = std::fs::symlink_metadata(&current) {
            if linked(&metadata) {
                return false;
            }
        } else {
            return false;
        }
    }
    true
}

pub fn payload_path(download_dir: &Path, hash: &str, name: &str) -> Option<PathBuf> {
    if !valid_hash(hash) || !safe_relative(name) {
        return None;
    }
    let path = download_dir.join(hash).join(name);
    (unlinked_path(&path) && path.is_file()).then_some(path)
}

pub async fn remember_files(
    cache_root: &Path,
    hash: &str,
    names: &[String],
    requested: &str,
    index: usize,
) {
    if !valid_hash(hash) {
        return;
    }
    let mut mapping: HashMap<String, String> = names
        .iter()
        .enumerate()
        .filter(|(_, name)| safe_relative(name))
        .map(|(i, name)| (i.to_string(), name.clone()))
        .collect();
    if let Some(name) = names.get(index).filter(|name| safe_relative(name)) {
        mapping.insert(requested.to_owned(), name.clone());
    }
    let directory = cache_root.join("playback-files");
    let result = async {
        tokio::fs::create_dir_all(&directory).await?;
        anyhow::ensure!(
            unlinked_path(&directory),
            "linked playback identity directory"
        );
        let destination = directory.join(hash);
        let bytes = serde_json::to_vec(&mapping)?;
        if tokio::fs::read(&destination).await.ok().as_deref() == Some(&bytes) {
            return Ok(());
        }
        let temporary = directory.join(format!("{hash}.{}.tmp", uuid::Uuid::new_v4()));
        tokio::fs::write(&temporary, bytes).await?;
        if let Err(error) = tokio::fs::rename(&temporary, destination).await {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(error.into());
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if let Err(error) = result {
        tracing::warn!(info_hash = hash, %error, "cache_cleanup_identity_write_failed");
    }
}

pub fn resolve_filename(cache_root: &Path, hash: &str, raw: &str) -> Option<String> {
    if !valid_hash(hash) {
        return None;
    }
    let mapping: HashMap<String, String> =
        std::fs::read(cache_root.join("playback-files").join(hash))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
    let key = raw.split('?').next().unwrap_or(raw);
    if let Some(name) = mapping.get(key).filter(|name| safe_relative(name)) {
        return Some(name.clone());
    }
    if !safe_relative(raw) {
        return None;
    }
    // A real relative filename is retained. Numeric URL components need metadata.
    if raw == "video" || raw.parse::<usize>().is_ok() {
        return None;
    }
    Some(raw.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_response_body_keeps_cleanup_out_until_the_reader_is_dropped() {
        use tower::ServiceExt;
        let router = axum::Router::new()
            .route("/", axum::routing::get(|| async { "video fixture" }))
            .layer(axum::middleware::from_fn(guard_request));
        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            access_gate().try_write_owned().is_err(),
            "headers do not end a stream's lease"
        );
        drop(response);
        assert!(
            access_gate().try_write_owned().is_ok(),
            "dropping the player response releases the lease"
        );
    }
    #[tokio::test]
    async fn resolves_signed_stream_index_without_selecting_another_episode() {
        let root = tempfile::tempdir().unwrap();
        let hash = "a".repeat(40);
        remember_files(
            root.path(),
            &hash,
            &["Season/ep1.mkv".into(), "Season/ep2.mkv".into()],
            "video",
            1,
        )
        .await;
        assert_eq!(
            resolve_filename(root.path(), &hash, "1?_secure_cap=secret").as_deref(),
            Some("Season/ep2.mkv")
        );
        assert_eq!(
            resolve_filename(root.path(), &hash, "video?token=secret").as_deref(),
            Some("Season/ep2.mkv")
        );
        assert!(resolve_filename(root.path(), &hash, "7?token=secret").is_none());
    }
    #[test]
    fn rejects_paths_outside_cache() {
        for name in [
            "../own.mp4",
            "a/../../own.mp4",
            "C:\\Films\\own.mp4",
            "/Films/own.mp4",
            "..\\own.mp4",
        ] {
            assert!(!safe_relative(name), "{name}");
        }
    }
}
