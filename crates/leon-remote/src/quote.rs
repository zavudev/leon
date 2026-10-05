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
    if !word.is_empty() && word.bytes().all(is_plain) {
        return word.to_owned();
    }
    let mut quoted = String::with_capacity(word.len() + 2);
    quoted.push('\'');
    for character in word.chars() {
        if character == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(character);
        }
    }
    quoted.push('\'');
    quoted
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
