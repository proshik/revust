use serde::{Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

// Fraction accumulation/conversion adapted from Go 1.26.4 time/format.go.
// Copyright 2010 The Go Authors. All rights reserved.
// BSD-3-Clause notice and patent grant: third_party/go/{LICENSE,PATENTS}.

/// Signed nanoseconds, including negative legacy values that config inspection
/// accepts even when they would not be useful to a running review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacyDuration(pub i64);

impl FromStr for LegacyDuration {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let invalid = || format!("invalid duration {input:?}");
        let (negative, mut rest) = match input.as_bytes().first() {
            Some(b'-') => (true, &input[1..]),
            Some(b'+') => (false, &input[1..]),
            _ => (false, input),
        };
        if rest == "0" {
            return Ok(Self(0));
        }
        if rest.is_empty() {
            return Err(invalid());
        }
        let mut total = 0_u128;
        while !rest.is_empty() {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            let whole = if digits == 0 {
                0
            } else {
                rest[..digits].parse::<u128>().map_err(|_| invalid())?
            };
            rest = &rest[digits..];
            let mut fraction = "";
            if rest.starts_with('.') {
                rest = &rest[1..];
                let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
                fraction = &rest[..digits];
                rest = &rest[digits..];
            }
            if digits == 0 && fraction.is_empty() {
                return Err(invalid());
            }
            let units = [
                ("ns", 1_u128),
                ("us", 1_000),
                ("µs", 1_000),
                ("μs", 1_000),
                ("ms", 1_000_000),
                ("s", 1_000_000_000),
                ("m", 60_000_000_000),
                ("h", 3_600_000_000_000),
            ];
            let (unit, scale) = units
                .iter()
                .find(|(unit, _)| rest.starts_with(unit))
                .ok_or_else(|| {
                    if rest.is_empty() {
                        format!("missing unit in duration {input:?}")
                    } else {
                        invalid()
                    }
                })?;
            rest = &rest[unit.len()..];
            // Preserve Go's precision cutoff and floating-point operation order;
            // exact decimal truncation can differ by a nanosecond.
            let mut numerator = 0_u64;
            let mut denominator = 1.0_f64;
            for digit in fraction.bytes() {
                if numerator > i64::MAX as u64 / 10 {
                    break;
                }
                let next = numerator * 10 + u64::from(digit - b'0');
                if next > 1_u64 << 63 {
                    break;
                }
                numerator = next;
                denominator *= 10.0;
            }
            let part = (numerator as f64 * (*scale as f64 / denominator)) as u128;
            total = whole
                .checked_mul(*scale)
                .and_then(|v| v.checked_add(part))
                .and_then(|v| total.checked_add(v))
                .ok_or_else(invalid)?;
            if total > i64::MAX as u128 + u128::from(negative) {
                return Err(invalid());
            }
        }
        let nanos = if negative {
            -(total as i128)
        } else {
            total as i128
        };
        Ok(Self(nanos as i64))
    }
}

impl fmt::Display for LegacyDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = self.0.unsigned_abs();
        if self.0 < 0 {
            write!(f, "-")?;
        }
        if value == 0 {
            return write!(f, "0s");
        }
        let decimal = |value: u64, scale: u64, places: usize| {
            let integer = value / scale;
            let remainder = value % scale;
            if remainder == 0 {
                integer.to_string()
            } else {
                format!(
                    "{integer}.{}",
                    format!("{remainder:0places$}").trim_end_matches('0')
                )
            }
        };
        if value < 1_000 {
            return write!(f, "{value}ns");
        }
        if value < 1_000_000 {
            return write!(f, "{}µs", decimal(value, 1_000, 3));
        }
        if value < 1_000_000_000 {
            return write!(f, "{}ms", decimal(value, 1_000_000, 6));
        }
        let hours = value / 3_600_000_000_000;
        let minutes = (value / 60_000_000_000) % 60;
        if hours != 0 {
            write!(f, "{hours}h")?;
        }
        if hours != 0 || minutes != 0 {
            write!(f, "{minutes}m")?;
        }
        write!(f, "{}s", decimal(value % 60_000_000_000, 1_000_000_000, 9))
    }
}

impl Serialize for LegacyDuration {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}
