//! Human-facing storage sizes. Persisted byte counts remain exact integers.

pub const BYTES_PER_GIB: u64 = 1 << 30;
const DEFAULT_DECIMALS: u32 = 3;
const MAX_DECIMALS: u32 = 10;

fn rounded_units(bytes: u64, decimals: u32) -> u128 {
    (u128::from(bytes) * 10_u128.pow(decimals) + u128::from(BYTES_PER_GIB / 2))
        / u128::from(BYTES_PER_GIB)
}

fn render(bytes: u64, decimals: u32) -> String {
    let scale = 10_u128.pow(decimals);
    let value = rounded_units(bytes, decimals);
    format!(
        "{}.{:0width$} GiB",
        value / scale,
        value % scale,
        width = decimals as usize
    )
}

fn preferred_decimals(bytes: u64) -> u32 {
    // Preserve short exact fractions used by hard limits (e.g. 0.0625 GiB).
    if let Some(exact) = (DEFAULT_DECIMALS..=MAX_DECIMALS).find(|decimals| {
        (u128::from(bytes) * 10_u128.pow(*decimals)).is_multiple_of(u128::from(BYTES_PER_GIB))
    }) {
        return exact;
    }
    let mut decimals = DEFAULT_DECIMALS;
    while decimals < MAX_DECIMALS && bytes != 0 && rounded_units(bytes, decimals) == 0 {
        decimals += 1;
    }
    decimals
}

/// Three decimals normally, preserving short exact fractions through ten places;
/// small positive sizes never display as zero.
/// Integer arithmetic preserves distinctions even beyond f64's integer range.
pub fn format_gib(bytes: u64) -> String {
    render(bytes, preferred_decimals(bytes))
}

/// A comparison uses equal precision and never rounds unequal sizes to equality.
pub fn format_gib_pair(required: u64, allowed: u64) -> (String, String) {
    let mut decimals = preferred_decimals(required).max(preferred_decimals(allowed));
    while decimals < MAX_DECIMALS {
        let left = rounded_units(required, decimals);
        let right = rounded_units(allowed, decimals);
        if (required == 0 || left != 0)
            && (allowed == 0 || right != 0)
            && (required == allowed || left != right)
        {
            break;
        }
        decimals += 1;
    }
    (render(required, decimals), render(allowed, decimals))
}

/// Editor initialization only. Do not round-trip an unchanged integer through f64.
pub fn bytes_to_gib(bytes: u64) -> f64 {
    bytes as f64 / BYTES_PER_GIB as f64
}

/// Convert a changed decimal GiB input to the nearest whole byte, half upward.
/// Callers retain their existing byte-domain minimum and maximum validation.
pub fn gib_to_bytes(value: f64) -> Result<u64, String> {
    if !value.is_finite() || value < 0.0 {
        return Err("Enter a finite, nonnegative GiB value".into());
    }
    let rounded = (value * BYTES_PER_GIB as f64).round();
    // u64::MAX rounds to 2^64 as f64, so reject that boundary before casting.
    if !rounded.is_finite() || rounded >= u64::MAX as f64 {
        return Err("GiB value exceeds the supported storage-size range".into());
    }
    Ok(rounded as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gib_format_is_integer_based_and_small_positive_values_stay_visible() {
        assert_eq!(format_gib(0), "0.000 GiB");
        assert_eq!(format_gib(BYTES_PER_GIB), "1.000 GiB");
        assert_eq!(format_gib(BYTES_PER_GIB / 4), "0.250 GiB");
        assert_eq!(format_gib(64 * 1024 * 1024), "0.0625 GiB");
        assert_eq!(format_gib(16 * 1024 * 1024), "0.015625 GiB");
        assert_eq!(format_gib(1024 * 1024), "0.0009765625 GiB");
        assert_eq!(format_gib(1), "0.000000001 GiB");
        assert_eq!(format_gib(u64::MAX), "17179869184.000 GiB");
    }

    #[test]
    fn gib_comparisons_preserve_unequal_exact_sizes_and_shared_precision() {
        for (a, b) in [
            (1, 2),
            (7, 8),
            (u64::MAX, u64::MAX - 1),
            (1 << 60, (1 << 60) + 1),
            (17, 18),
            (0, 1),
        ] {
            let (left, right) = format_gib_pair(a, b);
            assert_ne!(left, right, "{a} versus {b}");
            assert_eq!(
                left.split('.').nth(1).unwrap().len(),
                right.split('.').nth(1).unwrap().len()
            );
        }
        assert_eq!(format_gib_pair(123, 123).0, format_gib_pair(123, 123).1);
        assert_eq!(
            format_gib_pair(7, 8),
            ("0.0000000065 GiB".into(), "0.0000000075 GiB".into())
        );
        assert_eq!(
            format_gib_pair(1 << 30, (1 << 30) + 1),
            ("1.000000000 GiB".into(), "1.000000001 GiB".into())
        );
    }

    #[test]
    fn decimal_gib_conversion_keeps_fractional_values_and_refuses_invalid_input() {
        for bytes in [0, 1, 16 * 1024 * 1024, 64 * 1024 * 1024, 8 * BYTES_PER_GIB] {
            assert_eq!(gib_to_bytes(bytes_to_gib(bytes)).unwrap(), bytes);
        }
        assert_eq!(
            gib_to_bytes(1.5).unwrap(),
            BYTES_PER_GIB + BYTES_PER_GIB / 2
        );
        assert_eq!(gib_to_bytes(0.5 / BYTES_PER_GIB as f64).unwrap(), 1);
        for value in [
            f64::NAN,
            f64::INFINITY,
            -1.0,
            u64::MAX as f64 / BYTES_PER_GIB as f64,
        ] {
            assert!(gib_to_bytes(value).is_err());
        }
    }
}
