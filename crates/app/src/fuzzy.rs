//! Fuzzy ranking for the palette.
//!
//! A hand-written scorer rather than a dependency: the palette ranks a few
//! hundred short names per keystroke, and the rules are the product's, so they
//! are written down and tested here.
//!
//! The best answer is the whole name, then a name that starts with the query,
//! then its initials ("nw" for "New worktree"), then a word that starts with
//! it, then having it anywhere, then having its letters in order. Among equals
//! the shorter name wins.

/// How well `text` answers `query`, higher being better; `None` when it does
/// not.
pub fn score(query: &str, text: &str) -> Option<u32> {
    let (query, text) = (query.trim().to_lowercase(), text.to_lowercase());
    if query.is_empty() {
        return Some(0);
    }
    let brevity = 99u32.saturating_sub(text.chars().count() as u32);
    if text == query {
        return Some(1000);
    }
    if text.starts_with(&query) {
        return Some(800 + brevity);
    }
    let words = || {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
    };
    let initials: String = words().filter_map(|word| word.chars().next()).collect();
    let letters: String = query.chars().filter(|c| !c.is_whitespace()).collect();
    if letters.chars().count() >= 2 && initials.starts_with(&letters) {
        return Some(700 + brevity);
    }
    if words().any(|word| word.starts_with(&query)) {
        return Some(600 + brevity);
    }
    if text.contains(&query) {
        return Some(400 + brevity);
    }
    let mut wanted_letters = letters.chars();
    let mut wanted = wanted_letters.next();
    for c in text.chars() {
        if Some(c) == wanted {
            wanted = wanted_letters.next();
        }
    }
    (wanted.is_none() && letters.chars().count() >= 2).then_some(200 + brevity)
}

/// Which characters of `text` answer `query`, as places among its characters:
/// the ones [`score`] counted, for emphasising them. Empty when `text` does not
/// answer (or the query is empty).
pub fn matched_chars(query: &str, text: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() || score(&query, text).is_none() {
        return Vec::new();
    }
    let chars: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    // Lowercasing may change the length of exotic text; then nothing is shown.
    if chars.len() != text.chars().count() {
        return Vec::new();
    }
    let wanted: Vec<char> = query.chars().collect();
    let span = |at: usize| (at..at + wanted.len()).collect::<Vec<_>>();
    let find = |from: usize| {
        (from..=chars.len().saturating_sub(wanted.len()))
            .find(|at| chars[*at..].starts_with(&wanted))
    };
    if chars == wanted || chars.starts_with(&wanted) {
        return span(0);
    }
    // Words and their first letters.
    let mut starts = Vec::new();
    let mut previous_alphanumeric = false;
    for (at, c) in chars.iter().enumerate() {
        if c.is_alphanumeric() && !previous_alphanumeric {
            starts.push(at);
        }
        previous_alphanumeric = c.is_alphanumeric();
    }
    let letters: Vec<char> = wanted
        .iter()
        .copied()
        .filter(|c| !c.is_whitespace())
        .collect();
    let initials: Vec<char> = starts.iter().map(|at| chars[*at]).collect();
    if letters.len() >= 2 && initials.starts_with(&letters) {
        return starts[..letters.len()].to_vec();
    }
    if let Some(at) = starts
        .iter()
        .copied()
        .find(|at| chars[*at..].starts_with(&wanted))
    {
        return span(at);
    }
    if let Some(at) = find(0) {
        return span(at);
    }
    let mut next = letters.iter();
    let mut want = next.next();
    let mut found = Vec::new();
    for (at, c) in chars.iter().enumerate() {
        if Some(c) == want {
            found.push(at);
            want = next.next();
        }
    }
    found
}

/// `items` that answer `query`, best first; equals keep their order.
pub fn rank<T>(query: &str, items: Vec<T>, text: impl Fn(&T) -> String) -> Vec<T> {
    let mut scored: Vec<(u32, T)> = items
        .into_iter()
        .filter_map(|item| score(query, &text(&item)).map(|score| (score, item)))
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored.into_iter().map(|(_, item)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(query: &str, items: &[&str]) -> Vec<String> {
        rank(
            query,
            items.iter().map(|s| (*s).to_owned()).collect(),
            |s| s.clone(),
        )
    }

    #[test]
    fn the_whole_name_beats_a_prefix_which_beats_a_word_which_beats_anywhere() {
        assert_eq!(
            names(
                "new",
                &[
                    "Renew lease",
                    "Brand new day",
                    "New worktree",
                    "New",
                    "Next"
                ]
            ),
            ["New", "New worktree", "Brand new day", "Renew lease"]
        );
    }

    #[test]
    fn among_names_that_start_with_the_query_the_shorter_one_wins() {
        assert_eq!(names("ap", &["api-gateway", "api"]), ["api", "api-gateway"]);
    }

    #[test]
    fn initials_find_a_multi_word_name() {
        assert_eq!(
            names("nw", &["Remove worktree", "New worktree"]),
            ["New worktree"]
        );
    }

    #[test]
    fn letters_in_order_find_a_name_but_a_single_letter_does_not() {
        assert_eq!(names("ngr", &["New chat", "New group"]), ["New group"]);
        assert_eq!(score("x", "Settings"), None);
    }

    #[test]
    fn case_and_surrounding_spaces_do_not_matter() {
        assert_eq!(names("  LEON ", &["leon core"]), ["leon core"]);
    }

    #[test]
    fn equals_keep_their_order_and_an_empty_query_keeps_everything() {
        assert_eq!(names("a", &["Ana", "Abe"]), ["Ana", "Abe"]);
        assert_eq!(names("", &["b", "a"]), ["b", "a"]);
    }

    #[test]
    fn the_matched_characters_are_the_ones_the_tier_counted() {
        assert_eq!(matched_chars("api", "api-gateway"), [0, 1, 2]);
        assert_eq!(matched_chars("nw", "New worktree"), [0, 4]);
        assert_eq!(matched_chars("work", "New worktree"), [4, 5, 6, 7]);
        assert_eq!(matched_chars("ork", "New worktree"), [5, 6, 7]);
        assert_eq!(matched_chars("ngr", "New group"), [0, 4, 5]);
        assert_eq!(matched_chars("LEON", "leon core"), [0, 1, 2, 3]);
        assert!(matched_chars("zz", "alpha").is_empty());
        assert!(matched_chars("", "alpha").is_empty());
    }

    #[test]
    fn a_name_without_the_letters_is_dropped() {
        assert_eq!(names("zz", &["alpha", "beta"]), Vec::<String>::new());
    }
}
