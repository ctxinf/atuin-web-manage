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

static INDEX_HTML: &str = include_str!("../static/index.html");

#[derive(Parser, Debug)]
#[command(about = "Local web UI to manage atuin shell history")]
struct Args {
    /// Address to bind, e.g. 127.0.0.1:8787
    #[arg(long, default_value = "127.0.0.1:8787")]
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
    let needle = q.q.trim().to_string();

    let mut where_clauses: Vec<&str> = vec!["deleted_at IS NULL"];
    let mut bind: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let like = format!("%{}%", needle);
    if !needle.is_empty() {
        where_clauses.push("command LIKE ?");
        bind.push(Box::new(like));
    }
    if q.multiline {
        where_clauses.push("instr(command, char(10)) > 0");
    }
    let where_sql = where_clauses.join(" AND ");

    let conn = state.conn.lock().unwrap();

    let count_sql = format!("SELECT COUNT(*) FROM history WHERE {}", where_sql);
    let list_sql = format!(
        "SELECT id, timestamp, duration, exit, command, cwd, hostname
         FROM history WHERE {} ORDER BY timestamp DESC LIMIT ? OFFSET ?",
        where_sql
    );

    let count_params: Vec<&dyn rusqlite::ToSql> = bind.iter().map(|b| b.as_ref()).collect();
    let total: i64 = conn
        .query_row(&count_sql, count_params.as_slice(), |r| r.get(0))
        .map_err(map_err)?;

    let limit_box: Box<dyn rusqlite::ToSql> = Box::new(limit);
    let offset_box: Box<dyn rusqlite::ToSql> = Box::new(offset);
    let mut list_bind = bind;
    list_bind.push(limit_box);
    list_bind.push(offset_box);
    let list_params: Vec<&dyn rusqlite::ToSql> = list_bind.iter().map(|b| b.as_ref()).collect();

    let mut stmt = conn.prepare(&list_sql).map_err(map_err)?;
    let rows = stmt
        .query_map(list_params.as_slice(), row_to_history)
        .map_err(map_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_err)?;

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

async fn stats(
    State(state): State<AppState>,
) -> Result<Json<StatsResp>, (StatusCode, String)> {
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
