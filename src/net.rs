use std::collections::HashSet;
use std::future::Future;
use std::net::SocketAddr;
use std::ops::ControlFlow;
use std::path::Path;
use std::time::Duration;

use crate::common::{
	task_counter, Address, AddressSubport, CError, CResult, RouterEnv, TaskId, TaskIdType,
};
use crate::routers::load_config;
use log::{info, trace, warn};
use notify::Watcher;
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
	StopSignalComplete(TaskId),
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
		while wait_for.len() > 0 {
			match rx.recv().await.unwrap() {
				RootEvent::StopSignalComplete(id) => {
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
					if let Err(_) = res.send(env.start_route(addr).await) {
						warn!("{id} route request disconnect!");
					}
				}
				RootEvent::Exit => break true,
				RootEvent::StopSignalComplete(id) => warn!("{id} stopped before signal sent out"),
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

fn safe_task<R, T: Future<Output = CResult<R>> + Send + 'static>(id: TaskId, task: T) {
	tokio::spawn(async move {
		info!("{id} spawn task");
		match task.await {
			Err(e) => {
				warn!("{id} error in task");
				e.warn();
			}
			Ok(_) => trace!("{id} task finished"),
		}
	});
}

async fn try_load_config(
	config_path: &Path,
	env: &mut RouterEnv,
	root_tx: mpsc::Sender<RootEvent>,
) -> CResult<()> {
	load_config(config_path, env).await?;
	let listen_addresses = env.listen_addresses().await?;
	trace!("bind: {:?}", listen_addresses);
	for addr in listen_addresses {
		let id = task_counter(TaskIdType::Conn).await;
		let signal = add_signal(id, &root_tx).await?;
		safe_task(id, conn_loop(id, signal, root_tx.clone(), addr));
	}
	Ok(())
}

async fn conn_loop(
	id: TaskId,
	mut stop_signal: StopSignalReceiver,
	root_tx: mpsc::Sender<RootEvent>,
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
				safe_task(id, pipe_loop(id, signal, root_tx.clone(), tcp_stream, socket_addr));
			},
			res = on_stop_signal::<()>(id, &mut stop_signal, &root_tx) => {
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
	tcp_stream: TcpStream,
	socket_addr: SocketAddr,
) -> CResult<()> {
	loop {
		select! {
			res = on_stop_signal::<()>(id, &mut stop_signal, &root_tx) => {
				res?;
				break;
			}
		}
	}
	Ok(())
}

async fn on_stop_signal<T>(
	id: TaskId,
	stop_signal: &mut StopSignalReceiver,
	root_tx: &mpsc::Sender<RootEvent>,
) -> CResult<ControlFlow<(), T>> {
	stop_signal.await?;
	root_tx.send(RootEvent::StopSignalComplete(id)).await?;
	Ok(ControlFlow::Break(()))
}
