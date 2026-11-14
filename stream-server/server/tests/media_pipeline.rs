//! Optional native integration test using only our generated legal MP4 and loopback.
//! Requires the reviewed mpv DLL, C test runner and fixture; never contacts a public tracker.
#![cfg(all(windows, feature = "secure"))]
use anyhow::{Context, Result, ensure};
use enginefs::backend::{TorrentHandle, TorrentSource};
use sha1::{Digest, Sha1};
use std::{
    collections::HashMap,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::timeout,
};

const PIECE: usize = 16_384;
fn torrent(data: &[u8]) -> (Vec<u8>, [u8; 20]) {
    let hashes: Vec<_> = data
        .chunks(PIECE)
        .flat_map(|piece| Sha1::digest(piece).to_vec())
        .collect();
    let mut info = format!(
        "d6:lengthi{}e4:name11:fixture.mp412:piece lengthi{}e6:pieces{}:",
        data.len(),
        PIECE,
        hashes.len()
    )
    .into_bytes();
    info.extend(hashes);
    info.push(b'e');
    let hash: [u8; 20] = Sha1::digest(&info).into();
    let mut bytes = b"d4:info".to_vec();
    bytes.extend(info);
    bytes.push(b'e');
    (bytes, hash)
}
async fn write_message(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    id: u8,
    payload: &[u8],
) -> Result<()> {
    writer.write_u32((payload.len() + 1) as u32).await?;
    writer.write_u8(id).await?;
    writer.write_all(payload).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires generated legal fixture and reviewed native decoder; run via the integration script"]
async fn playback_seek_and_subtitles_before_torrent_completion() -> Result<()> {
    let fixture =
        std::path::PathBuf::from(std::env::var("SECURE_MEDIA_FIXTURE").context("fixture path")?);
    let decoder = std::env::var("SECURE_MPV_TEST_RUNNER").context("decoder test runner")?;
    let dll = std::env::var("SECURE_MPV_TEST_DLL").context("reviewed decoder DLL")?;
    let data = Arc::new(std::fs::read(&fixture)?);
    ensure!(
        data.len() > 1024 * 1024,
        "fixture must be large enough for the partial-download gate"
    );
    let (metadata, hash) = torrent(&data);
    let hash_hex = hex::encode(hash);
    let pieces = data.len().div_ceil(PIECE);
    let gate = Arc::new(AtomicU8::new(0));
    let uploaded = Arc::new(AtomicU64::new(0));
    let protocol = Arc::new(AtomicU64::new(0));
    let peer = TcpListener::bind("127.0.0.1:0").await?;
    let peer_address = peer.local_addr()?;
    let peer_gate = gate.clone();
    let peer_data = data.clone();
    let peer_uploaded = uploaded.clone();
    let peer_protocol = protocol.clone();
    let seeder = tokio::spawn(async move {
        let (mut socket, address) = peer.accept().await?;
        ensure!(address.ip().is_loopback(), "peer escaped loopback");
        let mut handshake = [0; 68];
        socket.read_exact(&mut handshake).await?;
        ensure!(handshake[28..48] == hash, "incorrect fixture hash");
        handshake[20..28].fill(0);
        handshake[48..68].copy_from_slice(b"-SSMEDIA-00000000000");
        socket.write_all(&handshake).await?;
        peer_protocol.fetch_add(68, Ordering::Relaxed);
        let (mut reader, mut writer) = socket.into_split();
        let mut bitfield = vec![0u8; pieces.div_ceil(8)];
        for index in 0..pieces {
            bitfield[index / 8] |= 1 << (7 - index % 8);
        }
        write_message(&mut writer, 5, &bitfield).await?;
        write_message(&mut writer, 1, &[]).await?;
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        let receiver = tokio::spawn(async move {
            loop {
                let length = match reader.read_u32().await {
                    Ok(length) => length as usize,
                    Err(_) => break,
                };
                if length > 1024 * 1024 {
                    break;
                }
                peer_protocol.fetch_add(4, Ordering::Relaxed);
                if length == 0 {
                    continue;
                }
                let mut message = vec![0; length];
                if reader.read_exact(&mut message).await.is_err() {
                    break;
                }
                let payload = if message[0] == 7 {
                    length.saturating_sub(9)
                } else {
                    0
                };
                peer_uploaded.fetch_add(payload as u64, Ordering::Relaxed);
                peer_protocol.fetch_add((length - payload) as u64, Ordering::Relaxed);
                if tx.send(message).await.is_err() {
                    break;
                }
            }
        });
        let mut pending = HashMap::new();
        let mut tick = tokio::time::interval(Duration::from_millis(20));
        loop {
            tokio::select! {
                message=rx.recv()=> {
                    let Some(message)=message else { break };
                    if message.first() == Some(&6) && message.len()==13 {
                        let index=u32::from_be_bytes(message[1..5].try_into()?) as usize;
                        let offset=u32::from_be_bytes(message[5..9].try_into()?) as usize;
                        let count=u32::from_be_bytes(message[9..13].try_into()?) as usize;
                        ensure!(index<pieces && offset+count<=PIECE && index*PIECE+offset+count<=peer_data.len(),"request outside fixture");
                        ensure!(pending.len()<512,"unexpected request flood");
                        pending.insert((index,offset),count);
                    }
                }
                _=tick.tick()=>{}
            }
            let stage = peer_gate.load(Ordering::SeqCst);
            let allowed = match stage {
                0 => 32,
                1 => pieces - 2,
                _ => pieces,
            };
            let ready: Vec<_> = pending
                .keys()
                .filter(|(index, _)| *index < allowed)
                .copied()
                .collect();
            for (index, offset) in ready {
                let count = pending.remove(&(index, offset)).unwrap();
                let mut message = (index as u32).to_be_bytes().to_vec();
                message.extend((offset as u32).to_be_bytes());
                message.extend_from_slice(
                    &peer_data[index * PIECE + offset..index * PIECE + offset + count],
                );
                if write_message(&mut writer, 7, &message).await.is_err() {
                    receiver.abort();
                    return Ok::<_, anyhow::Error>(());
                }
            }
        }
        receiver.abort();
        Ok::<_, anyhow::Error>(())
    });
    let tracker = TcpListener::bind("127.0.0.1:0").await?;
    let tracker_address = tracker.local_addr()?;
    let compact = [
        127,
        0,
        0,
        1,
        (peer_address.port() >> 8) as u8,
        peer_address.port() as u8,
    ];
    let tracker_app = axum::Router::new().route(
        "/announce",
        axum::routing::get(move || async move {
            // librqbit rejects a response that omits complete/incomplete and then
            // waits 60s. Keys stay in bencode order and the only peer is loopback.
            let mut response = b"d8:completei1e10:incompletei0e8:intervali30e5:peers6:".to_vec();
            response.extend(compact);
            response.push(b'e');
            response
        }),
    );
    let tracker_task = tokio::spawn(async move { axum::serve(tracker, tracker_app).await });
    let directory = tempfile::tempdir()?;
    let engine = Arc::new(
        enginefs::EngineFS::new(directory.path().join("cache"), Default::default()).await?,
    );
    let torrent_engine = engine
        .add_torrent_with_trackers(
            TorrentSource::Bytes(metadata),
            vec![format!("http://{tracker_address}/announce")],
        )
        .await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let mut state = stream_server::AppState::new(
        engine.clone(),
        Default::default(),
        directory.path().join("config"),
    );
    state.base_url = format!("http://{address}");
    state.http_addr = address;
    let signed = state.local_auth.authorize_media(
        &format!("{}/stream/{hash_hex}/0", state.base_url),
        &state.base_url,
    )?;
    let router = stream_server::build_router(state);
    let http = tokio::spawn(async move { axum::serve(listener, router).await });
    let mut child = tokio::process::Command::new(decoder)
        .args([
            dll,
            signed,
            fixture.with_extension("srt").to_string_lossy().into_owned(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    let mut lines = BufReader::new(child.stdout.take().context("decoder stdout")?).lines();
    let result = async {
        let started = timeout(Duration::from_secs(30), lines.next_line()).await??;
        if started.as_deref() != Some("DECODER_READY") {
            let stats = torrent_engine.handle.stats().await;
            let status = child.try_wait()?;
            anyhow::bail!(
                "decoder did not start (stdout={started:?}, exit={status:?}, downloaded={}, finished={})",
                stats.downloaded,
                stats.is_finished
            );
        }
        let first = torrent_engine.handle.stats().await;
        ensure!(
            first.downloaded > 0 && first.downloaded < data.len() as u64 && !first.is_finished,
            "playback was not before completion"
        );
        println!(
            "playback_started_bytes={} total={}",
            first.downloaded,
            data.len()
        );
        gate.store(1, Ordering::SeqCst);
        child
            .stdin
            .as_mut()
            .context("decoder stdin")?
            .write_all(b"seek\n")
            .await?;
        ensure!(
            timeout(Duration::from_secs(30), lines.next_line())
                .await??
                .as_deref()
                == Some("SEEK_AND_SUBTITLES_OK"),
            "seek/subtitles failed"
        );
        ensure!(
            !torrent_engine.handle.stats().await.is_finished,
            "seek test downloaded the complete torrent"
        );
        child.stdin.as_mut().unwrap().write_all(b"quit\n").await?;
        ensure!(
            timeout(Duration::from_secs(5), child.wait())
                .await??
                .success(),
            "native decoder failed"
        );
        ensure!(
            uploaded.load(Ordering::Relaxed) == 0,
            "PIECE payload was uploaded"
        );
        println!(
            "piece_payload_uploaded_bytes=0 other_bt_wire_bytes={}",
            protocol.load(Ordering::Relaxed)
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let _ = child.kill().await;
    engine.stop().await;
    seeder.abort();
    tracker_task.abort();
    http.abort();
    result
}
