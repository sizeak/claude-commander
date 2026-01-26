//! Cached pane content capture
//!
//! Provides efficient pane content capture with:
//! - TTL-based caching to avoid redundant captures
//! - Async-first design for non-blocking operation
//! - Hash-based change detection

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tracing::{debug, instrument};
use xxhash_rust::xxh3::xxh3_64;

use super::TmuxExecutor;
use crate::error::Result;
use crate::session::SessionId;

/// Default cache TTL (50ms)
pub const DEFAULT_CACHE_TTL: Duration = Duration::from_millis(50);

/// Captured pane content with metadata
#[derive(Debug, Clone)]
pub struct CapturedContent {
    /// The captured text content
    pub content: String,
    /// Content hash for change detection
    pub hash: u64,
    /// When the content was captured
    pub captured_at: Instant,
    /// Number of lines captured
    pub line_count: usize,
}

impl CapturedContent {
    /// Create a new captured content
    pub fn new(content: String) -> Self {
        let hash = xxh3_64(content.as_bytes());
        let line_count = content.lines().count();

        Self {
            content,
            hash,
            captured_at: Instant::now(),
            line_count,
        }
    }

    /// Check if this content is stale
    pub fn is_stale(&self, ttl: Duration) -> bool {
        self.captured_at.elapsed() > ttl
    }

    /// Get the age of this capture
    pub fn age(&self) -> Duration {
        self.captured_at.elapsed()
    }

    /// Check if content has changed from another capture
    pub fn has_changed(&self, other: &Self) -> bool {
        self.hash != other.hash
    }
}

/// Cached pane content capture manager
///
/// Maintains a cache of captured pane content with configurable TTL.
/// Fast path returns cached content if fresh; slow path performs async capture.
pub struct ContentCapture {
    /// Tmux executor for actual captures
    executor: TmuxExecutor,
    /// Cache of session ID -> captured content
    cache: Arc<RwLock<HashMap<SessionId, CapturedContent>>>,
    /// Cache TTL
    ttl: Duration,
}

impl ContentCapture {
    /// Create a new content capture manager
    pub fn new(executor: TmuxExecutor) -> Self {
        Self::with_ttl(executor, DEFAULT_CACHE_TTL)
    }

    /// Create with custom TTL
    pub fn with_ttl(executor: TmuxExecutor, ttl: Duration) -> Self {
        Self {
            executor,
            cache: Arc::new(RwLock::new(HashMap::new())),
            ttl,
        }
    }

    /// Get content for a session, using cache if fresh
    #[instrument(skip(self))]
    pub async fn get_content(
        &self,
        session_id: &SessionId,
        tmux_session_name: &str,
    ) -> Result<CapturedContent> {
        // Fast path: check cache with read lock
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(session_id) {
                if !cached.is_stale(self.ttl) {
                    debug!(
                        "Cache hit for session {}, age: {:?}",
                        session_id,
                        cached.age()
                    );
                    return Ok(cached.clone());
                }
            }
        }

        // Slow path: capture fresh content
        debug!("Cache miss for session {}, capturing fresh", session_id);
        self.capture_fresh(session_id, tmux_session_name).await
    }

    /// Force a fresh capture, bypassing cache
    pub async fn capture_fresh(
        &self,
        session_id: &SessionId,
        tmux_session_name: &str,
    ) -> Result<CapturedContent> {
        // Capture with scrollback (last 1000 lines)
        let content = self
            .executor
            .capture_pane(tmux_session_name, Some(-1000), None)
            .await?;

        let captured = CapturedContent::new(content);

        // Update cache
        {
            let mut cache = self.cache.write().await;
            cache.insert(*session_id, captured.clone());
        }

        Ok(captured)
    }

    /// Invalidate cache for a session
    pub async fn invalidate(&self, session_id: &SessionId) {
        let mut cache = self.cache.write().await;
        cache.remove(session_id);
    }

    /// Clear all cached content
    pub async fn clear(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }

    /// Get all cached sessions
    pub async fn cached_sessions(&self) -> Vec<SessionId> {
        let cache = self.cache.read().await;
        cache.keys().copied().collect()
    }
}

impl Clone for ContentCapture {
    fn clone(&self) -> Self {
        Self {
            executor: self.executor.clone(),
            cache: self.cache.clone(),
            ttl: self.ttl,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_captured_content_creation() {
        let content = CapturedContent::new("Hello, World!".to_string());
        assert_eq!(content.line_count, 1);
        assert!(!content.is_stale(Duration::from_secs(1)));
    }

    #[test]
    fn test_content_hash_change_detection() {
        let content1 = CapturedContent::new("Hello".to_string());
        let content2 = CapturedContent::new("Hello".to_string());
        let content3 = CapturedContent::new("World".to_string());

        assert!(!content1.has_changed(&content2));
        assert!(content1.has_changed(&content3));
    }

    #[test]
    fn test_content_staleness() {
        let content = CapturedContent::new("Test".to_string());

        // Freshly created content should not be stale
        assert!(!content.is_stale(Duration::from_millis(100)));

        // With zero TTL, content is immediately stale
        assert!(content.is_stale(Duration::ZERO));
    }
}
