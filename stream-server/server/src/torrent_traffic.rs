use enginefs::backend::EngineStats;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

pub const TRAFFIC_FILE: &str = "torrent-traffic.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentTrafficReport {
    pub upload_disabled: bool,
    pub incoming_listen: bool,
    pub dht_disabled: bool,
    pub seeding_policy_enabled: bool,
    pub piece_payload_uploaded_bytes: u64,
    pub session_piece_payload_uploaded_bytes: u64,
    pub upload_speed_bytes_per_sec: f64,
    pub peak_upload_speed_bytes_per_sec: f64,
    pub active_torrents: usize,
    pub last_sample_unix: i64,
    pub note: String,
}

impl Default for TorrentTrafficReport {
    fn default() -> Self {
        Self {
            upload_disabled: true,
            incoming_listen: false,
            dht_disabled: true,
            seeding_policy_enabled: false,
            piece_payload_uploaded_bytes: 0,
            session_piece_payload_uploaded_bytes: 0,
            upload_speed_bytes_per_sec: 0.0,
            peak_upload_speed_bytes_per_sec: 0.0,
            active_torrents: 0,
            last_sample_unix: 0,
            note: String::new(),
        }
    }
}

struct Inner {
    per_hash: HashMap<String, u64>,
    lifetime_uploaded: u64,
    peak_upload_speed: f64,
}

pub struct TorrentTrafficLedger {
    path: PathBuf,
    inner: Mutex<Inner>,
}

impl TorrentTrafficLedger {
    pub async fn load(config_dir: &Path) -> Arc<Self> {
        let path = config_dir.join(TRAFFIC_FILE);
        let persisted = tokio::fs::read_to_string(&path)
            .await
            .ok()
            .and_then(|contents| serde_json::from_str::<TorrentTrafficReport>(&contents).ok());
        Arc::new(Self {
            path,
            inner: Mutex::new(Inner {
                per_hash: HashMap::new(),
                lifetime_uploaded: persisted
                    .as_ref()
                    .map(|report| report.piece_payload_uploaded_bytes)
                    .unwrap_or(0),
                peak_upload_speed: persisted
                    .as_ref()
                    .map(|report| report.peak_upload_speed_bytes_per_sec)
                    .unwrap_or(0.0),
            }),
        })
    }

    pub async fn sample(
        &self,
        stats: &HashMap<String, EngineStats>,
        seeding_policy_enabled: bool,
    ) -> TorrentTrafficReport {
        let mut inner = self.inner.lock().await;
        let samples: Vec<(String, u64, f64)> = stats
            .iter()
            .map(|(hash, stat)| (hash.clone(), stat.uploaded, stat.upload_speed))
            .collect();
        let report = apply_sample(&mut inner, &samples, seeding_policy_enabled);
        drop(inner);
        if let Ok(json) = serde_json::to_string_pretty(&report) {
            let temporary = self.path.with_extension("json.tmp");
            if tokio::fs::write(&temporary, json.as_bytes()).await.is_ok() {
                let _ = tokio::fs::rename(&temporary, &self.path).await;
            }
        }
        report
    }
}

fn apply_sample(
    inner: &mut Inner,
    samples: &[(String, u64, f64)],
    seeding_policy_enabled: bool,
) -> TorrentTrafficReport {
    let current: HashMap<&str, (u64, f64)> = samples
        .iter()
        .map(|(hash, uploaded, speed)| (hash.as_str(), (*uploaded, *speed)))
        .collect();
    inner
        .per_hash
        .retain(|hash, _| current.contains_key(hash.as_str()));

    let mut session_uploaded = 0u64;
    let mut upload_speed = 0.0;
    for (hash, uploaded, speed) in samples {
        session_uploaded = session_uploaded.saturating_add(*uploaded);
        upload_speed += *speed;
        let last = inner.per_hash.get(hash).copied().unwrap_or(0);
        let delta = if *uploaded >= last {
            uploaded.saturating_sub(last)
        } else {
            *uploaded
        };
        inner.lifetime_uploaded = inner.lifetime_uploaded.saturating_add(delta);
        inner.per_hash.insert(hash.clone(), *uploaded);
    }
    if upload_speed > inner.peak_upload_speed {
        inner.peak_upload_speed = upload_speed;
    }

    TorrentTrafficReport {
        upload_disabled: true,
        incoming_listen: false,
        dht_disabled: true,
        seeding_policy_enabled,
        piece_payload_uploaded_bytes: inner.lifetime_uploaded,
        session_piece_payload_uploaded_bytes: session_uploaded,
        upload_speed_bytes_per_sec: upload_speed,
        peak_upload_speed_bytes_per_sec: inner.peak_upload_speed,
        active_torrents: samples.len(),
        last_sample_unix: unix_now(),
        note: "Counted bytes are BitTorrent PIECE payload only. Handshake, have, request, and TCP/IP overhead are not included. unknown is not zero.".to_owned(),
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

pub async fn run_sampler(
    config_dir: PathBuf,
    engine: Arc<enginefs::EngineFS>,
    download_engine: Arc<enginefs::EngineFS>,
) {
    let ledger = TorrentTrafficLedger::load(&config_dir).await;
    loop {
        let mut stats = engine.get_all_statistics().await;
        for (hash, value) in download_engine.get_all_statistics().await {
            stats.insert(hash, value);
        }
        let seeding_policy_enabled = engine.seeding_enabled() || download_engine.seeding_enabled();
        let report = ledger.sample(&stats, seeding_policy_enabled).await;
        if report.piece_payload_uploaded_bytes > 0 || report.upload_speed_bytes_per_sec > 0.0 {
            tracing::warn!(
                uploaded = report.piece_payload_uploaded_bytes,
                upload_speed = report.upload_speed_bytes_per_sec,
                "torrent piece payload left this process"
            );
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifetime_counts_deltas_and_survives_handle_reset() {
        let mut inner = Inner {
            per_hash: HashMap::new(),
            lifetime_uploaded: 0,
            peak_upload_speed: 0.0,
        };
        let first = apply_sample(
            &mut inner,
            &[("abc".to_owned(), 100, 0.0)],
            false,
        );
        assert_eq!(first.piece_payload_uploaded_bytes, 100);
        let second = apply_sample(
            &mut inner,
            &[("abc".to_owned(), 150, 12.0)],
            false,
        );
        assert_eq!(second.piece_payload_uploaded_bytes, 150);
        assert_eq!(second.peak_upload_speed_bytes_per_sec, 12.0);
        let after_remove = apply_sample(&mut inner, &[], false);
        assert_eq!(after_remove.piece_payload_uploaded_bytes, 150);
        let restarted = apply_sample(
            &mut inner,
            &[("abc".to_owned(), 20, 0.0)],
            false,
        );
        assert_eq!(restarted.piece_payload_uploaded_bytes, 170);
    }
}
