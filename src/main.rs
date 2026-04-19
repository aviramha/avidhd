use serde::{Deserialize, Serialize};
use tauri::{
    Manager, WebviewUrl, WebviewWindowBuilder,
    menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder},
};
use turso::params;

#[derive(Serialize, Deserialize, Clone)]
struct Item {
    id: i64,
    text: String,
    position: i64,
    status: String,
    snoozed_until: Option<i64>,
    kind: String, // "task" | "pr"
    pr_url: Option<String>,
    pr_repo: Option<String>,
    pr_number: Option<i64>,
    pr_role: Option<String>, // "authored" | "review_requested"
    pr_draft: Option<bool>,
}

struct AppState {
    db: turso::Database,
    token_cache: std::sync::Mutex<Option<String>>,
}

// ── Settings window ───────────────────────────────────────────────────────────

fn open_settings_window(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("Settings — avidhd")
        .inner_size(460.0, 340.0)
        .resizable(false)
        .build();
}

#[tauri::command]
fn open_settings(app: tauri::AppHandle) {
    open_settings_window(&app);
}

// ── DB commands ───────────────────────────────────────────────────────────────

const SELECT_COLS: &str = "id, text, position, status, snoozed_until, kind, pr_url, pr_repo, pr_number, pr_role, pr_draft";

fn map_item(row: &turso::Row) -> Result<Item, String> {
    Ok(Item {
        id: row.get(0).map_err(|e| e.to_string())?,
        text: row.get(1).map_err(|e| e.to_string())?,
        position: row.get(2).map_err(|e| e.to_string())?,
        status: row.get(3).map_err(|e| e.to_string())?,
        snoozed_until: row.get(4).ok(),
        kind: row.get::<String>(5).unwrap_or_else(|_| "task".into()),
        pr_url: row.get(6).ok(),
        pr_repo: row.get(7).ok(),
        pr_number: row.get(8).ok(),
        pr_role: row.get(9).ok(),
        pr_draft: row.get::<i64>(10).ok().map(|v| v != 0),
    })
}

#[tauri::command]
async fn get_items(
    state: tauri::State<'_, AppState>,
    include_done: bool,
    include_snoozed: bool,
    query: String,
) -> Result<Vec<Item>, String> {
    let conn = state.db.connect().map_err(|e| e.to_string())?;
    let search = format!("%{}%", query);

    let sql = if include_done {
        format!(
            "SELECT {SELECT_COLS} FROM items \
             WHERE text LIKE ?1 \
             ORDER BY CASE WHEN status = 'open' THEN 0 ELSE 1 END, position ASC"
        )
    } else if include_snoozed {
        format!(
            "SELECT {SELECT_COLS} FROM items \
             WHERE status = 'open' AND text LIKE ?1 \
             ORDER BY \
               CASE WHEN snoozed_until IS NOT NULL \
                    AND snoozed_until > CAST(strftime('%s','now') AS INTEGER) \
                    THEN 1 ELSE 0 END, \
               position ASC"
        )
    } else {
        format!(
            "SELECT {SELECT_COLS} FROM items \
             WHERE status = 'open' \
               AND (snoozed_until IS NULL \
                    OR snoozed_until <= CAST(strftime('%s','now') AS INTEGER)) \
               AND text LIKE ?1 \
             ORDER BY position ASC"
        )
    };

    let mut rows = conn
        .query(&sql, params![search])
        .await
        .map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        items.push(map_item(&row)?);
    }
    Ok(items)
}

#[tauri::command]
async fn add_item(state: tauri::State<'_, AppState>, text: String) -> Result<Item, String> {
    let conn = state.db.connect().map_err(|e| e.to_string())?;

    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM items WHERE status = 'open'",
            (),
        )
        .await
        .map_err(|e| e.to_string())?;

    let next_pos: i64 = if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        row.get(0).unwrap_or(0)
    } else {
        0
    };

    conn.execute(
        "INSERT INTO items (text, position, status, kind) VALUES (?1, ?2, 'open', 'task')",
        params![text.clone(), next_pos],
    )
    .await
    .map_err(|e| e.to_string())?;

    let mut rows = conn
        .query("SELECT last_insert_rowid()", ())
        .await
        .map_err(|e| e.to_string())?;

    let id: i64 = if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        row.get(0).unwrap_or(0)
    } else {
        0
    };

    Ok(Item {
        id,
        text,
        position: next_pos,
        status: "open".into(),
        snoozed_until: None,
        kind: "task".into(),
        pr_url: None,
        pr_repo: None,
        pr_number: None,
        pr_role: None,
        pr_draft: None,
    })
}

#[tauri::command]
async fn close_item(state: tauri::State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.db.connect().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE items SET status = 'done' WHERE id = ?1",
        params![id],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn reopen_item(state: tauri::State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.db.connect().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE items SET status = 'open' WHERE id = ?1",
        params![id],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn delete_item(state: tauri::State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.db.connect().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM items WHERE id = ?1", params![id])
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn update_item(
    state: tauri::State<'_, AppState>,
    id: i64,
    text: String,
) -> Result<(), String> {
    let conn = state.db.connect().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE items SET text = ?1 WHERE id = ?2",
        params![text, id],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn reorder_items(state: tauri::State<'_, AppState>, ids: Vec<i64>) -> Result<(), String> {
    let conn = state.db.connect().map_err(|e| e.to_string())?;
    for (pos, id) in ids.iter().enumerate() {
        conn.execute(
            "UPDATE items SET position = ?1 WHERE id = ?2",
            params![pos as i64, *id],
        )
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn snooze_item(state: tauri::State<'_, AppState>, id: i64, until: i64) -> Result<(), String> {
    let conn = state.db.connect().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE items SET snoozed_until = ?1 WHERE id = ?2",
        params![until, id],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn unsnooze_item(state: tauri::State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.db.connect().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE items SET snoozed_until = NULL WHERE id = ?1",
        params![id],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn clear_done_items(state: tauri::State<'_, AppState>) -> Result<u64, String> {
    let conn = state.db.connect().map_err(|e| e.to_string())?;
    let rows_affected = conn
        .execute("DELETE FROM items WHERE status = 'done'", ())
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows_affected)
}

#[tauri::command]
fn get_db_path() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    format!("{}/.avidhd/data.db", home)
}

// ── GitHub PAT (stored in OS keyring) ────────────────────────────────────────

fn keyring_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new("avidhd", "github_token").map_err(|e| e.to_string())
}

fn token_from_state(state: &AppState) -> Result<String, String> {
    {
        let cache = state.token_cache.lock().unwrap();
        if let Some(ref t) = *cache {
            return Ok(t.clone());
        }
    }
    let token = match keyring_entry()?.get_password() {
        Ok(t) => t,
        Err(keyring::Error::NoEntry) => String::new(),
        Err(e) => return Err(e.to_string()),
    };
    *state.token_cache.lock().unwrap() = Some(token.clone());
    Ok(token)
}

#[tauri::command]
fn save_github_token(state: tauri::State<'_, AppState>, token: String) -> Result<(), String> {
    keyring_entry()?
        .set_password(&token)
        .map_err(|e| e.to_string())?;
    *state.token_cache.lock().unwrap() = Some(token);
    Ok(())
}

#[tauri::command]
fn get_github_token(state: tauri::State<'_, AppState>) -> Result<String, String> {
    token_from_state(&state)
}

#[tauri::command]
fn clear_github_token(state: tauri::State<'_, AppState>) -> Result<(), String> {
    match keyring_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(e) => return Err(e.to_string()),
    }
    *state.token_cache.lock().unwrap() = Some(String::new());
    Ok(())
}

#[tauri::command]
async fn verify_github_token(token: String) -> Result<String, String> {
    let client = reqwest::Client::new();
    let res = client
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "avidhd")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if res.status().is_success() {
        let json: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
        Ok(json["login"].as_str().unwrap_or("unknown").to_string())
    } else {
        Err(format!("GitHub returned {}", res.status()))
    }
}

// ── Pull Request sync ─────────────────────────────────────────────────────────

struct FetchedPr {
    github_id: i64,
    title: String,
    url: String,
    repo: String,
    number: i64,
    role: String,
    draft: bool,
}

async fn fetch_github_prs(token: &str) -> Result<Vec<FetchedPr>, String> {
    let client = reqwest::Client::new();
    let mut prs: Vec<FetchedPr> = Vec::new();
    let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();

    let queries: &[(&str, &str)] = &[
        ("is:open is:pr author:@me archived:false", "authored"),
        (
            "is:open is:pr review-requested:@me archived:false",
            "review_requested",
        ),
    ];

    for (q, role) in queries {
        let url = format!(
            "https://api.github.com/search/issues?q={}&per_page=50&sort=updated",
            q.replace(' ', "+")
        );
        let body: serde_json::Value = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", "avidhd")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;

        for item in body["items"].as_array().unwrap_or(&vec![]) {
            let id = item["id"].as_i64().unwrap_or(0);
            if !seen.insert(id) {
                continue;
            }
            let html_url = item["html_url"].as_str().unwrap_or("").to_string();
            let parts: Vec<&str> = html_url.splitn(8, '/').collect();
            let repo = if parts.len() >= 5 {
                format!("{}/{}", parts[3], parts[4])
            } else {
                String::new()
            };
            prs.push(FetchedPr {
                github_id: id,
                title: item["title"].as_str().unwrap_or("").to_string(),
                url: html_url,
                repo,
                number: item["number"].as_i64().unwrap_or(0),
                role: role.to_string(),
                draft: item["draft"].as_bool().unwrap_or(false),
            });
        }
    }
    Ok(prs)
}

/// Fetch open PRs from GitHub and upsert them as items.
/// PRs that have disappeared (merged/closed) are marked done.
#[tauri::command]
async fn sync_pull_requests(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let token = token_from_state(&state)?;
    if token.is_empty() {
        return Ok(());
    }

    let fetched = fetch_github_prs(&token).await?;
    let fetched_ids: std::collections::HashSet<i64> = fetched.iter().map(|p| p.github_id).collect();

    let conn = state.db.connect().map_err(|e| e.to_string())?;

    // Collect existing open PR github_ids from DB
    let mut rows = conn
        .query(
            "SELECT pr_id FROM items WHERE kind = 'pr' AND status = 'open' AND pr_id IS NOT NULL",
            (),
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut existing_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        if let Ok(id) = row.get::<i64>(0) {
            existing_ids.insert(id);
        }
    }

    // Upsert fetched PRs
    for pr in &fetched {
        if existing_ids.contains(&pr.github_id) {
            conn.execute(
                "UPDATE items SET text = ?1, pr_role = ?2, pr_draft = ?3 \
                 WHERE pr_id = ?4 AND kind = 'pr'",
                params![
                    pr.title.clone(),
                    pr.role.clone(),
                    pr.draft as i64,
                    pr.github_id
                ],
            )
            .await
            .map_err(|e| e.to_string())?;
        } else {
            let mut pos_rows = conn
                .query(
                    "SELECT COALESCE(MAX(position) + 1, 0) FROM items WHERE status = 'open'",
                    (),
                )
                .await
                .map_err(|e| e.to_string())?;
            let pos: i64 = if let Some(r) = pos_rows.next().await.map_err(|e| e.to_string())? {
                r.get(0).unwrap_or(0)
            } else {
                0
            };
            conn.execute(
                "INSERT INTO items \
                 (text, position, status, kind, pr_url, pr_repo, pr_number, pr_role, pr_draft, pr_id) \
                 VALUES (?1, ?2, 'open', 'pr', ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    pr.title.clone(),
                    pos,
                    pr.url.clone(),
                    pr.repo.clone(),
                    pr.number,
                    pr.role.clone(),
                    pr.draft as i64,
                    pr.github_id
                ],
            )
            .await
            .map_err(|e| e.to_string())?;
        }
    }

    // Mark closed PRs (disappeared from GitHub) as done
    for old_id in &existing_ids {
        if !fetched_ids.contains(old_id) {
            conn.execute(
                "UPDATE items SET status = 'done' WHERE pr_id = ?1 AND kind = 'pr'",
                params![*old_id],
            )
            .await
            .map_err(|e| e.to_string())?;
        }
    }

    Ok(())
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    opener::open(url).map_err(|e| e.to_string())
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            // Menu
            let settings_item = MenuItemBuilder::with_id("settings", "Settings...")
                .accelerator("CmdOrCtrl+,")
                .build(app)?;

            let menu = MenuBuilder::new(app)
                .items(&[
                    &SubmenuBuilder::new(app, "avidhd")
                        .item(&settings_item)
                        .separator()
                        .quit()
                        .build()?,
                    &SubmenuBuilder::new(app, "Edit")
                        .undo()
                        .redo()
                        .separator()
                        .cut()
                        .copy()
                        .paste()
                        .separator()
                        .select_all()
                        .build()?,
                ])
                .build()?;

            app.set_menu(menu)?;
            app.on_menu_event(|app, event| {
                if event.id().as_ref() == "settings" {
                    open_settings_window(app);
                }
            });

            // Database
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            let db_dir = std::path::Path::new(&home).join(".avidhd");
            std::fs::create_dir_all(&db_dir)?;
            let db_path = db_dir.join("data.db");
            let db_path_str = db_path.to_string_lossy().to_string();

            let db = tauri::async_runtime::block_on(async move {
                let db = turso::Builder::new_local(&db_path_str).build().await?;
                let conn = db.connect()?;
                conn.execute(
                    "CREATE TABLE IF NOT EXISTS items (
                        id       INTEGER PRIMARY KEY AUTOINCREMENT,
                        text     TEXT    NOT NULL,
                        position INTEGER NOT NULL DEFAULT 0,
                        status   TEXT    NOT NULL DEFAULT 'open'
                    )",
                    (),
                )
                .await?;
                conn.execute(
                    "CREATE TABLE IF NOT EXISTS config (
                        key   TEXT PRIMARY KEY,
                        value TEXT NOT NULL
                    )",
                    (),
                )
                .await?;
                // migrations
                let _ = conn
                    .execute(
                        "ALTER TABLE items ADD COLUMN status TEXT DEFAULT 'open'",
                        (),
                    )
                    .await;
                let _ = conn
                    .execute("UPDATE items SET status = 'open' WHERE status IS NULL", ())
                    .await;
                let _ = conn
                    .execute("ALTER TABLE items ADD COLUMN snoozed_until INTEGER", ())
                    .await;
                let _ = conn
                    .execute("ALTER TABLE items ADD COLUMN kind TEXT DEFAULT 'task'", ())
                    .await;
                let _ = conn
                    .execute("UPDATE items SET kind = 'task' WHERE kind IS NULL", ())
                    .await;
                let _ = conn
                    .execute("ALTER TABLE items ADD COLUMN pr_url TEXT", ())
                    .await;
                let _ = conn
                    .execute("ALTER TABLE items ADD COLUMN pr_repo TEXT", ())
                    .await;
                let _ = conn
                    .execute("ALTER TABLE items ADD COLUMN pr_number INTEGER", ())
                    .await;
                let _ = conn
                    .execute("ALTER TABLE items ADD COLUMN pr_role TEXT", ())
                    .await;
                let _ = conn
                    .execute("ALTER TABLE items ADD COLUMN pr_draft INTEGER", ())
                    .await;
                let _ = conn
                    .execute("ALTER TABLE items ADD COLUMN pr_id INTEGER", ())
                    .await;
                Ok::<_, turso::Error>(db)
            })?;

            app.manage(AppState {
                db,
                token_cache: std::sync::Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_items,
            add_item,
            close_item,
            reopen_item,
            delete_item,
            update_item,
            reorder_items,
            open_settings,
            clear_done_items,
            get_db_path,
            save_github_token,
            get_github_token,
            clear_github_token,
            verify_github_token,
            sync_pull_requests,
            open_url,
            snooze_item,
            unsnooze_item,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
