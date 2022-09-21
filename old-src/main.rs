use log::{error, info};

mod common;
mod def;
mod net;
mod routers;

#[tokio::main]
async fn main() {
	env_logger::builder()
		.filter_level(log::LevelFilter::Trace)
		.init();
	info!("starting connpipe v{}", env!("CARGO_PKG_VERSION"));
	if let Err(e) = net::run().await {
		e.error();
		error!("fatal error, exiting...");
	}
	info!("exiting");
}
