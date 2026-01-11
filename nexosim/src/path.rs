//! Paths for models and endpoints identifiers.
//!
//! This module provides the [`Path`] object and related iterators that may be
//! used for the management of models and endpoints identifiers.

use std::fmt::{self, Write};
use std::iter::FusedIterator;
use std::ops::{Bound, Index, RangeBounds};
use std::vec;

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

/// A path made of named segments.
///
/// A `Path` is functionally equivalent to an immutable `Vec<String>`. It can be
/// used as a unique namespaced identifier for simulation endpoints and models.
///
/// Paths are usually created with one of the `From<T>` implementation, for
/// instance:
///
/// - from a single `String` or `&str` (single-segment path),
/// - from an array, slice or `Vec` of `String` or `&str`.
///
/// A path can also be created from an iterator over a `String` or `&str`, or
/// with [`Path::join`] and [`Path::subpath`].
///
///
/// # Naming conventions
///
/// The use of empty strings (`""`) or strings containing the `/` character is
/// discouraged due to potential confusion and ambiguities when displaying a
/// path to the user. Note in particular that the `fmt::Display` implementation
/// for `Path` displays path separators as `/`.
///
/// # Examples
///
/// ```
/// use nexosim::path::Path;
///
/// let root = Path::from("root");
/// let full_path = root.join(["foo", "bar"]);
///
/// assert_eq!(full_path, Path::from(["root", "foo", "bar"]));
/// assert_eq!(&full_path[1], "foo");
/// ```
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Path(Box<[SmolStr]>);

impl Path {
    /// Creates a new path by appending `other` to the path.
    pub fn join(&self, other: impl Into<Path>) -> Self {
        let other = other.into();
        let mut path = Vec::with_capacity(self.len() + other.len());
        path.extend(self.0.iter().cloned());
        path.extend(other.0.iter().cloned());

        Self(path.into_boxed_slice())
    }

    /// Appends a single segment to the path.
    pub fn join_segment(&self, segment: &str) -> Self {
        let mut path = Vec::with_capacity(self.len() + 1);
        path.extend(self.0.iter().cloned());
        path.push(SmolStr::new(segment));

        Self(path.into_boxed_slice())
    }

    /// Returns an iterator over the segments making up the path.
    pub fn iter(&self) -> PathIter<'_> {
        PathIter(self.0.iter())
    }

    /// Returns the number of segments.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns `true` if the path has no segments.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Gets a reference to a single segment.
    pub fn get(&self, index: usize) -> Option<&str> {
        self.0.get(index).map(|s| s.as_str())
    }

    /// Creates a new path from a range of segments.
    pub fn subpath<R>(&self, range: R) -> Option<Self>
    where
        R: RangeBounds<usize>,
    {
        let len = self.len();
        let start = match range.start_bound() {
            Bound::Included(&start) => start,
            Bound::Excluded(&start) => start.checked_add(1)?,
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&end) => end.checked_add(1)?,
            Bound::Excluded(&end) => end,
            Bound::Unbounded => len,
        };
        if start > end || end > len {
            return None;
        }

        self.0.get(start..end).map(|slice| Self(Box::from(slice)))
    }
}

impl Index<usize> for Path {
    type Output = str;

    fn index(&self, index: usize) -> &Self::Output {
        self.0[index].as_str()
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, segment) in self.iter().enumerate() {
            if i > 0 {
                f.write_char('/')?;
            }
            f.write_str(segment)?;
        }
        Ok(())
    }
}

impl fmt::Debug for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

/// An iterator over the segments of a [`Path`].
#[derive(Debug)]
pub struct PathIter<'a>(std::slice::Iter<'a, SmolStr>);

impl<'a> Iterator for PathIter<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|s| s.as_str())
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<'a> ExactSizeIterator for PathIter<'a> {}

impl<'a> FusedIterator for PathIter<'a> {}

impl<'a> DoubleEndedIterator for PathIter<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0.next_back().map(|s| s.as_str())
    }
}

impl<'a> IntoIterator for &'a Path {
    type Item = &'a str;
    type IntoIter = PathIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// An owning iterator over the segments of a [`Path`]`.
#[derive(Debug)]
pub struct PathIntoIter(vec::IntoIter<SmolStr>);

impl Iterator for PathIntoIter {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(String::from)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl ExactSizeIterator for PathIntoIter {}

impl FusedIterator for PathIntoIter {}

impl DoubleEndedIterator for PathIntoIter {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0.next_back().map(String::from)
    }
}

impl IntoIterator for Path {
    type Item = String;
    type IntoIter = PathIntoIter;

    fn into_iter(self) -> Self::IntoIter {
        PathIntoIter(self.0.into_vec().into_iter())
    }
}

impl FromIterator<String> for Path {
    fn from_iter<T: IntoIterator<Item = String>>(iter: T) -> Self {
        let path: Vec<SmolStr> = iter.into_iter().map(SmolStr::from).collect();
        Self(path.into_boxed_slice())
    }
}

impl<'a> FromIterator<&'a str> for Path {
    fn from_iter<T: IntoIterator<Item = &'a str>>(iter: T) -> Self {
        let path: Vec<SmolStr> = iter.into_iter().map(SmolStr::new).collect();
        Self(path.into_boxed_slice())
    }
}

impl From<&Path> for Path {
    fn from(p: &Path) -> Self {
        p.clone()
    }
}

impl From<String> for Path {
    fn from(s: String) -> Self {
        Self(Box::new([SmolStr::from(s)]))
    }
}

impl From<&str> for Path {
    fn from(s: &str) -> Self {
        Self(Box::new([SmolStr::new(s)]))
    }
}

impl From<&mut str> for Path {
    fn from(s: &mut str) -> Self {
        Self(Box::new([SmolStr::new(s)]))
    }
}

impl<const N: usize> From<[String; N]> for Path {
    fn from(arr: [String; N]) -> Self {
        let vec: Vec<SmolStr> = arr.into_iter().map(SmolStr::from).collect();
        Self(vec.into_boxed_slice())
    }
}

impl<const N: usize> From<[&str; N]> for Path {
    fn from(arr: [&str; N]) -> Self {
        let vec: Vec<SmolStr> = arr.iter().map(|&s| SmolStr::new(s)).collect();
        Self(vec.into_boxed_slice())
    }
}

impl<const N: usize> From<[&mut str; N]> for Path {
    fn from(arr: [&mut str; N]) -> Self {
        let vec: Vec<SmolStr> = arr.into_iter().map(SmolStr::new).collect();
        Self(vec.into_boxed_slice())
    }
}

impl From<&[String]> for Path {
    fn from(slice: &[String]) -> Self {
        let vec: Vec<SmolStr> = slice.iter().map(SmolStr::from).collect();
        Self(vec.into_boxed_slice())
    }
}

impl From<&[&str]> for Path {
    fn from(slice: &[&str]) -> Self {
        Self::from_iter(slice.iter().copied())
    }
}

impl From<&[&mut str]> for Path {
    fn from(slice: &[&mut str]) -> Self {
        slice.iter().map(|s| &**s).collect()
    }
}

impl From<Vec<String>> for Path {
    fn from(segments: Vec<String>) -> Self {
        let path: Vec<SmolStr> = segments.into_iter().map(SmolStr::from).collect();
        Self(path.into_boxed_slice())
    }
}

impl From<Vec<&str>> for Path {
    fn from(segments: Vec<&str>) -> Self {
        let path: Vec<SmolStr> = segments.into_iter().map(SmolStr::from).collect();
        Self(path.into_boxed_slice())
    }
}

impl From<Vec<&mut str>> for Path {
    fn from(segments: Vec<&mut str>) -> Self {
        let path: Vec<SmolStr> = segments.into_iter().map(SmolStr::from).collect();
        Self(path.into_boxed_slice())
    }
}
