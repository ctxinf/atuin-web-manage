use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, Json},
    routing::{get, patch, post},
    Router,
};
use clap::Parser;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

mod atuin_query;
mod search;

static INDEX_HTML: &str = include_str!("../static/index.html");

#[derive(Parser, Debug)]
#[command(about = "Local web UI to manage atuin shell history")]
struct Args {
    /// Address to bind, e.g. 127.0.0.1:28057
    #[arg(long, default_value = "127.0.0.1:28057")]
    bind: String,

    /// Path to atuin history.db (default: ~/.local/share/atuin/history.db)
    #[arg(long)]
    db: Option<PathBuf>,
}

#[derive(Clone)]
struct AppState {
    conn: Arc<Mutex<Connection>>,
}

#[derive(Debug, Serialize)]
struct HistoryRow {
    id: String,
    timestamp: i64,
    duration: i64,
    exit: i64,
    command: String,
    cwd: String,
    hostname: String,
}

#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum SortOrder {
    Time,
    #[default]
    Relevance,
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default)]
    q: String,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
    #[serde(default)]
    multiline: bool,
    #[serde(default)]
    sort: SortOrder,
}
fn default_limit() -> i64 {
    50
}

#[derive(Debug, Serialize)]
struct ListResp {
    total: i64,
    rows: Vec<HistoryRow>,
}

#[derive(Debug, Deserialize)]
struct DeleteReq {
    ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct EditReq {
    command: String,
}

#[derive(Debug, Serialize)]
struct AffectedResp {
    affected: usize,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().init();

    let args = Args::parse();

    let db_path = match args.db {
        Some(p) => p,
        None => {
            let home = std::env::var("HOME").map_err(|_| "HOME not set")?;
            PathBuf::from(home).join(".local/share/atuin/history.db")
        }
    };
    if !db_path.exists() {
        return Err(format!("db not found: {}", db_path.display()).into());
    }

    let conn = Connection::open(&db_path)?;
    search::register(&conn)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;

    let addr: SocketAddr = args.bind.parse()?;
    if !addr.ip().is_loopback() {
        eprintln!(
            "WARNING: binding to non-loopback address {}. Anyone reachable on this interface can read and modify your shell history.",
            addr
        );
    }

    let state = AppState {
        conn: Arc::new(Mutex::new(conn)),
    };

    let app = Router::new()
        .route("/", get(index))
        .route("/api/history", get(list_history))
        .route("/api/stats", get(stats))
        .route("/api/delete", post(delete_history))
        .route("/api/dedupe/preview", get(dedupe_preview))
        .route("/api/dedupe", post(dedupe))
        .route("/api/edit/:id", patch(edit_history))
        .with_state(state);

    tracing::info!("listening on http://{}  (db: {})", addr, db_path.display());
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

fn map_err<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

async fn list_history(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<ListResp>, (StatusCode, String)> {
    let limit = q.limit.clamp(1, 500);
    let offset = q.offset.max(0);
    if !search::valid_regexes(&q.q) {
        return Ok(Json(ListResp {
            total: 0,
            rows: Vec::new(),
        }));
    }
    let conn = state.conn.lock().unwrap();
    let has_author = conn.prepare("SELECT author FROM history LIMIT 0").is_ok();
    let relevance = q.sort == SortOrder::Relevance;
    let sql = search::query(&q.q, relevance, q.multiline, has_author);
    let inner = sql.trim().trim_end_matches(';');
    let (total, rows) = if relevance {
        // Match CLI: newest 200 distinct results first, rank that entire set,
        // then apply web pagination. Ranking a page would change the results.
        let mut stmt = conn.prepare(inner).map_err(map_err)?;
        let mut rows = stmt
            .query_map([], row_to_history)
            .map_err(map_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_err)?;
        if !q.q.is_empty() {
            search::reorder(&q.q, &mut rows);
        }
        let total = rows.len() as i64;
        let page = rows
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect();
        (total, page)
    } else {
        let total = conn
            .query_row(&format!("SELECT count(*) FROM ({inner})"), [], |r| r.get(0))
            .map_err(map_err)?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT * FROM ({inner}) ORDER BY timestamp DESC, id DESC LIMIT ? OFFSET ?"
            ))
            .map_err(map_err)?;
        let rows = stmt
            .query_map(params![limit, offset], row_to_history)
            .map_err(map_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_err)?;
        (total, rows)
    };

    Ok(Json(ListResp { total, rows }))
}

fn row_to_history(r: &rusqlite::Row) -> rusqlite::Result<HistoryRow> {
    Ok(HistoryRow {
        id: r.get(0)?,
        timestamp: r.get(1)?,
        duration: r.get(2)?,
        exit: r.get(3)?,
        command: r.get(4)?,
        cwd: r.get(5)?,
        hostname: r.get(6)?,
    })
}

async fn delete_history(
    State(state): State<AppState>,
    Json(req): Json<DeleteReq>,
) -> Result<Json<AffectedResp>, (StatusCode, String)> {
    if req.ids.is_empty() {
        return Ok(Json(AffectedResp { affected: 0 }));
    }
    let mut conn = state.conn.lock().unwrap();
    let tx = conn.transaction().map_err(map_err)?;
    let mut affected = 0usize;
    {
        let mut stmt = tx
            .prepare("DELETE FROM history WHERE id = ?1")
            .map_err(map_err)?;
        for id in &req.ids {
            affected += stmt.execute(params![id]).map_err(map_err)?;
        }
    }
    tx.commit().map_err(map_err)?;
    Ok(Json(AffectedResp { affected }))
}

#[derive(Debug, Serialize)]
struct StatsResp {
    total: i64,
    distinct_commands: i64,
    oldest_ts: Option<i64>,
    newest_ts: Option<i64>,
}

async fn stats(State(state): State<AppState>) -> Result<Json<StatsResp>, (StatusCode, String)> {
    let conn = state.conn.lock().unwrap();
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM history WHERE deleted_at IS NULL",
            [],
            |r| r.get(0),
        )
        .map_err(map_err)?;
    let distinct_commands: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT command) FROM history WHERE deleted_at IS NULL",
            [],
            |r| r.get(0),
        )
        .map_err(map_err)?;
    let (oldest_ts, newest_ts): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT MIN(timestamp), MAX(timestamp) FROM history WHERE deleted_at IS NULL",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(map_err)?;
    Ok(Json(StatsResp {
        total,
        distinct_commands,
        oldest_ts,
        newest_ts,
    }))
}

#[derive(Debug, Serialize)]
struct DedupePreviewResp {
    duplicates: i64,
    total: i64,
    distinct_commands: i64,
}

// A row is a duplicate iff another row with the same command has a strictly
// greater timestamp, or the same timestamp and a greater id. Keeps exactly one
// row per distinct command (the newest).
const DEDUPE_WHERE: &str = "deleted_at IS NULL AND EXISTS (
    SELECT 1 FROM history h2
    WHERE h2.deleted_at IS NULL
      AND h2.command = history.command
      AND (h2.timestamp > history.timestamp
           OR (h2.timestamp = history.timestamp AND h2.id > history.id))
)";

async fn dedupe_preview(
    State(state): State<AppState>,
) -> Result<Json<DedupePreviewResp>, (StatusCode, String)> {
    let conn = state.conn.lock().unwrap();
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM history WHERE deleted_at IS NULL",
            [],
            |r| r.get(0),
        )
        .map_err(map_err)?;
    let distinct_commands: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT command) FROM history WHERE deleted_at IS NULL",
            [],
            |r| r.get(0),
        )
        .map_err(map_err)?;
    Ok(Json(DedupePreviewResp {
        duplicates: total - distinct_commands,
        total,
        distinct_commands,
    }))
}

async fn dedupe(State(state): State<AppState>) -> Result<Json<AffectedResp>, (StatusCode, String)> {
    let conn = state.conn.lock().unwrap();
    let sql = format!("DELETE FROM history WHERE {}", DEDUPE_WHERE);
    let affected = conn.execute(&sql, []).map_err(map_err)?;
    Ok(Json(AffectedResp { affected }))
}

async fn edit_history(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<EditReq>,
) -> Result<Json<AffectedResp>, (StatusCode, String)> {
    let conn = state.conn.lock().unwrap();
    let affected = conn
        .execute(
            "UPDATE history SET command = ?1 WHERE id = ?2",
            params![req.command, id],
        )
        .map_err(map_err)?;
    Ok(Json(AffectedResp { affected }))
}

#[cfg(test)]
mod search_api_tests {
    use super::*;

    #[tokio::test]
    async fn search_pagination_counts_duplicates_and_multiline() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE history (
                id TEXT, timestamp INTEGER, duration INTEGER DEFAULT 0,
                exit INTEGER DEFAULT 0, command TEXT, cwd TEXT DEFAULT '',
                hostname TEXT DEFAULT '', deleted_at INTEGER
            );",
        )
        .unwrap();
        for (id, timestamp, command, deleted) in [
            ("one", 1, "A yy B yxxx C lyy", None),
            ("two", 2, "A yy B yxxx C lyy", None),
            ("three", 3, "A\nB\nC", None),
            ("four", 4, "A B C", Some(4)),
            ("five", 5, "A B", None),
        ] {
            conn.execute(
                "INSERT INTO history (id, timestamp, command, deleted_at) VALUES (?, ?, ?, ?)",
                params![id, timestamp, command, deleted],
            )
            .unwrap();
        }
        let state = AppState {
            conn: Arc::new(Mutex::new(conn)),
        };
        for (offset, multiline, total, expected) in [
            (0, false, 3, "three"),
            (1, false, 3, "two"),
            (2, false, 3, "one"),
            (0, true, 1, "three"),
        ] {
            let Json(result) = list_history(
                State(state.clone()),
                Query(ListQuery {
                    q: "A B C".into(),
                    limit: 1,
                    offset,
                    multiline,
                    sort: SortOrder::Time,
                }),
            )
            .await
            .unwrap();
            assert_eq!(result.total, total);
            assert_eq!(result.rows.len(), 1);
            assert_eq!(result.rows[0].id, expected);
        }
    }
}
