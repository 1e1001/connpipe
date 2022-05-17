#![feature(async_closure, try_blocks)]
use std::cell::RefCell;
use std::collections::HashMap;
use std::net::{AddrParseError, IpAddr, SocketAddr};
use std::pin::Pin;
use std::time::Duration;
use std::{fmt, io};

use futures::stream::FuturesUnordered;
use futures::{Future, StreamExt};
use log::*;
use mlua::{chunk, FromLua, Function, Integer, Lua, LuaOptions, StdLib, ToLua, UserData, Value};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{self, JoinError};

#[derive(Debug, Clone)]
struct Address {
	addr: Option<String>,
	port: u16,
}

impl Address {
	fn resolve(&self, default: &str) -> CResult<(IpAddr, u16)> {
		Ok((self.addr.as_deref().unwrap_or(default).parse()?, self.port))
	}
}

impl fmt::Display for Address {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			f,
			"{}:{}",
			self.addr.as_deref().unwrap_or_default(),
			self.port
		)
	}
}

fn parse_wildcard(v: String) -> Option<String> {
	if v == "" {
		None
	} else {
		Some(v)
	}
}

#[derive(Debug)]
struct ResolveResult {
	addr: String,
	port: u16,
	subport: Option<String>,
}

impl<'lua> FromLua<'lua> for ResolveResult {
	fn from_lua(lua_value: Value<'lua>, lua: &'lua Lua) -> mlua::Result<Self> {
		match lua_value {
			Value::Table(table) => {
				let vec: Result<Vec<Value>, _> = table.sequence_values().collect();
				match &vec?[..] {
					[addr, port, subport] => Ok(ResolveResult {
						addr: String::from_lua(addr.clone(), lua)?,
						port: u16::from_lua(port.clone(), lua)?,
						subport: Some(String::from_lua(subport.clone(), lua)?),
					}),
					[addr, port] => Ok(ResolveResult {
						addr: String::from_lua(addr.clone(), lua)?,
						port: u16::from_lua(port.clone(), lua)?,
						subport: None,
					}),
					vec => Err(mlua::Error::FromLuaConversionError {
						from: "table",
						to: "ResolveResult",
						message: Some(format!("expected length of 2 or 3, got {}", vec.len())),
					}),
				}
			}
			_ => Err(mlua::Error::FromLuaConversionError {
				from: lua_value.type_name(),
				to: "ResolveResult",
				message: Some("expected list".into()),
			}),
		}
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
	fn add_methods<'lua, M: mlua::UserDataMethods<'lua, Self>>(methods: &mut M) {
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

#[derive(Error, Debug)]
enum CError {
	#[error("lua error:\n{0}")]
	LuaError(#[from] mlua::Error),
	#[error("io error:\n{0}")]
	IoError(#[from] io::Error),
	#[error("join error:\n{0}")]
	JoinError(#[from] JoinError),
	#[error("addr parse error:\n{0}")]
	AddrParseError(#[from] AddrParseError),
	#[error("todo error")]
	TodoError,
}

impl CError {
	fn print(&self) {
		error!("{}", self);
	}
}

type CResult<T> = Result<T, CError>;

const T_I_REGISTRY: u8 = 1;
const T_I_CALLBACKS: u8 = 2;
const T_I_CALLBACKS_REVERSE: u8 = 2;
const T_I_RESOLVE: u8 = 3;
const T_I_MAX_SUBPORT_LENGTH: u8 = 4;
const T_I_FINISH: u8 = 5;
const MAGIC: u128 = u128::from_be(0x893d1d4e4e4fc6862d1e10010d0a1a0a);
const BUFFER_SIZE: usize = 1024;

async fn init_lua() -> CResult<Lua> {
	let lua = Lua::new_with(
		StdLib::MATH | StdLib::STRING | StdLib::TABLE | StdLib::COROUTINE | StdLib::UTF8,
		LuaOptions::new(),
	)?;
	{
		let globals = lua.globals();
		let internals = lua.create_table()?;
		internals.set(
			T_I_REGISTRY,
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
			int[$T_I_CALLBACKS] = {};
			int[$T_I_CALLBACKS_REVERSE] = {};
			int[$T_I_RESOLVE] = function(callback)
				local res = int[$T_I_CALLBACKS_REVERSE][callback];
				if res == nil then
					table.insert(int[$T_I_CALLBACKS], callback);
					local len = #int[$T_I_CALLBACKS];
					int[$T_I_CALLBACKS_REVERSE][callback] = len;
					return len;
				else
					return res;
				end
			end
			int[$T_I_MAX_SUBPORT_LENGTH] = 256;
			int[$T_I_FINISH] = function()
				_G.route = function()
					error("can't create new routes after initialization, consider extending an existing route?");
				end
			end;
			_G.resolve = function(addr, port, subport)
				local res = int[$T_I_REGISTRY]:lookup(addr, port, subport);
				if res == nil then
					return nil;
				else
					return int[$T_I_CALLBACKS][res](addr, port, subport);
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
				local cb_id = int[$T_I_RESOLVE](callback);
				if subport == nil then
					int[$T_I_REGISTRY]:add_generic(addr, port, cb_id);
				else
					int[$T_I_REGISTRY]:add_specific(addr, port, subport, cb_id);
				end
				modified = true;
				return function(addr, port, subport)
					local res = int[$T_I_REGISTRY]:lookup(addr, port, subport);
					if res ~= nil then
						local cb = int[$T_I_CALLBACKS][res];
						if cb == callback then
							return cb(addr, port, subport);
						end
					end
					return nil;
				end
			end
		})
		.exec_async()
		.await?;
	}
	Ok(lua)
}

async fn run() -> CResult<()> {
	info!("starting lua");
	let lua = init_lua().await?;
	info!("loading configuration");
	let ret: Function = lua.load(chunk! { internal() }).eval_async().await?;
	lua.load(include_str!("test-config.lua"))
		.set_name("test-config.lua")?
		.exec_async()
		.await?;
	let (ret1, ret2) = (ret.clone(), ret.clone());
	let max_subport_length: usize = lua
		.load(chunk! { $ret1()[$T_I_MAX_SUBPORT_LENGTH] })
		.eval_async()
		.await?;
	let registry: Registry = lua
		.load(chunk! { $ret2()[$T_I_REGISTRY] })
		.eval_async()
		.await?;
	loop {
		info!("starting server");
		let registry = registry.clone();
		trace!("registry: {:?}", registry);
		match run_server(
			&lua,
			registry,
			max_subport_length,
			|lua: &Lua, Address { addr, port }, subport| async move {
				let addr = addr.unwrap_or_default();
				let subport = subport.unwrap_or_default();
				Ok(lua
					.load(chunk! { resolve($addr, $port, $subport) })
					.eval_async()
					.await?)
			},
		)
		.await
		{
			Ok(()) => {}
			Err(e) => {
				error!("error while processing requests");
				e.print();
				tokio::time::sleep(Duration::from_secs(5)).await;
				info!("attempting to restart...");
			}
		}
	}
}

async fn run_server<'a, A, B>(
	lua: &'a Lua,
	reg: Registry,
	max_subport_length: usize,
	resolve: A,
) -> CResult<()>
where
	A: Fn(&'a Lua, Address, Option<String>) -> B,
	B: Future<Output = CResult<ResolveResult>>,
{
	let items: Vec<_> = {
		reg.items
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
	if items.len() == 0 {
		return Ok(());
	}
	enum TaskMessage {
		Accept(CResult<((TcpStream, SocketAddr), TcpListener, Address)>),
		NeedResolving(
			usize,
			CResult<(TcpStream, SocketAddr, Address, Option<String>, [u8; 16], u8)>,
		),
		ConnectionComplete(usize, CResult<()>),
	}
	let mut tasks = FuturesUnordered::new();
	let mut next_task_id = 0usize;
	let run_task = |item: Address, listener: TcpListener| {
		task::spawn(async move {
			TaskMessage::Accept(async move { Ok((listener.accept().await?, listener, item)) }.await)
		})
	};
	for item in items.into_iter() {
		let listener = TcpListener::bind(item.resolve("0.0.0.0")?).await?;
		info!("listening on {}", item);
		tasks.push(run_task(item, listener));
	}
	let mut pinned_tasks = Pin::new(&mut tasks);
	loop {
		match pinned_tasks.next().await {
			Some(v) => match v? {
				TaskMessage::Accept(v) => {
					let ((mut stream, socket), listener, address) = v?;
					pinned_tasks.push(run_task(address.clone(), listener));
					let id = next_task_id;
					next_task_id += 1;
					info!("[handle {}] connection on {} from {}", id, address, socket);
					pinned_tasks.push(task::spawn(async move {
						TaskMessage::NeedResolving(
							id,
							async {
								let mut buf = [0; 16];
								let mut total = 0usize;
								while total < 16 {
									let n = stream.read(&mut buf[total..]).await?;
									if n == 0 {
										break;
									}
									total += n;
								}
								trace!("[thread {}] buf: {:0>2x?}", id, &buf[0..total]);
								let (subport, end) =
									if total == 16 && u128::from_ne_bytes(buf) == MAGIC {
										trace!("[thread {}] connection has subport", id);
										todo!("subport parsing!");
									} else {
										(None, total)
									};
								trace!("[thread {}] subport is {:?}", id, subport);
								Ok((
									stream,
									socket,
									address,
									subport,
									buf,
									end.try_into().unwrap(),
								))
							}
							.await,
						)
					}));
				}
				TaskMessage::NeedResolving(
					id,
					Ok((stream, socket, address, subport, buf, end)),
				) => {
					let resolved = resolve(lua, address.clone(), subport.clone()).await?;
					trace!("[handle {}] resolved output to {:?}", id, resolved);
					pinned_tasks.push(task::spawn(async move {
						TaskMessage::ConnectionComplete(
							id,
							async move {
								trace!("[thread {}]", id);
								let outwards =
									TcpStream::connect(address.resolve("0.0.0.0")?).await?;
								let (inwards_read, inwards_write) = stream.into_split();
								let (_read, mut outwards_write) = outwards.into_split();
								if let Some(subport) = resolved.subport {
									outwards_write.write_all(&MAGIC.to_ne_bytes()).await?;
									let mut subport_len = subport.len();
									while subport_len > 0 {
										let shift = (subport_len & 0x7F).try_into().unwrap();
										subport_len >>= 7;
										let shift =
											if subport_len > 0 { shift | 0x80 } else { shift };
										outwards_write.write_all(&[shift]).await?;
									}
									outwards_write.write_all(subport.as_bytes()).await?;
									// let mut buf = [0; BUFFER_SIZE];
								}
								Ok(())
							}
							.await,
						)
					}));
				}
				TaskMessage::ConnectionComplete(id, Ok(_)) => {
					info!("[handle {}] connection finished", id)
				}
				TaskMessage::ConnectionComplete(id, Err(e))
				| TaskMessage::NeedResolving(id, Err(e)) => {
					error!("[handle {}] connection failed\n{}", id, e);
				}
			},
			None => break Ok(()),
		}
	}
}

#[tokio::main]
async fn main() {
	env_logger::builder()
		.filter_level(LevelFilter::Trace)
		.init();
	if let Err(e) = run().await {
		e.print();
	}
}
