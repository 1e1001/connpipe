use std::fs;
use std::rc::Rc;

use crate::parser::try_parse_config;

mod parser;

mod graphics;

fn main() {
	// let source = Rc::new("config.txt".to_string());
	// let text = Rc::new(fs::read_to_string(source.as_ref()).unwrap());
	// println!("{:#?}", try_parse_config(source, text).unwrap());
	graphics::start();
}
