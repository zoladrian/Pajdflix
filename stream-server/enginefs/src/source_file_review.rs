//! A desktop-only, per-source acknowledgement of suspicious media filenames.
//! No HTTP endpoint can grant this acknowledgement. Dropping the guard revokes it.
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

struct Review {
    reason: String,
    created: Instant,
    approved: bool,
}

fn reviews() -> &'static Mutex<HashMap<String, Review>> {
    static REVIEWS: OnceLock<Mutex<HashMap<String, Review>>> = OnceLock::new();
    REVIEWS.get_or_init(Default::default)
}

fn current(review: &Review) -> bool {
    review.approved || review.created.elapsed() < Duration::from_secs(600)
}

pub(crate) fn record(info_hash: &str, reason: String) {
    let mut reviews = reviews().lock().unwrap_or_else(|e| e.into_inner());
    reviews.retain(|_, review| current(review));
    if reviews.len() >= 128 && !reviews.contains_key(info_hash) {
        if let Some(oldest) = reviews
            .iter()
            .filter(|(_, r)| !r.approved)
            .min_by_key(|(_, r)| r.created)
            .map(|(hash, _)| hash.clone())
        {
            reviews.remove(&oldest);
        } else {
            return;
        }
    }
    reviews
        .entry(info_hash.to_owned())
        .or_insert_with(|| Review {
            reason,
            created: Instant::now(),
            approved: false,
        });
}

pub fn pending_reason(info_hash: &str) -> Option<String> {
    let reviews = reviews().lock().unwrap_or_else(|e| e.into_inner());
    let review = reviews.get(info_hash)?;
    (current(review) && !review.approved).then(|| review.reason.clone())
}

pub(crate) fn is_approved(info_hash: &str) -> bool {
    reviews()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(info_hash)
        .is_some_and(|r| r.approved)
}

/// Kept by the player only until the source changes or playback is closed.
#[derive(Debug)]
pub struct SourceApproval(String);

pub fn approve(info_hash: &str) -> Option<SourceApproval> {
    let mut reviews = reviews().lock().unwrap_or_else(|e| e.into_inner());
    let review = reviews.get_mut(info_hash)?;
    if !current(review) || review.approved {
        return None;
    }
    review.approved = true;
    Some(SourceApproval(info_hash.to_owned()))
}

impl Drop for SourceApproval {
    fn drop(&mut self) {
        reviews()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_requires_a_block_and_is_scoped_and_revocable() {
        assert!(approve("never-blocked").is_none());
        record("review-a", "Suspicious filename".into());
        record("review-b", "Another source".into());
        let approval = approve("review-a").unwrap();
        assert!(is_approved("review-a"));
        assert!(!is_approved("review-b"));
        assert!(pending_reason("review-a").is_none());
        assert!(pending_reason("review-b").is_some());
        assert!(approve("review-a").is_none());
        drop(approval);
        assert!(!is_approved("review-a"));
        assert!(approve("review-a").is_none());
    }

    #[test]
    fn stale_blocks_cannot_be_approved() {
        reviews().lock().unwrap().insert(
            "stale-review".into(),
            Review {
                reason: "Old error".into(),
                created: Instant::now() - Duration::from_secs(601),
                approved: false,
            },
        );
        assert!(pending_reason("stale-review").is_none());
        assert!(approve("stale-review").is_none());
    }
}
