//! Performance profiler with hierarchical timing collection.
//!
//! This module provides a lightweight profiling system designed for:
//! - Tracking cumulative time in tight render loops
//! - Hierarchical span collection (call trees)
//! - JSON export for LLM analysis and custom visualization
//!
//! # Usage
//!
//! ```rust,ignore
//! use crate::profiler::{profile_span, profile_fn};
//!
//! fn render_chat() {
//!     let _span = profile_span!("render_chat");
//!     // ... work ...
//! }
//!
//! // Or with the function macro
//! profile_fn!(fn expensive_computation() {
//!     // ...
//! });
//! ```

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Global profiler instance
static PROFILER: OnceLock<Mutex<Profiler>> = OnceLock::new();

/// Whether profiling is enabled (atomic for fast path checking)
static PROFILING_ENABLED: AtomicBool = AtomicBool::new(false);

/// Global frame counter for tracking render cycles
static FRAME_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Thread-local span stack for hierarchical tracking
thread_local! {
    static SPAN_STACK: RefCell<Vec<SpanId>> = RefCell::new(Vec::with_capacity(32));
}

/// Unique identifier for a span in the call tree
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpanId(u64);

/// A recorded span with timing information
#[derive(Debug, Clone)]
pub struct SpanRecord {
    /// Name of the span (function or region)
    pub name: &'static str,
    /// Parent span ID (None for root spans)
    pub parent: Option<SpanId>,
    /// Start time relative to profiling start
    pub start_us: u64,
    /// Duration in microseconds
    pub duration_us: u64,
    /// Frame number when this span was recorded
    pub frame: u64,
    /// Additional metadata (e.g., counts, sizes)
    pub metadata: Option<SpanMetadata>,
}

/// Optional metadata for a span
#[derive(Debug, Clone, Default)]
pub struct SpanMetadata {
    /// Number of items processed (e.g., cells, lines)
    pub count: Option<u64>,
    /// Bytes processed
    pub bytes: Option<u64>,
    /// Custom key-value pairs
    pub extra: HashMap<String, String>,
}

/// Aggregated statistics for a span name
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SpanStats {
    /// Total number of calls
    pub calls: u64,
    /// Total time in microseconds
    pub total_us: u64,
    /// Minimum duration
    pub min_us: u64,
    /// Maximum duration
    pub max_us: u64,
    /// Self time (excluding children)
    pub self_us: u64,
    /// Total items processed (if tracked)
    pub total_count: Option<u64>,
}

/// The profiler collects span records during execution
pub struct Profiler {
    /// When profiling started
    start_time: Instant,
    /// All recorded spans (append-only during profiling)
    spans: Vec<SpanRecord>,
    /// Next span ID
    next_id: u64,
    /// Session metadata
    session: SessionInfo,
}

/// Session-level metadata
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionInfo {
    /// When the session started (ISO 8601)
    pub started_at: String,
    /// Application version
    pub version: String,
    /// Platform info
    pub platform: String,
    /// Terminal size at start
    pub terminal_size: (u16, u16),
}

/// RAII guard for span timing
pub struct SpanGuard {
    id: SpanId,
    name: &'static str,
    start: Instant,
    metadata: Option<SpanMetadata>,
}

impl SpanGuard {
    /// Add metadata to this span
    pub fn with_count(mut self, count: u64) -> Self {
        self.metadata.get_or_insert_with(SpanMetadata::default).count = Some(count);
        self
    }

    /// Add custom metadata
    pub fn with_meta(mut self, key: &str, value: impl ToString) -> Self {
        self.metadata
            .get_or_insert_with(SpanMetadata::default)
            .extra
            .insert(key.to_string(), value.to_string());
        self
    }
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        if !is_profiling_enabled() {
            return;
        }

        let duration = self.start.elapsed();

        // Pop from thread-local stack
        SPAN_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            if let Some(id) = stack.pop() {
                debug_assert_eq!(id, self.id, "Span stack mismatch");
            }
        });

        // Record the completed span
        if let Some(profiler) = PROFILER.get() {
            if let Ok(mut profiler) = profiler.lock() {
                profiler.record_span_end(self.id, self.name, duration, self.metadata.take());
            }
        }
    }
}

impl Profiler {
    /// Create a new profiler
    pub fn new(terminal_size: (u16, u16)) -> Self {
        Self {
            start_time: Instant::now(),
            spans: Vec::with_capacity(100_000),
            next_id: 0,
            session: SessionInfo {
                started_at: chrono::Utc::now().to_rfc3339(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                platform: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
                terminal_size,
            },
        }
    }

    /// Start a new span and return its ID
    fn begin_span(&mut self, name: &'static str, parent: Option<SpanId>) -> SpanId {
        let id = SpanId(self.next_id);
        self.next_id += 1;

        let start_us = self.start_time.elapsed().as_micros() as u64;
        let frame = FRAME_COUNTER.load(Ordering::Relaxed);

        // Pre-record with zero duration (will be updated on end)
        self.spans.push(SpanRecord {
            name,
            parent,
            start_us,
            duration_us: 0,
            frame,
            metadata: None,
        });

        id
    }

    /// Record span completion
    fn record_span_end(
        &mut self,
        id: SpanId,
        _name: &'static str,
        duration: Duration,
        metadata: Option<SpanMetadata>,
    ) {
        // Find and update the span
        let idx = id.0 as usize;
        if idx < self.spans.len() {
            self.spans[idx].duration_us = duration.as_micros() as u64;
            self.spans[idx].metadata = metadata;
        }
    }

    /// Get aggregated statistics per span name
    pub fn aggregate_stats(&self) -> HashMap<&'static str, SpanStats> {
        let mut stats: HashMap<&'static str, SpanStats> = HashMap::new();

        // First pass: calculate totals
        for span in &self.spans {
            let entry = stats.entry(span.name).or_default();
            entry.calls += 1;
            entry.total_us += span.duration_us;
            entry.min_us = if entry.min_us == 0 {
                span.duration_us
            } else {
                entry.min_us.min(span.duration_us)
            };
            entry.max_us = entry.max_us.max(span.duration_us);

            if let Some(ref meta) = span.metadata {
                if let Some(count) = meta.count {
                    *entry.total_count.get_or_insert(0) += count;
                }
            }
        }

        // Second pass: calculate self time (total - children)
        for span in &self.spans {
            let child_time: u64 = self
                .spans
                .iter()
                .filter(|s| s.parent == Some(SpanId(span.name.as_ptr() as u64)))
                .map(|s| s.duration_us)
                .sum();

            if let Some(entry) = stats.get_mut(span.name) {
                entry.self_us = entry.total_us.saturating_sub(child_time);
            }
        }

        stats
    }

    /// Get the raw span records
    pub fn spans(&self) -> &[SpanRecord] {
        &self.spans
    }

    /// Get session info
    pub fn session(&self) -> &SessionInfo {
        &self.session
    }

    /// Number of frames recorded
    pub fn frame_count(&self) -> u64 {
        FRAME_COUNTER.load(Ordering::Relaxed)
    }

    /// Total profiling duration
    pub fn duration(&self) -> Duration {
        self.start_time.elapsed()
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Initialize the global profiler. Call once at startup.
pub fn init(terminal_size: (u16, u16)) {
    let _ = PROFILER.set(Mutex::new(Profiler::new(terminal_size)));
    PROFILING_ENABLED.store(true, Ordering::Release);
}

/// Check if profiling is enabled (fast path)
#[inline]
pub fn is_profiling_enabled() -> bool {
    PROFILING_ENABLED.load(Ordering::Relaxed)
}

/// Stop profiling and prepare for export
pub fn stop() {
    PROFILING_ENABLED.store(false, Ordering::Release);
}

/// Begin a new profiling span. Returns a guard that records timing on drop.
#[inline]
pub fn begin_span(name: &'static str) -> Option<SpanGuard> {
    if !is_profiling_enabled() {
        return None;
    }

    // Get current parent from thread-local stack
    let parent = SPAN_STACK.with(|stack| stack.borrow().last().copied());

    let id = PROFILER.get()?.lock().ok()?.begin_span(name, parent);

    // Push onto thread-local stack
    SPAN_STACK.with(|stack| stack.borrow_mut().push(id));

    Some(SpanGuard {
        id,
        name,
        start: Instant::now(),
        metadata: None,
    })
}

/// Increment the frame counter. Call once per draw.
#[inline]
pub fn increment_frame() {
    if is_profiling_enabled() {
        FRAME_COUNTER.fetch_add(1, Ordering::Relaxed);
    }
}

/// Export profiling data to JSON file
pub fn export_json(path: impl AsRef<Path>) -> std::io::Result<()> {
    let profiler = match PROFILER.get() {
        Some(p) => p.lock().unwrap(),
        None => return Ok(()),
    };

    let file = File::create(path)?;
    let writer = BufWriter::new(file);

    let export = ProfileExport {
        session: profiler.session().clone(),
        duration_ms: profiler.duration().as_millis() as u64,
        frame_count: profiler.frame_count(),
        stats: profiler.aggregate_stats(),
        spans: profiler
            .spans()
            .iter()
            .map(|s| SpanExport {
                name: s.name.to_string(),
                parent_idx: s.parent.map(|p| p.0),
                start_us: s.start_us,
                duration_us: s.duration_us,
                frame: s.frame,
                count: s.metadata.as_ref().and_then(|m| m.count),
            })
            .collect(),
        call_tree: build_call_tree(&profiler),
    };

    serde_json::to_writer_pretty(writer, &export)?;
    Ok(())
}

/// Export format for JSON serialization
#[derive(serde::Serialize)]
struct ProfileExport {
    session: SessionInfo,
    duration_ms: u64,
    frame_count: u64,
    stats: HashMap<&'static str, SpanStats>,
    spans: Vec<SpanExport>,
    call_tree: CallTreeNode,
}

#[derive(serde::Serialize)]
struct SpanExport {
    name: String,
    parent_idx: Option<u64>,
    start_us: u64,
    duration_us: u64,
    frame: u64,
    count: Option<u64>,
}

/// Hierarchical call tree for flame graph visualization
#[derive(serde::Serialize, Default)]
struct CallTreeNode {
    name: String,
    total_us: u64,
    self_us: u64,
    calls: u64,
    children: Vec<CallTreeNode>,
}

fn build_call_tree(profiler: &Profiler) -> CallTreeNode {
    // Build a tree from parent relationships
    let mut root = CallTreeNode {
        name: "root".to_string(),
        ..Default::default()
    };

    // Group spans by name with parent context
    let mut by_path: HashMap<String, (u64, u64)> = HashMap::new();

    for span in profiler.spans() {
        // Build path from root to this span
        let path = span.name.to_string();
        let entry = by_path.entry(path).or_default();
        entry.0 += span.duration_us;
        entry.1 += 1;
    }

    // Convert to tree structure (simplified - real impl would track full paths)
    for (name, (total_us, calls)) in by_path {
        root.children.push(CallTreeNode {
            name,
            total_us,
            self_us: total_us, // Simplified
            calls,
            children: vec![],
        });
        root.total_us += total_us;
    }

    root.children.sort_by(|a, b| b.total_us.cmp(&a.total_us));
    root
}

// ============================================================================
// Macros for convenient instrumentation
// ============================================================================

/// Profile a span with automatic timing. Usage: `let _span = profile_span!("name");`
#[macro_export]
macro_rules! profile_span {
    ($name:expr) => {{
        #[cfg(feature = "profiling")]
        {
            $crate::profiler::begin_span($name)
        }
        #[cfg(not(feature = "profiling"))]
        {
            None::<()>
        }
    }};
}

/// Profile a span with count metadata. Usage: `let _span = profile_span_count!("name", 100);`
#[macro_export]
macro_rules! profile_span_count {
    ($name:expr, $count:expr) => {{
        #[cfg(feature = "profiling")]
        {
            $crate::profiler::begin_span($name).map(|s| s.with_count($count as u64))
        }
        #[cfg(not(feature = "profiling"))]
        {
            let _ = $count;
            None::<()>
        }
    }};
}

/// Mark a frame boundary. Call once per draw.
#[macro_export]
macro_rules! profile_frame {
    () => {{
        #[cfg(feature = "profiling")]
        {
            $crate::profiler::increment_frame()
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_span_recording() {
        init((80, 24));

        {
            let _span = begin_span("test_outer");
            std::thread::sleep(Duration::from_micros(100));
            {
                let _inner = begin_span("test_inner");
                std::thread::sleep(Duration::from_micros(50));
            }
        }

        let profiler = PROFILER.get().unwrap().lock().unwrap();
        assert_eq!(profiler.spans().len(), 2);

        let stats = profiler.aggregate_stats();
        assert!(stats.contains_key("test_outer"));
        assert!(stats.contains_key("test_inner"));
    }
}
