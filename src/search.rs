//! Default GLOBAL fuzzy search, pinned to Atuin v18.16.1.
//! SQL construction and ranking adapted from atuin-client database.rs/ordering.rs.
//! Copyright (c) 2021 Ellie Huxtable. MIT; see ../licenses/Atuin-MIT.txt.
use crate::{
    atuin_query::{QueryToken, QueryTokenizer},
    HistoryRow,
};
use rusqlite::{functions::FunctionFlags, Connection};
use sql_builder::{bind::Bind, quote, SqlBuilder};

pub fn register(conn: &Connection) -> rusqlite::Result<()> {
    conn.create_scalar_function(
        "regexp",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let regex = ctx.get_or_create_aux(0, |value| {
                regex::Regex::new(value.as_str()?)
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
            })?;
            Ok(regex.is_match(ctx.get_raw(1).as_str()?))
        },
    )
}

pub fn valid_regexes(query: &str) -> bool {
    QueryTokenizer::new(query).all(|token| match token {
        QueryToken::Regex(pattern) => regex::Regex::new(pattern).is_ok(),
        _ => true,
    })
}

pub fn query(query: &str, relevance: bool, multiline: bool, has_author: bool) -> String {
    let mut sql = SqlBuilder::select_from("history");
    sql.field("id, timestamp, duration, exit, command, cwd, hostname");
    sql.order_desc("timestamp");
    if relevance {
        sql.group_by("command").having("max(timestamp)").limit(200);
    }
    let mut regexes = Vec::new();
    let mut is_or = false;
    for token in QueryTokenizer::new(query) {
        // TODO smart case mode could be made configurable like in fzf
        let (is_glob, glob) = if token.has_uppercase() {
            (true, "*")
        } else {
            (false, "%")
        };
        let param = match token {
            QueryToken::Regex(r) => {
                regexes.push(String::from(r));
                continue;
            }
            QueryToken::Or => {
                if !is_or {
                    is_or = true;
                    continue;
                } else {
                    format!("{glob}|{glob}")
                }
            }
            QueryToken::MatchStart(term, _) => {
                format!("{term}{glob}")
            }
            QueryToken::MatchEnd(term, _) => {
                format!("{glob}{term}")
            }
            QueryToken::MatchFull(term, _) => {
                format!("{glob}{term}{glob}")
            }
            QueryToken::Match(term, _) => term.split("").collect::<Vec<_>>().join(glob),
        };

        let op = if is_glob { "GLOB" } else { "LIKE" };
        let inverse = if token.is_inverse() { "NOT " } else { "" };
        let condition = format!("command {inverse}{op} {}", quote(param));
        if is_or {
            sql.or_where(condition);
        } else {
            sql.and_where(condition);
        }
        is_or = false;
    }

    for regex in regexes {
        sql.and_where("command regexp ?".bind(&regex));
    }
    if relevance {
        // Atuin's interactive engine uses $all-user, including hostname fallback.
        let author = "CASE WHEN instr(hostname, ':') > 0 THEN substr(hostname, instr(hostname, ':') + 1) ELSE hostname END";
        let author = if has_author {
            format!("CASE WHEN author IS NULL OR trim(author) = '' THEN {author} ELSE author END")
        } else {
            author.to_owned()
        };
        sql.and_where(format!(
            "{author} NOT IN ('claude-code', 'codex', 'copilot', 'pi')"
        ));
    }
    sql.and_where_is_null("deleted_at");
    if multiline {
        sql.and_where("instr(command, char(10)) > 0");
    }
    sql.sql().expect("valid static SQL structure")
}

pub fn reorder(query: &str, rows: &mut [HistoryRow]) {
    // Keep upstream's raw-query scoring, case sensitivity and fallback score.
    // Do not strip operators/spaces: that would change v18.16.1 ordering.
    let query = query.chars().collect();
    let count = rows.len();
    rows.sort_by_cached_key(|row| {
        let (from, to) =
            minspan::minspan::span(&query, &row.command.chars().collect()).unwrap_or((0, count));
        1 + to - from
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{list_history, AppState, ListQuery, SortOrder};
    use axum::{
        extract::{Query, State},
        Json,
    };
    use rusqlite::params;
    use std::sync::{Arc, Mutex};

    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        register(&conn).unwrap();
        conn.execute_batch(
            "CREATE TABLE history (
            id TEXT, timestamp INTEGER, duration INTEGER DEFAULT 0, exit INTEGER DEFAULT 0,
            command TEXT, cwd TEXT DEFAULT '', hostname TEXT DEFAULT 'host:user',
            author TEXT, deleted_at INTEGER
        )",
        )
        .unwrap();
        conn
    }

    fn insert(conn: &Connection, id: i64, command: &str) {
        conn.execute(
            "INSERT INTO history(id,timestamp,command) VALUES (?,?,?)",
            params![id.to_string(), id, command],
        )
        .unwrap();
    }

    fn matches(input: &str, command: &str) -> bool {
        let conn = database();
        insert(&conn, 1, command);
        let result = conn
            .prepare(&query(input, true, false, true))
            .unwrap()
            .query_map([], crate::row_to_history)
            .unwrap()
            .count()
            == 1;
        result
    }

    #[test]
    fn cli_query_syntax() {
        for (q, cmd, expected) in [
            ("A B C", "A yy B yxxx C lyy", true),
            ("A B C", "C xxx A yy B", true),
            ("ABC", "A yy B yxxx C", true),
            ("ABC", "C xxx B yy A", false),
            ("git", "GIT status", true),
            ("Git", "git status", false),
            ("你好", "echo 你 好", true),
            ("'git", "g i t", false),
            ("^git", "sudo git status", false),
            ("log$", "git log", true),
            ("!^sudo", "sudo git log", false),
            ("!log$", "git log", false),
            ("!git", "g i t", false), // upstream negation is fuzzy unless quoted
            ("!'git", "g i t", true),
            ("r/^git (log|status)$/", "git status", true),
            ("r/^git (log|status)$/", "sudo git status", false),
            ("r/git / !push", "git status", true),
            ("'a_b", "axb", true), // upstream SQL wildcards
            ("'a%b", "axxxb", true),
            ("'A?B", "AxB", true), // uppercase uses GLOB
            ("'it's", "echo it's fine", true),
            ("';DROP", "ordinary", false),
            ("", "anything", true),
        ] {
            assert_eq!(matches(q, cmd), expected, "query={q:?}, command={cmd:?}");
        }
        assert!(!valid_regexes("r/[/"));
    }

    async fn request(
        state: &AppState,
        q: &str,
        sort: SortOrder,
        offset: i64,
        limit: i64,
    ) -> crate::ListResp {
        let Json(result) = list_history(
            State(state.clone()),
            Query(ListQuery {
                q: q.into(),
                sort,
                offset,
                limit,
                multiline: false,
            }),
        )
        .await
        .unwrap();
        result
    }

    #[tokio::test]
    async fn relevance_ranks_before_pagination_and_time_keeps_duplicates() {
        let conn = database();
        insert(&conn, 1, "sshuttle -r u1 pc");
        insert(&conn, 2, "ssh -p 28022 u1 pc");
        insert(&conn, 3, "ssh -p 28022 u1 pc");
        let state = AppState {
            conn: Arc::new(Mutex::new(conn)),
        };
        let first = request(&state, "ssh u1 pc", SortOrder::Relevance, 0, 1).await;
        assert_eq!(first.total, 2);
        assert_eq!(first.rows[0].id, "1");
        let second = request(&state, "ssh u1 pc", SortOrder::Relevance, 1, 1).await;
        assert_eq!(second.rows[0].id, "3");
        let time = request(&state, "ssh u1 pc", SortOrder::Time, 0, 50).await;
        assert_eq!(time.total, 3);
        assert_eq!(
            time.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["3", "2", "1"]
        );
        let invalid = request(&state, "r/[/", SortOrder::Relevance, 0, 50).await;
        assert_eq!(invalid.total, 0);
    }

    #[tokio::test]
    async fn cli_window_filters_and_empty_search() {
        let conn = database();
        insert(&conn, 1, "git");
        for id in 2..=205 {
            insert(&conn, id, &format!("git command {id}"));
        }
        conn.execute("UPDATE history SET author='codex' WHERE id='205'", [])
            .unwrap();
        conn.execute("UPDATE history SET deleted_at=1 WHERE id='204'", [])
            .unwrap();
        conn.execute(
            "UPDATE history SET hostname='host:claude-code' WHERE id='203'",
            [],
        )
        .unwrap();
        let state = AppState {
            conn: Arc::new(Mutex::new(conn)),
        };
        let relevance = request(&state, "git", SortOrder::Relevance, 0, 500).await;
        assert_eq!(relevance.total, 200);
        assert!(!relevance
            .rows
            .iter()
            .any(|r| ["1", "203", "204", "205"].contains(&r.id.as_str())));
        let empty = request(&state, "", SortOrder::Relevance, 0, 1).await;
        assert_eq!(empty.rows[0].id, "202");
        let time = request(&state, "git", SortOrder::Time, 0, 500).await;
        assert_eq!(time.total, 204);
        assert_eq!(time.rows[0].id, "205");
    }

    #[test]
    fn ranking_keeps_upstream_raw_query_fallback_and_stable_ties() {
        let conn = database();
        for (id, command) in [(1, "GIT"), (2, "git"), (3, "g i t")] {
            insert(&conn, id, command);
        }
        let mut stmt = conn.prepare(&query("git", true, false, true)).unwrap();
        let mut rows = stmt
            .query_map([], crate::row_to_history)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        reorder("git", &mut rows);
        assert_eq!(
            rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["2", "1", "3"]
        );
    }
}
