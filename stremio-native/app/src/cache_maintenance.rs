//! A small headless process for expired torrent payloads while the GUI is closed.
use std::{path::Path, time::Duration};

pub fn install_autostart() -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let executable = std::env::current_exe()?;
        let command = format!("\"{}\" --cache-maintenance", executable.display());
        let result = std::process::Command::new("reg.exe")
            .args([
                "add",
                "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                "/v",
                "PajdflixCacheMaintenance",
                "/t",
                "REG_SZ",
                "/d",
                &command,
                "/f",
            ])
            .creation_flags(0x08000000)
            .output()?;
        anyhow::ensure!(
            result.status.success(),
            "could not register cache maintenance autostart"
        );
        tracing::info!("cache_cleanup_autostart_registered");
        std::process::Command::new(executable)
            .arg("--cache-maintenance")
            .creation_flags(0x08000000)
            .spawn()?;
    }
    Ok(())
}

pub async fn run(cache_root: &Path, logs: &Path) -> anyhow::Result<()> {
    // One helper per user, even if several builds are launched.
    let _instance = match tokio::net::TcpListener::bind("127.0.0.1:35985").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    std::fs::create_dir_all(logs)?;
    let log_path = logs.join("cache-cleaner.log");
    if std::fs::metadata(&log_path).is_ok_and(|m| m.len() > 20 * 1024 * 1024) {
        let previous = logs.join("cache-cleaner.previous.log");
        let _ = std::fs::remove_file(&previous);
        std::fs::rename(&log_path, previous)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .with_writer(std::sync::Mutex::new(file))
        .init();
    tracing::info!("cache_cleanup_background_started");
    let mut recover = true;
    let mut gui_was_running = false;
    loop {
        // The running GUI owns its active torrents. Never inspect/delete them here.
        let gui_running = std::net::TcpListener::bind("127.0.0.1:35984").is_err();
        if !gui_running {
            stream_server::cache_cleaner::offline_cleanup(cache_root, recover || gui_was_running)
                .await;
            recover = false;
        }
        gui_was_running = gui_running;
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}
