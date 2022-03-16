use parser::tokenize_and_parse;

mod structs;
mod parser;

fn main() {
	let text = "prefix.{address1 address2 address3-{inner: a b c}}.suffix:80 > example.com:8888!{80 81 {inner: 82..84}};";
	tokenize_and_parse(text);
}
