use std::collections::{HashMap, HashSet};

use async_trait::async_trait;

use crate::common::{
	Address, AddressPartial, AddressSubport, CResult, RouterEnv, RouterInst,
};

pub type Config = HashMap<String, String>;

#[derive(Clone, Debug)]
enum TableEntry {
	Address(AddressPartial),
	Router(String, AddressPartial),
}

#[derive(Debug)]
pub struct Router {
	pub dirty: bool,
	data: HashMap<AddressSubport, TableEntry>,
}

impl Router {
	pub async fn new(config: Config, env: &RouterEnv) -> CResult<Self> {
		let mut data = HashMap::new();
		for (addr, entry) in config {
			let addr = AddressSubport::from_str(&addr)?;
			data.insert(
				addr,
				if entry.as_bytes()[0] == b'>' {
					let mut iter = entry.rsplitn(2, ' ');
					let addr = AddressPartial::from_str(&iter.next().unwrap())?;
					let host = iter.next().ok_or_else(|| {
						env.router_err(format!("invalid entry {entry} expected \">[router] [addr]\""))
					})?;
					TableEntry::Router(host[1..].to_owned(), addr)
				} else {
					TableEntry::Address(AddressPartial::from_str(&entry)?)
				},
			);
		}
		Ok(Self {
			dirty: false,
			data,
		})
	}
}

#[async_trait(?Send)]
impl RouterInst for Router {
	async fn route(
		&self,
		addr: AddressSubport,
		env: &RouterEnv,
	) -> CResult<Option<AddressSubport>> {
		Ok(match self.data.get(&addr).cloned() {
			Some(TableEntry::Address(part)) => Some(addr.with(part)),
			Some(TableEntry::Router(id, part)) => env.route(&id, addr.with(part)).await?,
			None => None,
		})
	}
	async fn listen_addresses(&self) -> CResult<Vec<Address>> {
		let mut out = HashSet::new();
		for addr in self.data.keys() {
			out.insert(addr.clone().pure_part());
		}
		Ok(out.into_iter().collect())
	}
}
