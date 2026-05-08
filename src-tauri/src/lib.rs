use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(windows)]
fn silent_command(program: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut c = Command::new(program);
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

#[cfg(not(windows))]
fn silent_command(program: &str) -> Command {
    Command::new(program)
}

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tauri::{Emitter, Manager, State};

#[cfg(windows)]
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, TRUE, WPARAM};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowTextW, IsIconic, IsWindow, PostMessageW,
    SetForegroundWindow, ShowWindow, SW_RESTORE, WM_CLOSE,
};

#[derive(Serialize, Clone)]
struct SessionMeta {
    id: String,
    first_user_message: String,
    alias: String,
    last_activity_ms: u64,
    message_count: usize,
    is_running: bool,
}

#[derive(Serialize, Clone)]
struct ProjectGroup {
    project_path: String,
    project_name: String,
    alias: String,
    encoded_dir: String,
    is_starred: bool,
    is_archived: bool,
    last_activity_ms: u64,
    running_count: usize,
    todo_pending_count: usize,
    sessions: Vec<SessionMeta>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Todo {
    id: String,
    title: String,
    details: String,
    completed: bool,
}

#[derive(Serialize, Deserialize, Clone)]
struct GlobalTab {
    id: String,
    name: String,
    todos: Vec<Todo>,
}

#[derive(Default)]
struct AliasStore {
    projects: HashMap<String, String>,
    sessions: HashMap<String, String>,
}

#[derive(Serialize, Clone)]
struct Script {
    title: String,
    file_name: String,
    kind: String,
    is_running: bool,
}

#[derive(Serialize, Clone)]
struct InboxEntry {
    dir_name: String,
    title: String,
    description: String,
    attachments: Vec<String>,
    created_at_ms: u64,
    status: String, // "pending" | "sent" | "applied" | "ignored"
}

#[derive(Serialize, Clone)]
struct MessagePreview {
    timestamp: String,
    text: String,
}

#[derive(Deserialize)]
struct InboxAttachmentInput {
    name: String,
    content_base64: String,
}

struct ScriptRun {
    script_id: String,
    hwnd: Option<isize>,
}

#[derive(Default)]
struct CachedMeta {
    mtime_ms: u64,
    cwd: String,
    message_count: usize,
    first_user_message: String,
}

struct AppState {
    running: Mutex<HashMap<String, u32>>,
    meta_cache: Mutex<HashMap<PathBuf, CachedMeta>>,
    windows: Mutex<HashMap<String, isize>>,
    starred: Mutex<HashSet<String>>,
    running_scripts: Mutex<HashMap<String, ScriptRun>>,
    aliases: Mutex<AliasStore>,
    archived: Mutex<HashSet<String>>,
}

fn ccpilot_data_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("CCPilot"))
}

fn stars_path() -> Option<PathBuf> {
    ccpilot_data_dir().map(|d| d.join("stars.json"))
}

fn migrate_session_stars_to_projects(old_session_ids: &HashSet<String>) -> HashSet<String> {
    let root = match projects_root() {
        Some(r) => r,
        None => return HashSet::new(),
    };
    if !root.exists() {
        return HashSet::new();
    }

    let mut new_paths: HashSet<String> = HashSet::new();
    let dirs = match fs::read_dir(&root) {
        Ok(d) => d,
        Err(_) => return HashSet::new(),
    };
    for proj_dir in dirs.flatten() {
        if !proj_dir.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let encoded = proj_dir.file_name().to_string_lossy().to_string();
        let inner = match fs::read_dir(proj_dir.path()) {
            Ok(d) => d,
            Err(_) => continue,
        };
        for jsonl in inner.flatten() {
            let path = jsonl.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let id = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            if !old_session_ids.contains(&id) {
                continue;
            }
            let cwd = extract_cwd(&path).unwrap_or_else(|| decode_dir_name(&encoded));
            new_paths.insert(cwd);
        }
    }
    new_paths
}

fn load_stars() -> HashSet<String> {
    let path = match stars_path() {
        Some(p) => p,
        None => return HashSet::new(),
    };
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return HashSet::new(),
    };

    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
        // v2 format: { "v": 2, "projects": [...] }
        if v.get("v").and_then(|x| x.as_i64()) == Some(2) {
            if let Some(arr) = v.get("projects").and_then(|p| p.as_array()) {
                return arr
                    .iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect();
            }
            return HashSet::new();
        }
        // Legacy: bare array of session_ids → migrate to project_paths.
        if let Some(arr) = v.as_array() {
            let session_ids: HashSet<String> = arr
                .iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect();
            if session_ids.is_empty() {
                return HashSet::new();
            }
            let migrated = migrate_session_stars_to_projects(&session_ids);
            let _ = save_stars(&migrated);
            return migrated;
        }
    }
    HashSet::new()
}

fn save_stars(stars: &HashSet<String>) -> Result<(), String> {
    let path = stars_path().ok_or("no data dir")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let list: Vec<&String> = stars.iter().collect();
    let obj = serde_json::json!({ "v": 2, "projects": list });
    let json = serde_json::to_string_pretty(&obj).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

fn aliases_path() -> Option<PathBuf> {
    ccpilot_data_dir().map(|d| d.join("aliases.json"))
}

fn load_aliases() -> AliasStore {
    let path = match aliases_path() {
        Some(p) => p,
        None => return AliasStore::default(),
    };
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return AliasStore::default(),
    };
    let v: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return AliasStore::default(),
    };
    let mut store = AliasStore::default();
    if let Some(obj) = v.get("projects").and_then(|x| x.as_object()) {
        for (k, val) in obj {
            if let Some(s) = val.as_str() {
                if !s.is_empty() {
                    store.projects.insert(k.clone(), s.to_string());
                }
            }
        }
    }
    if let Some(obj) = v.get("sessions").and_then(|x| x.as_object()) {
        for (k, val) in obj {
            if let Some(s) = val.as_str() {
                if !s.is_empty() {
                    store.sessions.insert(k.clone(), s.to_string());
                }
            }
        }
    }
    store
}

fn save_aliases(store: &AliasStore) -> Result<(), String> {
    let path = aliases_path().ok_or("no data dir")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let obj = serde_json::json!({
        "v": 1,
        "projects": store.projects,
        "sessions": store.sessions,
    });
    let json = serde_json::to_string_pretty(&obj).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

fn archived_path() -> Option<PathBuf> {
    ccpilot_data_dir().map(|d| d.join("archived.json"))
}

fn load_archived() -> HashSet<String> {
    let path = match archived_path() {
        Some(p) => p,
        None => return HashSet::new(),
    };
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return HashSet::new(),
    };
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
        if let Some(arr) = v.get("projects").and_then(|p| p.as_array()) {
            return arr
                .iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect();
        }
    }
    HashSet::new()
}

fn save_archived(set: &HashSet<String>) -> Result<(), String> {
    let path = archived_path().ok_or("no data dir")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let list: Vec<&String> = set.iter().collect();
    let obj = serde_json::json!({ "v": 1, "projects": list });
    let json = serde_json::to_string_pretty(&obj).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

fn projects_root() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

fn decode_dir_name(name: &str) -> String {
    let mut chars = name.chars();
    if let Some(d) = chars.next() {
        let rest: String = chars.collect();
        if let Some(stripped) = rest.strip_prefix("--") {
            return format!("{}:\\{}", d, stripped.replace('-', "\\"));
        }
    }
    name.replace('-', "\\")
}

fn extract_cwd(path: &Path) -> Option<String> {
    let f = fs::File::open(path).ok()?;
    let reader = BufReader::new(f);
    for line in reader.lines().take(20).map_while(Result::ok) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            if let Some(cwd) = v.get("cwd").and_then(|c| c.as_str()) {
                if !cwd.is_empty() {
                    return Some(cwd.to_string());
                }
            }
        }
    }
    None
}

fn is_user_record(v: &serde_json::Value) -> bool {
    if v.get("type").and_then(|t| t.as_str()) == Some("user") {
        return true;
    }
    v.get("message")
        .and_then(|m| m.get("role"))
        .and_then(|r| r.as_str())
        == Some("user")
}

fn extract_user_text(v: &serde_json::Value) -> Option<String> {
    let content = v.get("message").and_then(|m| m.get("content"))?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    if let Some(arr) = content.as_array() {
        let mut out = String::new();
        for block in arr {
            if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                    if !out.is_empty() {
                        out.push(' ');
                    }
                    out.push_str(t);
                }
            }
        }
        if !out.is_empty() {
            return Some(out);
        }
    }
    None
}

fn truncate_chars(s: &str, max: usize) -> String {
    let trimmed: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.chars().count() <= max {
        return trimmed;
    }
    let mut out: String = trimmed.chars().take(max).collect();
    out.push('…');
    out
}

struct ParsedMeta {
    cwd: String,
    first_user_message: String,
    message_count: usize,
}

fn parse_session_meta(path: &Path) -> ParsedMeta {
    let mut meta = ParsedMeta {
        cwd: String::new(),
        first_user_message: String::new(),
        message_count: 0,
    };
    let f = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return meta,
    };
    let reader = BufReader::new(f);

    for line in reader.lines().map_while(Result::ok) {
        meta.message_count += 1;
        if !meta.cwd.is_empty() && !meta.first_user_message.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            if meta.cwd.is_empty() {
                if let Some(c) = v.get("cwd").and_then(|c| c.as_str()) {
                    if !c.is_empty() {
                        meta.cwd = c.to_string();
                    }
                }
            }
            if meta.first_user_message.is_empty() && is_user_record(&v) {
                if let Some(t) = extract_user_text(&v) {
                    meta.first_user_message = truncate_chars(&t, 200);
                }
            }
        }
    }
    meta
}

fn alive_session_ids() -> HashSet<String> {
    let mut sys = System::new();

    // Pass 1: refresh process names only (cheap; no cmdline read).
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::new(),
    );

    // Filter to processes that could plausibly be a Claude session.
    let candidate_pids: Vec<sysinfo::Pid> = sys
        .processes()
        .iter()
        .filter_map(|(pid, proc)| {
            let name = proc.name().to_string_lossy().to_lowercase();
            if name.contains("claude") || name.contains("node") {
                Some(*pid)
            } else {
                None
            }
        })
        .collect();

    if candidate_pids.is_empty() {
        return HashSet::new();
    }

    // Pass 2: read cmdlines only for candidates.
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&candidate_pids),
        false,
        ProcessRefreshKind::new().with_cmd(UpdateKind::Always),
    );

    let mut alive = HashSet::new();
    for pid in &candidate_pids {
        let proc = match sys.process(*pid) {
            Some(p) => p,
            None => continue,
        };
        let cmd = proc.cmd();
        let mut iter = cmd.iter();
        while let Some(arg) = iter.next() {
            if arg.to_str() == Some("--resume") {
                if let Some(next) = iter.next() {
                    if let Some(id) = next.to_str() {
                        alive.insert(id.to_string());
                    }
                }
            }
        }
    }
    alive
}

#[tauri::command]
fn list_projects(state: State<'_, AppState>) -> Result<Vec<ProjectGroup>, String> {
    let root = projects_root().ok_or("home dir not found")?;
    if !root.exists() {
        return Ok(vec![]);
    }

    let alive = alive_session_ids();
    let stars = state.starred.lock().unwrap().clone();
    let archived = state.archived.lock().unwrap().clone();
    let (project_aliases, session_aliases) = {
        let a = state.aliases.lock().unwrap();
        (a.projects.clone(), a.sessions.clone())
    };
    let mut cache = state.meta_cache.lock().unwrap();
    let mut groups: HashMap<String, ProjectGroup> = HashMap::new();
    let mut seen_paths = HashSet::new();

    for proj_dir in fs::read_dir(&root).map_err(|e| e.to_string())?.flatten() {
        if !proj_dir.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let encoded = proj_dir.file_name().to_string_lossy().to_string();
        let proj_path = proj_dir.path();

        let entries = match fs::read_dir(&proj_path) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for jsonl in entries.flatten() {
            let path = jsonl.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let id = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => continue,
            };

            let mtime = jsonl
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);

            seen_paths.insert(path.clone());

            let (cwd, message_count, first_user_message) = match cache.get(&path) {
                Some(c) if c.mtime_ms == mtime => (
                    c.cwd.clone(),
                    c.message_count,
                    c.first_user_message.clone(),
                ),
                _ => {
                    let parsed = parse_session_meta(&path);
                    let cwd = if parsed.cwd.is_empty() {
                        decode_dir_name(&encoded)
                    } else {
                        parsed.cwd
                    };
                    cache.insert(
                        path.clone(),
                        CachedMeta {
                            mtime_ms: mtime,
                            cwd: cwd.clone(),
                            message_count: parsed.message_count,
                            first_user_message: parsed.first_user_message.clone(),
                        },
                    );
                    (cwd, parsed.message_count, parsed.first_user_message)
                }
            };

            let is_running = alive.contains(&id);
            let session_alias = session_aliases.get(&id).cloned().unwrap_or_default();
            let session = SessionMeta {
                id,
                first_user_message,
                alias: session_alias,
                last_activity_ms: mtime,
                message_count,
                is_running,
            };

            let group = groups.entry(cwd.clone()).or_insert_with(|| {
                let project_name = Path::new(&cwd)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&cwd)
                    .to_string();
                ProjectGroup {
                    project_path: cwd.clone(),
                    project_name,
                    alias: project_aliases.get(&cwd).cloned().unwrap_or_default(),
                    encoded_dir: encoded.clone(),
                    is_starred: stars.contains(&cwd),
                    is_archived: archived.contains(&cwd),
                    last_activity_ms: 0,
                    running_count: 0,
                    todo_pending_count: 0,
                    sessions: Vec::new(),
                }
            });
            if mtime > group.last_activity_ms {
                group.last_activity_ms = mtime;
            }
            if is_running {
                group.running_count += 1;
            }
            group.sessions.push(session);
        }
    }

    cache.retain(|p, _| seen_paths.contains(p));

    let mut result: Vec<ProjectGroup> = groups.into_values().collect();
    for g in &mut result {
        g.sessions
            .sort_by(|a, b| b.last_activity_ms.cmp(&a.last_activity_ms));
        g.todo_pending_count = load_todos(&g.project_path)
            .iter()
            .filter(|t| !t.completed)
            .count();
    }
    result.sort_by(|a, b| {
        b.is_starred
            .cmp(&a.is_starred)
            .then(b.last_activity_ms.cmp(&a.last_activity_ms))
    });

    Ok(result)
}

fn ccpscript_dir(project_path: &str) -> PathBuf {
    PathBuf::from(project_path).join(".ccpscript")
}

fn parse_title(path: &Path) -> Option<String> {
    let f = fs::File::open(path).ok()?;
    let reader = BufReader::new(f);
    for line in reader.lines().take(5).map_while(Result::ok) {
        let trimmed = line.trim();
        for prefix in &["# Title:", ":: Title:", "REM Title:", "rem Title:"] {
            if let Some(rest) = trimmed.strip_prefix(prefix) {
                let title = rest.trim();
                if !title.is_empty() {
                    return Some(title.to_string());
                }
            }
        }
        if !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && !trimmed.starts_with("::")
            && !trimmed.to_lowercase().starts_with("rem")
        {
            break;
        }
    }
    None
}

fn pretty_name(stem: &str) -> String {
    let s = stem.replace(['-', '_'], " ");
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => s,
    }
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect::<String>()
        .to_lowercase()
}

fn script_key(project_path: &str, file_name: &str) -> String {
    format!("{}|{}", project_path, file_name)
}

fn short_random_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("s{:08x}", nanos)
}

fn prune_dead_scripts(state: &State<'_, AppState>) {
    let mut map = state.running_scripts.lock().unwrap();
    map.retain(|_, run| {
        #[cfg(windows)]
        {
            if let Some(addr) = run.hwnd {
                return unsafe { IsWindow(HWND(addr)) }.as_bool();
            }
        }
        // No HWND yet — could still be in spawn poll window. Keep it.
        true
    });
}

#[tauri::command]
fn list_scripts(state: State<'_, AppState>, project_path: String) -> Result<Vec<Script>, String> {
    prune_dead_scripts(&state);
    let dir = ccpscript_dir(&project_path);
    if !dir.is_dir() {
        return Ok(vec![]);
    }

    let running_keys: HashSet<String> = state
        .running_scripts
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(k, run)| {
            #[cfg(windows)]
            {
                if let Some(addr) = run.hwnd {
                    if unsafe { IsWindow(HWND(addr)) }.as_bool() {
                        return Some(k.clone());
                    }
                }
            }
            #[cfg(not(windows))]
            {
                let _ = run;
            }
            None
        })
        .collect();

    let mut scripts = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) if !n.starts_with('_') => n.to_string(),
            _ => continue,
        };
        let kind = match path.extension().and_then(|s| s.to_str()) {
            Some(k @ ("ps1" | "bat" | "cmd")) => k.to_string(),
            _ => continue,
        };
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if stem.is_empty() {
            continue;
        }
        let title = parse_title(&path).unwrap_or_else(|| pretty_name(&stem));
        let key = script_key(&project_path, &file_name);
        let is_running = running_keys.contains(&key);
        scripts.push(Script {
            title,
            file_name,
            kind,
            is_running,
        });
    }
    scripts.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    Ok(scripts)
}

#[tauri::command]
fn run_script(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    project_path: String,
    file_name: String,
) -> Result<(), String> {
    let key = script_key(&project_path, &file_name);

    // If already running, focus existing window instead of spawning another.
    #[cfg(windows)]
    {
        let map = state.running_scripts.lock().unwrap();
        if let Some(run) = map.get(&key) {
            if let Some(addr) = run.hwnd {
                let hwnd = HWND(addr);
                if unsafe { IsWindow(hwnd) }.as_bool() {
                    unsafe {
                        if IsIconic(hwnd).as_bool() {
                            let _ = ShowWindow(hwnd, SW_RESTORE);
                        }
                        let _ = SetForegroundWindow(hwnd);
                    }
                    return Ok(());
                }
            }
        }
    }

    let script_path = ccpscript_dir(&project_path).join(&file_name);
    if !script_path.is_file() {
        return Err(format!("script not found: {}", file_name));
    }
    let kind = script_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    let script_id = short_random_id();
    let title = format!("Script · {} [{}]", file_name, script_id);

    let mut cmd = Command::new("wt.exe");
    cmd.args(["-w", "new", "--title", &title, "-d", &project_path]);

    match kind {
        "ps1" => {
            // Read .ps1 file via [IO.File]::ReadAllText to force UTF-8 decoding
            // (PowerShell 5.1's `-File` defaults to ANSI/GBK on Chinese Windows
            // and mangles non-ASCII paths/strings inside the script).
            let abs_path = script_path.to_string_lossy().to_string();
            let escaped = abs_path.replace('\'', "''");
            let ps_cmd = format!(
                "& ([scriptblock]::Create([System.IO.File]::ReadAllText('{}')))",
                escaped
            );
            cmd.args([
                "powershell",
                "-ExecutionPolicy",
                "Bypass",
                "-NoExit",
                "-Command",
                &ps_cmd,
            ]);
        }
        "bat" | "cmd" => {
            let rel = format!(".ccpscript\\{}", file_name);
            cmd.args(["cmd", "/K", &rel]);
        }
        _ => return Err(format!("unsupported script kind: {}", kind)),
    }

    cmd.spawn()
        .map_err(|e| format!("failed to spawn wt.exe: {}", e))?;

    state.running_scripts.lock().unwrap().insert(
        key.clone(),
        ScriptRun {
            script_id: script_id.clone(),
            hwnd: None,
        },
    );

    #[cfg(windows)]
    {
        let app = app.clone();
        std::thread::spawn(move || {
            let needle = format!("[{}]", script_id);
            for _ in 0..40 {
                std::thread::sleep(std::time::Duration::from_millis(150));
                if let Some(hwnd) = find_window_with_marker(&needle) {
                    let st = app.state::<AppState>();
                    let mut map = st.running_scripts.lock().unwrap();
                    if let Some(entry) = map.get_mut(&key) {
                        entry.hwnd = Some(hwnd.0);
                    }
                    break;
                }
            }
        });
    }
    #[cfg(not(windows))]
    let _ = app;

    Ok(())
}

const GENERATE_SCRIPTS_PROMPT: &str = include_str!("../prompts/generate_scripts.md");
const ASK_CLAUDE_PROMPT: &str = include_str!("../prompts/ask_claude.md");
const SEND_INBOX_PROMPT: &str = include_str!("../prompts/send_inbox.md");

fn spawn_claude_for_scripts(project_path: &str, prompt: &str, title_hint: &str) -> Result<(), String> {
    let project_dir = PathBuf::from(project_path);
    if !project_dir.is_dir() {
        return Err("project path is not a directory".into());
    }

    let temp_dir = std::env::temp_dir();
    let id = short_random_id();
    let wrapper_file = temp_dir.join(format!("ccpilot-wrapper-{}.ps1", id));

    // Build wrapper script with prompt embedded as a single-quoted here-string.
    // Single-quoted here-strings don't interpolate, so the prompt is passed verbatim.
    let mut wrapper_body = String::new();
    wrapper_body.push_str("$ErrorActionPreference = 'Continue'\n");
    wrapper_body.push_str("$OutputEncoding = [System.Text.Encoding]::UTF8\n");
    wrapper_body.push_str("[Console]::OutputEncoding = [System.Text.Encoding]::UTF8\n");
    wrapper_body.push_str("Write-Host ''\n");
    wrapper_body.push_str("Write-Host '== CCPilot · ");
    wrapper_body.push_str(title_hint);
    wrapper_body.push_str(" ==' -ForegroundColor Cyan\n");
    wrapper_body.push_str("Write-Host 'Spawning Claude in this project. First-call cold start can take ~5 seconds.' -ForegroundColor DarkGray\n");
    wrapper_body.push_str("Write-Host ''\n");
    wrapper_body.push_str("$prompt = @'\n");
    wrapper_body.push_str(prompt);
    if !prompt.ends_with('\n') {
        wrapper_body.push('\n');
    }
    wrapper_body.push_str("'@\n");
    wrapper_body.push_str("\n");
    wrapper_body.push_str("Write-Host '> echo $prompt | claude --dangerously-skip-permissions --verbose -p' -ForegroundColor DarkGray\n");
    wrapper_body.push_str("Write-Host ''\n");
    // Pipe prompt via stdin to avoid PowerShell/cmd argv splitting on flag-like
    // tokens in the prompt body (e.g., '-Raw' was being interpreted by claude).
    wrapper_body.push_str("$prompt | claude --dangerously-skip-permissions --verbose -p\n");
    wrapper_body.push_str("$exitCode = $LASTEXITCODE\n");
    wrapper_body.push_str("Write-Host ''\n");
    wrapper_body.push_str("if ($exitCode -eq 0) {\n");
    wrapper_body.push_str("  Write-Host '== Done. You can close this window. ==' -ForegroundColor Green\n");
    wrapper_body.push_str("} else {\n");
    wrapper_body.push_str("  Write-Host \"== Claude exited with code $exitCode ==\" -ForegroundColor Yellow\n");
    wrapper_body.push_str("}\n");
    wrapper_body.push_str("Remove-Item -Force -LiteralPath $MyInvocation.MyCommand.Path -ErrorAction SilentlyContinue\n");

    // PowerShell 5.1 reads .ps1 files using the system ANSI codepage unless a
    // UTF-8 BOM is present. Without the BOM, multibyte UTF-8 chars (Chinese
    // prompt content) get mis-decoded and can break here-string parsing.
    let mut bytes = vec![0xEFu8, 0xBB, 0xBF];
    bytes.extend_from_slice(wrapper_body.as_bytes());
    fs::write(&wrapper_file, &bytes).map_err(|e| e.to_string())?;

    let project_name = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");
    let title = format!("Claude · {} · {}", title_hint, project_name);
    let wrapper_str = wrapper_file.to_string_lossy().to_string();

    Command::new("wt.exe")
        .args([
            "-w",
            "new",
            "--title",
            &title,
            "-d",
            project_path,
            "powershell",
            "-ExecutionPolicy",
            "Bypass",
            "-NoExit",
            "-File",
            &wrapper_str,
        ])
        .spawn()
        .map_err(|e| format!("failed to spawn wt.exe: {}", e))?;

    Ok(())
}

#[tauri::command]
fn generate_scripts(project_path: String) -> Result<(), String> {
    spawn_claude_for_scripts(&project_path, GENERATE_SCRIPTS_PROMPT, "Detect & generate")
}

#[tauri::command]
fn ask_claude_for_scripts(project_path: String, instruction: String) -> Result<(), String> {
    let trimmed = instruction.trim();
    if trimmed.is_empty() {
        return Err("instruction cannot be empty".into());
    }
    let prompt = ASK_CLAUDE_PROMPT.replace("{USER_INSTRUCTION}", trimmed);
    spawn_claude_for_scripts(&project_path, &prompt, "Ask Claude")
}

// ─── Todos ────────────────────────────────────────────────────────────

fn todos_path(project_path: &str) -> PathBuf {
    PathBuf::from(project_path).join(".ccpilot").join("todos.json")
}

fn load_todos(project_path: &str) -> Vec<Todo> {
    let path = todos_path(project_path);
    if !path.exists() {
        return Vec::new();
    }
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let v: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    if let Some(arr) = v.get("todos").and_then(|t| t.as_array()) {
        return arr
            .iter()
            .filter_map(|x| serde_json::from_value::<Todo>(x.clone()).ok())
            .collect();
    }
    Vec::new()
}

fn save_todos(project_path: &str, todos: &[Todo]) -> Result<(), String> {
    let path = todos_path(project_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let obj = serde_json::json!({
        "v": 1,
        "todos": todos,
    });
    let json = serde_json::to_string_pretty(&obj).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn list_todos(project_path: String) -> Result<Vec<Todo>, String> {
    Ok(load_todos(&project_path))
}

#[tauri::command]
fn add_todo(project_path: String, title: String, details: String) -> Result<Todo, String> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("title cannot be empty".into());
    }
    let mut todos = load_todos(&project_path);
    let todo = Todo {
        id: short_random_id(),
        title,
        details,
        completed: false,
    };
    todos.push(todo.clone());
    save_todos(&project_path, &todos)?;
    Ok(todo)
}

#[tauri::command]
fn update_todo(
    project_path: String,
    id: String,
    title: String,
    details: String,
) -> Result<(), String> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("title cannot be empty".into());
    }
    let mut todos = load_todos(&project_path);
    let mut found = false;
    for t in todos.iter_mut() {
        if t.id == id {
            t.title = title.clone();
            t.details = details.clone();
            found = true;
            break;
        }
    }
    if !found {
        return Err("todo not found".into());
    }
    save_todos(&project_path, &todos)
}

#[tauri::command]
fn toggle_todo(project_path: String, id: String) -> Result<bool, String> {
    let mut todos = load_todos(&project_path);
    let mut new_state = false;
    let mut found = false;
    for t in todos.iter_mut() {
        if t.id == id {
            t.completed = !t.completed;
            new_state = t.completed;
            found = true;
            break;
        }
    }
    if !found {
        return Err("todo not found".into());
    }
    save_todos(&project_path, &todos)?;
    Ok(new_state)
}

#[tauri::command]
fn delete_todo(project_path: String, id: String) -> Result<(), String> {
    let mut todos = load_todos(&project_path);
    let before = todos.len();
    todos.retain(|t| t.id != id);
    if todos.len() == before {
        return Err("todo not found".into());
    }
    save_todos(&project_path, &todos)
}

// ─── Global Todos (tabs, separate window) ─────────────────────────────

fn global_todos_path() -> Option<PathBuf> {
    ccpilot_data_dir().map(|d| d.join("global_todos.json"))
}

fn load_global_tabs() -> Vec<GlobalTab> {
    let path = match global_todos_path() {
        Some(p) => p,
        None => return Vec::new(),
    };
    if !path.exists() {
        return Vec::new();
    }
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let v: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    if let Some(arr) = v.get("tabs").and_then(|t| t.as_array()) {
        return arr
            .iter()
            .filter_map(|x| serde_json::from_value::<GlobalTab>(x.clone()).ok())
            .collect();
    }
    Vec::new()
}

fn save_global_tabs(tabs: &[GlobalTab]) -> Result<(), String> {
    let path = global_todos_path().ok_or("no data dir")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let obj = serde_json::json!({
        "v": 1,
        "tabs": tabs,
    });
    let json = serde_json::to_string_pretty(&obj).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn list_global_tabs() -> Result<Vec<GlobalTab>, String> {
    let tabs = load_global_tabs();
    if tabs.is_empty() {
        let default_tab = GlobalTab {
            id: short_random_id(),
            name: "Default".to_string(),
            todos: Vec::new(),
        };
        let _ = save_global_tabs(std::slice::from_ref(&default_tab));
        return Ok(vec![default_tab]);
    }
    Ok(tabs)
}

#[tauri::command]
fn create_global_tab(name: String) -> Result<GlobalTab, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("name cannot be empty".into());
    }
    let mut tabs = load_global_tabs();
    let tab = GlobalTab {
        id: short_random_id(),
        name,
        todos: Vec::new(),
    };
    tabs.push(tab.clone());
    save_global_tabs(&tabs)?;
    Ok(tab)
}

#[tauri::command]
fn rename_global_tab(id: String, name: String) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("name cannot be empty".into());
    }
    let mut tabs = load_global_tabs();
    for t in tabs.iter_mut() {
        if t.id == id {
            t.name = name;
            return save_global_tabs(&tabs);
        }
    }
    Err("tab not found".into())
}

#[tauri::command]
fn delete_global_tab(id: String) -> Result<(), String> {
    let mut tabs = load_global_tabs();
    let before = tabs.len();
    tabs.retain(|t| t.id != id);
    if tabs.len() == before {
        return Err("tab not found".into());
    }
    save_global_tabs(&tabs)
}

#[tauri::command]
fn add_global_todo(tab_id: String, title: String, details: String) -> Result<Todo, String> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("title cannot be empty".into());
    }
    let mut tabs = load_global_tabs();
    for t in tabs.iter_mut() {
        if t.id == tab_id {
            let todo = Todo {
                id: short_random_id(),
                title,
                details,
                completed: false,
            };
            t.todos.push(todo.clone());
            save_global_tabs(&tabs)?;
            return Ok(todo);
        }
    }
    Err("tab not found".into())
}

#[tauri::command]
fn update_global_todo(
    tab_id: String,
    id: String,
    title: String,
    details: String,
) -> Result<(), String> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("title cannot be empty".into());
    }
    let mut tabs = load_global_tabs();
    for t in tabs.iter_mut() {
        if t.id == tab_id {
            for todo in t.todos.iter_mut() {
                if todo.id == id {
                    todo.title = title;
                    todo.details = details;
                    return save_global_tabs(&tabs);
                }
            }
            return Err("todo not found".into());
        }
    }
    Err("tab not found".into())
}

#[tauri::command]
fn toggle_global_todo(tab_id: String, id: String) -> Result<bool, String> {
    let mut tabs = load_global_tabs();
    for t in tabs.iter_mut() {
        if t.id == tab_id {
            for todo in t.todos.iter_mut() {
                if todo.id == id {
                    todo.completed = !todo.completed;
                    let s = todo.completed;
                    save_global_tabs(&tabs)?;
                    return Ok(s);
                }
            }
            return Err("todo not found".into());
        }
    }
    Err("tab not found".into())
}

#[tauri::command]
fn delete_global_todo(tab_id: String, id: String) -> Result<(), String> {
    let mut tabs = load_global_tabs();
    for t in tabs.iter_mut() {
        if t.id == tab_id {
            let before = t.todos.len();
            t.todos.retain(|x| x.id != id);
            if t.todos.len() == before {
                return Err("todo not found".into());
            }
            return save_global_tabs(&tabs);
        }
    }
    Err("tab not found".into())
}

#[tauri::command]
fn reorder_global_todos(tab_id: String, ordered_ids: Vec<String>) -> Result<(), String> {
    let mut tabs = load_global_tabs();
    for t in tabs.iter_mut() {
        if t.id == tab_id {
            let mut id_to: HashMap<String, Todo> = t
                .todos
                .iter()
                .cloned()
                .map(|x| (x.id.clone(), x))
                .collect();
            let mut reordered = Vec::new();
            for id in &ordered_ids {
                if let Some(x) = id_to.remove(id) {
                    reordered.push(x);
                }
            }
            for (_, x) in id_to {
                reordered.push(x);
            }
            t.todos = reordered;
            return save_global_tabs(&tabs);
        }
    }
    Err("tab not found".into())
}

#[tauri::command]
fn open_global_todos_window(app: tauri::AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window("global-todos")
        .ok_or("global-todos window not found in tauri.conf.json")?;
    win.show().map_err(|e| e.to_string())?;
    win.unminimize().ok();
    win.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn reorder_todos(project_path: String, ordered_ids: Vec<String>) -> Result<(), String> {
    let todos = load_todos(&project_path);
    let mut id_to_todo: HashMap<String, Todo> =
        todos.into_iter().map(|t| (t.id.clone(), t)).collect();
    let mut new_todos = Vec::new();
    for id in &ordered_ids {
        if let Some(t) = id_to_todo.remove(id) {
            new_todos.push(t);
        }
    }
    // Append leftovers (unlikely but safe).
    for (_, t) in id_to_todo {
        new_todos.push(t);
    }
    save_todos(&project_path, &new_todos)
}

// ─── Inbox ────────────────────────────────────────────────────────────

fn inbox_dir(project_path: &str) -> PathBuf {
    PathBuf::from(project_path).join(".ccpilot").join("inbox")
}

fn slugify(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else if c.is_whitespace() {
                '-'
            } else if !c.is_ascii() {
                c
            } else {
                '-'
            }
        })
        .collect();
    let mut prev_dash = false;
    let mut out = String::new();
    for c in cleaned.chars() {
        if c == '-' {
            if !prev_dash {
                out.push(c);
            }
            prev_dash = true;
        } else {
            out.push(c);
            prev_dash = false;
        }
    }
    out.trim_matches('-').to_string()
}

fn parse_inbox_feedback(path: &Path) -> (String, String, Vec<String>, String) {
    // Returns (title, description, attachments, status)
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return (String::new(), String::new(), Vec::new(), "pending".into()),
    };
    let mut title = String::new();
    let mut description = String::new();
    let mut attachments = Vec::new();
    let mut status: String = "pending".into();
    let mut section: &str = "";
    for line in content.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("# ") {
            if title.is_empty() {
                title = rest.trim().to_string();
            }
        } else if t.starts_with("## ") {
            let sec = t[3..].trim().to_lowercase();
            section = if sec.contains("description") || sec.contains("notes") {
                "description"
            } else if sec.contains("attachment") {
                "attachments"
            } else {
                ""
            };
        } else if let Some(rest) = t.strip_prefix("> Status:") {
            let s = rest.trim().to_lowercase();
            if matches!(s.as_str(), "pending" | "sent" | "applied" | "ignored") {
                status = s;
            }
        } else if section == "description" && !t.starts_with('>') {
            if !description.is_empty() {
                description.push('\n');
            }
            description.push_str(line);
        } else if section == "attachments" {
            if let Some(rest) = t.strip_prefix("- ") {
                attachments.push(rest.trim().to_string());
            }
        }
    }
    (title, description.trim().to_string(), attachments, status)
}

#[tauri::command]
fn list_inbox(project_path: String) -> Result<Vec<InboxEntry>, String> {
    let dir = inbox_dir(&project_path);
    if !dir.is_dir() {
        return Ok(vec![]);
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let dir_name = match p.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let feedback_md = p.join("feedback.md");
        let (title, description, attachments, status) = parse_inbox_feedback(&feedback_md);
        let created_at_ms = entry
            .metadata()
            .ok()
            .and_then(|m| m.created().or_else(|_| m.modified()).ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        entries.push(InboxEntry {
            dir_name,
            title: if title.is_empty() {
                "Untitled feedback".into()
            } else {
                title
            },
            description,
            attachments,
            created_at_ms,
            status,
        });
    }
    entries.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
    Ok(entries)
}

#[tauri::command]
fn create_inbox_entry(
    project_path: String,
    title: String,
    description: String,
    attachments: Vec<InboxAttachmentInput>,
) -> Result<String, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("title cannot be empty".into());
    }

    let now: chrono::DateTime<chrono::Local> = chrono::Local::now();
    let ts_prefix = now.format("%Y-%m-%d-%H%M").to_string();
    let slug = slugify(title);
    let dir_name = if slug.is_empty() {
        ts_prefix.clone()
    } else {
        format!("{}-{}", ts_prefix, slug)
    };

    let entry_dir = inbox_dir(&project_path).join(&dir_name);
    fs::create_dir_all(&entry_dir).map_err(|e| e.to_string())?;

    // Write attachments
    let mut written_names = Vec::new();
    for att in &attachments {
        let safe_name = att
            .name
            .chars()
            .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.') || !c.is_ascii())
            .collect::<String>();
        if safe_name.is_empty() {
            continue;
        }
        let target = entry_dir.join(&safe_name);
        let bytes = B64
            .decode(att.content_base64.as_bytes())
            .map_err(|e| format!("base64 decode: {}", e))?;
        fs::write(&target, &bytes).map_err(|e| e.to_string())?;
        written_names.push(safe_name);
    }

    // Write feedback.md
    let mut md = String::new();
    md.push_str(&format!("# {}\n\n", title));
    md.push_str(&format!(
        "> Captured: {}\n",
        now.format("%Y-%m-%d %H:%M")
    ));
    md.push_str("> Status: pending\n\n");
    md.push_str("## Description\n\n");
    let desc = description.trim();
    if desc.is_empty() {
        md.push_str("(no notes)\n");
    } else {
        md.push_str(desc);
        md.push('\n');
    }
    if !written_names.is_empty() {
        md.push_str("\n## Attachments\n\n");
        for name in &written_names {
            md.push_str(&format!("- {}\n", name));
        }
    }
    fs::write(entry_dir.join("feedback.md"), md).map_err(|e| e.to_string())?;

    Ok(dir_name)
}

#[tauri::command]
fn update_inbox_entry(
    project_path: String,
    dir_name: String,
    title: String,
    description: String,
) -> Result<(), String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("title cannot be empty".into());
    }
    let entry_dir = inbox_dir(&project_path).join(&dir_name);
    if !entry_dir.is_dir() {
        return Err("entry not found".into());
    }

    // Re-discover attachments from disk so they survive edits.
    let mut attachments = Vec::new();
    if let Ok(rd) = fs::read_dir(&entry_dir) {
        for e in rd.flatten() {
            if let Some(name) = e.file_name().to_str() {
                if name == "feedback.md" || name == "request.md" {
                    continue;
                }
                if e.path().is_file() {
                    attachments.push(name.to_string());
                }
            }
        }
    }
    attachments.sort();

    // Preserve existing status when updating.
    let (_, _, _, existing_status) = parse_inbox_feedback(&entry_dir.join("feedback.md"));

    let now: chrono::DateTime<chrono::Local> = chrono::Local::now();
    let mut md = String::new();
    md.push_str(&format!("# {}\n\n", title));
    md.push_str(&format!(
        "> Captured: {}\n",
        now.format("%Y-%m-%d %H:%M")
    ));
    md.push_str(&format!("> Status: {}\n\n", existing_status));
    md.push_str("## Description\n\n");
    let desc = description.trim();
    if desc.is_empty() {
        md.push_str("(no notes)\n");
    } else {
        md.push_str(desc);
        md.push('\n');
    }
    if !attachments.is_empty() {
        md.push_str("\n## Attachments\n\n");
        for name in &attachments {
            md.push_str(&format!("- {}\n", name));
        }
    }
    fs::write(entry_dir.join("feedback.md"), md).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn set_inbox_status(
    project_path: String,
    dir_name: String,
    status: String,
) -> Result<(), String> {
    if !matches!(status.as_str(), "pending" | "sent" | "applied" | "ignored") {
        return Err("invalid status".into());
    }
    let entry_dir = inbox_dir(&project_path).join(&dir_name);
    let path = entry_dir.join("feedback.md");
    if !path.is_file() {
        return Err("feedback.md not found".into());
    }
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut new_lines: Vec<String> = Vec::new();
    let mut found = false;
    for line in content.lines() {
        if line.trim_start().starts_with("> Status:") {
            new_lines.push(format!("> Status: {}", status));
            found = true;
        } else {
            new_lines.push(line.to_string());
        }
    }
    if !found {
        // Insert after the captured line (or after title) if missing.
        let mut inserted = false;
        let mut out: Vec<String> = Vec::new();
        for line in new_lines.iter() {
            out.push(line.clone());
            if !inserted && line.trim_start().starts_with("> Captured:") {
                out.push(format!("> Status: {}", status));
                inserted = true;
            }
        }
        if !inserted {
            out.insert(0, format!("> Status: {}", status));
        }
        new_lines = out;
    }
    let body = new_lines.join("\n");
    fs::write(&path, body).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn get_session_preview(
    encoded_dir: String,
    session_id: String,
    limit: usize,
) -> Result<Vec<MessagePreview>, String> {
    let root = projects_root().ok_or("home dir not found")?;
    let path = root.join(&encoded_dir).join(format!("{}.jsonl", session_id));
    let f = fs::File::open(&path).map_err(|e| e.to_string())?;
    let reader = BufReader::new(f);
    let mut messages: Vec<MessagePreview> = Vec::new();
    for line in reader.lines().map_while(Result::ok) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            if is_user_record(&v) {
                if let Some(text) = extract_user_text(&v) {
                    let timestamp = v
                        .get("timestamp")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    messages.push(MessagePreview {
                        timestamp,
                        text: truncate_chars(&text, 600),
                    });
                }
            }
        }
    }
    let cap = limit.max(1);
    let start = messages.len().saturating_sub(cap);
    Ok(messages.into_iter().skip(start).collect())
}

#[tauri::command]
fn open_text_in_editor(
    app: tauri::AppHandle,
    content: String,
    suggested_name: String,
) -> Result<String, String> {
    let safe_name: String = suggested_name
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.') || !c.is_ascii())
        .collect();
    let safe_name = if safe_name.is_empty() {
        "ccpilot-edit.md".to_string()
    } else {
        safe_name
    };
    let id = short_random_id();
    let temp_file = std::env::temp_dir().join(format!("ccpilot-{}-{}", id, safe_name));
    // Write with UTF-8 BOM so Notepad on Chinese Windows recognises encoding.
    let mut bytes = vec![0xEFu8, 0xBB, 0xBF];
    bytes.extend_from_slice(content.as_bytes());
    fs::write(&temp_file, &bytes).map_err(|e| e.to_string())?;
    Command::new("notepad.exe")
        .arg(&temp_file)
        .spawn()
        .map_err(|e| format!("failed to open notepad: {}", e))?;

    // Background mtime poller. Emits `editor-saved` whenever the temp file
    // gets re-saved (Notepad Ctrl+S). Polls every 400ms for up to 30 minutes,
    // then gives up to avoid leaking threads.
    let path_for_event = temp_file.to_string_lossy().to_string();
    let app_clone = app.clone();
    std::thread::spawn(move || {
        let path = PathBuf::from(&path_for_event);
        let mut last_mtime = fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok());
        for _ in 0..(30 * 60 * 1000 / 400u64) {
            std::thread::sleep(std::time::Duration::from_millis(400));
            let current = fs::metadata(&path).ok().and_then(|m| m.modified().ok());
            if current.is_none() {
                // File removed (e.g., we deleted it on cancel). Stop watching.
                break;
            }
            if current != last_mtime {
                let _ = app_clone.emit("editor-saved", &path_for_event);
                last_mtime = current;
            }
        }
    });

    Ok(temp_file.to_string_lossy().to_string())
}

#[tauri::command]
fn read_temp_file(path: String) -> Result<String, String> {
    let bytes = fs::read(&path).map_err(|e| e.to_string())?;
    // Strip UTF-8 BOM if present.
    let content_bytes = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        &bytes[..]
    };
    String::from_utf8(content_bytes.to_vec()).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_temp_file(path: String) -> Result<(), String> {
    let _ = fs::remove_file(&path);
    Ok(())
}

#[tauri::command]
fn delete_inbox_entry(project_path: String, dir_name: String) -> Result<(), String> {
    let p = inbox_dir(&project_path).join(&dir_name);
    if !p.exists() {
        return Err("entry not found".into());
    }
    fs::remove_dir_all(&p).map_err(|e| e.to_string())
}

#[tauri::command]
fn read_inbox_attachment_data_url(
    project_path: String,
    dir_name: String,
    file_name: String,
) -> Result<String, String> {
    let p = inbox_dir(&project_path).join(&dir_name).join(&file_name);
    if !p.is_file() {
        return Err("attachment not found".into());
    }
    let bytes = fs::read(&p).map_err(|e| e.to_string())?;
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    };
    Ok(format!("data:{};base64,{}", mime, B64.encode(&bytes)))
}

#[tauri::command]
fn open_inbox_attachment(
    project_path: String,
    dir_name: String,
    file_name: String,
) -> Result<(), String> {
    let p = inbox_dir(&project_path).join(&dir_name).join(&file_name);
    if !p.is_file() {
        return Err("attachment not found".into());
    }
    let path_str = p.to_string_lossy().to_string();
    Command::new("cmd")
        .args(["/c", "start", "", &path_str])
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn open_inbox_entry_folder(project_path: String, dir_name: String) -> Result<(), String> {
    let p = inbox_dir(&project_path).join(&dir_name);
    if !p.exists() {
        return Err("entry not found".into());
    }
    Command::new("explorer.exe")
        .arg(&p)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn launch_snipping_tool() -> Result<(), String> {
    Command::new("explorer.exe")
        .arg("ms-screenclip:")
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn spawn_claude_interactive(
    project_path: &str,
    prompt: &str,
    title_hint: &str,
    extra_flags: &[&str],
) -> Result<(), String> {
    let project_dir = PathBuf::from(project_path);
    if !project_dir.is_dir() {
        return Err("project path is not a directory".into());
    }

    let temp_dir = std::env::temp_dir();
    let id = short_random_id();
    let wrapper_file = temp_dir.join(format!("ccpilot-wrapper-{}.ps1", id));

    let mut wrapper_body = String::new();
    wrapper_body.push_str("$ErrorActionPreference = 'Continue'\n");
    wrapper_body.push_str("$OutputEncoding = [System.Text.Encoding]::UTF8\n");
    wrapper_body.push_str("[Console]::OutputEncoding = [System.Text.Encoding]::UTF8\n");
    wrapper_body.push_str("Write-Host ''\n");
    wrapper_body.push_str("Write-Host '== CCPilot · ");
    wrapper_body.push_str(title_hint);
    wrapper_body.push_str(" ==' -ForegroundColor Cyan\n");
    wrapper_body.push_str("Write-Host 'Sending the feedback as the first user message. Claude will read the inbox files and ask you for any clarifications before changing code.' -ForegroundColor DarkGray\n");
    wrapper_body.push_str("Write-Host ''\n");
    wrapper_body.push_str("$prompt = @'\n");
    wrapper_body.push_str(prompt);
    if !prompt.ends_with('\n') {
        wrapper_body.push('\n');
    }
    wrapper_body.push_str("'@\n");
    wrapper_body.push_str("\n");
    wrapper_body.push_str("# Pass the prompt as a single positional argv to interactive claude.\n");
    let extra = if extra_flags.is_empty() {
        String::new()
    } else {
        format!("{} ", extra_flags.join(" "))
    };
    wrapper_body.push_str(&format!(
        "& claude --dangerously-skip-permissions {}$prompt\n",
        extra
    ));
    wrapper_body
        .push_str("Remove-Item -Force -LiteralPath $MyInvocation.MyCommand.Path -ErrorAction SilentlyContinue\n");

    let mut bytes = vec![0xEFu8, 0xBB, 0xBF];
    bytes.extend_from_slice(wrapper_body.as_bytes());
    fs::write(&wrapper_file, &bytes).map_err(|e| e.to_string())?;

    let project_name = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");
    let title = format!("Claude · {} · {}", title_hint, project_name);
    let wrapper_str = wrapper_file.to_string_lossy().to_string();

    Command::new("wt.exe")
        .args([
            "-w",
            "new",
            "--title",
            &title,
            "-d",
            project_path,
            "powershell",
            "-ExecutionPolicy",
            "Bypass",
            "-NoExit",
            "-File",
            &wrapper_str,
        ])
        .spawn()
        .map_err(|e| format!("failed to spawn wt.exe: {}", e))?;

    Ok(())
}

#[tauri::command]
fn send_inbox_to_claude(
    project_path: String,
    dir_name: String,
    mode: String,
) -> Result<(), String> {
    let entry_path = inbox_dir(&project_path).join(&dir_name);
    if !entry_path.is_dir() {
        return Err("inbox entry not found".into());
    }
    let prompt = SEND_INBOX_PROMPT.replace("{ENTRY_DIR}", &dir_name);
    let (flags, label): (&[&str], &str) = match mode.as_str() {
        "continue" => (&["--continue"], "Inbox+Continue"),
        _ => (&[], "Inbox"),
    };
    spawn_claude_interactive(
        &project_path,
        &prompt,
        &format!("{} · {}", label, dir_name),
        flags,
    )?;
    // Auto-mark this entry as sent. Best-effort; failures don't block.
    let _ = set_inbox_status(project_path, dir_name, "sent".to_string());
    Ok(())
}

#[tauri::command]
fn close_script(
    state: State<'_, AppState>,
    project_path: String,
    file_name: String,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        let key = script_key(&project_path, &file_name);
        let mut map = state.running_scripts.lock().unwrap();
        let run = match map.get(&key) {
            Some(r) => r,
            None => return Err("script not running".into()),
        };
        let hwnd_from_cache = run.hwnd.and_then(|addr| {
            let h = HWND(addr);
            if unsafe { IsWindow(h) }.as_bool() {
                Some(h)
            } else {
                None
            }
        });
        let hwnd = hwnd_from_cache
            .or_else(|| find_window_with_marker(&format!("[{}]", run.script_id)));
        if let Some(h) = hwnd {
            unsafe {
                PostMessageW(h, WM_CLOSE, WPARAM(0), LPARAM(0))
                    .map_err(|e| format!("PostMessage failed: {:?}", e))?;
            }
        }
        map.remove(&key);
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (state, project_path, file_name);
        Err("not supported on this platform".into())
    }
}

#[tauri::command]
fn add_script(
    project_path: String,
    name: String,
    title: String,
    content: String,
) -> Result<(), String> {
    let safe = sanitize_filename(&name);
    if safe.is_empty() {
        return Err("name must contain alphanumeric, dash, or underscore".into());
    }
    let dir = ccpscript_dir(&project_path);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file_path = dir.join(format!("{}.ps1", safe));
    if file_path.exists() {
        return Err(format!("script already exists: {}.ps1", safe));
    }

    let mut body = String::new();
    let title_trim = title.trim();
    if !title_trim.is_empty() {
        body.push_str(&format!("# Title: {}\n", title_trim));
    }
    body.push_str(content.trim_end());
    body.push('\n');
    fs::write(&file_path, body).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn delete_script(project_path: String, file_name: String) -> Result<(), String> {
    let script_path = ccpscript_dir(&project_path).join(&file_name);
    if !script_path.exists() {
        return Err(format!("not found: {}", file_name));
    }
    fs::remove_file(&script_path).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn open_script_in_editor(project_path: String, file_name: String) -> Result<(), String> {
    let script_path = ccpscript_dir(&project_path).join(&file_name);
    Command::new("notepad.exe")
        .arg(&script_path)
        .spawn()
        .map_err(|e| format!("failed to open editor: {}", e))?;
    Ok(())
}

#[tauri::command]
fn open_ccpscript_dir(project_path: String) -> Result<(), String> {
    let dir = ccpscript_dir(&project_path);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Command::new("explorer.exe")
        .arg(&dir)
        .spawn()
        .map_err(|e| format!("failed to open folder: {}", e))?;
    Ok(())
}

#[tauri::command]
fn toggle_star(state: State<'_, AppState>, project_path: String) -> Result<bool, String> {
    let mut stars = state.starred.lock().unwrap();
    let now_starred = if stars.contains(&project_path) {
        stars.remove(&project_path);
        false
    } else {
        stars.insert(project_path.clone());
        true
    };
    save_stars(&stars)?;
    Ok(now_starred)
}

#[tauri::command]
fn delete_session_file(encoded_dir: String, session_id: String) -> Result<(), String> {
    let root = projects_root().ok_or("home dir not found")?;
    let path = root.join(&encoded_dir).join(format!("{}.jsonl", session_id));
    if !path.exists() {
        return Err("session file not found".into());
    }
    fs::remove_file(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_project_alias(
    state: State<'_, AppState>,
    project_path: String,
    alias: String,
) -> Result<(), String> {
    let mut store = state.aliases.lock().unwrap();
    let trimmed = alias.trim().to_string();
    if trimmed.is_empty() {
        store.projects.remove(&project_path);
    } else {
        store.projects.insert(project_path, trimmed);
    }
    save_aliases(&*store)
}

#[tauri::command]
fn set_session_alias(
    state: State<'_, AppState>,
    session_id: String,
    alias: String,
) -> Result<(), String> {
    let mut store = state.aliases.lock().unwrap();
    let trimmed = alias.trim().to_string();
    if trimmed.is_empty() {
        store.sessions.remove(&session_id);
    } else {
        store.sessions.insert(session_id, trimmed);
    }
    save_aliases(&*store)
}

#[tauri::command]
fn toggle_archive(state: State<'_, AppState>, project_path: String) -> Result<bool, String> {
    let mut set = state.archived.lock().unwrap();
    let now_archived = if set.contains(&project_path) {
        set.remove(&project_path);
        false
    } else {
        set.insert(project_path.clone());
        true
    };
    save_archived(&set)?;
    Ok(now_archived)
}

fn session_title(project_name: &str, session_id: &str) -> String {
    let prefix: String = session_id.chars().take(8).collect();
    format!("Claude - {} [{}]", project_name, prefix)
}

#[tauri::command]
fn open_session(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    project_path: String,
    project_name: String,
) -> Result<(), String> {
    let claude_cmd = format!(
        "claude --dangerously-skip-permissions --resume {}",
        session_id
    );
    let title = session_title(&project_name, &session_id);

    let child = Command::new("wt.exe")
        .args([
            "-w", "new",
            "--title", &title,
            "-d", &project_path,
            "powershell",
            "-NoExit",
            "-Command",
            &claude_cmd,
        ])
        .spawn()
        .map_err(|e| format!("failed to spawn wt.exe: {}", e))?;

    state
        .running
        .lock()
        .unwrap()
        .insert(session_id.clone(), child.id());

    // Capture HWND in background while title still matches our marker.
    #[cfg(windows)]
    {
        let id = session_id.clone();
        let app = app.clone();
        std::thread::spawn(move || {
            for _ in 0..40 {
                std::thread::sleep(std::time::Duration::from_millis(150));
                if let Some(hwnd) = find_window_for_session(&id) {
                    let st = app.state::<AppState>();
                    st.windows
                        .lock()
                        .unwrap()
                        .insert(id.clone(), hwnd.0 as isize);
                    break;
                }
            }
        });
    }
    #[cfg(not(windows))]
    let _ = app;

    Ok(())
}

#[cfg(windows)]
struct FindCtx {
    needle: String,
    found: Option<HWND>,
}

#[cfg(windows)]
unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = unsafe { &mut *(lparam.0 as *mut FindCtx) };

    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return TRUE;
    }

    let mut buffer = vec![0u16; (len + 1) as usize];
    let n = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    if n <= 0 {
        return TRUE;
    }

    let title = String::from_utf16_lossy(&buffer[..n as usize]);
    if title.contains(&ctx.needle) {
        ctx.found = Some(hwnd);
        return BOOL(0); // stop enumeration
    }
    TRUE
}

#[cfg(windows)]
fn find_window_with_marker(needle: &str) -> Option<HWND> {
    let mut ctx = FindCtx {
        needle: needle.to_string(),
        found: None,
    };
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut ctx as *mut _ as isize));
    }
    ctx.found
}

#[cfg(windows)]
fn find_window_for_session(session_id: &str) -> Option<HWND> {
    let prefix: String = session_id.chars().take(8).collect();
    find_window_with_marker(&format!("[{}]", prefix))
}

#[cfg(windows)]
fn lookup_hwnd(state: &State<'_, AppState>, session_id: &str) -> Option<HWND> {
    // Try cached HWND with liveness check.
    let cached = state.windows.lock().unwrap().get(session_id).copied();
    if let Some(addr) = cached {
        let hwnd = HWND(addr);
        if unsafe { IsWindow(hwnd) }.as_bool() {
            return Some(hwnd);
        }
        // Stale: drop it.
        state.windows.lock().unwrap().remove(session_id);
    }
    // Fallback: title search.
    if let Some(hwnd) = find_window_for_session(session_id) {
        state
            .windows
            .lock()
            .unwrap()
            .insert(session_id.to_string(), hwnd.0 as isize);
        return Some(hwnd);
    }
    None
}

#[tauri::command]
fn focus_session(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        let hwnd = lookup_hwnd(&state, &session_id).ok_or("window not found")?;
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (state, session_id);
        Err("not supported on this platform".into())
    }
}

#[tauri::command]
fn close_session(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        let hwnd = lookup_hwnd(&state, &session_id).ok_or("window not found")?;
        unsafe {
            PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0))
                .map_err(|e| format!("PostMessage failed: {:?}", e))?;
        }
        state.windows.lock().unwrap().remove(&session_id);
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (state, session_id);
        Err("not supported on this platform".into())
    }
}

#[tauri::command]
fn open_folder(path: String) -> Result<(), String> {
    Command::new("explorer.exe")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("failed to open folder: {}", e))?;
    Ok(())
}

fn spawn_wt_with_claude(path: &str, title: &str) -> Result<(), String> {
    Command::new("wt.exe")
        .args([
            "-w", "new",
            "--title", title,
            "-d", path,
            "powershell",
            "-NoExit",
            "-Command",
            "claude --dangerously-skip-permissions",
        ])
        .spawn()
        .map_err(|e| format!("failed to spawn wt.exe: {}", e))?;
    Ok(())
}

#[tauri::command]
fn new_temp_session() -> Result<(), String> {
    let temp_root = PathBuf::from("E:\\ccpilot-tasks");
    fs::create_dir_all(&temp_root).map_err(|e| e.to_string())?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let dir = temp_root.join(format!("task-{}", now));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let path = dir.to_string_lossy().to_string();
    let title = format!("Claude: task-{}", now);
    spawn_wt_with_claude(&path, &title)
}

#[tauri::command]
fn new_session_in_dir(path: String) -> Result<(), String> {
    let dir = PathBuf::from(&path);
    if !dir.is_dir() {
        return Err(format!("not a directory: {}", path));
    }
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&path)
        .to_string();
    let title = format!("Claude: {}", name);
    spawn_wt_with_claude(&path, &title)
}

#[tauri::command]
fn new_terminal(path: Option<String>) -> Result<(), String> {
    let dir = match path {
        Some(p) => p,
        None => dirs::home_dir()
            .ok_or("home dir not found")?
            .to_string_lossy()
            .to_string(),
    };

    Command::new("wt.exe")
        .args([
            "-w", "new",
            "--title", "PowerShell",
            "-d", &dir,
            "powershell",
        ])
        .spawn()
        .map_err(|e| format!("failed to spawn wt.exe: {}", e))?;
    Ok(())
}

// ─── System info / proxy monitor ──────────────────────────────────────

static START_TIME: OnceLock<Instant> = OnceLock::new();
static NODE_VERSION: Mutex<Option<Option<String>>> = Mutex::new(None);

#[tauri::command]
fn app_uptime_ms() -> u64 {
    START_TIME
        .get()
        .map(|t| t.elapsed().as_millis() as u64)
        .unwrap_or(0)
}

fn read_node_version() -> Option<String> {
    let out = silent_command("node").arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let v = s.trim().trim_start_matches('v').to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

#[tauri::command]
fn node_version() -> Option<String> {
    let mut g = NODE_VERSION.lock().unwrap();
    if g.is_none() {
        *g = Some(read_node_version());
    }
    g.clone().unwrap_or(None)
}

#[tauri::command]
fn node_version_refresh() -> Option<String> {
    let v = read_node_version();
    *NODE_VERSION.lock().unwrap() = Some(v.clone());
    v
}

#[derive(Serialize, Default)]
struct NvmInfo {
    available: bool,
    current: Option<String>,
    versions: Vec<String>,
    error: Option<String>,
}

#[tauri::command]
fn nvm_list() -> NvmInfo {
    let mut info = NvmInfo::default();
    let out = match silent_command("nvm").arg("list").output() {
        Ok(o) => o,
        Err(_) => {
            info.error = Some("nvm not found in PATH".into());
            return info;
        }
    };
    if !out.status.success() {
        info.error = Some(format!(
            "nvm list failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
        return info;
    }
    info.available = true;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    // nvm-windows output: lines like "  * 22.5.1 (Currently using 64-bit executable)"
    // or "    20.10.0".
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("No version") {
            continue;
        }
        let is_current = line.trim_start().starts_with('*');
        let cleaned = t.trim_start_matches('*').trim();
        let ver: String = cleaned
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if ver.is_empty() {
            continue;
        }
        if is_current {
            info.current = Some(ver.clone());
        }
        info.versions.push(ver);
    }
    info
}

#[tauri::command]
fn nvm_use(version: String) -> Result<String, String> {
    let out = silent_command("nvm")
        .args(["use", &version])
        .output()
        .map_err(|e| format!("nvm spawn failed: {}", e))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        return Err(if !stderr.is_empty() { stderr } else { stdout });
    }
    let combined = if stdout.is_empty() { stderr } else { stdout };
    // Best-effort: refresh cached node version after switch.
    let _ = node_version_refresh();
    Ok(combined)
}

fn build_proxy_arg(host: &str, port: u16) -> String {
    if port == 10808 {
        format!("socks5h://{}:{}", host, port)
    } else {
        format!("http://{}:{}", host, port)
    }
}

fn check_proxy_listen(host: &str, port: u16) -> Result<(), String> {
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;
    let addr: SocketAddr = format!("{}:{}", host, port)
        .parse()
        .map_err(|e: std::net::AddrParseError| format!("bad addr: {}", e))?;
    TcpStream::connect_timeout(&addr, Duration::from_millis(300))
        .map(|_| ())
        .map_err(|_| "proxy port not listening".into())
}

#[derive(Serialize, Clone, Default)]
struct ProxyLatency {
    alive: bool,
    latency_ms: Option<u32>,
    error: Option<String>,
}

#[tauri::command]
fn proxy_latency(host: String, port: u16) -> ProxyLatency {
    let mut s = ProxyLatency::default();
    if let Err(e) = check_proxy_listen(&host, port) {
        s.error = Some(e);
        return s;
    }
    s.alive = true;
    let proxy_arg = build_proxy_arg(&host, port);
    let out = silent_command("curl")
        .args([
            "-x",
            &proxy_arg,
            "-m",
            "5",
            "-s",
            "-o",
            "NUL",
            "-w",
            "%{time_total}",
            "https://www.gstatic.com/generate_204",
        ])
        .output();
    let out = match out {
        Ok(o) => o,
        Err(e) => {
            s.error = Some(format!("curl spawn: {}", e));
            return s;
        }
    };
    if !out.status.success() {
        s.error = Some(format!(
            "curl exit {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
        return s;
    }
    let secs_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if let Ok(secs) = secs_str.parse::<f64>() {
        s.latency_ms = Some((secs * 1000.0) as u32);
    } else {
        s.error = Some(format!("bad time output: {}", secs_str));
    }
    s
}

#[derive(Serialize, Clone, Default)]
struct ProxyGeo {
    alive: bool,
    ip: Option<String>,
    country: Option<String>,
    region: Option<String>,
    city: Option<String>,
    error: Option<String>,
}

#[tauri::command]
fn proxy_geo(host: String, port: u16) -> ProxyGeo {
    let mut s = ProxyGeo::default();
    if let Err(e) = check_proxy_listen(&host, port) {
        s.error = Some(e);
        return s;
    }
    s.alive = true;
    let proxy_arg = build_proxy_arg(&host, port);
    let out = silent_command("curl")
        .args([
            "-x",
            &proxy_arg,
            "-m",
            "10",
            "-s",
            "-A",
            "Mozilla/5.0 (CCPilot)",
            "https://ipinfo.io/json",
        ])
        .output();
    let out = match out {
        Ok(o) => o,
        Err(e) => {
            s.error = Some(format!("curl spawn: {}", e));
            return s;
        }
    };
    if !out.status.success() {
        s.error = Some(format!(
            "curl exit {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
        return s;
    }
    let body = String::from_utf8_lossy(&out.stdout).to_string();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
        s.ip = v.get("ip").and_then(|x| x.as_str()).map(String::from);
        s.country = v
            .get("country_name")
            .and_then(|x| x.as_str())
            .or_else(|| v.get("country").and_then(|x| x.as_str()))
            .map(String::from);
        s.region = v.get("region").and_then(|x| x.as_str()).map(String::from);
        s.city = v.get("city").and_then(|x| x.as_str()).map(String::from);
        if s.ip.is_none() {
            if let Some(err) = v
                .get("reason")
                .and_then(|x| x.as_str())
                .or_else(|| v.get("error").and_then(|x| x.as_str()))
            {
                s.error = Some(err.to_string());
            }
        }
    } else {
        let snippet = body.chars().take(120).collect::<String>();
        s.error = Some(format!("non-JSON: {}", snippet));
    }
    s
}

// ─── Claude Code subscription usage monitor ───────────────────────────
//
// Strategy: register a tiny statusline shim with Claude Code. Claude Code
// already pipes `rate_limits` (the same data behind /usage) on stdin to any
// configured statusline command. The shim reads that JSON, writes a snapshot
// file, and outputs nothing — so the user's terminal sees no visual change.

const USAGE_SHIM_JS: &str = r#"#!/usr/bin/env node
// CCPilot statusline shim. Captures rate_limits from Claude Code's stdin
// payload and writes it to a snapshot file for CCPilot to display.
const fs = require('fs');
const path = require('path');
const os = require('os');

// Match Rust dirs::data_local_dir() on Windows: %LOCALAPPDATA%\CCPilot
const dir = path.join(
  process.env.LOCALAPPDATA || path.join(os.homedir(), 'AppData', 'Local'),
  'CCPilot'
);
const snap = path.join(dir, 'usage-snapshot.json');

let raw = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', (c) => { raw += c; });
process.stdin.on('end', () => {
  try {
    const j = JSON.parse(raw || '{}');
    const rl = j.rate_limits || {};
    const out = {
      updated_at: Date.now(),
      five_hour: rl.five_hour || null,
      seven_day: rl.seven_day || null,
    };
    fs.mkdirSync(dir, { recursive: true });
    const tmp = snap + '.tmp';
    fs.writeFileSync(tmp, JSON.stringify(out));
    fs.renameSync(tmp, snap);
  } catch (_) {}
  // Silent: no output, so user's claude prompt looks unchanged.
  process.exit(0);
});
// Safety: if stdin never closes, exit anyway.
setTimeout(() => process.exit(0), 3000);
"#;

fn ccpilot_data_dir_ensured() -> Result<PathBuf, String> {
    let dir = ccpilot_data_dir().ok_or("no LOCALAPPDATA")?;
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {}", dir.display(), e))?;
    Ok(dir)
}

fn shim_path() -> Result<PathBuf, String> {
    Ok(ccpilot_data_dir_ensured()?.join("usage-shim.js"))
}

fn snapshot_path() -> Result<PathBuf, String> {
    Ok(ccpilot_data_dir_ensured()?.join("usage-snapshot.json"))
}

fn claude_settings_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("no home dir")?;
    Ok(home.join(".claude").join("settings.json"))
}

fn write_atomic(path: &Path, contents: &str) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents).map_err(|e| format!("write {}: {}", tmp.display(), e))?;
    fs::rename(&tmp, path).map_err(|e| format!("rename {}: {}", path.display(), e))?;
    Ok(())
}

#[derive(Serialize, Default)]
struct UsageMonitorStatus {
    enabled: bool,
    settings_exists: bool,
    shim_path: Option<String>,
    snapshot_path: Option<String>,
    error: Option<String>,
}

#[tauri::command]
fn usage_monitor_status() -> UsageMonitorStatus {
    let mut s = UsageMonitorStatus::default();
    let settings = match claude_settings_path() {
        Ok(p) => p,
        Err(e) => {
            s.error = Some(e);
            return s;
        }
    };
    s.settings_exists = settings.exists();
    if s.settings_exists {
        if let Ok(text) = fs::read_to_string(&settings) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(sl) = v.get("statusLine") {
                    let cmd = sl
                        .get("command")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_lowercase();
                    s.enabled = cmd.contains("ccpilot") && cmd.contains("usage-shim");
                }
            }
        }
    }
    if let Ok(p) = shim_path() {
        s.shim_path = Some(p.to_string_lossy().into_owned());
    }
    if let Ok(p) = snapshot_path() {
        s.snapshot_path = Some(p.to_string_lossy().into_owned());
    }
    s
}

#[tauri::command]
fn enable_usage_monitor() -> Result<String, String> {
    // 1. Write the shim file to %APPDATA%\ccpilot\usage-shim.js
    let shim = shim_path()?;
    write_atomic(&shim, USAGE_SHIM_JS)?;

    // 2. Update ~/.claude/settings.json — add/replace statusLine
    let settings_path = claude_settings_path()?;
    if let Some(parent) = settings_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {}", parent.display(), e))?;
    }
    let existing = fs::read_to_string(&settings_path).unwrap_or_else(|_| "{}".to_string());
    let mut v: serde_json::Value =
        serde_json::from_str(&existing).map_err(|e| format!("settings.json parse: {}", e))?;
    if !v.is_object() {
        v = serde_json::json!({});
    }
    let shim_str = shim.to_string_lossy().replace('\\', "/");
    v["statusLine"] = serde_json::json!({
        "type": "command",
        "command": format!("node \"{}\"", shim_str)
    });
    let pretty = serde_json::to_string_pretty(&v)
        .map_err(|e| format!("settings.json serialize: {}", e))?;
    write_atomic(&settings_path, &pretty)?;
    Ok(shim_str)
}

#[tauri::command]
fn disable_usage_monitor() -> Result<(), String> {
    let settings_path = claude_settings_path()?;
    if !settings_path.exists() {
        return Ok(());
    }
    let text = fs::read_to_string(&settings_path)
        .map_err(|e| format!("read settings: {}", e))?;
    let mut v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("settings.json parse: {}", e))?;
    if let Some(obj) = v.as_object_mut() {
        if let Some(sl) = obj.get("statusLine").cloned() {
            let cmd = sl
                .get("command")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_lowercase();
            if cmd.contains("ccpilot") && cmd.contains("usage-shim") {
                obj.remove("statusLine");
            }
        }
    }
    let pretty = serde_json::to_string_pretty(&v)
        .map_err(|e| format!("settings.json serialize: {}", e))?;
    write_atomic(&settings_path, &pretty)?;
    Ok(())
}

#[derive(Serialize, Default)]
struct UsageWindow {
    used_percentage: Option<f64>,
    resets_at: Option<String>,
}

#[derive(Serialize, Default)]
struct UsageSnapshot {
    available: bool,
    updated_at_ms: Option<i64>,
    five_hour: Option<UsageWindow>,
    seven_day: Option<UsageWindow>,
    error: Option<String>,
}

fn parse_window(v: &serde_json::Value) -> Option<UsageWindow> {
    if v.is_null() {
        return None;
    }
    let used = v.get("used_percentage").and_then(|x| x.as_f64());
    let resets = v
        .get("resets_at")
        .and_then(|x| x.as_str())
        .map(String::from);
    if used.is_none() && resets.is_none() {
        return None;
    }
    Some(UsageWindow {
        used_percentage: used,
        resets_at: resets,
    })
}

#[tauri::command]
fn read_usage_snapshot() -> UsageSnapshot {
    let mut out = UsageSnapshot::default();
    let path = match snapshot_path() {
        Ok(p) => p,
        Err(e) => {
            out.error = Some(e);
            return out;
        }
    };
    if !path.exists() {
        return out; // available=false, no error — just no snapshot yet
    }
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            out.error = Some(format!("read snapshot: {}", e));
            return out;
        }
    };
    let v: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            out.error = Some(format!("snapshot parse: {}", e));
            return out;
        }
    };
    out.updated_at_ms = v.get("updated_at").and_then(|x| x.as_i64());
    if let Some(fh) = v.get("five_hour") {
        out.five_hour = parse_window(fh);
    }
    if let Some(sd) = v.get("seven_day") {
        out.seven_day = parse_window(sd);
    }
    out.available = out.five_hour.is_some() || out.seven_day.is_some();
    out
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = START_TIME.set(Instant::now());
    tauri::Builder::default()
        .setup(|app| {
            use tauri::menu::{MenuBuilder, MenuItemBuilder};
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

            // Closing main / global-todos hides instead of destroying — quit only via tray.
            for label in ["main", "global-todos"] {
                if let Some(win) = app.get_webview_window(label) {
                    let win_clone = win.clone();
                    win.on_window_event(move |event| {
                        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                            api.prevent_close();
                            let _ = win_clone.hide();
                        }
                    });
                }
            }

            // System tray icon with show/quit menu.
            let show_item = MenuItemBuilder::with_id("show", "Show CCPilot").build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let menu = MenuBuilder::new(app)
                .items(&[&show_item, &quit_item])
                .build()?;

            let icon = app.default_window_icon().cloned().ok_or("no icon")?;

            let _ = TrayIconBuilder::with_id("ccpilot-tray")
                .icon(icon)
                .tooltip("CCPilot")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.unminimize();
                            let _ = w.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.unminimize();
                            let _ = w.set_focus();
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .manage(AppState {
            running: Mutex::new(HashMap::new()),
            meta_cache: Mutex::new(HashMap::new()),
            windows: Mutex::new(HashMap::new()),
            starred: Mutex::new(load_stars()),
            running_scripts: Mutex::new(HashMap::new()),
            aliases: Mutex::new(load_aliases()),
            archived: Mutex::new(load_archived()),
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            list_projects,
            open_session,
            open_folder,
            new_temp_session,
            new_session_in_dir,
            new_terminal,
            focus_session,
            close_session,
            toggle_star,
            delete_session_file,
            set_project_alias,
            set_session_alias,
            toggle_archive,
            list_scripts,
            run_script,
            close_script,
            add_script,
            delete_script,
            open_script_in_editor,
            open_ccpscript_dir,
            generate_scripts,
            ask_claude_for_scripts,
            list_todos,
            add_todo,
            update_todo,
            toggle_todo,
            delete_todo,
            reorder_todos,
            list_global_tabs,
            create_global_tab,
            rename_global_tab,
            delete_global_tab,
            add_global_todo,
            update_global_todo,
            toggle_global_todo,
            delete_global_todo,
            reorder_global_todos,
            open_global_todos_window,
            list_inbox,
            create_inbox_entry,
            update_inbox_entry,
            delete_inbox_entry,
            set_inbox_status,
            open_inbox_entry_folder,
            read_inbox_attachment_data_url,
            open_inbox_attachment,
            launch_snipping_tool,
            send_inbox_to_claude,
            get_session_preview,
            open_text_in_editor,
            read_temp_file,
            delete_temp_file,
            app_uptime_ms,
            node_version,
            node_version_refresh,
            nvm_list,
            nvm_use,
            proxy_latency,
            proxy_geo,
            usage_monitor_status,
            enable_usage_monitor,
            disable_usage_monitor,
            read_usage_snapshot,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
