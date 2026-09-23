//! Cost accounting for the two phases of the protocol.
//!
//! The offline phase is measured by how long the dealer runs, how much memory
//! it needs and how large the material it stores is.  The online phase is
//! measured by wall-clock time, peak memory, and the transcript of the channel:
//! rounds, bytes sent and bytes received.

use std::fmt;
use std::fs;
use std::time::Duration;

/// What one party sent and received over the channel.
#[derive(Clone, Copy, Debug, Default)]
pub struct Transcript {
    pub rounds: usize,
    pub bytes_sent: usize,
    pub bytes_received: usize,
}

/// Cost of the dealer's run.
#[derive(Clone, Debug)]
pub struct OfflineMetrics {
    pub triples: usize,
    /// Sampling the correlated randomness.
    pub generation: Duration,
    /// Serialising it and writing both files.
    pub storage: Duration,
    pub total: Duration,
    /// Bytes written per party.
    pub file_sizes: Vec<(String, u64)>,
    pub peak_memory: Option<u64>,
}

/// Cost of one party's run.
#[derive(Clone, Debug)]
pub struct OnlineMetrics {
    pub party: u8,
    /// Reading the circuit, the preprocessing and the inputs.
    pub loading: Duration,
    /// Establishing the channel. For party 0 this includes idling until party 1
    /// shows up, so it says more about the operator than about the protocol.
    pub connecting: Duration,
    /// Input sharing, circuit evaluation and output reconstruction.
    pub protocol: Duration,
    pub total: Duration,
    pub transcript: Transcript,
    pub peak_memory: Option<u64>,
}

/// Peak resident set size of this process in bytes, on platforms that report it.
///
/// This is the high-water mark of the whole process, so it covers the circuit,
/// the preprocessing material and the runtime together.
pub fn peak_memory() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            let kb: u64 = rest.trim().trim_end_matches(" kB").trim().parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

/// Human-readable byte count.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

/// Human-readable duration, scaled to something legible.
pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs >= 1.0 {
        format!("{secs:.3} s")
    } else if secs >= 1e-3 {
        format!("{:.3} ms", secs * 1e3)
    } else {
        format!("{:.3} us", secs * 1e6)
    }
}

fn memory_line(peak: Option<u64>) -> String {
    match peak {
        Some(bytes) => format_bytes(bytes),
        None => "not available on this platform".to_string(),
    }
}

impl fmt::Display for OfflineMetrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "offline phase")?;
        writeln!(f, "  beaver triples   {}", self.triples)?;
        writeln!(f, "  generation       {}", format_duration(self.generation))?;
        writeln!(f, "  storage          {}", format_duration(self.storage))?;
        writeln!(f, "  wall clock       {}", format_duration(self.total))?;
        for (path, size) in &self.file_sizes {
            writeln!(f, "  written          {path} ({})", format_bytes(*size))?;
        }
        write!(f, "  peak memory      {}", memory_line(self.peak_memory))
    }
}

impl fmt::Display for OnlineMetrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "online phase (party {})", self.party)?;
        writeln!(f, "  loading          {}", format_duration(self.loading))?;
        writeln!(
            f,
            "  connecting       {} (includes waiting for the peer)",
            format_duration(self.connecting)
        )?;
        writeln!(f, "  protocol         {}", format_duration(self.protocol))?;
        writeln!(f, "  wall clock       {}", format_duration(self.total))?;
        writeln!(f, "  rounds           {}", self.transcript.rounds)?;
        writeln!(
            f,
            "  sent             {}",
            format_bytes(self.transcript.bytes_sent as u64)
        )?;
        writeln!(
            f,
            "  received         {}",
            format_bytes(self.transcript.bytes_received as u64)
        )?;
        write!(f, "  peak memory      {}", memory_line(self.peak_memory))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_counts_are_scaled() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.00 KiB");
        assert_eq!(format_bytes(3 * 1024 * 1024), "3.00 MiB");
    }

    #[test]
    fn durations_are_scaled() {
        assert_eq!(format_duration(Duration::from_micros(5)), "5.000 us");
        assert_eq!(format_duration(Duration::from_millis(5)), "5.000 ms");
        assert_eq!(format_duration(Duration::from_secs(5)), "5.000 s");
    }
}
