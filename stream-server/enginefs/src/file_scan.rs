//! Completed-file scanning supplements realtime protection; partial streaming is
//! not an antivirus verdict. Never elevate the application or change exclusions.
use anyhow::{Context, Result, ensure};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};
use tokio::sync::Mutex;

#[derive(Clone, PartialEq, Eq)]
struct Fingerprint {
    length: u64,
    modified: SystemTime,
}
struct Entry {
    fingerprint: Fingerprint,
    failure: Option<String>,
}
#[derive(Default)]
pub struct FileScanner {
    entries: Mutex<HashMap<PathBuf, Arc<Mutex<Option<Entry>>>>>,
}
fn fingerprint(path: &Path) -> Result<Fingerprint> {
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "scan target is not a regular file"
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "scan target is a reparse point"
        );
    }
    Ok(Fingerprint {
        length: metadata.len(),
        modified: metadata.modified()?,
    })
}

impl FileScanner {
    /// A failed scan is cached for this exact file version and blocks access.
    /// Retry is explicit: remove/re-download the torrent after resolving Defender.
    pub async fn ensure_clean(&self, path: &Path) -> Result<()> {
        let lock = self
            .entries
            .lock()
            .await
            .entry(path.to_owned())
            .or_default()
            .clone();
        let mut entry = lock.lock().await;
        let before = fingerprint(path).context("completed file is unavailable for scanning")?;
        if let Some(previous) = entry
            .as_ref()
            .filter(|previous| previous.fingerprint == before)
        {
            if let Some(failure) = &previous.failure {
                anyhow::bail!("{failure}");
            }
            return Ok(());
        }
        tracing::info!(file = %path.display(), "Scanning completed media with Microsoft Defender");
        let result = scan_with_defender(path).await.and_then(|_| {
            ensure!(
                fingerprint(path)? == before,
                "file changed or was removed during Defender scan"
            );
            Ok(())
        });
        let failure = result.as_ref().err().map(|error| {
            format!("File blocked: Microsoft Defender scan did not approve this file: {error}")
        });
        if let Some(error) = &failure {
            tracing::error!(file = %path.display(), %error);
        } else {
            tracing::info!(file = %path.display(), "Defender completed-file scan passed");
        }
        *entry = Some(Entry {
            fingerprint: before,
            failure,
        });
        result
    }
}

#[cfg(windows)]
async fn scan_with_defender(path: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    let program_files = std::env::var_os("ProgramFiles").context("ProgramFiles is unavailable")?;
    let scanner = PathBuf::from(program_files).join("Windows Defender/MpCmdRun.exe");
    ensure!(
        scanner.is_file(),
        "Microsoft Defender scanner is unavailable"
    );
    let mut command = tokio::process::Command::new(scanner);
    command.args(["-Scan", "-ScanType", "3", "-File"]).arg(path);
    command.as_std_mut().creation_flags(0x08000000); // CREATE_NO_WINDOW
    command.kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(120), command.output())
        .await
        .context("Microsoft Defender scan timed out; file remains blocked")??;
    // Code 2 also denotes scanning errors; do not label every failure malware.
    ensure!(
        output.status.code() == Some(0),
        "threat detection, remediation required, or scan failure (exit {:?}); inspect Windows Security",
        output.status.code()
    );
    Ok(())
}

#[cfg(not(windows))]
async fn scan_with_defender(_path: &Path) -> Result<()> {
    anyhow::bail!(
        "This private Windows build requires Microsoft Defender; no antivirus fallback is configured"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn failed_scan_is_never_treated_as_clean() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.mkv");
        std::fs::write(&path, b"legal fixture").unwrap();
        let scanner = FileScanner::default();
        scanner.entries.lock().await.insert(
            path.clone(),
            Arc::new(Mutex::new(Some(Entry {
                fingerprint: fingerprint(&path).unwrap(),
                failure: Some("simulated detection or scanner error".to_owned()),
            }))),
        );
        assert!(
            scanner
                .ensure_clean(&path)
                .await
                .unwrap_err()
                .to_string()
                .contains("simulated")
        );
        assert!(
            scanner
                .ensure_clean(&dir.path().join("missing.mkv"))
                .await
                .is_err()
        );
    }
}
