//! Lightweight performance profiler for timing and resource tracking.
//!
//! Usage:
//! ```rust,ignore
//! let _span = profile_span!("render_chat");
//! let _span = profile_span_count!("render_cells", cell_count);
//! profile_frame!();
//! ```

use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessesToUpdate, System};

static PROFILER: OnceLock<Mutex<Profiler>> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(false);
static DRAW_COUNT: AtomicU64 = AtomicU64::new(0);

/// Aggregated statistics for a span
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Stats {
    pub calls: u64,
    pub total_us: u64,
    pub min_us: u64,
    pub max_us: u64,
    /// Cumulative item count (from `profile_span_count!`)
    #[serde(skip_serializing_if = "is_zero")]
    pub item_count: u64,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

impl Stats {
    fn record(&mut self, duration_us: u64) {
        self.calls += 1;
        self.total_us += duration_us;
        self.min_us = if self.min_us == 0 { duration_us } else { self.min_us.min(duration_us) };
        self.max_us = self.max_us.max(duration_us);
    }

    fn record_with_count(&mut self, duration_us: u64, count: u64) {
        self.record(duration_us);
        self.item_count += count;
    }
}

/// Resource usage summary
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Resources {
    pub peak_memory_bytes: u64,
    pub final_memory_bytes: u64,
    pub avg_cpu_percent: f32,
    pub peak_cpu_percent: f32,
    pub samples: usize,
}

struct Profiler {
    start_time: Instant,
    stats: HashMap<&'static str, Stats>,
    // Resource tracking
    system: System,
    pid: Pid,
    last_sample: Instant,
    memory_samples: Vec<u64>,
    cpu_samples: Vec<f32>,
    // Session info
    terminal_size: (u16, u16),
}

impl Profiler {
    fn new(terminal_size: (u16, u16)) -> Self {
        let mut system = System::new();
        let pid = Pid::from_u32(std::process::id());
        system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        
        Self {
            start_time: Instant::now(),
            stats: HashMap::new(),
            system,
            pid,
            last_sample: Instant::now(),
            memory_samples: Vec::new(),
            cpu_samples: Vec::new(),
            terminal_size,
        }
    }
    
    fn record(&mut self, name: &'static str, duration_us: u64) {
        self.stats.entry(name).or_default().record(duration_us);
    }

    fn record_with_count(&mut self, name: &'static str, duration_us: u64, count: u64) {
        self.stats.entry(name).or_default().record_with_count(duration_us, count);
    }
    
    fn sample_resources(&mut self) {
        if self.last_sample.elapsed() < Duration::from_millis(100) {
            return;
        }
        self.last_sample = Instant::now();
        self.system.refresh_processes(ProcessesToUpdate::Some(&[self.pid]), true);
        
        if let Some(proc) = self.system.process(self.pid) {
            self.memory_samples.push(proc.memory());
            self.cpu_samples.push(proc.cpu_usage());
        }
    }
    
    fn resources(&self) -> Resources {
        if self.memory_samples.is_empty() {
            return Resources::default();
        }
        Resources {
            peak_memory_bytes: self.memory_samples.iter().copied().max().unwrap_or(0),
            final_memory_bytes: self.memory_samples.last().copied().unwrap_or(0),
            avg_cpu_percent: self.cpu_samples.iter().sum::<f32>() / self.cpu_samples.len() as f32,
            peak_cpu_percent: self.cpu_samples.iter().copied().fold(0f32, f32::max),
            samples: self.memory_samples.len(),
        }
    }
}

/// RAII guard that records timing on drop
pub struct SpanGuard {
    name: &'static str,
    start: Instant,
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        if let Some(profiler) = PROFILER.get() {
            if let Ok(mut p) = profiler.lock() {
                p.record(self.name, self.start.elapsed().as_micros() as u64);
            }
        }
    }
}

/// RAII guard that records timing and an item count on drop
pub struct SpanCountGuard {
    name: &'static str,
    start: Instant,
    count: u64,
}

impl Drop for SpanCountGuard {
    fn drop(&mut self) {
        if let Some(profiler) = PROFILER.get() {
            if let Ok(mut p) = profiler.lock() {
                p.record_with_count(self.name, self.start.elapsed().as_micros() as u64, self.count);
            }
        }
    }
}

// ============================================================================
// Public API
// ============================================================================

pub fn init(terminal_size: (u16, u16)) {
    let _ = PROFILER.set(Mutex::new(Profiler::new(terminal_size)));
    ENABLED.store(true, Ordering::Release);
}

#[inline]
pub fn is_profiling_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn stop() {
    ENABLED.store(false, Ordering::Release);
}

#[inline]
pub fn begin_span(name: &'static str) -> Option<SpanGuard> {
    if !is_profiling_enabled() {
        return None;
    }
    Some(SpanGuard { name, start: Instant::now() })
}

#[inline]
pub fn begin_span_count(name: &'static str, count: u64) -> Option<SpanCountGuard> {
    if !is_profiling_enabled() {
        return None;
    }
    Some(SpanCountGuard { name, start: Instant::now(), count })
}

#[inline]
pub fn increment_frame() {
    if !is_profiling_enabled() {
        return;
    }
    DRAW_COUNT.fetch_add(1, Ordering::Relaxed);
    if let Some(profiler) = PROFILER.get() {
        if let Ok(mut p) = profiler.lock() {
            p.sample_resources();
        }
    }
}

pub fn export_json(path: impl AsRef<Path>) -> std::io::Result<()> {
    let p = match PROFILER.get() {
        Some(p) => p.lock().unwrap(),
        None => return Ok(()),
    };

    #[derive(serde::Serialize)]
    struct Export<'a> {
        duration_ms: u64,
        draw_count: u64,
        terminal_size: (u16, u16),
        resources: Resources,
        stats: &'a HashMap<&'static str, Stats>,
    }

    let export = Export {
        duration_ms: p.start_time.elapsed().as_millis() as u64,
        draw_count: DRAW_COUNT.load(Ordering::Relaxed),
        terminal_size: p.terminal_size,
        resources: p.resources(),
        stats: &p.stats,
    };

    serde_json::to_writer_pretty(BufWriter::new(File::create(path)?), &export)?;
    Ok(())
}

// ============================================================================
// Macros
// ============================================================================

#[macro_export]
macro_rules! profile_span {
    ($name:expr) => {{
        #[cfg(feature = "profiling")]
        { $crate::profiler::begin_span($name) }
        #[cfg(not(feature = "profiling"))]
        { None::<()> }
    }};
}

#[macro_export]
macro_rules! profile_span_count {
    ($name:expr, $count:expr) => {{
        #[cfg(feature = "profiling")]
        { $crate::profiler::begin_span_count($name, $count as u64) }
        #[cfg(not(feature = "profiling"))]
        { None::<()> }
    }};
}

#[macro_export]
macro_rules! profile_frame {
    () => {{
        #[cfg(feature = "profiling")]
        { $crate::profiler::increment_frame() }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_record_single() {
        let mut stats = Stats::default();
        stats.record(100);
        assert_eq!(stats.calls, 1);
        assert_eq!(stats.total_us, 100);
        assert_eq!(stats.min_us, 100);
        assert_eq!(stats.max_us, 100);
        assert_eq!(stats.item_count, 0);
    }

    #[test]
    fn stats_record_multiple() {
        let mut stats = Stats::default();
        stats.record(100);
        stats.record(200);
        stats.record(50);
        assert_eq!(stats.calls, 3);
        assert_eq!(stats.total_us, 350);
        assert_eq!(stats.min_us, 50);
        assert_eq!(stats.max_us, 200);
    }

    #[test]
    fn stats_record_with_count() {
        let mut stats = Stats::default();
        stats.record_with_count(100, 10);
        stats.record_with_count(200, 20);
        assert_eq!(stats.calls, 2);
        assert_eq!(stats.total_us, 300);
        assert_eq!(stats.item_count, 30);
    }

    #[test]
    fn resources_empty_samples() {
        let profiler = Profiler::new((80, 24));
        let resources = profiler.resources();
        assert_eq!(resources.peak_memory_bytes, 0);
        assert_eq!(resources.final_memory_bytes, 0);
        assert_eq!(resources.avg_cpu_percent, 0.0);
        assert_eq!(resources.peak_cpu_percent, 0.0);
        assert_eq!(resources.samples, 0);
    }

    #[test]
    fn resources_with_samples() {
        let mut profiler = Profiler::new((80, 24));
        profiler.memory_samples = vec![100, 200, 150];
        profiler.cpu_samples = vec![10.0, 30.0, 20.0];
        let resources = profiler.resources();
        assert_eq!(resources.peak_memory_bytes, 200);
        assert_eq!(resources.final_memory_bytes, 150);
        assert_eq!(resources.samples, 3);
        assert!((resources.avg_cpu_percent - 20.0).abs() < f32::EPSILON);
        assert!((resources.peak_cpu_percent - 30.0).abs() < f32::EPSILON);
    }

    #[test]
    fn profiler_record_spans() {
        let mut profiler = Profiler::new((120, 40));
        profiler.record("App::draw", 500);
        profiler.record("App::draw", 600);
        profiler.record("ChatView::render", 300);

        assert_eq!(profiler.stats.len(), 2);
        let draw = &profiler.stats["App::draw"];
        assert_eq!(draw.calls, 2);
        assert_eq!(draw.total_us, 1100);
        assert_eq!(draw.min_us, 500);
        assert_eq!(draw.max_us, 600);
    }

    #[test]
    fn profiler_record_with_count() {
        let mut profiler = Profiler::new((80, 24));
        profiler.record_with_count("render_cells", 100, 500);
        profiler.record_with_count("render_cells", 150, 300);

        let stats = &profiler.stats["render_cells"];
        assert_eq!(stats.calls, 2);
        assert_eq!(stats.item_count, 800);
    }

    #[test]
    fn export_json_format() {
        let mut profiler = Profiler::new((80, 24));
        profiler.record("test_span", 1234);
        profiler.memory_samples = vec![1024];
        profiler.cpu_samples = vec![5.0];

        let dir = std::env::temp_dir();
        let path = dir.join("codey_test_profile.json");

        // We can't use export_json directly since it relies on the global PROFILER,
        // but we can test the serialization format.
        #[derive(serde::Serialize)]
        struct Export<'a> {
            duration_ms: u64,
            draw_count: u64,
            terminal_size: (u16, u16),
            resources: Resources,
            stats: &'a HashMap<&'static str, Stats>,
        }

        let export = Export {
            duration_ms: 5000,
            draw_count: 100,
            terminal_size: profiler.terminal_size,
            resources: profiler.resources(),
            stats: &profiler.stats,
        };

        let json = serde_json::to_string_pretty(&export).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["duration_ms"], 5000);
        assert_eq!(parsed["draw_count"], 100);
        assert_eq!(parsed["terminal_size"][0], 80);
        assert_eq!(parsed["terminal_size"][1], 24);
        assert_eq!(parsed["resources"]["peak_memory_bytes"], 1024);
        assert_eq!(parsed["stats"]["test_span"]["calls"], 1);
        assert_eq!(parsed["stats"]["test_span"]["total_us"], 1234);
        // item_count should be absent when zero (skip_serializing_if)
        assert!(parsed["stats"]["test_span"].get("item_count").is_none());

        // Clean up
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn export_json_includes_item_count_when_nonzero() {
        let mut profiler = Profiler::new((80, 24));
        profiler.record_with_count("render_cells", 100, 42);

        #[derive(serde::Serialize)]
        struct Export<'a> {
            stats: &'a HashMap<&'static str, Stats>,
        }

        let export = Export { stats: &profiler.stats };
        let json = serde_json::to_string(&export).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["stats"]["render_cells"]["item_count"], 42);
    }

    #[test]
    fn is_zero_helper() {
        assert!(is_zero(&0));
        assert!(!is_zero(&1));
        assert!(!is_zero(&u64::MAX));
    }
}
