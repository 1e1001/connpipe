use std::rc::Rc;

use iced::{Column, TextInput, Sandbox, Element, Settings, text_input, Row, Text, Length};

use crate::parser::{ConfigEntry, try_parse_config};

#[derive(Default)]
struct Application {
	config: Vec<ConfigEntry>,
}

#[derive(Debug, Clone)]
enum Message {
}

impl Sandbox for Application {
	type Message = Message;
	fn new() -> Self {
		Self {
			config: try_parse_config(Rc::new("<inline>".to_string()), Rc::new(r##"
# fun example configuration file

@conn-map "this conn-map"
*:8080!conn-map > @conn-map;

# pipe a minecraft server with people needing more effort to access it
@conn-map "sample description value"
*:8000!mc-server > :25565;
			"##.to_string())).unwrap()
		}
	}
	fn title(&self) -> String {
		String::from("connpipe")
	}
	fn view(&mut self) -> Element<Message> {
		let mut grid = Column::new()
			.push(Row::new()
				.push(Text::new("from").width(Length::Fill))
				.push(Text::new("to").width(Length::Fill))
				.push(Text::new("status").width(Length::Fill)));
		for i in self.config.iter() {
			grid = grid.push(Row::new()
				.push(Text::new(format!("{:?}", i.left)).width(Length::Fill))
				.push(Text::new(format!("{:?}", i.right)).width(Length::Fill))
				.push(Text::new("config").width(Length::Fill)));
		}
		grid.into()
		// Column::new()
		// 	// .push(TextInput::new(&mut self.text_box, "placeholder", &self.text, Message::TextChanged))
		// 	.push(Row::new()
		// 		.push(Text::new("*:8080!mc-server").width(Length::Fill))
		// 		.push(Text::new(":25565").width(Length::Fill))
		// 		.push(Text::new("config").width(Length::Fill)))
		// 	.into()
	}
	fn update(&mut self, message: Message) {
		match message {
			// Message::TextChanged(text) => self.text = text,
		}
	}
}

pub fn start() {
	Application::run(Settings::default()).unwrap()
}
