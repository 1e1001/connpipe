use std::time::Duration;

// todo: put other magic numbers here
// off the top of my head:
// - poll frequency
//   - actually maybe delete the poll
// - stop signal timeout

pub const MAGIC: [u8; 16] = 0x893d1d4e4e4fc6862d1e10010d0a690a_u128.to_be_bytes();
pub const SUBPORT_READ_TIMEOUT: Duration = Duration::from_secs(10);
