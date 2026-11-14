//! The embedding VPN policy must run before even magnet metadata is requested.
use enginefs::backend::{TorrentBackend, TorrentSource, librqbit::LibrqbitBackend};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
async fn denied_admission_never_creates_torrents_or_payload_files() -> anyhow::Result<()> {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    enginefs::torrent_admission::install(Arc::new(move |_| {
        observed.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { anyhow::bail!("VPN_REQUIRED_TEST") })
    }))?;
    let directory = tempfile::tempdir()?;
    let (backend, restored) = LibrqbitBackend::new(directory.path().to_owned()).await?;
    assert!(restored.is_empty());
    for source in [
        TorrentSource::Url("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".into()),
        TorrentSource::Url("https://example.invalid/test.torrent".into()),
        TorrentSource::Bytes(vec![0, 1, 2]),
    ] {
        let error = match backend.add_torrent(source, vec![]).await {
            Ok(_) => anyhow::bail!("denied torrent was accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("VPN_REQUIRED_TEST"));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert!(backend.list_torrents().await.is_empty());
    backend.stop().await;
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 0);
    Ok(())
}
