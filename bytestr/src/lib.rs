#![feature(allocator_api)]
use std::borrow::Cow;
use std::{mem, ops};

#[cfg(test)]
pub mod tests;

// After making this i realized that maybe-string is a crate that exists, at least mine only has one unsafe!

enum Inner {
	String(String),
	Bytes(Vec<u8>),
}

/// Byte-vector that might be UTF-8 data, without the performace penalty of checking validity every time you need it
pub struct ByteString(Inner);

/// Holder for mutable bytes reference to ensure you don't do too much nonsense
pub struct BytesMutLock<'bs>(&'bs mut ByteString, Vec<u8>);

impl<'bs> ops::Deref for BytesMutLock<'bs> {
	type Target = Vec<u8>;
	fn deref(&self) -> &Self::Target {
		&self.1
	}
}
impl<'bs> ops::DerefMut for BytesMutLock<'bs> {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.1
	}
}
impl<'bs> ops::Drop for BytesMutLock<'bs> {
	fn drop(&mut self) {
		let value = mem::take(&mut self.1);
		self.0 .0 = match String::from_utf8(value) {
			Ok(s) => Inner::String(s),
			Err(e) => Inner::Bytes(e.into_bytes()),
		};
	}
}

impl ByteString {
	/// Create a new ByteString from existing UTF-8 data
	pub fn from_string(data: String) -> Self {
		Self(Inner::String(data))
	}
	/// Create a new ByteString from existing possibly-UTF-8 data
	pub fn from_vec(data: Vec<u8>) -> Self {
		match String::from_utf8(data) {
			Ok(s) => Self(Inner::String(s)),
			Err(e) => Self(Inner::Bytes(e.into_bytes())),
		}
	}
	/// returns true if the data is valid UTF-8, which means that the try_* functions will return a value.
	pub fn is_valid_utf8(&self) -> bool {
		matches!(&self.0, Inner::String(_))
	}
	/// Get a reference to the byte-wise data
	pub fn as_bytes(&self) -> &[u8] {
		match &self.0 {
			Inner::String(data) => data.as_bytes(),
			Inner::Bytes(data) => data,
		}
	}
	/// Get a mutable reference to the byte-wise data
	pub fn as_bytes_mut(&mut self) -> BytesMutLock {
		// replaces self.0 with an empty Vec since:
		// - BytesMutLock needs to own the data
		// - we have exclusive referencing over the data
		// - this doesn't induce any allocations
		// if you are able to access the empty vector this is a bug
		let bytes = match mem::replace(&mut self.0, Inner::Bytes(vec![])) {
			Inner::String(data) => data.into_bytes(),
			Inner::Bytes(data) => data,
		};
		BytesMutLock(self, bytes)
	}
	/// Consumes the string, returning the byte-wise data as a vector
	#[must_use = "`self` will be dropped if the result is not used"]
	pub fn into_bytes(self) -> Vec<u8> {
		match self.0 {
			Inner::String(data) => data.into_bytes(),
			Inner::Bytes(data) => data,
		}
	}
	/// Attempt to get a reference to the data as a UTF-8 string, returning None if it can't
	pub fn try_as_string(&self) -> Option<&str> {
		match &self.0 {
			Inner::String(data) => Some(data),
			Inner::Bytes(_) => None,
		}
	}
	/// Attempt to get a mutable reference to the data as a UTF-8 string, returning None if it can't
	// no lock here because you can't make a string invalid
	pub fn try_as_string_mut(&mut self) -> Option<&mut String> {
		match &mut self.0 {
			Inner::String(data) => Some(data),
			Inner::Bytes(_) => None,
		}
	}
	/// Consumes self, returning string data if it's valid UTF-8, returning None otherwise. A failure still consumes the value, use `try_into_string_fail` if you want it back on failure, or `is_valid_utf8` to check if it's valid
	#[must_use = "`self` will be dropped if the result is not used"]
	pub fn try_into_string(self) -> Option<String> {
		match self.0 {
			Inner::String(data) => Some(data),
			Inner::Bytes(_) => None,
		}
	}
	/// Consumes self, returning string data if it's valid UTF-8, and returning self again if it failed.
	#[must_use = "`self` will be dropped if the result is not used"]
	pub fn try_into_string_fail(self) -> Result<String, Self> {
		match self.0 {
			Inner::String(data) => Ok(data),
			Inner::Bytes(_) => Err(self),
		}
	}
	/// Get a reference to the data as a UTF-8 string, replacing invalid data with placeholders. Note that this will re-do the lossy conversion every time it's run, consider using `lossy_as_string_mut` if you want to store the result
	pub fn lossy_as_string(&self) -> Cow<str> {
		match &self.0 {
			Inner::String(data) => Cow::Borrowed(data),
			Inner::Bytes(data) => String::from_utf8_lossy(data),
		}
	}
	/// Get a mutable reference to the data as a UTF-8 string, replacing invalid data with placeholders before doing so.
	pub fn lossy_as_string_mut(&mut self) -> &mut String {
		if matches!(&self.0, Inner::Bytes(_)) {
			let old = match mem::replace(&mut self.0, Inner::Bytes(vec![])) {
				Inner::Bytes(v) => v,
				Inner::String(_) => unreachable!(),
			};
			let new = match String::from_utf8_lossy(&old) {
				Cow::Owned(val) => val,
				// SAFETY: if the cow is borrowed then the old data is valid
				// TODO: see if there's a way to lossily convert in a consuming manner
				Cow::Borrowed(_) => unsafe { String::from_utf8_unchecked(old) },
			};
			mem::drop(mem::replace(&mut self.0, Inner::String(new)));
		}
		if let Inner::String(data) = &mut self.0 {
			data
		} else {
			unreachable!();
		}
	}
	/// Consumes self, returning the data as a UTF-8 string, replacing invalid data with placeholders.
	#[must_use = "`self` will be dropped if the result is not used"]
	pub fn lossy_into_string(self) -> String {
		match self.0 {
			Inner::String(data) => data,
			Inner::Bytes(data) => String::from_utf8_lossy(&data).into_owned(),
		}
	}
}

impl From<String> for ByteString {
	fn from(v: String) -> Self {
		Self::from_string(v)
	}
}
impl From<Vec<u8>> for ByteString {
	fn from(v: Vec<u8>) -> Self {
		Self::from_vec(v)
	}
}
impl From<ByteString> for Vec<u8> {
	fn from(v: ByteString) -> Self {
		v.into_bytes()
	}
}
