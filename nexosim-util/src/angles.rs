//! Angle types and operations.
//!
//! This module provides strongly typed angle units (`Degrees` and `Radians`)
//! with safe conversions and arithmetic operations. It ensures that degrees
//! and radians are not accidentally mixed, while offering convenient methods
//! and operator overloads for addition, subtraction, multiplication, and division.
//! It also provides an `AngleLiteral` trait for converting numeric literals to
//! `Degrees` and `Radians` angles, and defines some useful constants like
//! `TWO_PI`, `PI_OVER_TWO` or `QUARTER_ANGLE_DEG`.
//!
//! # Examples
//!
//! ```rust
//! use nexosim_util::angles::*;
//!
//! let d = 90.0f64.deg();
//! let r = Radians::new(PI_OVER_TWO);
//!
//! let sum = d + r;                            // Degrees
//! let doubled = d * 2.0;                      // Degrees
//! let value = doubled.value();                // f64
//! let rad: Radians = (d + doubled).into();    // Radians
//! ```

pub use std::f64::consts::PI;
use std::ops::{Add, Div, Mul};

// Universal constants.
/// Two times PI.
pub const TWO_PI: f64 = 2.0 * PI;
/// PI divided by two.
pub const PI_OVER_TWO: f64 = PI / 2.0;
/// PI divided by four.
pub const PI_OVER_FOUR: f64 = PI / 4.0;
/// Full angle in degrees, 360 deg.
pub const FULL_ANGLE_DEG: f64 = 360.0;
/// Half angle in degrees, 180 deg.
pub const HALF_ANGLE_DEG: f64 = 180.0;
/// Quarter angle in degrees, 90 deg.
pub const QUARTER_ANGLE_DEG: f64 = 90.0;
/// Full angle in radians, 2 * PI.
pub const FULL_ANGLE_RAD: f64 = TWO_PI;
/// Half angle in radians, PI.
pub const HALF_ANGLE_RAD: f64 = PI;
/// Quarter angle in radians, PI / 2.
pub const QUARTER_ANGLE_RAD: f64 = PI_OVER_TWO;

/// Type for angles in degrees.
#[repr(transparent)]
#[derive(Default, Debug, Copy, Clone, PartialEq)]
pub struct Degrees {
    value: f64,
}

/// Type for angles in radians.
#[repr(transparent)]
#[derive(Default, Debug, Copy, Clone, PartialEq)]
pub struct Radians {
    value: f64,
}

impl Degrees {
    /// Creates new `Degrees` angle.
    pub fn new(value: f64) -> Self {
        Self { value }
    }

    /// Returns the value of the angle.
    pub fn value(&self) -> f64 {
        self.value
    }

    /// Converts the angle to radians.
    pub fn to_radians(self) -> Radians {
        Radians::new(self.value.to_radians())
    }

    /// Returns the absolute value of the angle.
    pub fn abs(self) -> Self {
        Self::new(self.value.abs())
    }
}

impl Radians {
    /// Creates new `Radians` angle.
    pub fn new(value: f64) -> Self {
        Self { value }
    }

    /// Returns the value of the angle.
    pub fn value(&self) -> f64 {
        self.value
    }

    /// Converts the angle to degrees.
    pub fn to_degrees(self) -> Degrees {
        Degrees::new(self.value.to_degrees())
    }

    /// Returns the absolute value of the angle.
    pub fn abs(self) -> Self {
        Self::new(self.value.abs())
    }
}

// Degrees + Degrees
impl Add for Degrees {
    type Output = Degrees;

    fn add(self, angle: Degrees) -> Degrees {
        Degrees {
            value: self.value + angle.value,
        }
    }
}

// Degrees + Radians -> Degrees
impl Add<Radians> for Degrees {
    type Output = Degrees;

    fn add(self, angle: Radians) -> Degrees {
        Degrees::new(self.value + angle.value.to_degrees())
    }
}

impl Mul<f64> for Degrees {
    type Output = Degrees;

    fn mul(self, scalar: f64) -> Degrees {
        Degrees::new(self.value * scalar)
    }
}

impl Div<f64> for Degrees {
    type Output = Degrees;

    fn div(self, scalar: f64) -> Degrees {
        Degrees::new(self.value / scalar)
    }
}

// Radians + Radians -> Radians
impl Add for Radians {
    type Output = Radians;

    fn add(self, angle: Radians) -> Radians {
        Radians::new(self.value + angle.value)
    }
}

// Radians + Degrees -> Radians
impl Add<Degrees> for Radians {
    type Output = Radians;

    fn add(self, angle: Degrees) -> Radians {
        Radians::new(self.value + angle.value.to_radians())
    }
}

impl Mul<f64> for Radians {
    type Output = Radians;

    fn mul(self, scalar: f64) -> Radians {
        Radians::new(self.value * scalar)
    }
}

impl Div<f64> for Radians {
    type Output = Radians;

    fn div(self, scalar: f64) -> Radians {
        Radians::new(self.value / scalar)
    }
}

impl From<Radians> for Degrees {
    fn from(r: Radians) -> Self {
        Self::new(r.value.to_degrees())
    }
}

impl From<Degrees> for Radians {
    fn from(d: Degrees) -> Self {
        Self::new(d.value.to_radians())
    }
}

/// Angle literals
pub trait AngleLiteral {
    /// Converts the literal to `Degrees` angle.
    fn deg(self) -> Degrees;
    /// Converts the literal to `Radians` angle.
    fn rad(self) -> Radians;
}

impl AngleLiteral for f64 {
    fn deg(self) -> Degrees {
        Degrees { value: self }
    }

    fn rad(self) -> Radians {
        Radians { value: self }
    }
}

/// Test angle addition.
#[test]
pub fn angle_addition_test() {
    // Degrees + Degrees
    let d1 = Degrees::new(10.0);
    let d2 = Degrees::new(20.0);
    let d3 = d1 + d2;
    assert_eq!(d3.value, 30.0);

    // Radians + Radians
    let r1 = Radians::new(PI);
    let r2 = Radians::new(PI);
    let r3 = r1 + r2;
    assert_eq!(r3.value, TWO_PI);

    // Degrees + Radians
    let d4 = Degrees::new(180.0);
    let r4 = Radians::new(PI);
    let d5 = d4 + r4;
    assert_eq!(d5.value, FULL_ANGLE_DEG);

    // Radians + Degrees
    let r5 = Radians::new(PI);
    let d6 = Degrees::new(180.0);
    let r6 = r5 + d6;
    assert_eq!(r6.value, TWO_PI);
}

/// Test angle literals
#[test]
pub fn angle_literals_test() {
    let d1 = 180.0.deg();
    let r1 = PI.rad();
    let d2 = d1 + r1;

    assert_eq!(d1.value, 180.0);
    assert_eq!(r1.value, PI);
    assert_eq!(d2.value, FULL_ANGLE_DEG);
}

/// Test assignment.
#[test]
pub fn angle_assignment_test() {
    let mut d1 = Degrees::new(10.0);
    assert_eq!(d1.value, 10.0);

    let r1 = Radians::new(PI);
    d1 = r1.into();
    assert_eq!(d1.value, HALF_ANGLE_DEG);

    let r2: Radians = (d1 * 2.0).into();
    assert_eq!(r2.value, TWO_PI);
}

/// Test multiplication.
#[test]
pub fn angle_multiplication_test() {
    let d1 = Degrees::new(10.0);
    let d2 = d1 * 2.0;
    assert_eq!(d2.value, 20.0);

    let r1 = Radians::new(PI);
    let r2 = r1 * 2.0;
    assert_eq!(r2.value, TWO_PI);
}

/// Test division.
#[test]
pub fn angle_division_test() {
    let d1 = Degrees::new(10.0);
    let d2 = d1 / 2.0;
    assert_eq!(d2.value, 5.0);

    let r1 = Radians::new(PI);
    let r2 = r1 / 2.0;
    assert_eq!(r2.value, PI_OVER_TWO);
}
