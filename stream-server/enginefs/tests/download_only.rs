#![cfg(feature = "librqbit")]

//! A controlled second BT client, using only deterministic bytes owned by this test.
//! Counters describe plaintext BT wire bytes, not IP/TCP overhead or production traffic.
use anyhow::{Context, Result, bail};
use enginefs::backend::librqbit::download_only_session_options;
use librqbit::{AddTorrent, AddTorrentOptions, Session};
use sha1::{Digest, Sha1};
use std::time::Duration;
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpListener, TcpStream}, time::{Instant, timeout}};

const PIECE: usize = 16_384;
const COUNT: usize = 4;

fn fixture() -> (Vec<u8>, Vec<u8>, [u8; 20]) {
    fixture_named("fixture.mkv")
}

fn fixture_named(name: &str) -> (Vec<u8>, Vec<u8>, [u8; 20]) {
    let data: Vec<u8> = (0..PIECE * COUNT).map(|n| ((n / PIECE + n % 251) % 256) as u8).collect();
    let hashes: Vec<u8> = data.chunks(PIECE).flat_map(|p| Sha1::digest(p).to_vec()).collect();
    let mut info = format!("d6:lengthi{}e4:name{}:{}12:piece lengthi{}e6:pieces{}:", data.len(), name.len(), name, PIECE, hashes.len()).into_bytes();
    info.extend(hashes);
    info.push(b'e');
    let info_hash: [u8; 20] = Sha1::digest(&info).into();
    let mut torrent = b"d4:info".to_vec();
    torrent.extend(info);
    torrent.push(b'e');
    (torrent, data, info_hash)
}

async fn send(stream: &mut TcpStream, id: u8, payload: &[u8]) -> Result<()> {
    stream.write_u32((payload.len() + 1) as u32).await?;
    stream.write_u8(id).await?;
    stream.write_all(payload).await?;
    Ok(())
}

#[derive(Default, Debug, serde::Serialize)]
struct WireCounters {
    client_piece_payload_bytes: u64,
    client_other_bt_wire_bytes: u64,
    client_downloaded_payload_bytes: u64,
}

async fn receive(stream: &mut TcpStream, counters: &mut WireCounters) -> Result<Option<Vec<u8>>> {
    let len = match stream.read_u32().await {
        Ok(len) => len as usize,
        Err(e) if matches!(e.kind(), std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    anyhow::ensure!(len <= PIECE + 1024, "unexpected test peer frame size");
    let mut message = vec![0; len];
    counters.client_other_bt_wire_bytes += 4;
    let mut received = 0;
    while received < len {
        let n = match stream.read(&mut message[received..]).await {
            Ok(0) => return Ok(None),
            Ok(n) => n,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let payload = if message[0] == 7 {
            (received + n).saturating_sub(9) - received.saturating_sub(9)
        } else { 0 };
        counters.client_piece_payload_bytes += payload as u64;
        counters.client_other_bt_wire_bytes += (n - payload) as u64;
        received += n;
    }
    Ok(Some(message))
}

async fn probe(disable_upload: bool, complete_download: bool) -> Result<WireCounters> {
    let temp = tempfile::tempdir()?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (torrent, data, info_hash) = fixture();
    let mut options = download_only_session_options();
    // Positive control belongs only to the controlled test client.
    options.disable_upload = disable_upload;
    let session = Session::new_with_opts(temp.path().to_path_buf(), options).await?;
    let handle = session.add_torrent(AddTorrent::from_bytes(torrent), Some(AddTorrentOptions {
        initial_peers: Some(vec![address]), disable_trackers: true,
        ..Default::default()
    })).await?.into_handle().context("torrent handle")?;

    let test = async {
        let (mut stream, peer_address) = listener.accept().await?;
        anyhow::ensure!(peer_address.ip().is_loopback(), "test escaped loopback");
        let mut handshake = [0; 68];
        stream.read_exact(&mut handshake).await?;
        anyhow::ensure!(&handshake[28..48] == info_hash.as_slice(), "wrong info hash");
        handshake[20..28].fill(0); // no extension protocol or encryption in this wire test
        handshake[48..68].copy_from_slice(b"-SSAUDT-000000000000");
        stream.write_all(&handshake).await?;
        send(&mut stream, 5, &[0xf0]).await?; // all four pieces available
        send(&mut stream, 1, &[]).await?; // unchoke our downloader
        let mut counters = WireCounters { client_other_bt_wire_bytes: 68, ..Default::default() };
        // Keep a stream lease so completed-download coverage can challenge a live connection.
        let _lease = handle.clone().stream(0)?;
        let target = if complete_download { COUNT } else { 1 };
        let mut served = std::collections::HashSet::new();
        let mut owned_piece = None;
        while served.len() < target {
            let msg = receive(&mut stream, &mut counters).await?.context("client closed while downloading")?;
            if msg.first() == Some(&6) {
                anyhow::ensure!(msg.len() == 13, "bad request");
                let index = u32::from_be_bytes(msg[1..5].try_into()?) as usize;
                let offset = u32::from_be_bytes(msg[5..9].try_into()?) as usize;
                let length = u32::from_be_bytes(msg[9..13].try_into()?) as usize;
                anyhow::ensure!(index < COUNT && offset + length <= PIECE, "request outside fixture");
                let mut piece = msg[1..9].to_vec();
                piece.extend_from_slice(&data[index * PIECE + offset .. index * PIECE + offset + length]);
                send(&mut stream, 7, &piece).await?;
                served.insert(index);
                owned_piece = Some(index as u32);
            }
        }
        let expected = (target * PIECE) as u64;
        let deadline = Instant::now() + Duration::from_secs(5);
        while handle.stats().progress_bytes < expected {
            anyhow::ensure!(Instant::now() < deadline, "download did not verify");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        counters.client_downloaded_payload_bytes = handle.stats().progress_bytes;
        anyhow::ensure!(handle.stats().finished == complete_download, "wrong completion state");
        let mut request = owned_piece.context("no owned piece")?.to_be_bytes().to_vec();
        request.extend_from_slice(&0u32.to_be_bytes());
        request.extend_from_slice(&(PIECE as u32).to_be_bytes());
        let challenge = async {
            send(&mut stream, 2, &[]).await?;
            send(&mut stream, 6, &request).await
        }.await;
        if let Err(error) = challenge {
            let disconnected = error.downcast_ref::<std::io::Error>().is_some_and(|e|
                matches!(e.kind(), std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe));
            // A secure completed torrent may have already closed the swarm connection.
            // Still drain/read below, so received PIECE bytes cannot be hidden by an EOF.
            anyhow::ensure!(disable_upload && complete_download && disconnected, "peer challenge failed: {error:#}");
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match timeout(remaining, receive(&mut stream, &mut counters)).await {
                Ok(Ok(Some(_))) if counters.client_piece_payload_bytes == 0 => continue,
                Ok(Ok(_)) => break,
                Ok(Err(e)) => return Err(e),
                Err(_) => bail!("peer request did not reach an observable result"),
            }
        }
        Ok::<_, anyhow::Error>(counters)
    };
    let result = timeout(Duration::from_secs(20), test).await;
    session.stop().await;
    result.context("peer test timed out")?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peer_downloads_but_cannot_request_pieces_during_download() -> Result<()> {
    let counts = probe(true, false).await?;
    println!("during_download={}", serde_json::to_string(&counts)?);
    assert!(counts.client_downloaded_payload_bytes > 0);
    assert!(counts.client_other_bt_wire_bytes >= 68);
    assert_eq!(counts.client_piece_payload_bytes, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_download_still_refuses_peer_requests() -> Result<()> {
    let counts = probe(true, true).await?;
    println!("after_completion={}", serde_json::to_string(&counts)?);
    assert_eq!(counts.client_downloaded_payload_bytes, (COUNT * PIECE) as u64);
    assert_eq!(counts.client_piece_payload_bytes, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_peer_really_can_receive_pieces_when_blocking_is_off() -> Result<()> {
    let counts = probe(false, false).await?;
    println!("positive_control={}", serde_json::to_string(&counts)?);
    assert!(counts.client_piece_payload_bytes > 0, "wire test must detect a real upload");
    Ok(())
}

#[tokio::test]
async fn unsafe_metadata_is_rejected_without_creating_payload_files() -> Result<()> {
    use enginefs::backend::{TorrentBackend, TorrentSource, librqbit::LibrqbitBackend};
    let temp = tempfile::tempdir()?;
    let (backend, restored) = LibrqbitBackend::new(temp.path().to_path_buf()).await?;
    assert!(restored.is_empty());
    for name in ["evil.exe", "film.cmd.mkv", "archive.zip", "../escape.mkv"] {
        let (torrent, _, _) = fixture_named(name);
        assert!(backend.add_torrent(TorrentSource::Bytes(torrent), vec![]).await.is_err(), "accepted {name}");
    }
    assert!(backend.list_torrents().await.is_empty());
    backend.stop().await;
    assert_eq!(std::fs::read_dir(temp.path())?.count(), 0, "payload was created before policy validation");
    Ok(())
}

#[tokio::test]
async fn completed_cache_is_paused_and_remains_seekable() -> Result<()> {
    use enginefs::backend::{TorrentBackend, TorrentHandle, TorrentSource, librqbit::LibrqbitBackend, priorities::PlaybackIntent};
    let temp = tempfile::tempdir()?;
    let (torrent, data, hash) = fixture();
    let hash = hex::encode(hash);
    let folder = temp.path().join(&hash);
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("fixture.mkv"), &data)?;
    let (backend, _) = LibrqbitBackend::new(temp.path().to_path_buf()).await?;
    let handle = backend.add_torrent(TorrentSource::Bytes(torrent), vec![]).await?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !handle.is_finished().await || handle.stats().await.downloaded < data.len() as u64 {
        anyhow::ensure!(Instant::now() < deadline, "cached pieces were not verified");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert!(handle.handle.stats().live.is_none(), "completed torrent still has a live swarm");
    let mut reader = handle.get_file_reader(0, 2000, 1, None, PlaybackIntent::DirectSeek).await?;
    let mut sample = [0; 100];
    reader.read_exact(&mut sample).await?;
    assert_eq!(&sample, &data[2000..2100]);
    assert!(backend.get_torrent(&hash).await.is_some());
    backend.remove_torrent(&hash).await?;
    assert!(backend.list_torrents().await.is_empty());
    backend.stop().await;
    Ok(())
}

#[tokio::test]
async fn legacy_proxy_cannot_silently_fall_back_to_direct_connections() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut config = enginefs::backend::BackendConfig::default();
    config.privacy.bt_proxy_type = enginefs::backend::TorrentProxyType::Socks5;
    assert!(enginefs::EngineFS::new(temp.path().to_owned(), config).await.is_err());
    let mut config = enginefs::backend::BackendConfig::default();
    config.privacy.bt_outgoing_interfaces = "test-vpn".to_owned();
    assert!(enginefs::EngineFS::new(temp.path().to_owned(), config).await.is_err());
    assert_eq!(std::fs::read_dir(temp.path())?.count(), 0);
    Ok(())
}
