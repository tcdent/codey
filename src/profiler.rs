//! Lightweight performance profiler for timing and resource tracking.
//!
//! Usage:
//! ```rust,ignore
//! let _span = profile_span!("render_chat");
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
}

impl Stats {
    fn record(&mut self, duration_us: u64) {
        self.calls += 1;
        self.total_us += duration_us;
        self.min_us = if self.min_us == 0 { duration_us } else { self.min_us.min(duration_us) };
        self.max_us = self.max_us.max(duration_us);
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
macro_rules! profile_frame {
    () => {{
        #[cfg(feature = "profiling")]
        { $crate::profiler::increment_frame() }
    }};
}
