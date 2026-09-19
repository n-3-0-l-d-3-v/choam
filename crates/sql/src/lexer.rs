//! Splits SQL text into tokens, each tagged with its starting byte
//! offset. Malformed input is always a typed `LexError`, never a panic.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// An identifier or keyword; the parser decides which (keywords are
    /// matched case-insensitively).
    Word(String),
    Integer(i64),
    Str(String),
    LParen,
    RParen,
    Comma,
    Semicolon,
    Star,
    Plus,
    Minus,
    Slash,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LexError {
    #[error("unexpected character {ch:?} at byte {pos}")]
    UnexpectedChar { ch: char, pos: usize },
    #[error("unterminated string literal starting at byte {pos}")]
    UnterminatedString { pos: usize },
    #[error("integer literal at byte {pos} does not fit in 64 bits")]
    IntegerOutOfRange { pos: usize },
}

pub fn lex(input: &str) -> Result<Vec<(Token, usize)>, LexError> {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let c = input[i..].chars().next().unwrap();
        match c {
            c if c.is_whitespace() => i += c.len_utf8(),
            '(' | ')' | ',' | ';' | '*' | '+' | '-' | '/' | '=' => {
                let tok = match c {
                    '(' => Token::LParen,
                    ')' => Token::RParen,
                    ',' => Token::Comma,
                    ';' => Token::Semicolon,
                    '*' => Token::Star,
                    '+' => Token::Plus,
                    '-' => Token::Minus,
                    '/' => Token::Slash,
                    _ => Token::Eq,
                };
                out.push((tok, start));
                i += 1;
            }
            '<' | '>' | '!' => {
                let next = bytes.get(i + 1).copied();
                let (tok, len) = match (c, next) {
                    ('<', Some(b'=')) => (Token::Le, 2),
                    ('<', Some(b'>')) => (Token::Ne, 2),
                    ('<', _) => (Token::Lt, 1),
                    ('>', Some(b'=')) => (Token::Ge, 2),
                    ('>', _) => (Token::Gt, 1),
                    ('!', Some(b'=')) => (Token::Ne, 2),
                    _ => return Err(LexError::UnexpectedChar { ch: c, pos: start }),
                };
                out.push((tok, start));
                i += len;
            }
            '\'' => {
                let mut s = String::new();
                i += 1;
                loop {
                    let Some(ch) = input[i..].chars().next() else {
                        return Err(LexError::UnterminatedString { pos: start });
                    };
                    i += ch.len_utf8();
                    if ch == '\'' {
                        if bytes.get(i) == Some(&b'\'') {
                            s.push('\'');
                            i += 1;
                        } else {
                            break;
                        }
                    } else {
                        s.push(ch);
                    }
                }
                out.push((Token::Str(s), start));
            }
            '0'..='9' => {
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let n = input[start..i]
                    .parse::<i64>()
                    .map_err(|_| LexError::IntegerOutOfRange { pos: start })?;
                out.push((Token::Integer(n), start));
            }
            c if c.is_alphabetic() || c == '_' => {
                while i < bytes.len() {
                    let ch = input[i..].chars().next().unwrap();
                    if ch.is_alphanumeric() || ch == '_' {
                        i += ch.len_utf8();
                    } else {
                        break;
                    }
                }
                out.push((Token::Word(input[start..i].to_string()), start));
            }
            other => {
                return Err(LexError::UnexpectedChar {
                    ch: other,
                    pos: start,
                })
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Token> {
        lex(s).unwrap().into_iter().map(|(t, _)| t).collect()
    }

    #[test]
    fn two_char_operators_win_over_one_char() {
        assert_eq!(
            toks("<= >= <> != < >"),
            vec![
                Token::Le,
                Token::Ge,
                Token::Ne,
                Token::Ne,
                Token::Lt,
                Token::Gt
            ]
        );
    }

    #[test]
    fn strings_unescape_doubled_quotes() {
        assert_eq!(toks("'it''s'"), vec![Token::Str("it's".into())]);
        assert_eq!(toks("''"), vec![Token::Str(String::new())]);
    }

    #[test]
    fn unterminated_string_is_an_error() {
        assert_eq!(lex("'abc"), Err(LexError::UnterminatedString { pos: 0 }));
    }

    #[test]
    fn integer_overflow_is_an_error_but_max_is_fine() {
        assert_eq!(toks("9223372036854775807"), vec![Token::Integer(i64::MAX)]);
        assert!(matches!(
            lex("9223372036854775808"),
            Err(LexError::IntegerOutOfRange { pos: 0 })
        ));
    }

    #[test]
    fn lone_bang_and_stray_chars_are_errors() {
        assert!(lex("!").is_err());
        assert!(lex("a @ b").is_err());
    }

    #[test]
    fn positions_are_byte_offsets() {
        let t = lex("ab  cd").unwrap();
        assert_eq!(t[1].1, 4);
    }

    #[test]
    fn keyword_prefixed_identifiers_stay_whole() {
        assert_eq!(toks("selective"), vec![Token::Word("selective".into())]);
    }
}
