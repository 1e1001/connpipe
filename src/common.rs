use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::{fmt, io};

use async_trait::async_trait;
use log::{debug, error, trace, warn};
use tokio::sync::RwLock;

#[derive(thiserror::Error, Debug)]
pub enum InvalidAddrError {
	#[error("expected a port after hostname")]
	ExpectedPort,
	#[error("expected a port between hostname and subport")]
	ExpectedPortWithSubport,
	#[error("invalid host {0:?}")]
	InvalidHost(String),
	#[error("invalid port {0:?}")]
	InvalidPort(String),
	#[error("blank host in non-partial address")]
	NoBlankHost,
	#[error("blank port in non-partial address")]
	NoBlankPort,
	#[error("blank subport in non-partial address")]
	NoBlankSubport,
}

#[derive(thiserror::Error, Debug)]
pub enum CError {
	#[error("lua error:\n{0}")]
	LuaError(#[from] mlua::Error),
	#[error("io error:\n{0}")]
	IoError(#[from] io::Error),
	#[error("toml deserialization error:\n{0}")]
	TomlDeserializeError(#[from] toml::de::Error),
	#[error("notify error:\n{0}")]
	NotifyError(#[from] notify::Error),
	#[error("mpsc send error:\n{0}")]
	MpscSendError0(#[from] tokio::sync::mpsc::error::SendError<crate::net::RootEvent>),
	#[error("mpsc send error:\n{0}")]
	MpscSendError1(#[from] tokio::sync::mpsc::error::SendError<TaskId>),
	#[error("invalid router type {0:?}")]
	InvalidRouterType(String),
	#[error("oneshot recv error: {0}")]
	OneshotRecvError(#[from] tokio::sync::oneshot::error::RecvError),
	#[error("invalid address {0:?}, {1}")]
	InvalidAddr(String, InvalidAddrError),
	#[error("host {0:?} is invalid")]
	InvalidHost(String),
	#[error("[{0}] {1}")]
	RouterError(&'static str, String),
	#[error("recursed too deep")]
	RecursionError,
	#[error("stop signals timed out")]
	StopSignalTimeout,
	#[error("todo!")]
	TodoError,
}

impl CError {
	pub fn error(&self) {
		error!("{self}");
		debug!("{self:?}");
	}
	pub fn warn(&self) {
		warn!("{self}");
		debug!("{self:?}");
	}
}

pub type CResult<T> = Result<T, CError>;

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum Hostname {
	V4([u8; 4]),
	V6([u16; 8]),
	Name(String),
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Address {
	pub host: Hostname,
	pub port: u16,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AddressSubport {
	pub host: Hostname,
	pub port: u16,
	pub subport: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AddressPartial {
	pub host: Option<Hostname>,
	pub port: Option<u16>,
	pub subport: Option<Option<String>>,
}

impl Hostname {
	pub fn to_ip_addr(&self) -> CResult<IpAddr> {
		match self {
			Hostname::V4([a, b, c, d]) => Ok(IpAddr::V4(Ipv4Addr::new(*a, *b, *c, *d))),
			Hostname::V6([a, b, c, d, e, f, g, h]) => {
				Ok(IpAddr::V6(Ipv6Addr::new(*a, *b, *c, *d, *e, *f, *g, *h)))
			}
			Hostname::Name(name) => name.parse().map_err(|v| CError::InvalidHost(name.clone())),
		}
	}
}

impl Address {
	pub fn with_subport(self, subport: Option<String>) -> AddressSubport {
		AddressSubport {
			host: self.host,
			port: self.port,
			subport,
		}
	}
	pub fn to_socket(&self) -> CResult<SocketAddr> {
		Ok(SocketAddr::new(self.host.to_ip_addr()?, self.port))
	}
}

impl AddressSubport {
	pub fn pure_part(self) -> Address {
		Address {
			host: self.host,
			port: self.port,
		}
	}
	pub fn with(self, top: AddressPartial) -> AddressSubport {
		AddressSubport {
			host: top.host.unwrap_or(self.host),
			port: top.port.unwrap_or(self.port),
			subport: top.subport.unwrap_or(self.subport),
		}
	}
	pub fn from_str(s: &str) -> CResult<AddressSubport> {
		let partial = address_from_str(s, false)?;
		Ok(AddressSubport {
			host: partial.host.unwrap(),
			port: partial.port.unwrap(),
			subport: partial.subport.unwrap(),
		})
	}
}

impl AddressPartial {
	pub const NULL: AddressPartial = AddressPartial {
		host: None,
		port: None,
		subport: None,
	};
	pub fn with(self, top: AddressPartial) -> AddressPartial {
		AddressPartial {
			host: top.host.or(self.host),
			port: top.port.or(self.port),
			subport: top.subport.or(self.subport),
		}
	}
	pub fn from_str(s: &str) -> CResult<AddressPartial> {
		address_from_str(s, true)
	}
}

impl fmt::Debug for Hostname {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		match self {
			Hostname::V4(addr) => write!(f, "{}.{}.{}.{}", addr[0], addr[1], addr[2], addr[3]),
			Hostname::V6(addr) => {
				let mut max_span_start = 0;
				let mut max_span_len = 0;
				let mut in_span = false;
				for i in 0..8 {
					if addr[i] == 0 {
						if in_span {
							max_span_len += 1;
						} else {
							in_span = true;
							max_span_start = i;
							max_span_len = 1;
						}
					} else {
						in_span = false;
					}
				}
				if max_span_len == 0 {
					write!(f, "[{}", addr[0])?;
					for part in &addr[1..] {
						write!(f, ":{part}")?;
					}
				} else {
					write!(f, "[")?;
					for part in &addr[0..max_span_start] {
						write!(f, "{part}:")?;
					}
					if max_span_start == 0 {
						write!(f, ":")?;
					}
					for part in &addr[max_span_start + max_span_len..] {
						write!(f, ":{part}")?;
					}
					if max_span_len == 8 {
						write!(f, ":")?;
					}
				}
				write!(f, "]")
			}
			Hostname::Name(addr) => f.write_str(addr),
		}
	}
}

impl fmt::Debug for Address {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "{}:{}", self.host, self.port)
	}
}

impl fmt::Debug for AddressSubport {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "{}:{}", self.host, self.port)?;
		match &self.subport {
			Some(subport) => write!(f, "!{subport}"),
			None => Ok(()),
		}
	}
}

impl fmt::Debug for AddressPartial {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		match &self.host {
			Some(host) => write!(f, "{host}"),
			None => Ok(()),
		}?;
		write!(f, ":")?;
		match &self.port {
			Some(port) => write!(f, "{port}"),
			None => Ok(()),
		}?;
		match &self.subport {
			Some(Some(subport)) => write!(f, "!{subport}"),
			Some(None) => Ok(()),
			None => write!(f, "!"),
		}
	}
}

impl fmt::Display for Hostname {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "{self:?}")
	}
}
impl fmt::Display for Address {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "{self:?}")
	}
}
impl fmt::Display for AddressSubport {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "{self:?}")
	}
}
impl fmt::Display for AddressPartial {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "{self:?}")
	}
}

fn address_from_str(s: &str, partial: bool) -> CResult<AddressPartial> {
	let make_err = |k| CError::InvalidAddr(s.to_string(), k);
	let parse_hex = |ch, host: &str| match ch {
		b'0'..=b'9' => Ok(ch - 0x30),
		b'A'..=b'F' => Ok(ch - 0x38),
		b'a'..=b'f' => Ok(ch - 0x58),
		_ => Err(make_err(InvalidAddrError::InvalidHost(host.to_string()))),
	};
	let mut iter = s.splitn(2, '!');
	let host_port = iter.next().unwrap();
	let subport = iter.next();
	let (host, port) =
		{
			let iter: Vec<_> = host_port.rsplitn(2, ':').collect();
			if iter.len() < 2 {
				return Err(make_err(if subport.is_some() {
					InvalidAddrError::ExpectedPortWithSubport
				} else {
					InvalidAddrError::ExpectedPort
				}));
			} else {
				let (port, host) = (iter[0], iter[1]);
				let make_host_err = || make_err(InvalidAddrError::InvalidHost(host.to_string()));
				let host = if host == "" {
					if partial {
						None
					} else {
						return Err(make_err(InvalidAddrError::NoBlankHost));
					}
				} else if host == "*" {
					Some(Hostname::V4([0, 0, 0, 0]))
				} else if host == "$" {
					Some(Hostname::V4([127, 0, 0, 1]))
				} else if host.len() >= 4
					&& host.as_bytes()[0] == b'['
					&& host.chars().rev().next().unwrap() == ']'
				{
					// is ipv6
					let mut out = [0; 8];
					let (head, tail) =
						match host[1..host.len() - 1].split("::").collect::<Vec<_>>()[..] {
							[head, tail] => (head, Some(tail)),
							[head] => (head, None),
							_ => return Err(make_host_err()),
						};
					let parse_ipv6_unit = |part: &str| {
						if part.len() != 4 {
							return Err(make_host_err());
						}
						let bytes = part.as_bytes();
						Ok((parse_hex(bytes[0], host)? as u16) << 12
							| (parse_hex(bytes[1], host)? as u16) << 8
							| (parse_hex(bytes[2], host)? as u16) << 4
							| parse_hex(bytes[3], host)? as u16)
					};
					if head.len() > 0 {
						let head_parts: Vec<_> = head.split(':').collect();
						if tail.is_none() {
							if head_parts.len() != 8 {
								return Err(make_host_err());
							}
						} else if head_parts.len() > 8 {
							return Err(make_host_err());
						}
						for (i, part) in head_parts.into_iter().enumerate() {
							out[i] = parse_ipv6_unit(part)?;
						}
					}
					if let Some(tail) = tail {
						if tail.len() > 0 {
							for (i, part) in tail.rsplit(':').enumerate() {
								out[7 - i] = parse_ipv6_unit(part)?;
							}
						}
					}
					Some(Hostname::V6(out))
				} else {
					// is host or ipv4
					let mut out = vec![];
					for part in host.split('.') {
						if part == "" {
							return Err(make_host_err());
						}
						for ch in part.chars() {
							match ch {
								'-' | '+' | '_' | '0'..='9' | 'A'..='Z' | 'a'..='z' => {}
								_ => {
									return Err(make_err(InvalidAddrError::InvalidHost(
										host.to_string(),
									)))
								}
							}
						}
						out.push(part);
					}
					if out.len() == 0 {
						return Err(make_host_err());
					} else if out.len() == 4 && out[0].as_bytes()[0].is_ascii_digit() {
						// is ipv4
						let mut out = [0; 4];
						for (i, part) in out.into_iter().enumerate() {
							out[i] = format!("+{part}").parse().map_err(|_| make_host_err())?;
						}
						Some(Hostname::V4(out))
					} else {
						// is host
						for part in out {
							if part.as_bytes()[0].is_ascii_digit() {
								return Err(make_host_err());
							}
						}
						Some(Hostname::Name(host.to_string()))
					}
				};
				(
					host,
					if port == "" {
						if partial {
							None
						} else {
							return Err(make_err(InvalidAddrError::NoBlankPort));
						}
					} else {
						Some(format!("+{port}").parse().map_err(|_| {
							make_err(InvalidAddrError::InvalidPort(port.to_string()))
						})?)
					},
				)
			}
		};
	let subport = match subport {
		Some("") => {
			if partial {
				None
			} else {
				return Err(make_err(InvalidAddrError::NoBlankSubport));
			}
		}
		_ => Some(subport.map(str::to_owned)),
	};
	Ok(AddressPartial {
		host,
		port,
		subport,
	})
}

#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct RouterConfig {
	/// maximum subport length before incoming connections are cancelled, does not affect outgoing connections
	pub max_subport_len: usize,
	/// maximum depth for recursion before exiting
	pub max_recursion: usize,
}

impl Default for RouterConfig {
	fn default() -> Self {
		Self {
			max_subport_len: 128,
			max_recursion: 128,
		}
	}
}

#[derive(Debug)]
pub struct RouterCommon {
	pub dirty: bool,
}

impl Default for RouterCommon {
	fn default() -> Self {
		Self { dirty: true }
	}
}

impl RouterCommon {
	pub fn is_dirty(&mut self) -> bool {
		let old_val = self.dirty;
		self.dirty = false;
		return old_val;
	}
}

#[async_trait(?Send)]
pub trait RouterInst: fmt::Debug {
	/// translate an address
	async fn route(&self, addr: AddressSubport, env: &RouterEnv)
		-> CResult<Option<AddressSubport>>;
	/// get the list of addresses to bind to
	async fn listen_addresses(&self) -> CResult<Vec<Address>>;
	/// return true if the configuration changed since the last time this function was called
	async fn is_dirty(&mut self) -> bool;
}

pub type RouterDyn = Box<dyn RouterInst>;

pub struct RouterEnv {
	path: PathBuf,
	pub config: RouterConfig,
	routers: HashMap<String, RouterDyn>,
	current_depth: Cell<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskIdType {
	Conn,
	Pipe,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(usize, TaskIdType);

impl std::fmt::Display for TaskId {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(
			f,
			"[{}/{}]",
			match self.1 {
				TaskIdType::Conn => "conn",
				TaskIdType::Pipe => "pipe",
			},
			self.0
		)
	}
}
static GLOBAL_TASK_COUNTER: RwLock<usize> = RwLock::const_new(0);
pub async fn task_counter(ty: TaskIdType) -> TaskId {
	let mut lock = GLOBAL_TASK_COUNTER.write().await;
	let id = *lock;
	*lock += 1;
	TaskId(id, ty)
}

impl RouterEnv {
	pub fn new(path: PathBuf) -> Self {
		Self {
			path,
			config: Default::default(),
			routers: HashMap::new(),
			current_depth: Cell::new(0),
		}
	}
	pub fn path(&self) -> &Path {
		&self.path
	}
	pub fn clear_routers(&mut self) {
		trace!("clearing routers");
		self.routers.clear();
	}
	pub fn set_routers(&mut self, map: HashMap<String, RouterDyn>) {
		trace!("new routers loaded");
		self.routers = map;
	}
	pub async fn listen_addresses(&self) -> CResult<Vec<Address>> {
		let mut out = HashSet::new();
		for (id, router) in self.routers.iter() {
			out.extend(router.listen_addresses().await?.into_iter());
		}
		Ok(out.into_iter().collect())
	}
	pub async fn start_route(&self, addr: AddressSubport) -> CResult<Option<AddressSubport>> {
		self.current_depth.set(0);
		self.route("main", addr).await
	}
	pub async fn route(&self, id: &str, addr: AddressSubport) -> CResult<Option<AddressSubport>> {
		trace!("routing {addr} with {id:?}");
		if self.current_depth.get() <= self.config.max_recursion {
			self.current_depth.set(self.current_depth.get() + 1);
			self.routers
				.get(id)
				.ok_or(CError::RecursionError)?
				.route(addr, self)
				.await
		} else {
			Err(CError::RecursionError)
		}
	}
}
