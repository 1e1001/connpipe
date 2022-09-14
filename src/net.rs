use std::collections::HashSet;
use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use crate::common::{
	task_counter, Address, AddressSubport, CError, CResult, PipeError, RouterEnv, RouterSharedEnv, TaskId,
	TaskIdType,
};
use crate::def;
use log::{info, trace, warn};
use notify::Watcher;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::select;
use tokio::sync::{mpsc, oneshot};

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

struct StopSignals(Vec<(TaskId, StopSignalSender)>);

impl StopSignals {
	fn new() -> Self {
		Self(vec![])
	}
	fn add(&mut self, id: TaskId, tx: StopSignalSender) {
		self.0.push((id, tx));
	}
	async fn close(self, rx: &mut mpsc::Receiver<RootEvent>) -> CResult<()> {
		let mut wait_for = HashSet::new();
		for (id, signal) in self.0 {
			match signal.send(()) {
				Err(_) => warn!("{id} stop signal disconnect"),
				Ok(_) => {
					wait_for.insert(id);
				}
			}
		}
		let mut timeout_counter = 10u8;
		while !wait_for.is_empty() {
			match rx.recv().await.unwrap() {
				RootEvent::TaskStopped(id) => {
					info!("{id} competed!");
					wait_for.remove(&id);
				}
				RootEvent::Poll => {
					if timeout_counter == 0 {
						return Err(CError::StopSignalTimeout);
					}
					timeout_counter -= 1;
				}
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
	let root_tx_int = root_tx.clone();
	ctrlc::set_handler(move || {
		println!();
		trace!("caught interrupt");
		root_tx_int.blocking_send(RootEvent::Exit).unwrap();
	})
	.expect("failed to set interrupt handler");
	let mut watcher = notify::recommended_watcher(NotifyHandler(root_tx.clone())).unwrap();
	watcher
		.watch(config_path, notify::RecursiveMode::Recursive)
		.unwrap();
	let mut env = RouterEnv::new("./config/".into());
	info!("starting routers...");
	let root_tx_thread = root_tx.clone();
	tokio::spawn(async {
		let root_tx = root_tx_thread;
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
					trace!("{id} registered stop signal");
				}
				RootEvent::Route(id, addr, res) => {
					trace!("{id} requested route for {addr}");
					if res.send(env.start_route(addr).await).is_err() {
						warn!("{id} route request disconnect!");
					}
				}
				RootEvent::Exit => break true,
				RootEvent::TaskStopped(id) => warn!("{id} stopped before signal emitted"),
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
		info!("{id} spawn task");
		match task.await {
			Ok(_) | Err(CError::TaskStop) => trace!("{id} task finished"),
			Err(e) => {
				warn!("{id} error in task");
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
				safe_task(id, root_tx.clone(), pipe_loop(id, signal, root_tx.clone(), shared.clone(), BufReader::new(tcp_stream), socket_addr));
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
	mut tcp_stream: BufferedTcp,
	socket_addr: SocketAddr,
) -> CResult<()> {
	macro_rules! stream_read {
		($buf:expr) => {
			select! {
				res = tcp_stream.read($buf) => Ok(res?),
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
	let subport = if valid {
		trace!("{id} magic found, reading subport");
		const MAX_BYTES: u32 = (usize::BITS + 6) / 7;
		const MAX_REMAINDER: u32 = 8 - usize::BITS % 7;
		let mut out = 0usize;
		let mut read_bits = 0;
		// read length only once to reduce lock time, and prevent over-reading before the length is lowered
		let max_config_length = shared.read().await.config.max_subport_len;
		for i in 0..MAX_BYTES {
			let byte = tcp_stream.read_u8().await?;
			// throw SubportTooLong here since the only tiem this can happen is if the length is more than usize::MAX
			// TODO: throw on trailing zeros
			if i == MAX_BYTES - 1 && byte.leading_zeros() < MAX_REMAINDER {
				return Err(CError::Pipe(id, PipeError::SubportTooLong));
			}
			out |= usize::from(byte) << read_bits;
			if out > max_config_len {
				return Err(CError::Pipe(id, PipeError::SubportTooLong));
			}
			read_bits += 7;
		}
		None
	} else {
		None
	};
	todo!("read");
	// route
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
