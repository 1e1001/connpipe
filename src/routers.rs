use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use log::{info, trace};
use serde::Deserialize;
use tokio::fs;

use crate::common::{CError, CResult, RouterDyn, RouterEnv};

macro_rules! define_routers {
	($($type_name:tt $string_name:tt $mod_name:tt),*$(,)?) => {
		$(mod $mod_name;)*
		#[derive(Debug, Clone, Copy)]
		#[non_exhaustive]
		pub enum RouterType {
			$($type_name,)*
		}
		pub trait RouterAuto {
			fn router_type(&self) -> RouterType;
			fn dirty_flag(&mut self) -> bool;
		}
		impl RouterType {
			pub fn from_str(s: &str) -> CResult<Self> {
				match s {
					$($string_name => Ok(RouterType::$type_name),)*
					s => Err(CError::InvalidRouterType(s.to_string()))
				}
			}
			pub fn to_str(&self) -> &'static str {
				match self {
					$(RouterType::$type_name => $string_name),*
				}
			}
			async fn create(&self, config: toml::Value, env: &RouterEnv) -> CResult<RouterDyn> {
				match self {
					$(RouterType::$type_name => Ok(Box::new($mod_name::Router::new(config.try_into()?, env).await?))),*
				}
			}
		}
		impl fmt::Display for RouterType {
			fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
				f.write_str(self.to_str())
			}
		}
		$(impl RouterAuto for $mod_name::Router {
			fn router_type(&self) -> RouterType {
				RouterType::$type_name
			}
			fn dirty_flag(&mut self) -> bool {
				let old_val = self.dirty;
				self.dirty = false;
				return old_val;
			}
		})*
	}
}

define_routers! {
	// Lua "lua" lua,
	Table "table" table,
}

#[derive(Deserialize, Debug)]
struct ConfigData {
	#[serde(rename = "type")]
	ty: String,
}

pub async fn load_config(path: &Path, env: &mut RouterEnv) -> CResult<()> {
	env.reset().await;
	info!("loading configuration...");
	// open {path}/config.toml
	let config_path = path.join("config.toml");
	trace!("reading config.toml");
	let routers: HashMap<String, toml::Value> = toml::from_slice(&fs::read(&config_path).await?)?;
	let mut router_objects = HashMap::new();
	for (id, mut config) in routers {
		trace!("loading router: {id:?}");
		let router_type = RouterType::from_str(&config.clone().try_into::<ConfigData>()?.ty)?;
		config.as_table_mut().unwrap().remove("type");
		router_objects.insert(id, router_type.create(config, env).await?);
	}
	trace!("loaded all");
	env.set_routers(router_objects);
	Ok(())
}
