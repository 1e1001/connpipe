#![allow(unused_variables, dead_code)]
use std::ops::Range;
use std::rc::Rc;

use logos::Logos;

use ariadne::{Color, Label, Report, ReportKind, Source};

macro_rules! valid_token {
	($token:expr, $e:expr, $($m:tt)*) => {
		if matches!($token.0, $($m)*) {
			Ok(())
		} else {
			Err(CError::UnexpectedToken($token.0.clone(), $e, $token.2.clone()))
		}
	};
}

#[derive(Debug, Clone)]
struct Span(Rc<String>, Rc<String>, Range<usize>);

#[derive(Logos, Debug, Clone, Hash, PartialEq, Eq)]
enum Token {
	#[token(":")]
	Colon,
	#[token("!")]
	Exclaim,
	#[token(";")]
	Semicolon,
	#[token("@")]
	AtSymbol,
	#[token(">")]
	Arrow,
	#[token("*")]
	Wildcard,
	#[regex(r#""(\\([abenrtv0"\\]|u\{[0-9a-fA-F]+\})|[^"\\])*""#)]
	String,
	#[regex(r"[a-zA-Z0-9_\-.]+")]
	Identifier,
	#[regex(r"#.*\n", logos::skip)]
	EndOfFile,
	#[error]
	#[regex(r"[ \t\n\f]+", logos::skip)]
	Error,
}

impl Token {
	fn as_name(&self) -> &'static str {
		match self {
			Token::Colon => "\u{2018}:\u{2019}",
			Token::Exclaim => "\u{2018}!\u{2019}",
			Token::Semicolon => "\u{2018};\u{2019}",
			Token::Arrow => "\u{2018}>\u{2019}",
			Token::AtSymbol => "\u{2018}@\u{2019}",
			Token::Wildcard => "\u{2018}*\u{2019}",
			Token::String => "string",
			Token::Identifier => "identifier",
			Token::EndOfFile => "end-of-file",
			Token::Error => "invalid token",
		}
	}
	fn name_particle(&self) -> &'static str {
		match self {
			Token::Colon => "a",
			Token::Exclaim => "a",
			Token::Semicolon => "a",
			Token::Arrow => "a",
			Token::AtSymbol => "a",
			Token::Wildcard => "a",
			Token::String => "a",
			Token::Identifier => "an",
			Token::EndOfFile => "an",
			Token::Error => "an",
		}
	}
}

#[derive(Debug, Clone)]
enum InvalidStringError {
	InvalidEscape,
	InvalidUnicodeEscape,
	LongUnicodeEscape,
	InvalidUnicodePoint,
	Malformed,
}

impl InvalidStringError {
	fn note(self) -> &'static str {
		match self {
			InvalidStringError::InvalidEscape => "This escape sequence is invalid",
			InvalidStringError::InvalidUnicodeEscape => "This character was unexpected",
			InvalidStringError::LongUnicodeEscape => "This escape is too long",
			InvalidStringError::InvalidUnicodePoint => "This isn't a real character",
			InvalidStringError::Malformed => "This string is fucked man",
		}
	}
}

#[derive(Debug, thiserror::Error)]
enum CError {
	#[error("Invalid Token")]
	InvalidToken(Span),
	#[error("Unexpected Token")]
	UnexpectedToken(Token, Vec<Token>, Span),
	#[error("Invalid Number")]
	InvalidNumber(Span),
	#[error("Invalid Tag")]
	InvalidTag(Span),
	#[error("Invalid String")]
	InvalidString(Span, Range<usize>, InvalidStringError),
	#[error("Invalid Domain Name")]
	InvalidDomainName(Span, Token, idna::Errors),
}

impl CError {
	fn report(self) {
		let (span, report) = match self {
			CError::InvalidToken(span) => (
				span.clone(),
				Report::build(ReportKind::Error, span.0.clone(), span.2.start)
					.with_message("Invalid Token")
					.with_label(Label::new((span.0, span.2)).with_message("This token is invalid")),
			),
			CError::UnexpectedToken(got, expected, span) => (span.clone(), {
				Report::build(ReportKind::Error, span.0.clone(), span.2.start)
					.with_message("Unexpected Token")
					.with_label(
						Label::new((span.0, span.2))
							.with_message({
								let mut out = match expected.len() {
									0 => "Expected nothing?".to_string(),
									1 => format!(
										"Expected {} {}",
										expected[0].name_particle(),
										expected[0].as_name()
									),
									2 => format!(
										"Expected {} {} or {}",
										expected[0].name_particle(),
										expected[0].as_name(),
										expected[1].as_name()
									),
									len => {
										let mut out = format!(
											"Expected {} {}",
											expected[0].name_particle(),
											expected[0].as_name()
										);
										for (i, v) in expected.iter().enumerate().skip(1) {
											out.push_str(", ");
											if i == len - 1 {
												out.push_str("or ");
											}
											out.push_str(v.as_name());
										}
										out
									}
								};
								out.push_str(&format!(
									", got {} {}",
									got.name_particle(),
									got.as_name()
								));
								out
							})
							.with_color(Color::Red),
					)
			}),
			CError::InvalidNumber(span) => (
				span.clone(),
				Report::build(ReportKind::Error, span.0.clone(), span.2.start)
					.with_message("Invalid Number")
					.with_label(
						Label::new((span.0, span.2))
							.with_message("This number is invalid")
							.with_color(Color::Red),
					),
			),
			CError::InvalidTag(span) => (
				span.clone(),
				Report::build(ReportKind::Error, span.0.clone(), span.2.start)
					.with_message("Invalid Tag")
					.with_label(
						Label::new((span.0, span.2))
							.with_message("This tag is invalid")
							.with_color(Color::Red),
					),
			),
			CError::InvalidString(span, sub_span, sub_error) => (
				span.clone(),
				Report::build(ReportKind::Error, span.0.clone(), span.2.start)
					.with_message("Invalid String")
					.with_label(
						Label::new((span.0.clone(), span.2.clone()))
							.with_message("This string is invalid")
							.with_color(Color::Red),
					)
					.with_label(
						Label::new((
							span.0,
							span.2.start + sub_span.start..span.2.start + sub_span.end,
						))
						.with_color(Color::Blue)
						.with_message(sub_error.note()),
					),
			),
			CError::InvalidDomainName(span, ty, errors) => (span.clone(), {
				let builder = Report::build(ReportKind::Error, span.0.clone(), span.2.start)
					.with_message("Invalid Domain Name")
					.with_label(
						Label::new((span.0, span.2))
							.with_message("This is invalid")
							.with_color(Color::Red),
					)
					.with_note(format!("Internal errors: {}", errors));
				if ty == Token::Identifier {
					builder.with_help("Maybe try putting the name in quotes")
				} else {
					builder
				}
			}),
		};
		report
			.finish()
			.print((span.0, Source::from(span.1.as_ref())))
			.unwrap();
	}
}

type CResult<T> = Result<T, CError>;

#[derive(Debug)]
pub enum Tag {
	ConnMap(String),
	Include(String),
}

#[derive(Debug)]
pub enum TagAddress {
	ConnMap,
}

#[derive(Debug)]
pub enum Address {
	Address(String),
	Wildcard,
	Localhost,
}

#[derive(Debug)]
pub enum ConfigSide {
	Address {
		address: Address,
		port: u16,
		subport: Option<String>,
	},
	Tag(TagAddress),
}

#[derive(Debug)]
pub struct ConfigEntry {
	pub tags: Vec<Tag>,
	pub left: ConfigSide,
	pub right: ConfigSide,
}

struct CToken<'a>(Token, &'a str, Span);

fn parse_string<'a>(token: &'a CToken) -> CResult<String> {
	let mut text = token.1.char_indices();
	let mut out = String::new();
	let span = token.2.clone();
	if let Some((_, '"')) = text.next() {
		loop {
			match text.next() {
				Some((_, '"')) => break,
				Some((t1, '\\')) => match text.next() {
					Some((_, 'a')) => out.push('\x07'),
					Some((_, 'b')) => out.push('\x08'),
					Some((_, 'e')) => out.push('\x1b'),
					Some((_, 'n')) => out.push('\n'),
					Some((_, 'r')) => out.push('\r'),
					Some((_, 't')) => out.push('\t'),
					Some((t2, 'u')) => {
						if let Some((t3, ch)) = text.next() {
							if ch != '{' {
								return Err(CError::InvalidString(
									span,
									t1..t3 + 1,
									InvalidStringError::InvalidEscape,
								));
							}
						} else {
							return Err(CError::InvalidString(
								span,
								token.1.len() - 1..token.1.len(),
								InvalidStringError::Malformed,
							));
						}
						let mut code = 0u32;
						let mut end = 0;
						let mut end_reached = false;
						for i in 0..=6 {
							match text.next() {
								Some((t3, '}')) => {
									end = t3;
									end_reached = true;
									break;
								}
								Some((t3, ch)) => {
									end = t3;
									let err = || {
										CError::InvalidString(
											span.clone(),
											t3..t3 + 1,
											InvalidStringError::InvalidUnicodeEscape,
										)
									};
									code = code.checked_shl(4).ok_or_else(err)?
										| ch.to_digit(16).ok_or_else(err)?;
								}
								None => {
									return Err(CError::InvalidString(
										span,
										token.1.len() - 1..token.1.len(),
										InvalidStringError::Malformed,
									))
								}
							}
						}
						if !end_reached {
							return Err(CError::InvalidString(
								span.clone(),
								t1..end + 1,
								InvalidStringError::LongUnicodeEscape,
							));
						}
						out.push(code.try_into().map_err(|_| {
							CError::InvalidString(
								span.clone(),
								t1..end + 1,
								InvalidStringError::InvalidUnicodePoint,
							)
						})?);
					}
					Some((_, 'v')) => out.push('\x0b'),
					Some((_, '0')) => out.push('\0'),
					Some((_, '"')) => out.push('"'),
					Some((_, '\\')) => out.push('\\'),
					Some((v, _)) => {
						return Err(CError::InvalidString(
							span,
							t1..v + 1,
							InvalidStringError::InvalidEscape,
						))
					}
					None => {
						return Err(CError::InvalidString(
							span,
							token.1.len() - 1..token.1.len(),
							InvalidStringError::Malformed,
						))
					}
				},
				Some((_, ch)) => out.push(ch),
				None => unreachable!(),
			}
		}
		if let Some((p, c)) = text.next() {
			Err(CError::InvalidString(
				span,
				p..p + 1,
				InvalidStringError::Malformed,
			))
		} else {
			Ok(out)
		}
	} else {
		return Err(CError::InvalidString(
			span,
			0..1,
			InvalidStringError::Malformed,
		));
	}
}

fn parse_u16<'a>(token: &'a CToken) -> CResult<u16> {
	let mut res: u16 = 0;
	let err = || CError::InvalidNumber(token.2.clone());
	for ch in token.1.chars() {
		res = res
			.checked_mul(10)
			.ok_or_else(err)?
			.checked_add(ch.to_digit(10).ok_or_else(err)?.try_into().unwrap())
			.ok_or_else(err)?;
	}
	Ok(res)
}

fn parse_dns<'a>(data: &'a CToken<'a>) -> CResult<String> {
	match &data.0 {
		Token::String => parse_string(data),
		Token::Identifier => {
			let (addr, errors) = idna::domain_to_unicode(data.1);
			errors.map_err(|v| CError::InvalidDomainName(data.2.clone(), data.0.clone(), v))?;
			Ok(addr)
		}
		token => Err(CError::UnexpectedToken(
			token.clone(),
			vec![Token::String, Token::Identifier],
			data.2.clone(),
		)),
	}
}

fn parse_address<'a>(data: &'a [CToken<'a>]) -> CResult<(Address, &'a [CToken<'a>])> {
	if data[0].0 == Token::Wildcard {
		Ok((Address::Wildcard, &data[1..]))
	} else if data[0].0 == Token::Colon {
		Ok((Address::Localhost, data))
	} else {
		valid_token!(
			data[0],
			vec![
				Token::Identifier,
				Token::String,
				Token::Wildcard,
				Token::Colon
			],
			Token::Identifier | Token::String
		)?;
		Ok((Address::Address(parse_dns(&data[0])?), &data[1..]))
	}
}

fn parse_port<'a>(data: &'a [CToken<'a>]) -> CResult<(u16, &'a [CToken<'a>])> {
	valid_token!(data[0], vec![Token::Colon], Token::Colon)?;
	valid_token!(data[1], vec![Token::Identifier], Token::Identifier)?;
	Ok((parse_u16(&data[1])?, &data[2..]))
}

fn parse_side_end<'a>(
	data: &'a [CToken<'a>],
	right: bool,
	subport: bool,
) -> CResult<&'a [CToken<'a>]> {
	if right {
		valid_token!(
			data[0],
			if subport {
				vec![Token::Exclaim, Token::Semicolon]
			} else {
				vec![Token::Semicolon]
			},
			Token::Semicolon
		)?;
	} else {
		valid_token!(data[0], vec![Token::Exclaim, Token::Arrow], Token::Arrow)?;
	}
	Ok(&data[1..])
}

fn parse_subport<'a>(
	data: &'a [CToken<'a>],
	right: bool,
) -> CResult<(Option<String>, &'a [CToken<'a>])> {
	let (data, res) = if data[0].0 == Token::Exclaim {
		(&data[2..], Some(parse_dns(&data[1])?))
	} else {
		(data, None)
	};
	Ok((res, parse_side_end(data, right, true)?))
}
fn parse_tag_address<'a>(data: &'a [CToken<'a>]) -> CResult<(TagAddress, &'a [CToken<'a>])> {
	match data[0].1 {
		"conn-map" => Ok((TagAddress::ConnMap, &data[1..])),
		_ => Err(CError::InvalidTag(data[0].2.clone())),
	}
}

fn parse_side<'a>(data: &'a [CToken<'a>], right: bool) -> CResult<(ConfigSide, &'a [CToken<'a>])> {
	if data[0].0 == Token::AtSymbol {
		let (res, data) = parse_tag_address(&data[1..])?;
		Ok((
			ConfigSide::Tag(res),
			parse_side_end(data, right, false)?,
		))
	} else {
		let (address, data) = parse_address(data)?;
		let (port, data) = parse_port(data)?;
		let (subport, data) = parse_subport(data, right)?;
		Ok((
			ConfigSide::Address {
				address,
				port,
				subport,
			},
			data,
		))
	}
}

fn parse_tag<'a>(data: &'a [CToken<'a>]) -> CResult<(Tag, &'a [CToken<'a>])> {
	match data[0].1 {
		"conn-map" => {
			valid_token!(data[1], vec![Token::String], Token::String)?;
			Ok((Tag::ConnMap(parse_string(&data[1])?), &data[2..]))
		}
		"include" => Ok((Tag::Include(parse_string(&data[1])?), &data[2..])),
		_ => Err(CError::InvalidTag(data[0].2.clone())),
	}
}

fn parse_entry<'a>(
	data: &'a [CToken<'a>],
	mut tags: Vec<Tag>,
) -> CResult<(ConfigEntry, &'a [CToken<'a>])> {
	if data[0].0 == Token::AtSymbol {
		valid_token!(data[1], vec![Token::Identifier], Token::Identifier)?;
		let (res, data) = parse_tag(&data[1..])?;
		tags.push(res);
		parse_entry(data, tags)
	} else {
		let (left, data) = parse_side(data, false)?;
		let (right, data) = parse_side(data, true)?;
		Ok((ConfigEntry { tags, left, right }, data))
	}
}

fn tokenize_and_parse(name: Rc<String>, data: Rc<String>) -> CResult<Vec<ConfigEntry>> {
	let mut lexer = Token::lexer(&data);
	let mut errors = vec![];
	let mut tokens = vec![];
	while let Some(token) = lexer.next() {
		let span = Span(name.clone(), data.clone(), lexer.span());
		if token == Token::Error {
			errors.push(CError::InvalidToken(span.clone()));
		}
		tokens.push(CToken(token, lexer.slice(), span));
	}
	tokens.push(CToken(
		Token::EndOfFile,
		"",
		Span(name, data.clone(), data.len() - 1..data.len()),
	));
	let mut outputs = vec![];
	let mut tail = tokens.as_slice();
	while tail[0].0 != Token::EndOfFile {
		let (res, new_tail) = parse_entry(tail, vec![])?;
		outputs.push(res);
		tail = new_tail;
	}
	Ok(outputs)
}

pub fn try_parse_config(source: Rc<String>, data: Rc<String>) -> Option<Vec<ConfigEntry>> {
	match tokenize_and_parse(source, data) {
		Ok(v) => Some(v),
		Err(e) => {
			e.report();
			None
		}
	}
}
