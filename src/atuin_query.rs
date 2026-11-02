// Vendored from atuinsh/atuin v18.16.1, crates/atuin-client/src/database.rs.
// Copyright (c) 2021 Ellie Huxtable. MIT; see ../licenses/Atuin-MIT.txt.

pub struct QueryTokenizer<'a> {
    query: &'a str,
    last_pos: usize,
}

pub enum QueryToken<'a> {
    Match(&'a str, bool),
    MatchStart(&'a str, bool),
    MatchEnd(&'a str, bool),
    MatchFull(&'a str, bool),
    Or,
    Regex(&'a str),
}

impl<'a> QueryToken<'a> {
    pub fn has_uppercase(&self) -> bool {
        match self {
            Self::Match(term, _)
            | Self::MatchStart(term, _)
            | Self::MatchEnd(term, _)
            | Self::MatchFull(term, _) => term.contains(char::is_uppercase),
            _ => false,
        }
    }

    pub fn is_inverse(&self) -> bool {
        match self {
            Self::Match(_, inv)
            | Self::MatchStart(_, inv)
            | Self::MatchEnd(_, inv)
            | Self::MatchFull(_, inv) => *inv,
            _ => false,
        }
    }
}

impl<'a> QueryTokenizer<'a> {
    pub fn new(query: &'a str) -> Self {
        Self { query, last_pos: 0 }
    }
}

impl<'a> Iterator for QueryTokenizer<'a> {
    type Item = QueryToken<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        let remaining = &self.query[self.last_pos..];
        if remaining.is_empty() {
            return None;
        }

        if let Some(remaining) = remaining.strip_prefix("r/") {
            let (regex, next_pos) = if let Some(end) = remaining.find("/ ") {
                (&remaining[..end], self.last_pos + 2 + end + 2)
            } else if let Some(remaining) = remaining.strip_suffix('/') {
                (remaining, self.query.len())
            } else {
                (remaining, self.query.len())
            };
            self.last_pos = next_pos;
            Some(QueryToken::Regex(regex))
        } else {
            let (mut part, next_pos) = if let Some(sp) = remaining.find(' ') {
                (&remaining[..sp], self.last_pos + sp + 1)
            } else {
                (remaining, self.query.len())
            };
            self.last_pos = next_pos;

            if part == "|" {
                return Some(QueryToken::Or);
            }

            let mut is_inverse = false;
            if let Some(s) = part.strip_prefix('!') {
                part = s;
                is_inverse = true;
            }
            let token = if let Some(s) = part.strip_prefix('^') {
                QueryToken::MatchStart(s, is_inverse)
            } else if let Some(s) = part.strip_suffix('$') {
                QueryToken::MatchEnd(s, is_inverse)
            } else if let Some(s) = part.strip_prefix('\'') {
                QueryToken::MatchFull(s, is_inverse)
            } else {
                QueryToken::Match(part, is_inverse)
            };
            Some(token)
        }
    }
}
