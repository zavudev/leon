//! POSIX shell quoting.
//!
//! A command sent over SSH is a single string interpreted by the remote
//! user's shell. Every program name, argument and path placed in that string
//! goes through [`sh_quote`] so that the shell sees exactly the bytes Leon
//! intended, whatever they contain. Getting this wrong is both a correctness
//! problem (paths with spaces) and a security problem (branch names or paths
//! that would otherwise be executed).

/// Quotes one word for a POSIX shell.
///
/// Words made only of characters that no shell treats specially are returned
/// unchanged, which keeps commands readable in logs. Everything else is
/// wrapped in single quotes, inside which the shell interprets nothing; a
/// single quote in the word is written as `'\''` (close the quotes, an
/// escaped quote, reopen). The empty string becomes `''` so it survives as an
/// argument.
pub fn sh_quote(word: &str) -> String {
    quote_with(word, false)
}

/// [`sh_quote`], with each backslash written outside the quotes when
/// `backslash_outside`.
fn quote_with(word: &str, backslash_outside: bool) -> String {
    if !word.is_empty() && word.bytes().all(is_plain) {
        return word.to_owned();
    }
    let mut quoted = String::with_capacity(word.len() + 2);
    quoted.push('\'');
    for character in word.chars() {
        match character {
            '\'' => quoted.push_str("'\\''"),
            '\\' if backslash_outside => quoted.push_str("'\\\\'"),
            other => quoted.push(other),
        }
    }
    quoted.push('\'');
    quoted
}

/// Quotes free text, such as a prompt, so that it can be *typed* into an
/// interactive shell that Leon knows nothing of (bash, zsh, dash, fish, any
/// login shell of a remote machine) and arrive as one argument, byte for byte.
///
/// Two things differ from [`sh_quote`] because the text goes through a line
/// editor and not into an `exec`:
///
/// * A backslash is written outside the single quotes (`'\\'`): bash and zsh
///   read it as a literal backslash either way, while fish reads a backslash
///   before a quote or another backslash inside single quotes as an escape.
///   This spelling means the same in all of them.
/// * Text with line breaks is kept on one physical line, as
///   `"$(printf '%b' '<text with \\, \n and \0041 for !>')"`. A line break typed
///   inside an open quote would start a new line for the shell's history
///   expansion, which reads `!` there even inside the quotes, and inside double
///   quotes a single quote does not protect it; so no `!` is left in the line.
///   Trailing line breaks are lost to the command substitution (callers trim
///   the text first), and fish needs to be version 3.4 or newer for `"$(...)"`.
///
/// Control characters other than the line feed must be kept out by the caller:
/// a tab typed into a shell completes, and an escape is a key.
pub fn sh_quote_typed(text: &str) -> String {
    if !text.contains('\n') {
        return quote_with(text, true);
    }
    let escaped = text
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('!', "\\0041");
    format!("\"$(printf '%b' {})\"", quote_with(&escaped, true))
}

/// Quotes each word and joins them with spaces into one shell command line.
pub fn sh_join<I, S>(words: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    words
        .into_iter()
        .map(|word| sh_quote(word.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether a byte can appear unquoted in any position of a shell word.
///
/// The set is deliberately small. Notably absent are `~` (expands at the
/// start of a word), `=` (would turn a leading word into an assignment),
/// `#` (starts a comment) and every non-ASCII byte.
fn is_plain(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'_' | b'-' | b'.' | b'/' | b':' | b'@' | b'%' | b'+' | b','
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_words_are_left_unquoted() {
        for word in [
            "git",
            "/srv/api",
            "feature/login-2",
            "user@host",
            "a.b,c+d%e:f",
        ] {
            assert_eq!(sh_quote(word), word);
        }
    }

    #[test]
    fn the_empty_string_becomes_an_empty_pair_of_quotes() {
        assert_eq!(sh_quote(""), "''");
    }

    #[test]
    fn spaces_are_protected_by_single_quotes() {
        assert_eq!(sh_quote("my project"), "'my project'");
        assert_eq!(sh_quote(" leading"), "' leading'");
        assert_eq!(sh_quote("tab\there"), "'tab\there'");
    }

    #[test]
    fn single_quotes_are_closed_escaped_and_reopened() {
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(sh_quote("'"), r"''\'''");
        assert_eq!(sh_quote("''"), r"''\'''\'''");
    }

    #[test]
    fn expansion_characters_are_neutralised() {
        assert_eq!(sh_quote("$HOME"), "'$HOME'");
        assert_eq!(sh_quote("$(reboot)"), "'$(reboot)'");
        assert_eq!(sh_quote("`reboot`"), "'`reboot`'");
        assert_eq!(sh_quote("a\\b"), "'a\\b'");
        assert_eq!(sh_quote("say \"hi\""), "'say \"hi\"'");
        assert_eq!(sh_quote("!history"), "'!history'");
    }

    #[test]
    fn control_operators_and_globs_are_neutralised() {
        for word in [
            "a;b", "a&b", "a|b", "a>b", "a<b", "(a)", "{a}", "*", "?", "[a]", "#c",
        ] {
            assert_eq!(sh_quote(word), format!("'{word}'"));
        }
    }

    #[test]
    fn newlines_are_kept_inside_the_quotes() {
        assert_eq!(sh_quote("line one\nline two"), "'line one\nline two'");
        assert_eq!(sh_quote("\n"), "'\n'");
    }

    /// What a shell makes of one word, for the forms [`sh_quote_typed`]
    /// writes: single quotes (inside which only fish reads `\\` and `\'` as
    /// escapes), a backslash outside them, and `"$(printf '%b' <word>)"`. It
    /// stands in for the shells, so the tests need none.
    fn read_word(word: &str, fish: bool) -> String {
        if let Some(inner) = word
            .strip_prefix("\"$(printf '%b' ")
            .and_then(|rest| rest.strip_suffix(")\""))
        {
            return printf_b(&read_word(inner, fish));
        }
        let mut out = String::new();
        let mut chars = word.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\'' => loop {
                    match chars.next().expect("a quote that is never closed") {
                        '\'' => break,
                        '\\' if fish && matches!(chars.peek(), Some('\\' | '\'')) => {
                            out.push(chars.next().unwrap());
                        }
                        other => out.push(other),
                    }
                },
                '\\' => out.push(chars.next().expect("a backslash with nothing after it")),
                other => {
                    assert!(
                        other.is_ascii_alphanumeric() || "_-.:/@%+,".contains(other),
                        "{other:?} is unquoted in {word:?}"
                    );
                    out.push(other);
                }
            }
        }
        out
    }

    /// `printf '%b'` of the escapes the writer makes: `\\`, `\n` and `\0ddd`.
    fn printf_b(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('0') => {
                    let digits: String = chars.by_ref().take(3).collect();
                    out.push(char::from_u32(u32::from_str_radix(&digits, 8).unwrap()).unwrap());
                }
                other => panic!("an escape the writer does not make: {other:?}"),
            }
        }
        out
    }

    const PROMPTS: [&str; 14] = [
        "fix the login bug",
        "it's broken, don't panic",
        "'''",
        "price is $5 and $HOME and ${PATH}",
        "run `rm -rf /` or $(reboot)",
        "wow! !! !$ !history ^a^b",
        "say \"hi\" and \\\"hi\\\"",
        r"C:\Users\me\ and \' and \\ and trailing\",
        "a;b&&c|d > e < f * ? [x] ~ # not a comment",
        "line one\nline two\n\nit's line four, with $x and `y` and !!",
        r"multi
line with \n inside and \0041 and 100%",
        "café 日本語 ✓",
        "- not a flag",
        "-n",
    ];

    #[test]
    fn typed_text_comes_back_as_one_word_in_a_posix_shell() {
        for prompt in PROMPTS {
            let quoted = sh_quote_typed(prompt);
            assert_eq!(read_word(&quoted, false), prompt, "{quoted}");
        }
    }

    #[test]
    fn typed_text_comes_back_the_same_in_fish() {
        for prompt in PROMPTS {
            let quoted = sh_quote_typed(prompt);
            assert_eq!(read_word(&quoted, true), prompt, "{quoted}");
        }
    }

    #[test]
    fn text_on_one_line_is_single_quoted_with_backslashes_outside() {
        assert_eq!(sh_quote_typed("plain"), "plain");
        assert_eq!(sh_quote_typed("fix it"), "'fix it'");
        assert_eq!(sh_quote_typed("it's $HOME!"), r"'it'\''s $HOME!'");
        assert_eq!(sh_quote_typed(r"a\b"), r"'a'\\'b'");
    }

    #[test]
    fn text_with_line_breaks_stays_on_one_physical_line_without_a_bang() {
        let quoted = sh_quote_typed("one!\ntwo's $x `y`");
        assert!(!quoted.contains('\n'), "{quoted}");
        assert!(!quoted.contains('!'), "{quoted}");
        assert_eq!(
            quoted,
            r#""$(printf '%b' 'one'\\'0041'\\'ntwo'\''s $x `y`')""#
        );
    }

    #[test]
    fn tilde_and_equals_are_quoted_so_they_stay_literal() {
        assert_eq!(sh_quote("~/code"), "'~/code'");
        assert_eq!(sh_quote("KEY=value"), "'KEY=value'");
    }

    #[test]
    fn non_ascii_text_is_quoted_and_preserved() {
        assert_eq!(sh_quote("café"), "'café'");
        assert_eq!(sh_quote("日本語"), "'日本語'");
    }

    #[test]
    fn words_are_joined_with_single_spaces() {
        assert_eq!(
            sh_join(["git", "commit", "-m", "it's done"]),
            r"git commit -m 'it'\''s done'"
        );
        assert_eq!(sh_join(Vec::<String>::new()), "");
    }

    /// Hands quoted words to a real shell and checks they come back intact.
    #[cfg(unix)]
    #[test]
    fn a_real_shell_reads_back_exactly_what_was_quoted() {
        let words = [
            "",
            "plain",
            "two words",
            "it's",
            "'''",
            "$HOME",
            "$(echo pwned)",
            "`echo pwned`",
            "a\\b",
            "back\\",
            "say \"hi\"",
            "line one\nline two",
            "~",
            "*",
            "a;b&&c|d",
            "-n",
            "café 日本語",
            "tab\there",
        ];
        for word in words {
            let script = format!("printf '%s' {}", sh_quote(word));
            let Ok(output) = std::process::Command::new("sh")
                .arg("-c")
                .arg(&script)
                .output()
            else {
                eprintln!("skipped: no POSIX shell available");
                return;
            };
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                word,
                "script was {script:?}"
            );
        }
    }
}
