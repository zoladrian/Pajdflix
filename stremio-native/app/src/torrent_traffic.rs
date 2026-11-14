use serde::Deserialize;

use crate::app_model::format_bytes;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
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
        }
    }
}

pub fn load_report() -> TorrentTrafficReport {
    let Some(paths) = crate::paths::try_get() else {
        return TorrentTrafficReport::default();
    };
    let path = paths.streaming_server().join("torrent-traffic.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default()
}

pub fn settings_status_label(report: &TorrentTrafficReport) -> String {
    let policy = if report.upload_disabled && !report.incoming_listen && !report.seeding_policy_enabled
    {
        "Wyłączone"
    } else {
        "Uwaga"
    };
    format!(
        "{policy} · {} wysłane",
        format_bytes(report.piece_payload_uploaded_bytes)
    )
}

pub fn player_seeding_status(
    report: &TorrentTrafficReport,
    has_torrent_stats: bool,
    uploaded: u64,
) -> String {
    if !has_torrent_stats {
        return "Brak swarmu".to_owned();
    }
    if report.upload_disabled && uploaded == 0 {
        "Wyłączone".to_owned()
    } else if uploaded == 0 {
        "0 B wysłane".to_owned()
    } else {
        format!("Wysłano {}", format_bytes(uploaded))
    }
}
