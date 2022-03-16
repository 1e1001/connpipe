use std::ops::Range;

use logos::Logos;

use crate::structs::{Address, RangePart, Port, ConfigSide, Subport, ConfigEntry, Token, CError};

macro_rules! match_token {
	($token:expr, $match_err:expr, $($match:tt)*) => {
		if matches!($token.0, $($match)*) {
			Ok($token)
		} else {
			Err(CError::UnexpectedToken($token.0, $match_err, $token.2))
		}
	};
}

pub fn tokenize_and_parse(data: &str) {
	let lexer = Token::lexer(data);
	let mut errors = vec![];
	let mut tokens = vec![];
	while let Some(token) = lexer.next() {
		if let Token::Error = token {
			errors.push(CError::InvalidToken(lexer.span()));
		}
		tokens.push((token, lexer.slice(), lexer.span()));
	}
	let mut outputs = vec![];
	let mut index = 0;
	loop {
		match (|| {
			let left_addr = match_token!(tokens[index], vec![Token::Identifier, Token::Wildcard], Token::Identifier | Token::Wildcard)?;
			match_token!(tokens[index + 1], vec![Token::Colon], Token::Colon)?;
			let left_port = match_token!(tokens[index + 2], vec![Token::Identifier], Token::Identifier)?;
			Ok(())
		})() {
			Ok(v) => outputs.push(v),
			Err(e) => errors.push(e),
		}
	}
}
