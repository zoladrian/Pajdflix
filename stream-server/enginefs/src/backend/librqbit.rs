use crate::backend::{
    BackendFileInfo, BackendMemoryDiagnostics, EngineStats, FileStreamTrait, Growler, PeerSearch,
    PieceReadiness, StatsFile, StatsOptions, SwarmCap, TorrentBackend, TorrentHandle,
    TorrentSource,
};
use anyhow::{Context, Result};
use librqbit::{ManagedTorrent, Session};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;
use tracing::{debug, warn};

pub struct LibrqbitBackend {
    pub session: Arc<Session>,
    completion_monitor: tokio::task::JoinHandle<()>,
    download_dir: PathBuf,
    scanner: Arc<crate::file_scan::FileScanner>,
    lifecycle_locks: std::sync::Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
}

/// Applied before Session starts any workers. The Cargo feature alone is insufficient.
pub fn download_only_session_options() -> librqbit::SessionOptions {
    librqbit::SessionOptions {
        disable_upload: true,
        enable_upnp_port_forwarding: false,
        listen_port_range: None,
        // No automatic restore may start downloading before metadata policy runs.
        persistence: None,
        fastresume: false,
        disable_dht: true,
        disable_dht_persistence: true,
        peer_opts: Some(librqbit::PeerConnectionOptions {
            connect_timeout: Some(Duration::from_secs(10)),
            read_write_timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        }),
        ..Default::default()
    }
}

impl LibrqbitBackend {
    pub async fn new(download_dir: PathBuf) -> Result<(Self, HashMap<String, LibrqbitHandle>)> {
        tokio::fs::create_dir_all(&download_dir).await?;
        debug!(path = ?download_dir, "Storing downloads");

        let session = Session::new_with_opts(download_dir.clone(), download_only_session_options()).await?;
        let weak = Arc::downgrade(&session);
        let scanner = Arc::new(crate::file_scan::FileScanner::default());
        let monitor_scanner = scanner.clone();
        let monitor_root = download_dir.clone();
        let completion_monitor = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(250));
            loop {
                interval.tick().await;
                let Some(session) = weak.upgrade() else { break };
                if session.cancellation_token().is_cancelled() { break; }
                let completed = session.with_torrents(|iter| iter
                    .filter(|(_, handle)| { let s = handle.stats(); s.finished && s.live.is_some() })
                    .map(|(_, handle)| handle.clone()).collect::<Vec<_>>());
                for handle in completed {
                    if let Err(error) = session.pause(&handle).await {
                        warn!(%error, "Could not pause completed download-only torrent");
                    }
                }
                let finished = session.with_torrents(|iter| iter.filter(|(_, h)| h.stats().finished).map(|(_, h)| h.clone()).collect::<Vec<_>>());
                for handle in finished {
                    if let Some(metadata) = handle.metadata.load_full() {
                        if let Ok(files) = metadata.info.iter_file_details() {
                            let root = monitor_root.join(handle.info_hash().as_string());
                            for file in files {
                                if let Ok(name) = file.filename.to_string() {
                                    if crate::media_policy::validate_source_path(&root, &handle.info_hash().as_string(), &name).is_ok() {
                                        let _ = monitor_scanner.ensure_clean(&root.join(name)).await;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
        Ok((Self { session, completion_monitor, download_dir, scanner, lifecycle_locks: Default::default() }, HashMap::new()))
    }

    fn wrap(&self, handle: Arc<ManagedTorrent>) -> LibrqbitHandle {
        let info_hash = handle.info_hash().as_string();
        let lifecycle = {
            let mut locks = self.lifecycle_locks.lock().unwrap_or_else(|e| e.into_inner());
            locks.retain(|_, lock| lock.strong_count() > 0);
            let lock = locks.get(&info_hash).and_then(Weak::upgrade).unwrap_or_else(|| Arc::new(tokio::sync::Mutex::new(())));
            locks.insert(info_hash.clone(), Arc::downgrade(&lock));
            lock
        };
        LibrqbitHandle { download_dir: self.download_dir.join(&info_hash), info_hash, handle, session: Arc::downgrade(&self.session), scanner: self.scanner.clone(), lifecycle }
    }

    pub async fn stop(&self) {
        self.completion_monitor.abort();
        self.session.stop().await;
    }
}

impl Drop for LibrqbitBackend {
    fn drop(&mut self) {
        self.completion_monitor.abort();
        self.session.cancellation_token().cancel();
    }
}

pub struct LibrqbitHandle {
    pub handle: Arc<ManagedTorrent>,
    pub info_hash: String,
    session: Weak<Session>,
    download_dir: PathBuf,
    scanner: Arc<crate::file_scan::FileScanner>,
    lifecycle: Arc<tokio::sync::Mutex<()>>,
}

#[async_trait::async_trait]
impl TorrentBackend for LibrqbitBackend {
    type Handle = LibrqbitHandle;

    async fn stop_session(&self) {
        LibrqbitBackend::stop(self).await;
    }

    async fn add_torrent(
        &self,
        source: TorrentSource,
        trackers: Vec<String>,
    ) -> Result<Self::Handle> {
        crate::torrent_admission::check(&source).await?;
        // librqbit reads trackers from a magnet link and ignores AddTorrentOptions::trackers
        // for that input. A bare info-hash magnet then has no DHT and no tracker, so metadata
        // inspection fails immediately and playback never starts.
        let add_torrent = match source {
            TorrentSource::Url(url) => {
                librqbit::AddTorrent::Url(magnet_with_trackers(url, &trackers).into())
            }
            TorrentSource::Bytes(bytes) => {
                librqbit::AddTorrent::from_bytes(bytes::Bytes::from(bytes))
            }
        };
        // Resolve metadata without creating payload files or starting the download.
        let preview = self.session.add_torrent(add_torrent, Some(librqbit::AddTorrentOptions {
            list_only: true, trackers: Some(trackers.clone()), ..Default::default()
        })).await.map_err(|error| anyhow::anyhow!("could not inspect torrent metadata: {error:#}"))?;
        let preview = match preview {
            librqbit::AddTorrentResponse::ListOnly(preview) => preview,
            librqbit::AddTorrentResponse::AlreadyManaged(_, handle) => {
                let handle = self.wrap(handle);
                let names = handle.get_files().await.into_iter().map(|f| f.name).collect::<Vec<_>>();
                crate::media_policy::select_source_files(&handle.download_dir, &handle.info_hash, &names)?;
                return Ok(handle);
            }
            _ => anyhow::bail!("metadata inspection unexpectedly started a download"),
        };
        let folder = preview.info_hash.as_string();
        let root = self.download_dir.join(&folder);
        let names = preview.info.iter_file_details()?.map(|file| {
            file.filename.to_string().context("torrent filename is not valid UTF-8")
        }).collect::<Result<Vec<_>>>()?;
        let selected = crate::media_policy::select_source_files(&root, &folder, &names)?;
        let response = self
            .session
            .add_torrent(
                librqbit::AddTorrent::from_bytes(preview.torrent_bytes),
                Some(librqbit::AddTorrentOptions {
                    // Reuse only the private, metadata-validated cache; rqbit verifies pieces.
                    overwrite: true,
                    sub_folder: Some(folder),
                    only_files: Some(selected),
                    initial_peers: Some(preview.seen_peers),
                    trackers: Some(trackers),
                    ..Default::default()
                }),
            )
            .await
            .context("Failed to add torrent to librqbit")?;

        let (_id, handle) = match response {
            librqbit::AddTorrentResponse::Added(id, handle)
            | librqbit::AddTorrentResponse::AlreadyManaged(id, handle) => (id, handle),
            _ => return Err(anyhow::anyhow!("Unexpected response from librqbit")),
        };

        Ok(self.wrap(handle))
    }

    async fn get_torrent(&self, info_hash: &str) -> Option<Self::Handle> {
        self.session.with_torrents(|iter| {
            for (_, handle) in iter {
                if handle.info_hash().as_string().eq_ignore_ascii_case(info_hash) {
                    return Some(self.wrap(handle.clone()));
                }
            }
            None
        })
    }

    async fn remove_torrent(&self, info_hash: &str) -> Result<()> {
        let id = self.session.with_torrents(|iter| {
            for (id, handle) in iter {
                if handle.info_hash().as_string().eq_ignore_ascii_case(info_hash) { return Some(id); }
            }
            None
        });
        if let Some(id) = id { self.session.delete(librqbit::api::TorrentIdOrHash::Id(id), false).await?; }
        Ok(())
    }

    async fn list_torrents(&self) -> Vec<String> {
        self.session.with_torrents(|iter| iter.map(|(_, handle)| handle.info_hash().as_string()).collect())
    }

    async fn memory_diagnostics(&self) -> BackendMemoryDiagnostics {
        BackendMemoryDiagnostics::default()
    }
}

#[async_trait::async_trait]
impl TorrentHandle for LibrqbitHandle {
    async fn is_finished(&self) -> bool { self.handle.stats().finished }

    async fn pause_torrent(&self) -> Result<()> {
        let _guard = self.lifecycle.lock().await;
        if let Some(session) = self.session.upgrade() { session.pause(&self.handle).await?; }
        Ok(())
    }

    async fn resume_torrent(&self) -> Result<()> {
        let _guard = self.lifecycle.lock().await;
        // Calling unpause during initialization starts another checksum task in rqbit.
        // Wait for the original task, then resume only an actually paused torrent.
        tokio::time::timeout(Duration::from_secs(120), self.handle.wait_until_initialized())
            .await.context("torrent initialization timed out")??;
        let paused = self.handle.with_state(|state| matches!(state, librqbit::ManagedTorrentState::Paused(_)));
        if paused && !self.handle.stats().finished {
            let session = self.session.upgrade().context("torrent session has stopped")?;
            session.unpause(&self.handle).await?;
        }
        Ok(())
    }
    fn info_hash(&self) -> String {
        self.handle.info_hash().as_string()
    }

    fn name(&self) -> Option<String> {
        self.handle
            .metadata
            .load_full()
            .and_then(|m| m.info.name.as_ref().map(|n| n.to_string()))
    }

    async fn stats(&self) -> EngineStats {
        let stats = self.handle.stats();
        let (download_speed, upload_speed) = if let Some(ref live) = stats.live {
            (
                live.download_speed.mbps * 1_048_576.0 / 8.0,
                live.upload_speed.mbps * 1_048_576.0 / 8.0,
            )
        } else {
            (0.0, 0.0)
        };

        let (downloaded, uploaded) = (stats.progress_bytes, stats.uploaded_bytes);

        let peers = stats
            .live
            .as_ref()
            .map(|l| l.snapshot.peer_stats.live as u64)
            .unwrap_or(0);

        let mut files = Vec::new();
        let mut total_size = 0u64;
        let mut offset = 0u64;
        if let Some(m) = self.handle.metadata.load_full() {
            if let Ok(iter) = m.info.iter_file_details() {
                for (index, f) in iter.enumerate() {
                    let filename = f.filename.to_string().unwrap_or_default();
                    let progress = stats.file_progress.get(index).copied().unwrap_or(0);
                    files.push(StatsFile {
                        name: filename.clone(),
                        path: filename,
                        length: f.len,
                        offset,
                        downloaded: progress,
                        progress: if f.len > 0 { progress as f64 / f.len as f64 } else { 1.0 },
                    });
                    total_size += f.len;
                    offset += f.len;
                }
            }
        }

        EngineStats {
            name: self.name().unwrap_or_else(|| "Unknown".to_string()),
            info_hash: self.info_hash(),
            files,
            sources: vec![],
            opts: StatsOptions {
                dht: false,
                tracker: true,
                path: "".to_string(),
                growler: Growler {
                    flood: 0,
                    pulse: None,
                },
                peer_search: PeerSearch {
                    max: 100,
                    min: 10,
                    sources: vec![],
                },
                swarm_cap: SwarmCap {
                    max_speed: None,
                    min_peers: None,
                },
                connections: None,
                handshake_timeout: None,
                timeout: None,
                r#virtual: false,
            },
            download_speed,
            upload_speed,
            downloaded,
            uploaded,
            peers,
            unchoked: peers,
            queued: 0,
            unique: peers,
            connection_tries: 0,
            peer_search_running: true,
            stream_len: total_size,
            stream_name: "".to_string(),
            stream_progress: if total_size > 0 {
                downloaded as f64 / total_size as f64
            } else {
                0.0
            },
            swarm_connections: peers,
            swarm_paused: stats.live.is_none(),
            swarm_size: peers,
            is_finished: total_size > 0 && downloaded >= total_size,
            has_metadata: total_size > 0,
        }
    }

    async fn add_trackers(&self, _trackers: Vec<String>) -> Result<()> {
        Ok(())
    }

    async fn get_file_reader(
        &self,
        file_idx: usize,
        start_offset: u64,
        _priority: u8,
        _bitrate: Option<u64>,
        intent: crate::backend::priorities::PlaybackIntent,
    ) -> Result<Box<dyn FileStreamTrait>> {
        use tokio::io::AsyncSeekExt;
        self.resume_torrent().await?;
        let files = self.get_files().await;
        let file = files.get(file_idx).context("torrent file index is out of range")?;
        crate::media_policy::validate_source_path(&self.download_dir, &self.info_hash, &file.name)?;
        if !crate::backend::playback_assist_enabled() {
            // Default path: sniff on a throwaway stream, then open playback.
            crate::content_sniff::ensure_safe_video_prefix(
                &self.info_hash,
                file_idx,
                &file.name,
                self.read_video_prefix(file_idx, &file.name, file.length),
            )
            .await?;
            if self.is_file_complete(file_idx).await {
                if let Some(path) = self.get_file_path(file_idx).await {
                    let mut file = tokio::fs::File::open(path).await?;
                    file.seek(std::io::SeekFrom::Start(start_offset)).await?;
                    return Ok(Box::new(file));
                }
                anyhow::bail!("Completed media is blocked or unavailable: check Microsoft Defender scan results");
            }
            let mut stream = self
                .handle
                .clone()
                .stream(file_idx)
                .context("Failed to stream from librqbit")?;
            stream.seek(std::io::SeekFrom::Start(start_offset)).await?;
            return Ok(Box::new(stream));
        }

        if self.is_file_complete(file_idx).await {
            crate::content_sniff::ensure_safe_video_prefix(
                &self.info_hash,
                file_idx,
                &file.name,
                self.read_video_prefix(file_idx, &file.name, file.length),
            )
            .await?;
            if let Some(path) = self.get_file_path(file_idx).await {
                let mut file = tokio::fs::File::open(path).await?;
                file.seek(std::io::SeekFrom::Start(start_offset)).await?;
                return Ok(Box::new(file));
            }
            anyhow::bail!("Completed media is blocked or unavailable: check Microsoft Defender scan results");
        }

        let mut playback = self
            .handle
            .clone()
            .stream(file_idx)
            .context("Failed to stream from librqbit")?;
        crate::content_sniff::ensure_safe_video_prefix(
            &self.info_hash,
            file_idx,
            &file.name,
            async {
                playback.seek(std::io::SeekFrom::Start(0)).await?;
                read_prefix_from_reader(&mut playback, file.length).await
            },
        )
        .await?;
        playback
            .seek(std::io::SeekFrom::Start(start_offset))
            .await?;

        let metadata_pin = match metadata_pin_offset(true, intent, start_offset, file.length) {
            Some(offset) => match self.handle.clone().stream(file_idx) {
                Ok(mut pin) => match pin.seek(std::io::SeekFrom::Start(offset)).await {
                    Ok(_) => {
                        debug!(
                            info_hash = %self.info_hash,
                            file_idx,
                            offset,
                            "playback assist pinned container metadata"
                        );
                        Some(pin)
                    }
                    Err(error) => {
                        debug!(%error, "playback assist metadata pin seek failed");
                        None
                    }
                },
                Err(error) => {
                    debug!(%error, "playback assist metadata pin stream failed");
                    None
                }
            },
            None => None,
        };

        Ok(Box::new(AssistedPlaybackStream {
            playback,
            _metadata_pin: metadata_pin,
        }))
    }

    async fn get_files(&self) -> Vec<BackendFileInfo> {
        let mut files = Vec::new();
        if let Some(m) = self.handle.metadata.load_full() {
            if let Ok(iter) = m.info.iter_file_details() {
                for f in iter {
                    files.push(BackendFileInfo {
                        name: f.filename.to_string().unwrap_or_default(),
                        length: f.len,
                    });
                }
            }
        }
        files
    }

    async fn get_file_path(&self, file_idx: usize) -> Option<String> {
        if !self.is_file_complete(file_idx).await { return None; }
        let files = self.get_files().await;
        let file = files.get(file_idx)?;
        crate::media_policy::validate_source_path(&self.download_dir, &self.info_hash, &file.name).ok()?;
        let path = self.download_dir.join(&file.name);
        self.scanner.ensure_clean(&path).await.ok()?;
        Some(path.to_string_lossy().into_owned())
    }

    async fn is_file_complete(&self, file_idx: usize) -> bool {
        let stats = self.handle.stats();
        let files = self.get_files().await;
        files.get(file_idx).is_some_and(|f| stats.file_progress.get(file_idx).copied().unwrap_or(0) >= f.length)
    }

    async fn prepare_file_for_streaming(&self, _file_idx: usize) -> Result<()> {
        self.resume_torrent().await
    }

    async fn keep_file_downloading(&self, _file_idx: usize) -> Result<()> {
        Ok(())
    }

    async fn clear_file_streaming(&self, _file_idx: usize) -> Result<()> {
        Ok(())
    }

    async fn wait_for_piece_ready(
        &self,
        _file_idx: usize,
        _offset: u64,
        _timeout: Duration,
        _intent: crate::backend::priorities::PlaybackIntent,
    ) -> Result<PieceReadiness> {
        Ok(PieceReadiness {
            ready: true,
            piece: -1,
            ready_pieces: 1,
            target_pieces: 1,
            elapsed_ms: 0,
            peers: 0,
            download_rate: 0,
            reason: "librqbit-reader".to_string(),
        })
    }
}

/// Extra FileStream held so librqbit interleaves Cues/moov with the playback head.
struct AssistedPlaybackStream<S> {
    playback: S,
    _metadata_pin: Option<S>,
}

impl<S: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for AssistedPlaybackStream<S> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        std::pin::Pin::new(&mut this.playback).poll_read(cx, buf)
    }
}

impl<S: tokio::io::AsyncSeek + Unpin> tokio::io::AsyncSeek for AssistedPlaybackStream<S> {
    fn start_seek(
        self: std::pin::Pin<&mut Self>,
        position: std::io::SeekFrom,
    ) -> std::io::Result<()> {
        std::pin::Pin::new(&mut self.get_mut().playback).start_seek(position)
    }

    fn poll_complete(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<u64>> {
        std::pin::Pin::new(&mut self.get_mut().playback).poll_complete(cx)
    }
}

/// When assist is on, keep a second librqbit stream at the container index
/// (MKV Cues / MP4 moov). Off, probes, and full-file downloads stay sequential.
pub(crate) fn metadata_pin_offset(
    assist_enabled: bool,
    intent: crate::backend::priorities::PlaybackIntent,
    start_offset: u64,
    file_len: u64,
) -> Option<u64> {
    use crate::backend::priorities::PlaybackIntent::*;
    if !assist_enabled {
        return None;
    }
    match intent {
        InternalProbe | Background | DownloadFull => return None,
        DirectInitial | DirectSeek | DirectSequential | HlsInitial | HlsSeek | HlsSequential
        | DownloadRange | ContainerMetadata => {}
    }
    let pin_at = crate::backend::priorities::container_metadata_start(file_len);
    if pin_at == 0 || start_offset >= pin_at {
        None
    } else {
        Some(pin_at)
    }
}

async fn read_prefix_from_reader<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    length: u64,
) -> Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let want = usize::try_from(length.min(256)).unwrap_or(256);
    if want == 0 {
        return Ok(Vec::new());
    }
    let mut buf = vec![0u8; want];
    let mut filled = 0;
    while filled < want {
        let n = reader.read(&mut buf[filled..]).await?;
        if n == 0 {
            break;
        }
        filled += n;
        if filled >= 64 {
            break;
        }
    }
    buf.truncate(filled);
    Ok(buf)
}

/// Puts tracker URLs into a magnet link. Other sources are returned unchanged.
fn magnet_with_trackers(url: String, trackers: &[String]) -> String {
    if trackers.is_empty() || !url.starts_with("magnet:") {
        return url;
    }
    let mut magnet = url;
    for tracker in trackers {
        let tracker = tracker.trim();
        if tracker.is_empty() {
            continue;
        }
        magnet.push_str("&tr=");
        magnet.push_str(&urlencoding::encode(tracker));
    }
    magnet
}

impl LibrqbitHandle {
    async fn read_video_prefix(&self, file_idx: usize, name: &str, length: u64) -> Result<Vec<u8>> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let want = usize::try_from(length.min(256)).unwrap_or(256);
        if want == 0 {
            return Ok(Vec::new());
        }
        if self.is_file_complete(file_idx).await
            && let Ok(mut file) = tokio::fs::File::open(self.download_dir.join(name)).await
        {
            let mut buf = vec![0u8; want];
            if let Ok(n) = file.read(&mut buf).await {
                buf.truncate(n);
                if n >= 64 || u64::try_from(n).unwrap_or(0) >= length {
                    return Ok(buf);
                }
            }
        }
        let mut stream = self
            .handle
            .clone()
            .stream(file_idx)
            .context("Failed to stream from librqbit")?;
        stream.seek(std::io::SeekFrom::Start(0)).await?;
        let mut buf = vec![0u8; want];
        let mut filled = 0;
        while filled < want {
            let n = stream.read(&mut buf[filled..]).await?;
            if n == 0 {
                break;
            }
            filled += n;
            if filled >= 64 {
                break;
            }
        }
        buf.truncate(filled);
        Ok(buf)
    }
}

impl Clone for LibrqbitHandle {
    fn clone(&self) -> Self {
        Self {
            handle: self.handle.clone(),
            info_hash: self.info_hash.clone(),
            session: self.session.clone(),
            download_dir: self.download_dir.clone(),
            scanner: self.scanner.clone(),
            lifecycle: self.lifecycle.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{magnet_with_trackers, metadata_pin_offset};
    use crate::backend::priorities::PlaybackIntent;

    #[test]
    fn magnet_keeps_supplied_trackers() {
        let magnet = magnet_with_trackers(
            "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".to_owned(),
            &[
                "udp://tracker.example:1337/announce".to_owned(),
                "  ".to_owned(),
            ],
        );
        assert!(magnet.contains("&tr=udp%3A%2F%2Ftracker.example%3A1337%2Fannounce"));
        assert_eq!(magnet.matches("&tr=").count(), 1);
    }

    #[test]
    fn http_source_is_left_unchanged() {
        let url = "https://example.test/file.torrent".to_owned();
        assert_eq!(
            magnet_with_trackers(url.clone(), &["udp://tracker.example:1".to_owned()]),
            url
        );
    }

    #[test]
    fn metadata_pin_stays_off_when_assist_disabled() {
        let file_len = 2 * 1024 * 1024 * 1024;
        assert_eq!(
            metadata_pin_offset(false, PlaybackIntent::DirectInitial, 0, file_len),
            None
        );
        assert!(!crate::backend::playback_assist_enabled());
    }

    #[test]
    fn metadata_pin_targets_cues_window_for_playback() {
        let file_len = 2 * 1024 * 1024 * 1024;
        let expected = crate::backend::priorities::container_metadata_start(file_len);
        assert_eq!(
            metadata_pin_offset(true, PlaybackIntent::DirectInitial, 0, file_len),
            Some(expected)
        );
        assert_eq!(
            metadata_pin_offset(true, PlaybackIntent::InternalProbe, 0, file_len),
            None
        );
        assert_eq!(
            metadata_pin_offset(true, PlaybackIntent::DownloadFull, 0, file_len),
            None
        );
        assert_eq!(
            metadata_pin_offset(true, PlaybackIntent::DirectSeek, expected, file_len),
            None
        );
    }
}
