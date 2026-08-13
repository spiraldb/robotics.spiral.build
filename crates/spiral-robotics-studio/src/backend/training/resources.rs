//! Low-frequency host resource sampling for training diagnostics.

use std::time::{Duration, Instant};

const GPU_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ResourceUsage {
    /// Studio process CPU usage; 100% is one fully occupied core.
    pub cpu_percent: Option<f64>,
    /// Whole-device GPU usage reported by macOS, not process attribution.
    pub gpu_percent: Option<f64>,
    /// Current resident memory owned by the Studio process.
    pub memory_bytes: Option<u64>,
}

pub(crate) struct ResourceSampler {
    last_sample: Instant,
    last_cpu_seconds: Option<f64>,
    sample_gpu: bool,
    last_gpu_sample: Option<Instant>,
    last_gpu_percent: Option<f64>,
}

impl ResourceSampler {
    pub(crate) fn new(sample_gpu: bool) -> Self {
        Self {
            last_sample: Instant::now(),
            last_cpu_seconds: process_cpu_seconds(),
            sample_gpu,
            last_gpu_sample: None,
            last_gpu_percent: None,
        }
    }

    pub(crate) fn sample(&mut self) -> ResourceUsage {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_sample).as_secs_f64();
        let cpu_seconds = process_cpu_seconds();
        let cpu_percent = self
            .last_cpu_seconds
            .zip(cpu_seconds)
            .filter(|_| elapsed > 0.0)
            .map(|(previous, current)| ((current - previous).max(0.0) / elapsed) * 100.0)
            .filter(|value| value.is_finite());
        self.last_sample = now;
        self.last_cpu_seconds = cpu_seconds;

        if self.sample_gpu
            && self
                .last_gpu_sample
                .is_none_or(|sampled| now.duration_since(sampled) >= GPU_SAMPLE_INTERVAL)
        {
            self.last_gpu_percent = apple_gpu_percent();
            self.last_gpu_sample = Some(now);
        }

        ResourceUsage {
            cpu_percent,
            gpu_percent: self.last_gpu_percent,
            memory_bytes: resident_memory_bytes(),
        }
    }
}

#[cfg(unix)]
fn process_cpu_seconds() -> Option<f64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: `getrusage` initializes the complete `rusage` value on success.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: the successful call above initialized `usage`.
    let usage = unsafe { usage.assume_init() };
    Some(timeval_seconds(usage.ru_utime) + timeval_seconds(usage.ru_stime))
}

#[cfg(unix)]
fn timeval_seconds(value: libc::timeval) -> f64 {
    value.tv_sec as f64 + value.tv_usec as f64 / 1_000_000.0
}

#[cfg(not(unix))]
fn process_cpu_seconds() -> Option<f64> {
    None
}

#[cfg(target_os = "macos")]
fn resident_memory_bytes() -> Option<u64> {
    use mach2::kern_return::KERN_SUCCESS;
    use mach2::task::task_info;
    use mach2::task_info::{
        MACH_TASK_BASIC_INFO, MACH_TASK_BASIC_INFO_COUNT, mach_task_basic_info,
    };
    use mach2::traps::mach_task_self;

    let mut info = mach_task_basic_info::default();
    let mut count = MACH_TASK_BASIC_INFO_COUNT;
    // SAFETY: `info` is writable for exactly `count` natural words and the
    // task port refers to this process.
    let status = unsafe {
        task_info(
            mach_task_self(),
            MACH_TASK_BASIC_INFO,
            (&mut info as *mut mach_task_basic_info).cast(),
            &mut count,
        )
    };
    (status == KERN_SUCCESS).then_some(info.resident_size)
}

#[cfg(target_os = "linux")]
fn resident_memory_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages = statm.split_whitespace().nth(1)?.parse::<u64>().ok()?;
    // SAFETY: `sysconf` is read-only and has no pointer arguments.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    (page_size > 0)
        .then(|| resident_pages.checked_mul(page_size as u64))
        .flatten()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn resident_memory_bytes() -> Option<u64> {
    None
}

#[cfg(target_os = "macos")]
fn apple_gpu_percent() -> Option<f64> {
    ["AGXAccelerator", "IOAccelerator"]
        .into_iter()
        .find_map(|class| {
            let output = std::process::Command::new("/usr/sbin/ioreg")
                .args(["-r", "-d", "1", "-w", "0", "-c", class])
                .output()
                .ok()?;
            output
                .status
                .success()
                .then(|| parse_apple_gpu_percent(&output.stdout))
                .flatten()
        })
}

#[cfg(not(target_os = "macos"))]
fn apple_gpu_percent() -> Option<f64> {
    None
}

#[cfg(target_os = "macos")]
fn parse_apple_gpu_percent(output: &[u8]) -> Option<f64> {
    const KEY: &str = "\"Device Utilization %\"=";
    let output = std::str::from_utf8(output).ok()?;
    let value = output.split_once(KEY)?.1;
    let digits = value
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    digits
        .parse::<f64>()
        .ok()
        .filter(|value| (0.0..=100.0).contains(value))
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    #[test]
    fn parses_apple_gpu_device_utilization() {
        let output =
            br#""PerformanceStatistics" = {"Renderer Utilization %"=74,"Device Utilization %"=68}"#;
        assert_eq!(super::parse_apple_gpu_percent(output), Some(68.0));
        assert_eq!(super::parse_apple_gpu_percent(b"no statistics"), None);
    }
}
