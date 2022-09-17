use std::collections::{HashSet, HashMap};
use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use crate::common::{
	task_counter, Address, AddressSubport, CError, CResult, PipeError, RouterEnv, RouterSharedEnv,
	TaskId, TaskIdType,
};
use crate::def;
use bytestr::ByteString;
use log::{info, trace, warn};
use notify::Watcher;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::select;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

struct NotifyHandler(mpsc::Sender<RootEvent>);

#[derive(Debug)]
pub enum RootEvent {
	FilesChanged,
	Route(
		TaskId,
		AddressSubport,
		oneshot::Sender<CResult<Option<AddressSubport>>>,
	),
	RegisterStopSignal(TaskId, StopSignalSender),
	TaskStopped(TaskId),
	Poll,
	Exit,
}

impl notify::EventHandler for NotifyHandler {
	fn handle_event(&mut self, event: notify::Result<notify::Event>) {
		match match event {
			Ok(e) => {
				trace!("fs event: {e:?}");
				match self.0.blocking_send(RootEvent::FilesChanged) {
					Ok(_) => Ok::<_, CError>(()),
					Err(e) => Err(e.into()),
				}
			}
			Err(e) => Err(e.into()),
		} {
			Ok(_) => {}
			Err(e) => e.warn(),
		}
	}
}

type StopSignalSender = oneshot::Sender<()>;
type StopSignalReceiver = oneshot::Receiver<()>;
type BufferedTcp = BufReader<TcpStream>;

struct StopSignals(HashMap<TaskId, StopSignalSender>);

impl StopSignals {
	fn new() -> Self {
		Self(HashMap::new())
	}
	fn add(&mut self, id: TaskId, tx: StopSignalSender) {
		self.0.insert(id, tx);
	}
	fn remove(&mut self, id: TaskId) {
		self.0.remove(&id);
	}
	async fn close(mut self, rx: &mut mpsc::Receiver<RootEvent>) -> CResult<()> {
		let mut timeout_counter = 10u8;
		// we can't re-use the hashmap because we need to consume the signals, and only optionally add to the output
		let mut wait_for = HashSet::new();
		for (id, signal) in self.0 {
			match signal.send(()) {
				Err(_) => warn!("{id}:main stop signal disconnect"),
				Ok(_) => { wait_for.insert(id); }
			}
		}
		while !wait_for.is_empty() {
			match rx.recv().await.unwrap() {
				RootEvent::TaskStopped(id) => {
					info!("{id}:main competed!");
					wait_for.remove(&id);
				}
				RootEvent::Poll => {
					if timeout_counter == 0 {
						return Err(CError::StopSignalTimeout);
					}
					timeout_counter -= 1;
				}
				RootEvent::Exit => return Err(CError::StopSignalCancel),
				evt => trace!("unrelated event: {evt:?}"),
			}
		}
		Ok(())
	}
}

pub async fn run() -> CResult<()> {
	let config_path: &Path = Path::new("./config/");
	let (root_tx, mut root_rx) = mpsc::channel(128);
	trace!("registering interrupt handle");
	let root_tx_clone = root_tx.clone();
	let interrupt_thread = tokio::spawn(async {
		let root_tx = root_tx_clone;
		loop {
			if let Err(e) = tokio::signal::ctrl_c().await {
				warn!("failed to interrupt handler!");
				CError::from(e).warn();
				warn!("interrupting this process will not be caught! any closing work done by routers will be bypassed");
			}
			println!();
			info!("interrupt received!");
			root_tx.send(RootEvent::Exit).await.unwrap();
		}
	});
	let mut watcher = notify::recommended_watcher(NotifyHandler(root_tx.clone())).unwrap();
	watcher
		.watch(config_path, notify::RecursiveMode::Recursive)
		.unwrap();
	let mut env = RouterEnv::new("./config/".into());
	info!("starting routers...");
	let root_tx_clone = root_tx.clone();
	tokio::spawn(async {
		let root_tx = root_tx_clone;
		loop {
			tokio::time::sleep(Duration::from_secs(1)).await;
			root_tx.send(RootEvent::Poll).await.unwrap();
		}
	});
	loop {
		let mut files_changed = 0;
		let mut stop_signals = StopSignals::new();
		// TODO: seperate config loading and startup
		if let Err(e) = try_load_config(config_path, &mut env, root_tx.clone()).await {
			warn!("error loading configuration:");
			e.warn();
			warn!("modify the configuration to re-load");
		}
		let brk = loop {
			match root_rx.recv().await.unwrap() {
				RootEvent::Poll => {
					if files_changed > 0 {
						files_changed -= 1;
						if files_changed == 0 {
							info!("reloading");
							break false;
						}
					}
				}
				RootEvent::FilesChanged => {
					if files_changed < 2 {
						info!("files changed, reloading shortly");
						files_changed = 2;
					}
				}
				RootEvent::RegisterStopSignal(id, signal) => {
					stop_signals.add(id, signal);
					trace!("{id}:main registered stop signal");
				}
				RootEvent::Route(id, addr, res) => {
					trace!("{id}:main requested route for {addr}");
					if res.send(env.start_route(addr).await).is_err() {
						warn!("{id}:main route request disconnect!");
					}
				}
				RootEvent::Exit => break true,
				RootEvent::TaskStopped(id) => {
					stop_signals.remove(id);
				},
			}
		};
		info!("waiting for all tasks to end");
		stop_signals.close(&mut root_rx).await?;
		if brk {
			info!("exiting...");
			break Ok(());
		} else {
			info!("restarting routers...");
		}
	}
	trace!("shutting down interrupt handler");
	task_kill(interrupt_handler).await?;
}

async fn task_kill(task: JoinHandle<()>) -> CResult<()> {
	task.abort();
	match task.await {
    Ok(_) => Ok(()),
    Err(e) => if e.is_cancelled() {
			Ok(())
		} else if e.is_panic() {
			let panic = e.into_panic();
			Ok(())
		},
	}
}

async fn add_signal(id: TaskId, root_tx: &mpsc::Sender<RootEvent>) -> CResult<StopSignalReceiver> {
	let (tx, rx) = oneshot::channel();
	root_tx.send(RootEvent::RegisterStopSignal(id, tx)).await?;
	Ok(rx)
}

fn safe_task<R, T: Future<Output = CResult<R>> + Send + 'static>(
	id: TaskId,
	root_tx: mpsc::Sender<RootEvent>,
	task: T,
) {
	tokio::spawn(async move {
		info!("{id}:task spawn task");
		match task.await {
			Ok(_) | Err(CError::TaskStop) => trace!("{id}:task task finished"),
			Err(e) => {
				warn!("{id}:task error in task");
				e.warn();
			}
		}
		root_tx.send(RootEvent::TaskStopped(id)).await.unwrap();
	});
}

async fn try_load_config(
	config_path: &Path,
	env: &mut RouterEnv,
	root_tx: mpsc::Sender<RootEvent>,
) -> CResult<()> {
	crate::routers::load_config(config_path, env).await?;
	let listen_addresses = env.listen_addresses().await?;
	trace!("bind: {:?}", listen_addresses);
	for addr in listen_addresses {
		let id = task_counter(TaskIdType::Conn).await;
		let signal = add_signal(id, &root_tx).await?;
		safe_task(
			id,
			root_tx.clone(),
			conn_loop(id, signal, root_tx.clone(), env.shared.clone(), addr),
		);
	}
	Ok(())
}

async fn conn_loop(
	id: TaskId,
	mut stop_signal: StopSignalReceiver,
	root_tx: mpsc::Sender<RootEvent>,
	shared: RouterSharedEnv,
	addr: Address,
) -> CResult<()> {
	let listener = TcpListener::bind(addr.to_socket()?).await?;
	loop {
		select! {
			req = listener.accept() => {
				trace!("{id} connection received");
				let (tcp_stream, socket_addr) = req?;
				let id = task_counter(TaskIdType::Pipe).await;
				let signal = add_signal(id, &root_tx).await?;
				safe_task(id, root_tx.clone(), pipe_loop(id, signal, root_tx.clone(), shared.clone(), BufReader::new(tcp_stream), Address::from_socket(socket_addr)));
			},
			res = on_stop_signal(id, &mut stop_signal) => {
				res?;
				break;
			}
		}
	}
	Ok(())
}

async fn pipe_loop(
	id: TaskId,
	mut stop_signal: StopSignalReceiver,
	root_tx: mpsc::Sender<RootEvent>,
	shared: RouterSharedEnv,
	mut conn_stream: BufferedTcp,
	conn_addr: Address,
) -> CResult<()> {
	macro_rules! stream_read {
		($buf:expr) => {
			select! {
				res = conn_stream.read($buf) => Ok(res?),
				res = on_stop_signal(id, &mut stop_signal) => {
					res?;
					Err(CError::TaskStop)
				}
				_ = tokio::time::sleep(def::SUBPORT_READ_TIMEOUT) => {
					Ok(0)
				}
			}
		};
	}
	// read
	let mut magic_buf = [0u8; 16];
	let mut total = 0usize;
	let mut valid = true;
	while total < 16 {
		let n = stream_read!(&mut magic_buf[total..])?;
		if n == 0 {
			valid = false;
			break;
		}
		total += n;
		// cancel as early as possible
		if magic_buf != def::MAGIC {
			valid = false;
			break;
		}
	}
	trace!("{id} magic: {:02x?}", &magic_buf[0..total]);
	trace!(
		"{id} debug: {:?}",
		ByteString::from_vec(magic_buf[0..total].to_vec())
	);
	let subport = if valid {
		trace!("{id} magic found, reading subport");
		const MAX_BYTES: u32 = (usize::BITS + 6) / 7;
		const MAX_REMAINDER: u32 = 8 - usize::BITS % 7;
		let mut read_len = 0usize;
		let mut read_bits = 0;
		// read length only once to reduce lock time, and prevent over-reading before the length is lowered
		let max_config_len = shared.read().await.config.max_subport_len;
		for i in 0..MAX_BYTES {
			let byte = conn_stream.read_u8().await?;
			// throw SubportTooLong here since the only time this can happen is if the length is more than usize::MAX
			if i == MAX_BYTES - 1 && byte.leading_zeros() < MAX_REMAINDER {
				return Err(CError::Pipe(id, PipeError::SubportTooLong));
			} else if byte == 0 && i > 0 {
				return Err(CError::Pipe(id, PipeError::SubportZeroTail));
			}
			read_len |= usize::from(byte & 0x7F) << read_bits;
			if read_len > max_config_len {
				return Err(CError::Pipe(id, PipeError::SubportTooLong));
			}
			if byte & 0x80 == 0 {
				break;
			}
			read_bits += 7;
		}
		trace!(
			"{id} subport is {read_len} byte{} long",
			if read_len == 1 { "" } else { "s" }
		);
		let mut subport_buf = ByteString::from_vec(vec![0; read_len]);
		let mut subport_read = 0;
		let mut buf = subport_buf.as_bytes_mut();
		while subport_read < read_len {
			let n = stream_read!(&mut buf[subport_read..])?;
			if n == 0 {
				return Err(CError::Pipe(id, PipeError::EarlyClose));
			}
			subport_read += n;
		}
		drop(buf);
		Some(subport_buf)
	} else {
		None
	};
	let address = conn_addr.with_subport(subport);
	trace!("{id} in: {address:?}");
	return Ok(());
	todo!("route");
	// connect
	todo!("connect");
	// pipe
	loop {
		select! {
			res = on_stop_signal(id, &mut stop_signal) => {
				res?;
				break;
			}
		}
	}
	Ok(())
}

async fn on_stop_signal(id: TaskId, stop_signal: &mut StopSignalReceiver) -> CResult<()> {
	stop_signal.await?;
	trace!("{id} got stop signal");
	Ok(())
}
