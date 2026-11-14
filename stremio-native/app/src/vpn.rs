//! Selective torrent admission. OS kill switch remains the packet-level barrier;
//! polling below is defense in depth, not a replacement for a VPN firewall.
use slint::ComponentHandle;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use stremio_core::{models::player::Selected, types::resource::StreamSource};

const REQUIRED: &str = "Ten torrent nie ma potwierdzonego zagranicznego kraju produkcji. Wymagany jest VPN. Połącz PrivadoVPN, włącz Kill Switch i wybierz źródło ponownie.";
const IPV6_RISK: &str = "Nie można potwierdzić ochrony IPv6: aktywny adres IPv6 pozostaje na zwykłym interfejsie sieciowym. Chroniony torrent nie został uruchomiony. Sama trasa IPv4 przez PrivadoVPN nie wystarcza.";
const SEEDING_BLOCKED: &str = "Odtwarzanie zatrzymane: ten build odtwarza tylko gdy seedowanie jest wyłączone. Wykryto wysyłanie kawałków albo odblokowany upload.";
const VPN_NOTICE: &str = "Polska produkcja. Seedowanie jest wyłączone — aplikacja nie wysyła kawałków filmu. Powinieneś użyć VPN (Privado) i Kill Switch przed oglądaniem.";

#[derive(Clone, Copy, Eq, PartialEq)]
enum RouteState {
    Ready,
    Disconnected,
    Ipv6Risk,
    Unavailable,
}

// Read-only: no registry, firewall, credentials, public-IP services or DNS queries.
// Require all IPv4 default halves on Privado and refuse an unprotected IPv6 path.
const ROUTE_PROBE: &str = r#"
$ErrorActionPreference = 'Stop'
try {
    $vpn = @(Get-NetAdapter -IncludeHidden | Where-Object { $_.Name -like 'PrivadoVPN*' -and $_.Status -eq 'Up' })
    if ($vpn.Count -eq 0) { exit 2 }
    $indices = @($vpn | Select-Object -ExpandProperty InterfaceIndex)
    foreach ($destination in @('1.1.1.1', '129.1.1.1')) {
        $route = @(Find-NetRoute -RemoteIPAddress $destination | Where-Object { $_.CimClass.CimClassName -eq 'MSFT_NetRoute' })
        if ($route.Count -ne 1 -or $indices -notcontains $route[0].InterfaceIndex) { exit 2 }
    }
    $ipv6 = @(Get-NetIPAddress -AddressFamily IPv6 -AddressState Preferred | Where-Object {
        $_.IPAddress -notmatch '^(fe80:|::1$)' -and $indices -notcontains $_.InterfaceIndex
    })
    if ($ipv6.Count -gt 0) { exit 4 }
    exit 0
} catch { exit 3 }
"#;

#[cfg(windows)]
fn probe_route() -> RouteState {
    use std::{
        os::windows::process::CommandExt,
        process::{Command, Stdio},
    };
    let powershell = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:\\Windows".into())
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let Ok(mut child) = Command::new(powershell)
        .args(["-NoProfile", "-NonInteractive", "-Command", ROUTE_PROBE])
        .creation_flags(0x08000000)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return RouteState::Unavailable;
    };
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return match status.code() {
                    Some(0) => RouteState::Ready,
                    Some(2) => RouteState::Disconnected,
                    Some(4) => RouteState::Ipv6Risk,
                    _ => RouteState::Unavailable,
                };
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return RouteState::Unavailable;
            }
        }
    }
}
#[cfg(not(windows))]
fn probe_route() -> RouteState {
    RouteState::Unavailable
}

async fn route_state() -> RouteState {
    tokio::task::spawn_blocking(probe_route)
        .await
        .unwrap_or(RouteState::Unavailable)
}

fn open_privado() {
    #[cfg(windows)]
    {
        // The supported Auto Connect setting can connect when this GUI starts.
        // No undocumented connect/disconnect switches or account extraction.
        static LAST_OPEN: AtomicU64 = AtomicU64::new(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now.saturating_sub(LAST_OPEN.load(Ordering::Relaxed)) < 30 {
            return;
        }
        LAST_OPEN.store(now, Ordering::Relaxed);
        let path = std::path::Path::new("C:\\Program Files (x86)\\PrivadoVPN\\PrivadoVPN.exe");
        if path.is_file() {
            let _ = std::process::Command::new(path).spawn();
        }
    }
}

pub fn selected_recommends_vpn(selected: &Selected) -> bool {
    match &selected.stream.source {
        StreamSource::Torrent { info_hash, .. } => {
            core_env::vpn_policy::recommends_vpn(Some(&hex::encode(info_hash)))
        }
        _ => false,
    }
}

/// Every player entry path binds the hash to metadata identity before Core can
/// create a torrent. Missing identity alone is not an exemption; an explicit
/// user declaration for this hash is handled separately by vpn_policy.
pub fn register_selection(selected: &Selected) {
    if let StreamSource::Torrent { info_hash, .. } = &selected.stream.source
        && let Some(request) = selected.meta_request.as_ref()
    {
        core_env::vpn_policy::register_torrent(
            &hex::encode(info_hash),
            &request.path.r#type,
            &request.path.id,
        );
    }
}

/// Stop the previous protected swarm on exit or when switching to another
/// info-hash. Keep the same multi-file swarm when moving between its episodes.
pub fn stop_previous(
    core: &Arc<stremio_core::runtime::Runtime<core_env::DesktopEnv, crate::AppModel>>,
    next: Option<&Selected>,
) {
    let next_hash = next.and_then(|selected| match &selected.stream.source {
        StreamSource::Torrent { info_hash, .. } => Some(hex::encode(info_hash)),
        _ => None,
    });
    let previous_hash =
        core.model().ok().and_then(
            |model| match &model.player.selected.as_ref()?.stream.source {
                StreamSource::Torrent { info_hash, .. } => Some(hex::encode(info_hash)),
                _ => None,
            },
        );
    let Some(hash) = previous_hash else { return };
    if next_hash.as_ref() == Some(&hash) || !core_env::vpn_policy::requires_vpn(Some(&hash)) {
        return;
    }
    tokio::spawn(async move {
        // A delayed close must not stop a newly opened replay of the same hash.
        let Ok(_access) = stream_server::playback_cache::access_gate().try_write_owned() else {
            return;
        };
        if stream_server::playback_cache::is_playing(&hash) { return; }
        let state = stream_server::GLOBAL_STATE
            .read()
            .ok()
            .and_then(|state| state.clone());
        if let Some(state) = state {
            let engine = state.stream_engine();
            if let Err(error) = engine.retire_unreferenced_engine(&hash).await {
                tracing::warn!(info_hash = %hash, %error, "closed torrent retirement deferred");
            }
        }
    });
}

async fn ensure_not_seeding() -> anyhow::Result<()> {
    let state = stream_server::GLOBAL_STATE
        .read()
        .ok()
        .and_then(|state| state.clone());
    let Some(state) = state else {
        anyhow::bail!(SEEDING_BLOCKED);
    };
    if state.engine.seeding_enabled() || state.download_engine.seeding_enabled() {
        anyhow::bail!(SEEDING_BLOCKED);
    }
    let report = crate::torrent_traffic::load_report();
    if report.seeding_policy_enabled || report.incoming_listen || !report.upload_disabled {
        anyhow::bail!(SEEDING_BLOCKED);
    }
    if report.session_piece_payload_uploaded_bytes > 0 {
        anyhow::bail!(SEEDING_BLOCKED);
    }
    for stats in state
        .engine
        .get_all_statistics()
        .await
        .into_values()
        .chain(state.download_engine.get_all_statistics().await.into_values())
    {
        if stats.uploaded > 0 || stats.upload_speed > 0.0 {
            anyhow::bail!(SEEDING_BLOCKED);
        }
    }
    Ok(())
}

fn set_vpn_notice(weak: slint::Weak<crate::MainWindow>, visible: bool) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_player_vpn_notice(visible);
        }
    });
}

fn show_error(weak: slint::Weak<crate::MainWindow>, message: &'static str) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_error_message(message.into());
            ui.set_player_error(message.into());
            ui.set_player_loading(false);
        }
    });
}

pub fn install(ui: &crate::MainWindow) -> anyhow::Result<()> {
    let weak = ui.as_weak();
    stream_server::torrent_admission::install(Arc::new(move |hash| {
        let weak = weak.clone();
        Box::pin(async move {
            if let Err(error) = ensure_not_seeding().await {
                show_error(weak, SEEDING_BLOCKED);
                return Err(error);
            }
            match core_env::vpn_policy::gate(hash.as_deref()) {
                core_env::vpn_policy::VpnGate::None => {
                    set_vpn_notice(weak, false);
                    return Ok(());
                }
                core_env::vpn_policy::VpnGate::Recommended => {
                    set_vpn_notice(weak, true);
                    tracing::info!(notice = VPN_NOTICE, "Polish torrent admitted without VPN");
                    return Ok(());
                }
                core_env::vpn_policy::VpnGate::Required => {
                    set_vpn_notice(weak.clone(), false);
                }
            }
            match route_state().await {
                RouteState::Ready => return Ok(()),
                RouteState::Ipv6Risk => {
                    show_error(weak, IPV6_RISK);
                    anyhow::bail!(IPV6_RISK)
                }
                _ => {}
            }
            open_privado();
            // Auto Connect in Privado can finish this without another click.
            // Until the check passes, librqbit has not received this torrent.
            let deadline = Instant::now() + Duration::from_secs(25);
            while Instant::now() < deadline {
                tokio::time::sleep(Duration::from_secs(2)).await;
                match route_state().await {
                    RouteState::Ready => return Ok(()),
                    RouteState::Ipv6Risk => {
                        show_error(weak, IPV6_RISK);
                        anyhow::bail!(IPV6_RISK)
                    }
                    _ => {}
                }
            }
            show_error(weak, REQUIRED);
            anyhow::bail!(REQUIRED)
        })
    }))?;
    let weak = ui.as_weak();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let state = stream_server::GLOBAL_STATE
                .read()
                .ok()
                .and_then(|state| state.clone());
            let Some(state) = state else { continue };
            let engine = state.stream_engine();
            let protected = engine
                .session_torrent_hashes()
                .await
                .into_iter()
                .filter(|hash| core_env::vpn_policy::requires_vpn(Some(hash)))
                .collect::<Vec<_>>();
            if protected.is_empty() || route_state().await == RouteState::Ready {
                continue;
            }
            for hash in protected {
                if engine.stop_torrent(&hash).await.is_err() {
                    // A failed per-torrent close must not leave active peers.
                    engine.stop().await;
                    break;
                }
            }
            show_error(
                weak.clone(),
                "Utracono bezpieczną trasę VPN. Zatrzymano chronione torrenty. Połącz PrivadoVPN i wybierz źródło ponownie.",
            );
        }
    });
    Ok(())
}
