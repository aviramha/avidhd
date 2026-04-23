use serde::{Deserialize, Serialize};
use serde_json::json;
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
    kind: String, // "task" | "pr" | "linear_notification" | "linear_issue"
    pr_url: Option<String>,
    pr_repo: Option<String>,
    pr_number: Option<i64>,
    pr_role: Option<String>, // "authored" | "review_requested"
    pr_draft: Option<bool>,
    pr_checks_status: Option<String>,
    linear_url: Option<String>,
    linear_subtitle: Option<String>,
    linear_identifier: Option<String>,
    linear_state: Option<String>,
    linear_notification_id: Option<String>,
    linear_issue_id: Option<String>,
}

struct AppState {
    db: turso::Database,
    github_token_cache: std::sync::Mutex<Option<String>>,
    linear_token_cache: std::sync::Mutex<Option<String>>,
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
        .inner_size(460.0, 760.0)
        .resizable(false)
        .build();
}

#[tauri::command]
fn open_settings(app: tauri::AppHandle) {
    open_settings_window(&app);
}

// ── DB commands ───────────────────────────────────────────────────────────────

const SELECT_COLS: &str = "id, text, position, status, snoozed_until, kind, pr_url, pr_repo, pr_number, pr_role, pr_draft, linear_url, linear_subtitle, linear_identifier, linear_state, linear_notification_id, linear_issue_id, pr_checks_status";

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
        pr_checks_status: row.get(17).ok(),
        linear_url: row.get(11).ok(),
        linear_subtitle: row.get(12).ok(),
        linear_identifier: row.get(13).ok(),
        linear_state: row.get(14).ok(),
        linear_notification_id: row.get(15).ok(),
        linear_issue_id: row.get(16).ok(),
    })
}

async fn next_external_insert_position(conn: &turso::Connection) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MIN(position) - 1, 0) FROM items WHERE status = 'open'",
            (),
        )
        .await
        .map_err(|e| e.to_string())?;

    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        Ok(row.get(0).unwrap_or(0))
    } else {
        Ok(0)
    }
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
        pr_checks_status: None,
        linear_url: None,
        linear_subtitle: None,
        linear_identifier: None,
        linear_state: None,
        linear_notification_id: None,
        linear_issue_id: None,
    })
}

#[tauri::command]
async fn close_item(state: tauri::State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.db.connect().map_err(|e| e.to_string())?;
    let mut rows = conn
        .query(
            "SELECT kind, linear_notification_id, linear_identifier, linear_issue_id FROM items WHERE id = ?1",
            params![id],
        )
        .await
        .map_err(|e| e.to_string())?;

    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let kind = row.get::<String>(0).unwrap_or_default();
        let notification_id = row.get::<String>(1).ok();
        let identifier = row.get::<String>(2).ok();
        let issue_id = row.get::<String>(3).ok();
        if kind == "linear_notification" {
            let token = linear_token_from_state(&state)?;
            if token.is_empty() {
                return Err("Linear is not connected".into());
            }

            if let Some(issue_id) = issue_id.as_deref().filter(|value| !value.is_empty()) {
                linear_archive_notifications_for_issue(&token, issue_id).await?;
            } else if let Some(notification_id) = notification_id {
                linear_archive_notification(&token, &notification_id).await?;
            }

            if let Some(issue_id) = issue_id.as_deref().filter(|value| !value.is_empty()) {
                conn.execute(
                    "UPDATE items SET status = 'done'
                     WHERE kind = 'linear_notification' AND linear_issue_id = ?1",
                    params![issue_id],
                )
                .await
                .map_err(|e| e.to_string())?;
                return Ok(());
            }

            if let Some(identifier) = identifier.as_deref().filter(|value| !value.is_empty()) {
                conn.execute(
                    "UPDATE items SET status = 'done'
                     WHERE kind = 'linear_notification' AND linear_identifier = ?1",
                    params![identifier],
                )
                .await
                .map_err(|e| e.to_string())?;
                return Ok(());
            }
        }
    }

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
    let mut rows = conn
        .query(
            "SELECT kind, linear_notification_id FROM items WHERE id = ?1",
            params![id],
        )
        .await
        .map_err(|e| e.to_string())?;

    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let kind = row.get::<String>(0).unwrap_or_default();
        let notification_id = row.get::<String>(1).ok();
        if kind == "linear_notification"
            && let Some(notification_id) = notification_id
        {
            let token = linear_token_from_state(&state)?;
            if token.is_empty() {
                return Err("Linear is not connected".into());
            }
            linear_unarchive_notification(&token, &notification_id).await?;
        }
    }

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

// ── Keyring helpers ───────────────────────────────────────────────────────────

fn github_keyring_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new("avidhd", "github_token").map_err(|e| e.to_string())
}

fn linear_keyring_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new("avidhd", "linear_token").map_err(|e| e.to_string())
}

fn token_from_state(
    env_var: &str,
    cache: &std::sync::Mutex<Option<String>>,
    entry: keyring::Entry,
) -> Result<String, String> {
    if let Some(t) = std::env::var(env_var).ok().filter(|t| !t.is_empty()) {
        return Ok(t);
    }
    {
        let cache = cache.lock().unwrap();
        if let Some(ref t) = *cache {
            return Ok(t.clone());
        }
    }
    let token = match entry.get_password() {
        Ok(t) => t,
        Err(keyring::Error::NoEntry) => String::new(),
        Err(e) => return Err(e.to_string()),
    };
    *cache.lock().unwrap() = Some(token.clone());
    Ok(token)
}

fn github_token_from_state(state: &AppState) -> Result<String, String> {
    token_from_state(
        "GITHUB_PAT",
        &state.github_token_cache,
        github_keyring_entry()?,
    )
}

fn linear_token_from_state(state: &AppState) -> Result<String, String> {
    token_from_state(
        "LINEAR_API_KEY",
        &state.linear_token_cache,
        linear_keyring_entry()?,
    )
}

// ── GitHub PAT (stored in OS keyring) ────────────────────────────────────────

#[tauri::command]
fn save_github_token(state: tauri::State<'_, AppState>, token: String) -> Result<(), String> {
    github_keyring_entry()?
        .set_password(&token)
        .map_err(|e| e.to_string())?;
    *state.github_token_cache.lock().unwrap() = Some(token);
    Ok(())
}

#[tauri::command]
fn get_github_token(state: tauri::State<'_, AppState>) -> Result<String, String> {
    github_token_from_state(&state)
}

#[tauri::command]
fn clear_github_token(state: tauri::State<'_, AppState>) -> Result<(), String> {
    match github_keyring_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(e) => return Err(e.to_string()),
    }
    *state.github_token_cache.lock().unwrap() = Some(String::new());
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

// ── Linear API key (stored in OS keyring) ────────────────────────────────────

#[derive(Deserialize)]
struct LinearGraphQlResponse {
    data: Option<serde_json::Value>,
    errors: Option<Vec<LinearGraphQlError>>,
}

#[derive(Deserialize)]
struct LinearGraphQlError {
    message: String,
}

async fn linear_graphql(
    token: &str,
    query: &str,
    variables: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();
    let res = client
        .post("https://api.linear.app/graphql")
        .header("Authorization", token)
        .header("Content-Type", "application/json")
        .json(&json!({
            "query": query,
            "variables": variables,
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!("Linear returned {}", res.status()));
    }

    let body: LinearGraphQlResponse = res.json().await.map_err(|e| e.to_string())?;
    if let Some(errors) = body.errors {
        let msg = errors
            .into_iter()
            .map(|err| err.message)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(msg);
    }

    body.data.ok_or_else(|| "Linear returned no data".into())
}

#[tauri::command]
fn save_linear_token(state: tauri::State<'_, AppState>, token: String) -> Result<(), String> {
    linear_keyring_entry()?
        .set_password(&token)
        .map_err(|e| e.to_string())?;
    *state.linear_token_cache.lock().unwrap() = Some(token);
    Ok(())
}

#[tauri::command]
fn get_linear_token(state: tauri::State<'_, AppState>) -> Result<String, String> {
    linear_token_from_state(&state)
}

#[tauri::command]
fn clear_linear_token(state: tauri::State<'_, AppState>) -> Result<(), String> {
    match linear_keyring_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(e) => return Err(e.to_string()),
    }
    *state.linear_token_cache.lock().unwrap() = Some(String::new());
    Ok(())
}

#[tauri::command]
async fn verify_linear_token(token: String) -> Result<String, String> {
    let data = linear_graphql(
        &token,
        "query VerifyLinearToken { viewer { name email } }",
        json!({}),
    )
    .await?;

    let viewer = &data["viewer"];
    let name = viewer["name"].as_str().unwrap_or("").trim();
    let email = viewer["email"].as_str().unwrap_or("").trim();
    if !name.is_empty() {
        Ok(name.to_string())
    } else if !email.is_empty() {
        Ok(email.to_string())
    } else {
        Ok("Linear user".into())
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
    checks_status: Option<String>,
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
                checks_status: None, // fetched separately
            });
        }
    }
    Ok(prs)
}

/// Fetches the overall check-run status for a PR by getting its head SHA
/// and then querying the check-runs API. Returns None if there are no checks.
async fn fetch_pr_checks_status(
    client: &reqwest::Client,
    token: &str,
    repo: &str,
    number: i64,
) -> Option<String> {
    let pr: serde_json::Value = client
        .get(format!(
            "https://api.github.com/repos/{}/pulls/{}",
            repo, number
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "avidhd")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    let sha = pr["head"]["sha"].as_str()?;

    let checks: serde_json::Value = client
        .get(format!(
            "https://api.github.com/repos/{}/commits/{}/check-runs?per_page=100",
            repo, sha
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "avidhd")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    let runs = checks["check_runs"].as_array()?;
    if runs.is_empty() {
        return None;
    }

    let has_failure = runs.iter().any(|r| {
        matches!(
            r["conclusion"].as_str(),
            Some("failure") | Some("timed_out") | Some("cancelled")
        )
    });

    if has_failure {
        return Some("failure".to_string());
    }

    if runs
        .iter()
        .any(|r| r["status"].as_str() != Some("completed"))
    {
        return Some("pending".to_string());
    }

    Some("success".to_string())
}

/// Fetch open PRs from GitHub and upsert them as items.
/// PRs that have disappeared (merged/closed) are marked done.
#[tauri::command]
async fn sync_pull_requests(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let token = github_token_from_state(&state)?;
    if token.is_empty() {
        return Ok(());
    }

    let mut fetched = fetch_github_prs(&token).await?;
    let fetched_ids: std::collections::HashSet<i64> = fetched.iter().map(|p| p.github_id).collect();

    // Fetch check status for each PR
    let client = reqwest::Client::new();
    for pr in &mut fetched {
        if !pr.repo.is_empty() {
            pr.checks_status = fetch_pr_checks_status(&client, &token, &pr.repo, pr.number).await;
        }
    }

    let conn = state.db.connect().map_err(|e| e.to_string())?;

    // Collect all known PR github_ids from DB with their status
    let mut rows = conn
        .query(
            "SELECT pr_id, status FROM items WHERE kind = 'pr' AND pr_id IS NOT NULL",
            (),
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut existing_pr_status: std::collections::HashMap<i64, String> =
        std::collections::HashMap::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        if let Ok(id) = row.get::<i64>(0) {
            let status = row.get::<String>(1).unwrap_or_else(|_| "open".into());
            existing_pr_status.insert(id, status);
        }
    }

    // Upsert fetched PRs
    for pr in &fetched {
        if existing_pr_status.contains_key(&pr.github_id) {
            conn.execute(
                "UPDATE items SET text = ?1, pr_role = ?2, pr_draft = ?3, pr_checks_status = ?4 \
                 WHERE pr_id = ?5 AND kind = 'pr'",
                params![
                    pr.title.clone(),
                    pr.role.clone(),
                    pr.draft as i64,
                    pr.checks_status.clone(),
                    pr.github_id
                ],
            )
            .await
            .map_err(|e| e.to_string())?;
        } else {
            let pos = next_external_insert_position(&conn).await?;
            conn.execute(
                "INSERT INTO items \
                 (text, position, status, kind, pr_url, pr_repo, pr_number, pr_role, pr_draft, pr_id, pr_checks_status) \
                 VALUES (?1, ?2, 'open', 'pr', ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    pr.title.clone(),
                    pos,
                    pr.url.clone(),
                    pr.repo.clone(),
                    pr.number,
                    pr.role.clone(),
                    pr.draft as i64,
                    pr.github_id,
                    pr.checks_status.clone()
                ],
            )
            .await
            .map_err(|e| e.to_string())?;
        }
    }

    // Mark open PRs that disappeared from GitHub as done
    for (old_id, status) in &existing_pr_status {
        if status == "open" && !fetched_ids.contains(old_id) {
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

// ── Linear sync ───────────────────────────────────────────────────────────────

struct FetchedLinearNotification {
    notification_id: String,
    text: String,
    subtitle: Option<String>,
    url: String,
    identifier: Option<String>,
    state: Option<String>,
    issue_id: Option<String>,
    updated_at: String,
}

struct FetchedLinearIssue {
    issue_id: String,
    identifier: String,
    title: String,
    url: String,
    state: Option<String>,
    updated_at: String,
}

fn linear_notification_dedup_key(
    identifier: Option<&str>,
    issue_id: Option<&str>,
    notification_id: &str,
) -> String {
    identifier
        .filter(|value| !value.is_empty())
        .map(|value| format!("issue:{value}"))
        .or_else(|| {
            issue_id
                .filter(|value| !value.is_empty())
                .map(|value| format!("issue-id:{value}"))
        })
        .unwrap_or_else(|| format!("notification:{notification_id}"))
}

fn linear_issue_dedup_key(identifier: &str, issue_id: &str) -> String {
    if !identifier.is_empty() {
        format!("issue:{identifier}")
    } else {
        format!("issue-id:{issue_id}")
    }
}

async fn fetch_linear_notifications(token: &str) -> Result<Vec<FetchedLinearNotification>, String> {
    let data = linear_graphql(
        token,
        r#"
        query LinearNotifications {
          notifications(first: 100) {
            nodes {
              __typename
              id
              archivedAt
              title
              subtitle
              type
              updatedAt
              url
              inboxUrl
              readAt
              snoozedUntilAt
              ... on IssueNotification {
                issue {
                  id
                  identifier
                  state {
                    name
                  }
                }
              }
            }
          }
        }
        "#,
        json!({}),
    )
    .await?;

    let mut notifications_by_issue: std::collections::HashMap<String, FetchedLinearNotification> =
        std::collections::HashMap::new();
    for node in data["notifications"]["nodes"]
        .as_array()
        .unwrap_or(&Vec::new())
    {
        if !node["archivedAt"].is_null() {
            continue;
        }
        if !node["snoozedUntilAt"].is_null() {
            continue;
        }

        let title = node["title"].as_str().unwrap_or("").trim();
        if title.is_empty() {
            continue;
        }

        let url = node["inboxUrl"]
            .as_str()
            .filter(|v| !v.is_empty())
            .or_else(|| node["url"].as_str())
            .unwrap_or("")
            .to_string();
        if url.is_empty() {
            continue;
        }

        let notification = FetchedLinearNotification {
            notification_id: node["id"].as_str().unwrap_or("").to_string(),
            text: title.to_string(),
            subtitle: node["subtitle"]
                .as_str()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string),
            url,
            identifier: node["issue"]["identifier"].as_str().map(str::to_string),
            state: node["issue"]["state"]["name"].as_str().map(str::to_string),
            issue_id: node["issue"]["id"].as_str().map(str::to_string),
            updated_at: node["updatedAt"].as_str().unwrap_or("").to_string(),
        };

        let dedup_key = linear_notification_dedup_key(
            notification.identifier.as_deref(),
            notification.issue_id.as_deref(),
            &notification.notification_id,
        );

        match notifications_by_issue.get(&dedup_key) {
            Some(existing) if existing.updated_at >= notification.updated_at => {}
            _ => {
                notifications_by_issue.insert(dedup_key, notification);
            }
        }
    }

    Ok(notifications_by_issue.into_values().collect())
}

async fn fetch_linear_assigned_issues(token: &str) -> Result<Vec<FetchedLinearIssue>, String> {
    let data = linear_graphql(
        token,
        r#"
        query LinearAssignedIssues {
          viewer {
            assignedIssues(first: 100) {
              nodes {
                id
                identifier
                title
                url
                updatedAt
                archivedAt
                canceledAt
                completedAt
                state {
                  name
                }
              }
            }
          }
        }
        "#,
        json!({}),
    )
    .await?;

    let mut issues_by_identifier: std::collections::HashMap<String, FetchedLinearIssue> =
        std::collections::HashMap::new();
    for node in data["viewer"]["assignedIssues"]["nodes"]
        .as_array()
        .unwrap_or(&Vec::new())
    {
        if !node["archivedAt"].is_null()
            || !node["canceledAt"].is_null()
            || !node["completedAt"].is_null()
        {
            continue;
        }

        let issue_id = node["id"].as_str().unwrap_or("").trim();
        let identifier = node["identifier"].as_str().unwrap_or("").trim();
        let title = node["title"].as_str().unwrap_or("").trim();
        let url = node["url"].as_str().unwrap_or("").trim();
        if issue_id.is_empty() || identifier.is_empty() || title.is_empty() || url.is_empty() {
            continue;
        }

        let issue = FetchedLinearIssue {
            issue_id: issue_id.to_string(),
            identifier: identifier.to_string(),
            title: title.to_string(),
            url: url.to_string(),
            state: node["state"]["name"].as_str().map(str::to_string),
            updated_at: node["updatedAt"].as_str().unwrap_or("").to_string(),
        };

        let dedup_key = linear_issue_dedup_key(&issue.identifier, &issue.issue_id);
        match issues_by_identifier.get(&dedup_key) {
            Some(existing) if existing.updated_at >= issue.updated_at => {}
            _ => {
                issues_by_identifier.insert(dedup_key, issue);
            }
        }
    }

    Ok(issues_by_identifier.into_values().collect())
}

async fn linear_archive_notification(token: &str, id: &str) -> Result<(), String> {
    let data = match linear_graphql(
        token,
        r#"
        mutation LinearArchiveNotification($id: String!) {
          notificationArchive(id: $id) {
            success
          }
        }
        "#,
        json!({ "id": id }),
    )
    .await
    {
        Ok(d) => d,
        Err(e) if e.contains("Entity not found") => return Ok(()),
        Err(e) => return Err(e),
    };

    if data["notificationArchive"]["success"]
        .as_bool()
        .unwrap_or(false)
    {
        Ok(())
    } else {
        Err("Failed to archive Linear notification".into())
    }
}

async fn linear_archive_notifications_for_issue(token: &str, issue_id: &str) -> Result<(), String> {
    let data = match linear_graphql(
        token,
        r#"
        mutation LinearArchiveNotificationsForIssue($issueId: String!) {
          notificationArchiveAll(input: { issueId: $issueId }) {
            success
          }
        }
        "#,
        json!({ "issueId": issue_id }),
    )
    .await
    {
        Ok(d) => d,
        Err(e) if e.contains("Entity not found") => return Ok(()),
        Err(e) => return Err(e),
    };

    if data["notificationArchiveAll"]["success"]
        .as_bool()
        .unwrap_or(false)
    {
        Ok(())
    } else {
        Err("Failed to archive Linear notifications for issue".into())
    }
}

async fn linear_unarchive_notification(token: &str, id: &str) -> Result<(), String> {
    let data = linear_graphql(
        token,
        r#"
        mutation LinearUnarchiveNotification($id: String!) {
          notificationUnarchive(id: $id) {
            success
          }
        }
        "#,
        json!({ "id": id }),
    )
    .await?;

    if data["notificationUnarchive"]["success"]
        .as_bool()
        .unwrap_or(false)
    {
        Ok(())
    } else {
        Err("Failed to unarchive Linear notification".into())
    }
}

#[tauri::command]
async fn sync_linear_items(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let token = linear_token_from_state(&state)?;
    if token.is_empty() {
        return Ok(());
    }

    let notifications = fetch_linear_notifications(&token).await?;
    let notification_issue_keys: std::collections::HashSet<String> = notifications
        .iter()
        .map(|notification| {
            linear_notification_dedup_key(
                notification.identifier.as_deref(),
                notification.issue_id.as_deref(),
                &notification.notification_id,
            )
        })
        .collect();
    let issues = fetch_linear_assigned_issues(&token)
        .await?
        .into_iter()
        .filter(|issue| {
            !notification_issue_keys
                .contains(&linear_issue_dedup_key(&issue.identifier, &issue.issue_id))
        })
        .collect::<Vec<_>>();
    let conn = state.db.connect().map_err(|e| e.to_string())?;

    let mut notification_rows = conn
        .query(
            "SELECT linear_notification_id, status FROM items WHERE kind = 'linear_notification' AND linear_notification_id IS NOT NULL",
            (),
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut existing_notification_status = std::collections::HashMap::new();
    while let Some(row) = notification_rows.next().await.map_err(|e| e.to_string())? {
        let remote_id = row.get::<String>(0).unwrap_or_default();
        let status = row.get::<String>(1).unwrap_or_else(|_| "open".into());
        existing_notification_status.insert(remote_id, status);
    }

    let fetched_notification_ids: std::collections::HashSet<String> = notifications
        .iter()
        .map(|notification| notification.notification_id.clone())
        .collect();

    for notification in &notifications {
        if existing_notification_status.contains_key(&notification.notification_id) {
            conn.execute(
                "UPDATE items
                 SET text = ?1,
                     status = CASE WHEN status = 'done' THEN 'done' ELSE 'open' END,
                     linear_url = ?2,
                     linear_subtitle = ?3,
                     linear_identifier = ?4,
                     linear_state = ?5,
                     linear_issue_id = ?6
                 WHERE linear_notification_id = ?7 AND kind = 'linear_notification'",
                params![
                    notification.text.clone(),
                    notification.url.clone(),
                    notification.subtitle.clone(),
                    notification.identifier.clone(),
                    notification.state.clone(),
                    notification.issue_id.clone(),
                    notification.notification_id.clone()
                ],
            )
            .await
            .map_err(|e| e.to_string())?;
        } else {
            let pos = next_external_insert_position(&conn).await?;
            conn.execute(
                "INSERT INTO items
                 (text, position, status, kind, linear_url, linear_subtitle, linear_identifier, linear_state, linear_notification_id, linear_issue_id)
                 VALUES (?1, ?2, 'open', 'linear_notification', ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    notification.text.clone(),
                    pos,
                    notification.url.clone(),
                    notification.subtitle.clone(),
                    notification.identifier.clone(),
                    notification.state.clone(),
                    notification.notification_id.clone(),
                    notification.issue_id.clone()
                ],
            )
            .await
            .map_err(|e| e.to_string())?;
        }
    }

    for notification_id in existing_notification_status.keys() {
        if !fetched_notification_ids.contains(notification_id) {
            conn.execute(
                "UPDATE items SET status = 'done' WHERE linear_notification_id = ?1 AND kind = 'linear_notification' AND status = 'open'",
                params![notification_id.clone()],
            )
            .await
            .map_err(|e| e.to_string())?;
        }
    }

    let mut issue_rows = conn
        .query(
            "SELECT linear_issue_id, status FROM items WHERE kind = 'linear_issue' AND linear_issue_id IS NOT NULL",
            (),
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut existing_issue_status = std::collections::HashMap::new();
    while let Some(row) = issue_rows.next().await.map_err(|e| e.to_string())? {
        let remote_id = row.get::<String>(0).unwrap_or_default();
        let status = row.get::<String>(1).unwrap_or_else(|_| "open".into());
        existing_issue_status.insert(remote_id, status);
    }

    let fetched_issue_ids: std::collections::HashSet<String> =
        issues.iter().map(|issue| issue.issue_id.clone()).collect();

    for issue in &issues {
        if let Some(status) = existing_issue_status.get(&issue.issue_id) {
            conn.execute(
                "UPDATE items
                 SET text = ?1,
                     status = ?2,
                     linear_url = ?3,
                     linear_identifier = ?4,
                     linear_state = ?5
                 WHERE linear_issue_id = ?6 AND kind = 'linear_issue'",
                params![
                    issue.title.clone(),
                    status.clone(),
                    issue.url.clone(),
                    issue.identifier.clone(),
                    issue.state.clone(),
                    issue.issue_id.clone()
                ],
            )
            .await
            .map_err(|e| e.to_string())?;
        } else {
            let pos = next_external_insert_position(&conn).await?;
            conn.execute(
                "INSERT INTO items
                 (text, position, status, kind, linear_url, linear_identifier, linear_state, linear_issue_id)
                 VALUES (?1, ?2, 'open', 'linear_issue', ?3, ?4, ?5, ?6)",
                params![
                    issue.title.clone(),
                    pos,
                    issue.url.clone(),
                    issue.identifier.clone(),
                    issue.state.clone(),
                    issue.issue_id.clone()
                ],
            )
            .await
            .map_err(|e| e.to_string())?;
        }
    }

    for issue_id in existing_issue_status.keys() {
        if !fetched_issue_ids.contains(issue_id) {
            conn.execute(
                "UPDATE items SET status = 'done' WHERE linear_issue_id = ?1 AND kind = 'linear_issue' AND status = 'open'",
                params![issue_id.clone()],
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
                let _ = conn
                    .execute("ALTER TABLE items ADD COLUMN linear_url TEXT", ())
                    .await;
                let _ = conn
                    .execute("ALTER TABLE items ADD COLUMN linear_subtitle TEXT", ())
                    .await;
                let _ = conn
                    .execute("ALTER TABLE items ADD COLUMN linear_identifier TEXT", ())
                    .await;
                let _ = conn
                    .execute("ALTER TABLE items ADD COLUMN linear_state TEXT", ())
                    .await;
                let _ = conn
                    .execute(
                        "ALTER TABLE items ADD COLUMN linear_notification_id TEXT",
                        (),
                    )
                    .await;
                let _ = conn
                    .execute("ALTER TABLE items ADD COLUMN linear_issue_id TEXT", ())
                    .await;
                let _ = conn
                    .execute("ALTER TABLE items ADD COLUMN pr_checks_status TEXT", ())
                    .await;
                Ok::<_, turso::Error>(db)
            })?;

            app.manage(AppState {
                db,
                github_token_cache: std::sync::Mutex::new(None),
                linear_token_cache: std::sync::Mutex::new(None),
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
            save_linear_token,
            get_linear_token,
            clear_linear_token,
            verify_linear_token,
            sync_pull_requests,
            sync_linear_items,
            open_url,
            snooze_item,
            unsnooze_item,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
