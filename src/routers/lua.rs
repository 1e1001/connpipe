use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CString;
use std::fs;
use std::path::PathBuf;

use async_trait::async_trait;
use log::{info, trace};
use mlua::{
	chunk, AsChunk, FromLua, Function, Integer, Lua, LuaOptions, StdLib, ToLua, UserData,
	UserDataMethods, Value,
};

use crate::server::{parse_wildcard, Address, CResult, ResolveResult, Router, RouterConfig};

fn resolve_result_from_lua<'lua>(
	lua_value: Value<'lua>,
	lua: &'lua Lua,
) -> CResult<Option<ResolveResult>> {
	match lua_value {
		Value::Nil => Ok(None),
		Value::Table(table) => {
			let vec: Result<Vec<Value>, _> = table.sequence_values().collect();
			match &vec?[..] {
				[addr, port, subport] => Ok(Some(ResolveResult {
					addr: String::from_lua(addr.clone(), lua)?,
					port: u16::from_lua(port.clone(), lua)?,
					subport: Some(String::from_lua(subport.clone(), lua)?),
				})),
				[addr, port] => Ok(Some(ResolveResult {
					addr: String::from_lua(addr.clone(), lua)?,
					port: u16::from_lua(port.clone(), lua)?,
					subport: None,
				})),
				vec => Err(mlua::Error::FromLuaConversionError {
					from: "table",
					to: "ResolveResult",
					message: Some(format!("expected length of 2 or 3, got {}", vec.len())),
				}
				.into()),
			}
		}
		_ => Err(mlua::Error::FromLuaConversionError {
			from: lua_value.type_name(),
			to: "ResolveResult",
			message: Some("expected list".into()),
		}
		.into()),
	}
}

#[derive(Debug, Clone)]
struct RegistryInner {
	specific: HashMap<Option<String>, Integer>,
	generic: Option<Integer>,
}

#[derive(Debug, Clone)]
struct Registry {
	items: RefCell<HashMap<Option<String>, HashMap<u16, RegistryInner>>>,
}

impl UserData for Registry {
	fn add_methods<'lua, M: UserDataMethods<'lua, Self>>(methods: &mut M) {
		methods.add_method(
			"lookup",
			|lua, this, (addr, port, subport): (String, u16, String)| match (|| {
				let items = this.items.borrow();
				let inner = match items.get(&parse_wildcard(addr)) {
					Some(v) => Some(v),
					None => items.get(&None),
				}?
				.get(&port)?;
				match inner.specific.get(&parse_wildcard(subport)) {
					Some(v) => Some(*v),
					None => Some(inner.generic?),
				}
			})() {
				Some(id) => id.to_lua(lua),
				None => Ok(Value::Nil),
			},
		);
		methods.add_method(
			"add_specific",
			|_lua,
			 this,
			 (addr, port, subport, callback): (Vec<String>, Vec<u16>, Vec<String>, Integer)| {
				let mut items = this.items.borrow_mut();
				for addr in addr.iter() {
					for port in port.iter() {
						for subport in subport.iter() {
							items
								.entry(parse_wildcard(addr.clone()))
								.or_insert_with(HashMap::new)
								.entry(*port)
								.or_insert_with(|| RegistryInner {
									specific: HashMap::new(),
									generic: None,
								})
								.specific
								.insert(parse_wildcard(subport.clone()), callback);
						}
					}
				}
				Ok(Value::Nil)
			},
		);
		methods.add_method(
			"add_generic",
			|_lua, this, (addr, port, callback): (Vec<String>, Vec<u16>, Integer)| {
				let mut items = this.items.borrow_mut();
				for addr in addr.iter() {
					for port in port.iter() {
						items
							.entry(parse_wildcard(addr.clone()))
							.or_insert_with(HashMap::new)
							.entry(*port)
							.or_insert_with(|| RegistryInner {
								specific: HashMap::new(),
								generic: None,
							})
							.generic = Some(callback);
					}
				}
				Ok(Value::Nil)
			},
		);
	}
}

const G_LOAD_FILE: u8 = 1;
const G_RESOLVE_PATH: u8 = 2;
const G_REGISTRY: u8 = 3;
const G_CALLBACKS: u8 = 4;
const G_CALLBACKS_REVERSE: u8 = 5;
const G_RESOLVE: u8 = 6;
const G_FINISH: u8 = 7;
const G_REQUIRE_CACHE: u8 = 8;

struct LuaFile {
	path: PathBuf,
	data: Vec<u8>,
}

impl LuaFile {
	fn new(path: impl Into<PathBuf>) -> CResult<Self> {
		let path = path.into();
		let data = fs::read(&path)?;
		Ok(Self { path, data })
	}
}

impl<'lua> AsChunk<'lua> for LuaFile {
	fn source(&self) -> &[u8] {
		&self.data
	}
	fn name(&self) -> Option<CString> {
		// TODO: some smarter logic here?
		Some(CString::new(self.path.as_os_str().to_string_lossy().as_bytes()).unwrap())
	}
}

pub struct LuaRouter {
	lua: Lua,
	addresses: Vec<Address>,
	config: RouterConfig,
}

impl LuaRouter {
	pub async fn new(path: impl Into<PathBuf>) -> CResult<Self> {
		info!("[lua] initializing");
		let lua = Lua::new_with(
			StdLib::MATH | StdLib::STRING | StdLib::TABLE | StdLib::COROUTINE | StdLib::UTF8,
			LuaOptions::new(),
		)?;
		{
			let globals = lua.globals();
			let internals = lua.create_table()?;
			async fn load_file(lua: &Lua, path: String) -> mlua::Result<Value> {
				lua.load(&match LuaFile::new(path) {
					Ok(v) => v,
					Err(e) => return Err(mlua::Error::ExternalError(std::sync::Arc::new(e))),
				})
				.eval_async()
				.await
			}
			internals.set(G_LOAD_FILE, lua.create_async_function(load_file)?)?;
			async fn resolve_path(
				lua: &Lua,
				(module_path, path): (String, String),
			) -> mlua::Result<Value> {
				let mut module_path = PathBuf::from(module_path);
				module_path.pop();
				module_path.push(path);
				Ok(Value::String(lua.create_string(
					module_path.as_os_str().to_string_lossy().as_bytes(),
				)?))
			}
			internals.set(G_RESOLVE_PATH, lua.create_async_function(resolve_path)?)?;
			internals.set(
				G_REGISTRY,
				Registry {
					items: RefCell::new(HashMap::new()),
				},
			)?;
			globals.set("internal", internals)?;
			lua.load(chunk! {
				local int = internal;
				internal = function()
					internal = nil;
					return function()
						return int;
					end;
				end;
				int[$G_CALLBACKS] = {};
				int[$G_CALLBACKS_REVERSE] = {};
				int[$G_RESOLVE] = function(callback)
					local res = int[$G_CALLBACKS_REVERSE][callback];
					if res == nil then
						table.insert(int[$G_CALLBACKS], callback);
						local len = #int[$G_CALLBACKS];
						int[$G_CALLBACKS_REVERSE][callback] = len;
						return len;
					else
						return res;
					end
				end
				int[$G_FINISH] = function()
					local old_route = _G.route;
					_G.route = function(addr, port, subport, callback)
						if int[$G_REGISTRY]:has_entry(addr, port) then
							return old_route(addr, port, subport, callback)
						else
							error("can't bind new addresses after initial loading");
						end
					end
				end;
				int[$G_REQUIRE_CACHE] = {};
				_G.require = function(module)
					local resolved = int[$G_RESOLVE_PATH](_G.__module_path, module);
					local existing = int[$G_REQUIRE_CACHE][resolved];
					if existing ~= nil then
						return existing[1];
					else
						local old = _G.__module_path;
						_G.__module_path = resolved;
						local loaded = int[$G_LOAD_FILE](resolved);
						_G.__module_path = old;
						// single element table as to not reload `nil`-producing modules
						int[$G_REQUIRE_CACHE][resolved] = {loaded};
						return loaded;
					end
				end
				_G.resolve = function(addr, port, subport)
					local res = int[$G_REGISTRY]:lookup(addr, port, subport);
					if res == nil then
						return nil;
					else
						return int[$G_CALLBACKS][res](addr, port, subport);
					end
				end
				_G.route = function(addr, port, subport, callback)
					if type(addr) == "string" then
						addr = {addr};
					end
					if type(port) == "number" then
						port = {port};
					end
					if callback == nil then
						callback = subport;
						subport = nil;
					end
					if type(subport) == "string" or type(subport) == "number" then
						subport = {subport};
					end
					local cb_id = int[$G_RESOLVE](callback);
					if subport == nil then
						int[$G_REGISTRY]:add_generic(addr, port, cb_id);
					else
						int[$G_REGISTRY]:add_specific(addr, port, subport, cb_id);
					end
					modified = true;
					return function(addr, port, subport)
						local res = int[$G_REGISTRY]:lookup(addr, port, subport);
						if res ~= nil then
							local cb = int[$G_CALLBACKS][res];
							if cb == callback then
								return cb(addr, port, subport);
							end
						end
						return nil;
					end
				end
				_G.config = {
					max_subport_len = 127;
				};
			})
			.exec_async()
			.await?;
		}
		let (addresses, config) = {
			info!("[lua] running configuration");
			let ret: Function = lua.load(chunk! { internal() }).eval_async().await?;
			let mut path: PathBuf = path.into();
			path.push("main.lua");
			let path = path.as_os_str().to_string_lossy();
			lua.load(chunk! {
				_G.__module_path = $path
				require("main.lua")
			})
			.exec_async()
			.await?;
			info!("[lua] acquiring results");
			let max_subport_len: usize = lua
				.load(chunk! { _G.config.max_subport_len })
				.eval_async()
				.await?;
			let registry: Registry = lua
				.load(chunk! { $ret()[$G_REGISTRY] })
				.eval_async()
				.await?;
			let addresses = {
				registry
					.items
					.borrow()
					.iter()
					.flat_map(|(addr, inner)| {
						inner.iter().map(|(port, _)| Address {
							addr: addr.clone(),
							port: *port,
						})
					})
					.collect()
			};
			(addresses, RouterConfig { max_subport_len })
		};
		Ok(Self {
			lua,
			addresses,
			config,
		})
	}
}

#[async_trait(?Send)]
impl Router for LuaRouter {
	async fn resolve(
		&self,
		address: Address,
		subport: Option<String>,
	) -> CResult<Option<ResolveResult>> {
		let Address { addr, port } = address;
		let addr = addr.unwrap_or_default();
		let subport = subport.unwrap_or_default();
		trace!("[lua] resolving {}:{}!{}", addr, port, subport);
		resolve_result_from_lua(
			self.lua
				.load(chunk! { resolve($addr, $port, $subport) })
				.eval_async::<Value>()
				.await?,
			&self.lua,
		)
	}
	async fn get_bind_addresses(&self) -> Vec<Address> {
		self.addresses.clone()
	}
	async fn get_config(&self) -> RouterConfig {
		self.config.clone()
	}
}
