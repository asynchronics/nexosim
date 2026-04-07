//! Lowpass filter.
//!
//! This module provides a simple first-order lowpass filter (alpha filter)
//! with constructors based on either a cutoff frequency or a direct alpha
//! coefficient.
//!
//! # Examples
//!
//! ```rust
//! use nexosim_util::lowpass::LowPassFilter;
//!
//! // 5 Hz cutoff, 100 Hz sample rate.
//! let mut filter = LowPassFilter::from_frequency(5.0, 100.0);
//!
//! let y0 = filter.update(1.0);
//! let y1 = filter.update(1.0);
//!
//! assert!(y1 >= y0);
//! assert!(y1 <= 1.0);
//! ```

use serde::{Deserialize, Serialize};

/// First order lowpass filter, "alpha filter"
#[derive(Debug, Serialize, Deserialize)]
pub struct LowPassFilter {
    /// Filter coefficient, 0 -> no update, 1 -> passthrough
    alpha: f64,
    /// Current filtered state
    value: f64,
}

impl LowPassFilter {
    /// Create a lowpass filter from cutoff frequency and sample rate
    #[inline]
    pub fn from_frequency(cutoff_hz: f64, sample_rate: f64) -> Self {
        // Avoid division by zero
        if sample_rate.abs() < 1e-9 {
            return LowPassFilter::from_alpha(0.0);
        }

        let dt = 1.0 / sample_rate;
        let rc = 1.0 / (2.0 * std::f64::consts::PI * cutoff_hz);
        let alpha = dt / (rc + dt);

        let clamped_alpha = alpha.clamp(0.0, 1.0);
        Self {
            alpha: clamped_alpha,
            value: 0.0,
        }
    }

    /// Create lowpass filter from alpha coefficient
    #[inline]
    pub fn from_alpha(filter_alpha: f64) -> Self {
        let clamped_alpha = filter_alpha.clamp(0.0, 1.0);
        Self {
            alpha: clamped_alpha,
            value: 0.0,
        }
    }

    /// Update lowpass with new sample
    #[inline]
    pub fn update(&mut self, input: f64) -> f64 {
        let new_value = self.value * (1.0 - self.alpha) + self.alpha * input;
        self.value = new_value;
        self.value
    }

    /// Get current filtered value
    #[inline]
    pub fn get_value(&self) -> f64 {
        self.value
    }

    /// Get alpha coefficient
    #[inline]
    pub fn get_alpha(&self) -> f64 {
        self.alpha
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const KINDA_SMALL_NUMBER: f64 = 1e-4;

    #[test]
    fn convergence_test() {
        let mut filter = LowPassFilter::from_alpha(0.1);
        for _ in 0..1000 {
            filter.update(3.0);
        }
        // Should converge near 3.0

        assert!((filter.get_value() - 3.0).abs() < KINDA_SMALL_NUMBER);
    }

    #[test]
    fn alpha_edge_cases() {
        let mut f = LowPassFilter::from_alpha(0.0);
        assert_eq!(f.update(10.0), 0.0); // no change

        let mut f = LowPassFilter::from_alpha(1.0);
        assert_eq!(f.update(10.0), 10.0); // passthrough
    }

    #[test]
    fn from_frequency_computes_alpha() {
        let cutoff_hz = 5.0;
        let sample_rate = 100.0;
        let filter = LowPassFilter::from_frequency(cutoff_hz, sample_rate);

        let dt = 1.0 / sample_rate;
        let rc = 1.0 / (2.0 * std::f64::consts::PI * cutoff_hz);
        let expected_alpha = dt / (rc + dt);

        assert!((filter.get_alpha() - expected_alpha).abs() < KINDA_SMALL_NUMBER);
    }

    #[test]
    fn from_frequency_clamps_alpha_for_edge_cases() {
        let cases = [(0.0, 100.0), (-10.0, 100.0), (10.0, 0.0), (10.0, -100.0)];

        for (cutoff_hz, sample_rate) in cases {
            let filter = LowPassFilter::from_frequency(cutoff_hz, sample_rate);
            let alpha = filter.get_alpha();
            assert!(
                (0.0..=1.0).contains(&alpha),
                "alpha out of range for cutoff {cutoff_hz}, sample_rate {sample_rate}"
            );
        }
    }

    #[test]
    fn from_frequency_monotonic_in_cutoff() {
        let sample_rate = 100.0;
        let low = LowPassFilter::from_frequency(1.0, sample_rate);
        let high = LowPassFilter::from_frequency(10.0, sample_rate);
        assert!(high.get_alpha() > low.get_alpha());
    }

    #[test]
    fn step_response_moves_toward_input_without_overshoot() {
        let mut filter = LowPassFilter::from_alpha(0.25);
        let mut last = filter.get_value();
        for _ in 0..50 {
            let value = filter.update(1.0);
            assert!(value >= last, "response should be non-decreasing");
            assert!(value <= 1.0, "response should not overshoot input");
            last = value;
        }
    }

    #[test]
    fn from_frequency_extremes_approximate_alpha_bounds() {
        let sample_rate = 100.0;
        let near_zero = LowPassFilter::from_frequency(1e-9, sample_rate);
        let near_one = LowPassFilter::from_frequency(1e9, sample_rate);

        assert!(near_zero.get_alpha() < KINDA_SMALL_NUMBER);
        assert!((1.0 - near_one.get_alpha()) < KINDA_SMALL_NUMBER);
    }
}
