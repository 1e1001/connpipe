use crate::ByteString;

#[test]
fn string_data() {
	let st = ByteString::from("test 💖 test".to_string());
	assert!(st.is_valid_utf8());
	assert_eq!(st.as_bytes(), b"test \xf0\x9f\x92\x96 test");
	assert_eq!(st.try_as_string(), Some("test 💖 test"));
	assert_eq!(st.lossy_as_string(), "test 💖 test");
	assert_eq!(st.try_into_string(), Some("test 💖 test".to_string()));
}

#[test]
fn byte_data() {
	let st = ByteString::from(b"test \x00\xff test".to_vec());
	assert!(!st.is_valid_utf8());
	assert_eq!(st.as_bytes(), b"test \x00\xff test");
	assert_eq!(st.try_as_string(), None);
	assert_eq!(st.lossy_as_string(), "test \0\u{fffd} test");
	assert_eq!(st.into_bytes(), b"test \x00\xff test");
}

#[test]
fn modify_validity() {
	let mut st = ByteString::from("test 💖".to_string());
	assert_eq!(st.try_as_string(), Some("test 💖"));
	{
		let mut data = st.as_bytes_mut();
		data.extend_from_slice(b" test");
	}
	assert_eq!(st.try_as_string(), Some("test 💖 test"));
	{
		let mut data = st.as_bytes_mut();
		data[5..9].fill(0xff);
	}
	assert_eq!(st.try_as_string(), None);
	assert_eq!(st.as_bytes(), b"test \xff\xff\xff\xff test");
	assert_eq!(
		st.lossy_as_string(),
		"test \u{fffd}\u{fffd}\u{fffd}\u{fffd} test"
	);
	{
		let mut data = st.as_bytes_mut();
		(data[5], data[6], data[7], data[8]) = (0xf0, 0x9f, 0x92, 0x96);
	}
	assert_eq!(st.try_as_string(), Some("test 💖 test"));
	{
		let mut data = st.as_bytes_mut();
		data.push(0xff);
	}
	assert_eq!(st.try_as_string(), None);
	assert_eq!(st.lossy_as_string(), "test 💖 test\u{fffd}");
	assert_eq!(st.try_as_string_mut(), None);
	{
		let data = st.lossy_as_string_mut();
		data.pop();
	}
	assert_eq!(st.try_as_string(), Some("test 💖 test"));
	{
		let data = st.lossy_as_string_mut();
		for _ in 0..5 {
			data.pop();
		}
	}
	assert_eq!(st.try_as_string(), Some("test 💖"));
}

#[test]
fn forget_test() {
	use std::mem;
	let mut st = ByteString::from("test 💖".to_string());
	{
		let data = st.as_bytes_mut();
		mem::forget(data);
	}
	assert_eq!(st.as_bytes(), []);
}

#[test]
fn impl_test() {
	let valid = ByteString::from("test 💖\n test".to_string());
	let invalid = ByteString::from(b"test \xff\x00 \xf0\x9f\x92\x96 test".to_vec());
	assert_eq!(format!("{valid}"), "test 💖\n test");
	assert_eq!(format!("{valid:?}"), "\"test 💖\\n test\"");
	assert_eq!(format!("{invalid}"), "test \u{fffd}\0 💖 test");
	assert_eq!(format!("{invalid:?}"), "\"test \\i{ff}\\0 💖 test\"");
}
