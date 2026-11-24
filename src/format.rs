//! Number and progress-bar formatting shared by the CLI and the terminal UI.

/// `4.6 MB`, `812 KB`, `0 B`. Uses 1024-based units, like Windows Explorer.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `3.2 MB/s`.
pub fn rate(bytes_per_sec: f64) -> String {
    format!("{}/s", bytes(bytes_per_sec.max(0.0) as u64))
}

/// `4:58` or `1:02:03`.
pub fn duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// `[██████░░░░]` for `fraction` in 0..=1, `width` cells wide (not counting the brackets).
pub fn bar(fraction: f64, width: usize) -> String {
    let filled = (fraction.clamp(0.0, 1.0) * width as f64).round() as usize;
    format!("[{}{}]", "█".repeat(filled), "░".repeat(width - filled))
}

/// `44.1 kHz` from a sample rate in Hz.
pub fn khz(hz: u32) -> String {
    let k = f64::from(hz) / 1000.0;
    if (k - k.round()).abs() < 0.05 {
        format!("{} kHz", k.round() as u32)
    } else {
        format!("{k:.1} kHz")
    }
}

/// `mono`, `stereo`, or `N channels`.
pub fn channels(n: u32) -> String {
    match n {
        1 => "mono".into(),
        2 => "stereo".into(),
        n => format!("{n} channels"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(4_823_449), "4.6 MB");
    }

    #[test]
    fn formats_durations_and_rates() {
        assert_eq!(duration(298), "4:58");
        assert_eq!(duration(3723), "1:02:03");
        assert_eq!(rate(3_355_443.0), "3.2 MB/s");
    }

    #[test]
    fn draws_bars() {
        assert_eq!(bar(0.5, 4), "[██░░]");
        assert_eq!(bar(0.0, 3), "[░░░]");
        assert_eq!(bar(2.0, 3), "[███]");
    }

    #[test]
    fn formats_audio_details() {
        assert_eq!(khz(44100), "44.1 kHz");
        assert_eq!(khz(48000), "48 kHz");
        assert_eq!(channels(2), "stereo");
        assert_eq!(channels(6), "6 channels");
    }
}
