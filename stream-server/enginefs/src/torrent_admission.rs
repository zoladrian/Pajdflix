//! Optional embedding policy, checked before metadata/tracker/peer activity.
use crate::backend::TorrentSource;
use anyhow::Result;
use futures::future::BoxFuture;
use std::sync::{Arc, OnceLock};

pub type Admission = Arc<dyn Fn(Option<String>) -> BoxFuture<'static, Result<()>> + Send + Sync>;
static ADMISSION: OnceLock<Admission> = OnceLock::new();

pub fn install(policy: Admission) -> Result<()> {
    ADMISSION
        .set(policy)
        .map_err(|_| anyhow::anyhow!("Torrent admission policy is already installed"))
}

fn source_hash(source: &TorrentSource) -> Option<String> {
    let TorrentSource::Url(raw) = source else {
        return None;
    };
    let url = url::Url::parse(raw).ok()?;
    if url.scheme() != "magnet" {
        return None;
    }
    let mut topics = url.query_pairs().filter(|(key, _)| key == "xt");
    let (_, topic) = topics.next()?;
    if topics.next().is_some() {
        return None;
    }
    let hash = topic.strip_prefix("urn:btih:")?;
    (hash.len() == 40 && hash.bytes().all(|c| c.is_ascii_hexdigit()))
        .then(|| hash.to_ascii_lowercase())
}

pub(crate) async fn check(source: &TorrentSource) -> Result<()> {
    if let Some(policy) = ADMISSION.get() {
        policy(source_hash(source)).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_unambiguous_magnets_can_use_a_registered_foreign_title() {
        let hash = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            source_hash(&TorrentSource::Url(format!("magnet:?xt=urn:btih:{hash}"))),
            Some(hash.into())
        );
        assert_eq!(
            source_hash(&TorrentSource::Url("https://example.com/a.torrent".into())),
            None
        );
        assert_eq!(source_hash(&TorrentSource::Bytes(vec![1, 2, 3])), None);
        assert_eq!(
            source_hash(&TorrentSource::Url("magnet:?xt=urn:btih:invalid".into())),
            None
        );
        assert_eq!(
            source_hash(&TorrentSource::Url(format!(
                "magnet:?xt=urn:btih:{hash}&xt=urn:btih:{hash}"
            ))),
            None
        );
    }
}
