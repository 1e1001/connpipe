#![feature(async_closure, try_blocks)]

use log::LevelFilter;

pub mod routers;
pub mod server;

#[tokio::main]
async fn main() {
	env_logger::builder()
		.filter_level(LevelFilter::Trace)
		.init();
	if let Err(e) = server::run(
		routers::lua::LuaRouter::new("connfig/main.lua")
			.await
			.unwrap(),
	)
	.await
	{
		e.print();
	}
}
