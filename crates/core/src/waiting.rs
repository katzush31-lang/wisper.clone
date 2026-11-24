//! Recordings that are saved but could not be processed yet (queued or failed): which ones
//! can still succeed, and the "Try all again" run over them.

use std::sync::Arc;

use crate::db::{HistoryItem, HistoryQuery};
use crate::pipeline::{Engine, PipelineError};
use crate::providers::ErrorKind;

/// A failed or queued recording whose audio is gone can never succeed, so it is not "waiting".
pub fn is_recoverable(item: &HistoryItem) -> bool {
    item.audio_path.as_deref().map(|p| std::path::Path::new(p).exists()).unwrap_or(false)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetryWaitingResult {
    pub done: usize,
    pub still_waiting: usize,
    /// The recordings tried, in order.
    pub attempted: Vec<String>,
}

/// Retries every waiting recording: queued ones first, then failed ones, each oldest first.
/// Stops early when the internet is still down.
pub async fn retry_waiting(engine: &Arc<Engine>) -> RetryWaitingResult {
    let mut order = Vec::new();
    for status in ["queued", "failed"] {
        let q = HistoryQuery { status: Some(status.into()), limit: Some(500), ..Default::default() };
        let mut items: Vec<HistoryItem> = engine.db.list(&q).unwrap_or_default().into_iter().filter(is_recoverable).collect();
        items.sort_by_key(|i| i.created_at);
        order.extend(items);
    }
    let total = order.len();
    let mut out = RetryWaitingResult::default();
    for it in order {
        out.attempted.push(it.id.clone());
        let r = engine.retry_dictation(&it.id, None).await;
        let offline = matches!(&r, Err(PipelineError::Queued { kind, .. }) if matches!(kind, ErrorKind::Network | ErrorKind::Timeout));
        if r.is_ok() {
            out.done += 1;
        }
        if offline {
            break;
        }
    }
    out.still_waiting = total - out.done;
    out
}
