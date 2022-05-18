use std::net::{AddrParseError, IpAddr, SocketAddr};
use std::pin::Pin;
use std::{fmt, io};

use async_trait::async_trait;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use log::{error, info, trace};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{self, JoinError};

#[derive(Debug, Clone)]
pub struct Address {
	pub addr: Option<String>,
	pub port: u16,
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

pub fn parse_wildcard(v: String) -> Option<String> {
	if v == "" {
		None
	} else {
		Some(v)
	}
}

#[derive(Debug)]
pub struct ResolveResult {
	pub addr: String,
	pub port: u16,
	pub subport: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum CError {
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
	pub fn print(&self) {
		error!("{}", self);
	}
}

pub type CResult<T> = Result<T, CError>;

#[derive(Debug, Clone)]
pub struct RouterConfig {
	pub max_subport_len: usize,
}

#[async_trait(?Send)]
pub trait Router {
	async fn resolve(
		&self,
		address: Address,
		subport: Option<String>,
	) -> CResult<Option<ResolveResult>>;
	async fn get_bind_addresses(&self) -> Vec<Address>;
	async fn get_config(&self) -> RouterConfig;
}

const MAGIC: u128 = u128::from_be(0x893d1d4e4e4fc6862d1e10010d0a690a);
const BUFFER_SIZE: usize = 1024;

pub async fn run(router: impl Router) -> CResult<()> {
	info!("[run] loading configuration");
	loop {
		info!("[run] starting server");
		match run_server(&router).await {
			Ok(_) => {
				info!("[run] server stopped successfully!");
				break;
			}
			Err(e) => {
				error!("[run] error while processing requests");
				e.print();
				error!("[run] server stopping!");
				break;
				// tokio::time::sleep(Duration::from_secs(5)).await;
				// info!("[run] attempting to restart...");
			}
		}
	}
	Ok(())
}

async fn run_server<T: Router>(router: &T) -> CResult<()> {
	let addresses = router.get_bind_addresses().await;
	if addresses.len() == 0 {
		info!("[server] no addresses to bind, exiting");
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
	for item in addresses.into_iter() {
		let listener = TcpListener::bind(item.resolve("0.0.0.0")?).await?;
		info!("[server] listening on {}", item);
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
								let mut magic_buf = [0; 16];
								let mut total = 0usize;
								while total < 16 {
									let n = stream.read(&mut magic_buf[total..]).await?;
									if n == 0 {
										break;
									}
									total += n;
								}
								trace!("[thread {}] magic_buf: {:0>2x?}", id, &magic_buf[0..total]);
								let (subport, end) = if total == 16
									&& u128::from_ne_bytes(magic_buf) == MAGIC
								{
									trace!("[thread {}] connection has a subport", id);
									const MAX_LEN_LEN: u32 = (usize::BITS + 6) / 7;
									let mut subport_len = 0usize;
									let mut len_buf = [0];
									for i in 0..MAX_LEN_LEN {
										stream.read_exact(&mut len_buf).await?;
										subport_len |=
											((len_buf[0] & 0x7F) as usize).wrapping_shl(i * 7);
										if len_buf[0] & 0x80 == 0 {
											break;
										}
									}
									trace!("[thread {}] subport length: {} bytes", id, subport_len);
									let mut subport_buf = vec![0; subport_len];
									stream.read_exact(&mut subport_buf).await?;
									match String::from_utf8(subport_buf) {
										Ok(v) => (Some(v), 0),
										Err(e) => {
											trace!(
												"[thread {}], utf-8 conversion error: {}",
												id,
												e
											);
											(None, total)
										}
									}
								} else {
									(None, total)
								};
								trace!("[thread {}] subport is {:?}", id, subport);
								Ok((
									stream,
									socket,
									address,
									subport,
									magic_buf,
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
				) => match router.resolve(address.clone(), subport.clone()).await? {
					None => trace!("[handle {}] resolution failed, disconnecting", id),
					Some(resolved) => {
						trace!("[handle {}] resolved output to {:?}", id, resolved);
						pinned_tasks.push(task::spawn(async move {
							TaskMessage::ConnectionComplete(
								id,
								async move {
									trace!("[thread {}]", id);
									let outwards =
										TcpStream::connect(address.resolve("0.0.0.0")?).await?;
									let (mut outwards_read, mut inwards_write) =
										stream.into_split();
									let (mut inwards_read, mut outwards_write) =
										outwards.into_split();
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
									}
									outwards_write.write_all(&buf[0..end as usize]).await?;
									let mut outwards_buf = [0; BUFFER_SIZE];
									let mut inwards_buf = [0; BUFFER_SIZE];
									loop {
										tokio::select! {
											n = outwards_read.read(&mut outwards_buf) => {
												match outwards_write.write_all(&outwards_buf[0..n?]).await {
													Ok(_) => {}
													Err(e) => match e.kind() {
														io::ErrorKind::ConnectionRefused
														| io::ErrorKind::ConnectionReset
														| io::ErrorKind::ConnectionAborted
														| io::ErrorKind::BrokenPipe
														| io::ErrorKind::UnexpectedEof => break Ok(()),
														_ => {
															trace!("[handle ]") // todo here
															break Err(e.into())
														},
													},
												}
											},
											n = inwards_read.read(&mut inwards_buf) => {
												match inwards_write.write_all(&inwards_buf[0..n?]).await {
													Ok(_) => {}
													Err(e) => match e.kind() {
														io::ErrorKind::ConnectionRefused
														| io::ErrorKind::ConnectionReset
														| io::ErrorKind::ConnectionAborted
														| io::ErrorKind::BrokenPipe
														| io::ErrorKind::UnexpectedEof => break Ok(()),
														_ => break Err(e.into()),
													},
												}
											}
										}
									}
								}
								.await,
							)
						}));
					}
				},
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
