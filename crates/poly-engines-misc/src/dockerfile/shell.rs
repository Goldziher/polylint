//! Minimal tokenizer for shell-form `RUN` bodies.
//!
//! Splits a command line into simple commands (on `&&`, `||`, `;`, `|`, `&`, newlines and
//! parentheses) and each command into words, honouring quotes and backslash escapes. It does not
//! expand variables or understand control flow; the Dockerfile rules only need to know which
//! program runs with which arguments.

/// A shell-form `RUN` body split into simple commands.
pub(super) struct Script {
    /// One word list per simple command, quotes removed.
    pub commands: Vec<Vec<String>>,
    /// Whether a single `|` pipe joins two commands outside quotes.
    pub has_pipe: bool,
}

#[derive(Default)]
struct Lexer {
    commands: Vec<Vec<String>>,
    words: Vec<String>,
    word: String,
    in_word: bool,
    has_pipe: bool,
}

impl Lexer {
    fn end_word(&mut self) {
        if self.in_word {
            self.words.push(std::mem::take(&mut self.word));
            self.in_word = false;
        }
    }

    fn end_command(&mut self) {
        self.end_word();
        if !self.words.is_empty() {
            self.commands.push(std::mem::take(&mut self.words));
        }
    }

    fn push(&mut self, c: char) {
        self.word.push(c);
        self.in_word = true;
    }
}

/// Tokenizes `text`, dropping leading `--flag` words (BuildKit `RUN --mount=...`) from the first
/// command.
pub(super) fn parse(text: &str) -> Script {
    let mut lx = Lexer::default();
    let mut quote: Option<char> = None;
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if c == '\\' && q == '"' {
                if let Some(next) = chars.next() {
                    lx.push(next);
                }
            } else {
                lx.push(c);
            }
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                lx.in_word = true;
            }
            '\\' => match chars.next() {
                Some('\n') | None => {}
                Some(next) => lx.push(next),
            },
            '|' => {
                if chars.peek() == Some(&'|') {
                    chars.next();
                } else {
                    lx.has_pipe = true;
                }
                lx.end_command();
            }
            '\n' | ';' | '&' | '(' | ')' => lx.end_command(),
            c if c.is_whitespace() => lx.end_word(),
            c => lx.push(c),
        }
    }
    lx.end_command();

    if let Some(first) = lx.commands.first_mut() {
        let flags = first.iter().take_while(|w| w.starts_with("--")).count();
        first.drain(..flags);
    }
    lx.commands.retain(|c| !c.is_empty());
    Script {
        commands: lx.commands,
        has_pipe: lx.has_pipe,
    }
}

/// Drops leading `NAME=value` environment assignments from a command's words.
pub(super) fn without_assignments(words: &[String]) -> &[String] {
    let skip = words.iter().take_while(|w| is_assignment(w)).count();
    &words[skip..]
}

fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmds(text: &str) -> Vec<Vec<String>> {
        parse(text).commands
    }

    #[test]
    fn splits_on_operators_and_quotes() {
        assert_eq!(
            cmds("cd /a && echo 'x && y' ; ls \"a b\""),
            vec![vec!["cd", "/a"], vec!["echo", "x && y"], vec!["ls", "a b"]]
        );
    }

    #[test]
    fn pipe_is_not_logical_or() {
        assert!(parse("a | b").has_pipe);
        assert!(!parse("a || b").has_pipe);
        assert!(!parse("echo 'a | b'").has_pipe);
    }

    #[test]
    fn drops_leading_run_flags() {
        assert_eq!(
            cmds("--mount=type=cache,target=/x pip install a"),
            vec![vec!["pip", "install", "a"]]
        );
    }

    #[test]
    fn strips_env_assignments() {
        let words = vec!["A=1".to_owned(), "pip".to_owned(), "x=y".to_owned()];
        assert_eq!(without_assignments(&words), &words[1..]);
    }
}
