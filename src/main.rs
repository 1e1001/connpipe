use std::collections::HashMap;

use tokio::io::{AsyncRead, AsyncWrite};

#[derive(thiserror::Error, Debug)]
enum CErr {
	#[error("{0}")]
	Custom(String),
}

type CRes<T> = Result<T, CErr>;

fn main() {
	println!("real");
}

macro_rules! dy {
	($tl:ty $(+ $tr:ty)*) => {
		Box<dyn $tl $(+ $tr)* +>
	}
}

struct ConnectorId(Vec<String>);

trait Address {
	fn id(&self) -> &ConnectorId;
	fn to_str(&self) -> String;
	fn to_table(&self) -> AddressTable;
}

enum AddressTableValue {
	U8(u8), U16(u16), U32(u32), U64(u64), U128(u128),
	I8(i8), I16(i16), I32(i32), I64(i64), I128(i128),
	// TODO: bigints?
	Bool(bool),
	String(String),
	List(Vec<AddressTableValue>),
	Map(HashMap<String, AddressTableValue>),
}

struct AddressTable {
	id: ConnectorId,
	parts: HashMap<String, AddressTableValue>,
}

trait Connector {
	fn id(&self) -> &ConnectorId;
	fn address_from_str(&self, s: &str) -> CRes<dy![Address]>;
	fn address_from_table(&self, t: AddressTable) -> CRes<dy![Address]>;
	fn bind(&self, addr: Dyn<Address>) -> CRes<dy![Connection]>;
}

trait Connection: AsyncRead + AsyncWrite {}

trait Router {
	fn route(&self);
}

// trait RouterConstructor
