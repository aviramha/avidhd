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

#[tauri::command]
async fn get_items(
    state: tauri::State<'_, AppState>,
    include_done: bool,
    query: String,
) -> Result<Vec<Item>, String> {
    let conn = state.db.connect().map_err(|e| e.to_string())?;
    let search = format!("%{}%", query);

    let sql = if include_done {
        "SELECT id, text, position, status FROM items \
         WHERE text LIKE ?1 \
         ORDER BY CASE WHEN status = 'open' THEN 0 ELSE 1 END, position ASC"
    } else {
        "SELECT id, text, position, status FROM items \
         WHERE status = 'open' AND text LIKE ?1 \
         ORDER BY position ASC"
    };

    let mut rows = conn
        .query(sql, params![search])
        .await
        .map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        items.push(Item {
            id: row.get(0).map_err(|e| e.to_string())?,
            text: row.get(1).map_err(|e| e.to_string())?,
            position: row.get(2).map_err(|e| e.to_string())?,
            status: row.get(3).map_err(|e| e.to_string())?,
        });
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
        "INSERT INTO items (text, position, status) VALUES (?1, ?2, 'open')",
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

// ── Pull Requests ─────────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
struct PullRequest {
    number: u64,
    title: String,
    html_url: String,
    repo: String,
    author: String,
    draft: bool,
    role: String, // "authored" | "review_requested"
}

#[tauri::command]
async fn get_pull_requests(state: tauri::State<'_, AppState>) -> Result<Vec<PullRequest>, String> {
    let token = get_github_token(state)?;
    if token.is_empty() {
        return Ok(vec![]);
    }

    let client = reqwest::Client::new();
    let mut prs: Vec<PullRequest> = Vec::new();
    let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();

    let queries: &[(&str, &str)] = &[
        ("is:open is:pr author:@me archived:false", "authored"),
        (
            "is:open is:pr review-requested:@me archived:false",
            "review_requested",
        ),
    ];

    for (q, role) in queries {
        let url = format!(
            "https://api.github.com/search/issues?q={}&per_page=30&sort=updated",
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
            let id = item["id"].as_u64().unwrap_or(0);
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
            prs.push(PullRequest {
                number: item["number"].as_u64().unwrap_or(0),
                title: item["title"].as_str().unwrap_or("").to_string(),
                html_url,
                repo,
                author: item["user"]["login"].as_str().unwrap_or("").to_string(),
                draft: item["draft"].as_bool().unwrap_or(false),
                role: role.to_string(),
            });
        }
    }

    Ok(prs)
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
                // migration: add status column to existing dbs
                let _ = conn
                    .execute(
                        "ALTER TABLE items ADD COLUMN status TEXT DEFAULT 'open'",
                        (),
                    )
                    .await;
                // ensure no NULL status values from migration
                let _ = conn
                    .execute("UPDATE items SET status = 'open' WHERE status IS NULL", ())
                    .await;
                Ok::<_, turso::Error>(db)
            })?;

            // Pre-warm token cache so polling never hits the keychain again
            let cached_token =
                match keyring_entry().and_then(|e| e.get_password().map_err(|ke| ke.to_string())) {
                    Ok(t) => Some(t),
                    Err(_) => Some(String::new()),
                };
            app.manage(AppState {
                db,
                token_cache: std::sync::Mutex::new(cached_token),
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
            get_pull_requests,
            open_url,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
