use async_trait::async_trait;
use log::debug;
use mlua::Lua;
use serde::Deserialize;

use crate::common::{
	Address, AddressSubport, CResult, RouteResult, RouterCommon, RouterConfig, RouterInst,
};

#[derive(Deserialize, Debug)]
pub struct Config {
	main: String,
}

pub struct Router {
	common: RouterCommon,
	lua: Lua,
}

impl Router {
	pub async fn new(config: Config) -> CResult<Self> {
		debug!("{:?}", config);
		todo!();
	}
}

#[async_trait(?Send)]
impl RouterInst for Router {
	async fn route(&mut self, addr: AddressSubport) -> CResult<RouteResult> {
		todo!();
	}
	async fn listen_addresses(&mut self) -> Vec<Address> {
		todo!()
	}
	async fn is_dirty(&mut self) -> bool {
		self.common.is_dirty()
	}
	async fn get_config(&mut self) -> RouterConfig {
		self.common.config
	}
}
