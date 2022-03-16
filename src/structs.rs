#![allow(unused_variables, dead_code)]

use logos::{Span, Logos};

#[derive(Logos, Debug, Clone, Hash, PartialEq, Eq)]
pub enum Token {
	#[token(":")]
	Colon,
	#[token("!")]
	Exclaim,
	#[token(";")]
	Semicolon,
	#[token(">")]
	Arrow,
	#[token("*")]
	Wildcard,
	#[regex("[a-zA-Z0-9_\\-.]+")]
	Identifier,
	#[error]
	#[regex(r"[ \t\n\f]+", logos::skip)]
	Error,
}

#[derive(Debug, thiserror::Error)]
pub enum CError {
	#[error("Invalid Token")]
	InvalidToken(Span),
	#[error("Unexpected Token")]
	UnexpectedToken(Token, Vec<Token>, Span),
}

pub type CResult<T> = Result<T, CError>;

#[derive(Debug)]
pub enum Address {
	Address(String),
	Wildcard,
}

#[derive(Debug)]
pub enum Subport {
	None,
	Int(u64),
	Name(String),
}

#[derive(Debug)]
pub struct ConfigSide {
	pub address: Address,
	pub port: u16,
	pub subport: Subport,
}

#[derive(Debug)]
pub struct ConfigEntry {
	pub left: ConfigSide,
	pub right: ConfigSide,
}
