use anyhow::{Result, anyhow, bail};

#[derive(Debug, Clone)]
pub(super) struct Token {
    pub(super) text: String,
    pub(super) line: usize,
}

pub(super) fn tokenize(source: &str) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut chars = source.chars().peekable();
    let mut line = 1usize;

    while let Some(ch) = chars.next() {
        match ch {
            '\n' => line += 1,
            '#' => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        line += 1;
                        break;
                    }
                }
            }
            '{' | '}' | ';' => tokens.push(Token { text: ch.to_string(), line }),
            '"' | '\'' => {
                let quote = ch;
                let start_line = line;
                let mut value = String::new();
                let mut closed = false;
                while let Some(next) = chars.next() {
                    match next {
                        '\n' => {
                            line += 1;
                            value.push('\n');
                        }
                        '\\' => {
                            let escaped = chars.next().ok_or_else(|| {
                                anyhow!("unterminated escape sequence on line {line}")
                            })?;
                            if escaped == '\n' {
                                line += 1;
                                continue;
                            }
                            value.push(escaped);
                        }
                        candidate if candidate == quote => {
                            closed = true;
                            break;
                        }
                        candidate => value.push(candidate),
                    }
                }
                if !closed {
                    bail!("unterminated quoted string starting on line {start_line}");
                }
                tokens.push(Token { text: value, line: start_line });
            }
            whitespace if whitespace.is_whitespace() => {}
            other => {
                let mut value = String::from(other);
                while let Some(peek) = chars.peek().copied() {
                    if peek.is_whitespace() || matches!(peek, '{' | '}' | ';' | '#') {
                        break;
                    }
                    value.push(peek);
                    chars.next();
                }
                tokens.push(Token { text: value, line });
            }
        }
    }

    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::tokenize;

    #[test]
    fn tokenize_treats_backslash_newline_in_quotes_as_line_continuation() {
        let tokens = tokenize("set \"hello\\\nworld\";").expect("tokenization should succeed");

        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0].text, "set");
        assert_eq!(tokens[0].line, 1);
        assert_eq!(tokens[1].text, "helloworld");
        assert_eq!(tokens[1].line, 1);
        assert_eq!(tokens[2].text, ";");
        assert_eq!(tokens[2].line, 2);
    }

    #[test]
    fn tokenize_skips_hash_comments_inline_and_on_their_own_line() {
        let tokens = tokenize("set value; # trailing comment\n# whole line comment\nnext;")
            .expect("tokenization should succeed");

        let rendered =
            tokens.iter().map(|token| (token.text.as_str(), token.line)).collect::<Vec<_>>();
        assert_eq!(rendered, vec![("set", 1), ("value", 1), (";", 1), ("next", 3), (";", 3)]);
    }

    #[test]
    fn tokenize_keeps_braces_and_semicolons_inside_quotes() {
        let tokens = tokenize("set \"{ keep; braces; }\";").expect("tokenization should succeed");

        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0].text, "set");
        assert_eq!(tokens[1].text, "{ keep; braces; }");
        assert_eq!(tokens[2].text, ";");
    }
}
