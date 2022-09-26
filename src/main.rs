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
	(& $($lt:lifetime)? mut $($tt:tt)+) => {
		&$($lt)? mut dyn $($tt)+
	};
	(& $($lt:lifetime)? $($tt:tt)+) => {
		&$($lt)? dyn $($tt)+
	};
	($($tt:tt)+) => {
		Box<dyn $($tt)+ +>
	};
}

struct ConnectorId(Vec<String>);

trait Address {
	fn conn(&self) -> dy![&Connector];
	fn id(&self) -> &ConnectorId;
	fn to_str(&self) -> String;
	fn to_table(&self) -> AddressTable;
	fn to_pure(&self) -> dy![Address] {
		self.conn().address_from_table(self.to_table().into_pure()).unwrao()
	}
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
	pure_parts: HashMap<String, AddressTableValue>,
	impure_parts: HashMap<String, AddressTableValue>,
}

impl AddressTable {
	fn into_pure(self) -> Self {
		Self {
			impure_parts: HashMap::new(),
			..self
		}
	}
}

trait Connector {
	fn id(&self) -> &ConnectorId;
	fn address_from_str(&self, s: &str) -> CRes<dy![Address]>;
	fn address_from_table(&self, t: AddressTable) -> CRes<dy![Address]>;
	fn bind(&self, addr: dy![Address]) -> CRes<dy![Connection]>;
}

trait Connection: AsyncRead + AsyncWrite {}

trait RouterConstructor {
	fn id(&self) -> &ConnectorId;
	fn new(&self /*constructor arguments*/) -> CRes<dy![Router]>;
}

trait Router {
	fn id(&self) -> &str;
	fn route(&self);
	fn get_addresses(&self) -> dy![Iterator<Item=dy![Address]>];
}

trait Plugin {

}
