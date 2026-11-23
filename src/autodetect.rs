use crate::types::{LineEnding, Parity, StopBits};

pub const CANDIDATE_BAUDS: &[u32] = &[
    115200, 9600, 74880, 57600, 38400, 230400, 460800, 921600, 19200, 4800, 1200,
];

pub const CANDIDATE_FRAMINGS: &[(u8, Parity, StopBits)] = &[
    (8, Parity::None, StopBits::One),
    (7, Parity::Even, StopBits::One),
    (7, Parity::Odd, StopBits::One),
    (8, Parity::Even, StopBits::One),
    (8, Parity::Odd, StopBits::One),
    (8, Parity::None, StopBits::Two),
];

// Win32 ClearCommError flags
const CE_RXPARITY: u32 = 0x0004;
const CE_FRAME: u32 = 0x0008;
const CE_BREAK: u32 = 0x0010;

/// Scores how much a received byte sample looks like valid serial text at the active baud/framing.
/// Returns a value in roughly 0.0 ..= 1.0 (1.0 = clean readable ASCII/UTF-8 with zero framing errors).
pub fn score_serial_sample(data: &[u8], comm_errors: u32) -> f32 {
    if data.is_empty() {
        return 0.0;
    }

    let mut good_bytes = 0usize;
    let mut alnum_bytes = 0usize;
    let mut bad_bytes = 0usize;
    let mut nul_or_ff = 0usize;
    let mut line_breaks = 0usize;

    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        match b {
            0x20..=0x7E => {
                good_bytes += 1;
                if b.is_ascii_alphanumeric() {
                    alnum_bytes += 1;
                }
                i += 1;
            }
            b'\r' | b'\n' => {
                good_bytes += 1;
                line_breaks += 1;
                i += 1;
            }
            b'\t' | 0x08 | 0x0C => {
                good_bytes += 1;
                i += 1;
            }
            // ANSI color escape sequence (very common on ESP-IDF logs: ESC [ ... m)
            0x1B if i + 1 < data.len() && data[i + 1] == b'[' => {
                good_bytes += 2;
                i += 2;
            }
            0x00 | 0xFF => {
                nul_or_ff += 1;
                bad_bytes += 1;
                i += 1;
            }
            _ if b >= 0xC2 => {
                // Check valid multi-byte UTF-8 sequence
                let seq_len = if b < 0xE0 {
                    2
                } else if b < 0xF0 {
                    3
                } else if b < 0xF5 {
                    4
                } else {
                    0
                };
                if seq_len >= 2
                    && i + seq_len <= data.len()
                    && std::str::from_utf8(&data[i..i + seq_len]).is_ok()
                {
                    good_bytes += seq_len;
                    i += seq_len;
                } else {
                    bad_bytes += 1;
                    i += 1;
                }
            }
            _ => {
                bad_bytes += 1;
                i += 1;
            }
        }
    }

    let total = data.len() as f32;
    let mut score = good_bytes as f32 / total;

    // Slight bonus for alphanumeric content and line breaks (real logs have words & newlines)
    if alnum_bytes > 0 {
        score += 0.04 * (alnum_bytes as f32 / total);
    }
    if line_breaks > 0 {
        score += 0.03;
    }

    // Strong penalties for framing/parity hardware errors and garbage patterns
    if (comm_errors & CE_FRAME) != 0 {
        score -= 0.45;
    }
    if (comm_errors & CE_RXPARITY) != 0 {
        score -= 0.35;
    }
    if (comm_errors & CE_BREAK) != 0 {
        score -= 0.25;
    }
    if nul_or_ff > 0 {
        score -= 0.40 * (nul_or_ff as f32 / total);
    }
    if bad_bytes > 0 {
        score -= 0.15 * (bad_bytes as f32 / total);
    }

    score.clamp(0.0, 1.0)
}

/// Detect if bytes read under 8N1 are actually 7E1 or 7O1 ASCII data.
pub fn detect_7bit_parity(data: &[u8]) -> Option<(u8, Parity)> {
    if data.len() < 12 {
        return None;
    }
    let mut high_bit_count = 0usize;
    let mut clean_7bit_count = 0usize;
    let mut even_parity_ok = 0usize;
    let mut odd_parity_ok = 0usize;

    for &b in data {
        if (b & 0x80) != 0 {
            high_bit_count += 1;
        }
        let low = b & 0x7F;
        if (0x20..=0x7E).contains(&low) || matches!(low, b'\r' | b'\n' | b'\t') {
            clean_7bit_count += 1;
        }
        if b.count_ones() % 2 == 0 {
            even_parity_ok += 1;
        } else {
            odd_parity_ok += 1;
        }
    }

    let total = data.len();
    // Need at least a few high-bit bytes (otherwise it's already plain 8N1 ASCII)
    if high_bit_count * 10 < total {
        return None;
    }
    if clean_7bit_count * 100 >= total * 92 {
        if even_parity_ok * 100 >= total * 95 {
            return Some((7, Parity::Even));
        }
        if odd_parity_ok * 100 >= total * 95 {
            return Some((7, Parity::Odd));
        }
    }
    None
}

/// Tracks incoming line endings across byte chunks and determines the dominant EOL.
#[derive(Debug, Default, Clone)]
pub struct LineEndingDetector {
    prev_cr: bool,
    crlf_count: u32,
    lf_count: u32,
    cr_count: u32,
}

impl LineEndingDetector {
    pub fn reset(&mut self) {
        self.prev_cr = false;
        self.crlf_count = 0;
        self.lf_count = 0;
        self.cr_count = 0;
    }

    pub fn feed(&mut self, data: &[u8]) {
        for &b in data {
            if self.prev_cr {
                if b == b'\n' {
                    self.crlf_count = self.crlf_count.saturating_add(1);
                    self.prev_cr = false;
                    continue;
                } else {
                    self.cr_count = self.cr_count.saturating_add(1);
                    self.prev_cr = false;
                }
            }
            if b == b'\r' {
                self.prev_cr = true;
            } else if b == b'\n' {
                self.lf_count = self.lf_count.saturating_add(1);
            }
        }
    }

    pub fn detected(&self) -> LineEnding {
        if self.crlf_count == 0 && self.lf_count == 0 && self.cr_count == 0 {
            return LineEnding::None;
        }
        if self.crlf_count >= self.lf_count && self.crlf_count >= self.cr_count {
            LineEnding::CrLf
        } else if self.lf_count >= self.cr_count {
            LineEnding::Lf
        } else {
            LineEnding::Cr
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_score_clean_ascii() {
        let s = score_serial_sample(b"ESP-ROM:esp32s3-20210327\r\nBuild:Mar 27 2021\r\n", 0);
        assert!(s > 0.95, "score was {}", s);
    }

    #[test]
    fn test_score_garbage_with_framing_error() {
        let s = score_serial_sample(&[0x00, 0xFE, 0x80, 0xFF, 0x13, 0x00, 0xE2], CE_FRAME);
        assert!(s < 0.2, "score was {}", s);
    }

    #[test]
    fn test_detect_7e1() {
        let msg = b"Hello ESP32 Serial 7E1!\r\n";
        let encoded: Vec<u8> = msg
            .iter()
            .map(|&b| {
                let p = if b.count_ones() % 2 == 1 { 0x80 } else { 0 };
                b | p
            })
            .collect();
        assert_eq!(detect_7bit_parity(&encoded), Some((7, Parity::Even)));
    }

    #[test]
    fn test_eol_detector() {
        let mut d = LineEndingDetector::default();
        d.feed(b"line1\r");
        d.feed(b"\nline2\r\n");
        assert_eq!(d.detected(), LineEnding::CrLf);
    }
}
