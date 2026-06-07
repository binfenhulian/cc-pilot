<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import {
    isPermissionGranted,
    requestPermission,
    sendNotification,
  } from "@tauri-apps/plugin-notification";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount, onDestroy } from "svelte";
  import Icon from "$lib/Icon.svelte";
  import GlobalTodosView from "$lib/GlobalTodosView.svelte";

  let isGlobalTodosWindow = $state(false);
  try {
    isGlobalTodosWindow = getCurrentWindow().label === "global-todos";
  } catch {
    // fallback: not in tauri context
  }

  type SessionMeta = {
    id: string;
    first_user_message: string;
    alias: string;
    last_activity_ms: number;
    message_count: number;
    is_running: boolean;
  };

  type ProjectGroup = {
    project_path: string;
    project_name: string;
    alias: string;
    encoded_dir: string;
    is_starred: boolean;
    is_archived: boolean;
    last_activity_ms: number;
    running_count: number;
    todo_pending_count: number;
    sessions: SessionMeta[];
  };

  type Todo = {
    id: string;
    title: string;
    details: string;
    completed: boolean;
    action_kind?: string;    // "" | "url" | "folder"
    action_target?: string;
  };

  type ScriptItem = {
    title: string;
    file_name: string;
    kind: string;
    is_running: boolean;
  };

  type InboxEntry = {
    dir_name: string;
    title: string;
    description: string;
    attachments: string[];
    created_at_ms: number;
    status: "pending" | "sent" | "applied" | "ignored";
  };

  type MessagePreview = {
    timestamp: string;
    text: string;
  };

  type InboxAttachment = {
    name: string;
    content_base64: string;
  };

  type StatusFilter = "all" | "starred" | "running" | "archived";
  type TimeFilter = "all" | "today" | "3d" | "week" | "older";

  let projects = $state<ProjectGroup[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);

  let searchQuery = $state("");
  let statusFilter = $state<StatusFilter>("all");
  let timeFilter = $state<TimeFilter>("all");

  let expandOverrides = $state<Record<string, boolean>>({});

  let menuOpen = $state(false);
  let menuRef: HTMLDivElement | undefined;
  let overflowFor = $state<string | null>(null); // project_path

  let scriptsMenuFor = $state<string | null>(null); // keyed by project_path
  let scriptsByPath = $state<Record<string, ScriptItem[]>>({});
  let addingFor = $state<string | null>(null);
  let addName = $state("");
  let addTitle = $state("");
  let addContent = $state("");
  let askingFor = $state<string | null>(null);
  let askInstruction = $state("");

  let inboxOpenFor = $state<string | null>(null); // project_path
  let inboxByPath = $state<Record<string, InboxEntry[]>>({});
  let addingInboxFor = $state<string | null>(null);
  let editingInboxFor = $state<{ projectPath: string; dirName: string } | null>(null);
  let newInboxTitle = $state("");
  let newInboxDescription = $state("");
  let newInboxAttachments = $state<InboxAttachment[]>([]);
  let descEditorPath = $state<string | null>(null);
  let screenshotWaiting = $state(false);
  let thumbnails = $state<Record<string, string>>({}); // key: <project>|<dir>|<file>
  let sendMenuFor = $state<string | null>(null); // <project>|<dir>

  let todosOpenFor = $state<string | null>(null); // project_path
  let todosByPath = $state<Record<string, Todo[]>>({});
  let dragSourceIdx = $state<number | null>(null);
  let dragOverIdx = $state<number | null>(null);

  let inboxStatusFilter = $state<"all" | "pending" | "sent" | "applied" | "ignored">("all");

  let sessionPreview = $state<{
    messages: MessagePreview[];
    title: string;
    sessionId: string;
    loading: boolean;
  } | null>(null);

  let prevRunningSessionIds = new Set<string>();
  let notificationsEnabled = $state(false);

  type ProxyLatency = { alive: boolean; latency_ms: number | null; error: string | null };
  type ProxyGeo = {
    alive: boolean;
    ip: string | null;
    country: string | null;
    region: string | null;
    city: string | null;
    error: string | null;
  };
  let nodeVer = $state<string | null>(null);
  let uptimeMs = $state(0);
  let proxyLat = $state<ProxyLatency | null>(null);
  let proxyGeo = $state<ProxyGeo | null>(null);
  let latChecking = $state(false);
  let geoChecking = $state(false);
  let proxyHost = $state("127.0.0.1");
  let proxyPort = $state(10809);

  // Diagnostic toggle: pause all background probes (latency, geo, usage,
  // node version). Project list polling (load()) is kept on so the UI keeps
  // working. Persisted across launches so an A/B test survives a restart.
  let probesPaused = $state(false);
  try {
    probesPaused = localStorage.getItem("probesPaused") === "1";
  } catch {}
  function toggleProbes() {
    probesPaused = !probesPaused;
    try { localStorage.setItem("probesPaused", probesPaused ? "1" : "0"); } catch {}
    if (!probesPaused) {
      // Resuming — kick a fresh refresh of each immediately.
      refreshLatency();
      refreshGeo();
      refreshUsage();
      invoke<string | null>("node_version_refresh").then((v) => (nodeVer = v)).catch(() => {});
    }
  }

  const latTier = $derived(
    !proxyLat ? "idle"
    : !proxyLat.alive ? "down"
    : proxyLat.error ? "error"
    : proxyLat.latency_ms == null ? "idle"
    : proxyLat.latency_ms < 300 ? "fast"
    : proxyLat.latency_ms < 1000 ? "slow"
    : "very-slow"
  );

  async function refreshLatency() {
    if (latChecking) return;
    latChecking = true;
    try {
      proxyLat = await invoke<ProxyLatency>("proxy_latency", { host: proxyHost, port: proxyPort });
    } catch (e) {
      proxyLat = { alive: false, latency_ms: null, error: String(e) };
    } finally {
      latChecking = false;
    }
  }

  async function refreshGeo() {
    if (geoChecking) return;
    geoChecking = true;
    try {
      proxyGeo = await invoke<ProxyGeo>("proxy_geo", { host: proxyHost, port: proxyPort });
    } catch (e) {
      proxyGeo = { alive: false, ip: null, country: null, region: null, city: null, error: String(e) };
    } finally {
      geoChecking = false;
    }
  }

  type UsageWindow = { used_percentage: number | null; resets_at: string | null };
  type UsageSnapshot = {
    available: boolean;
    updated_at_ms: number | null;
    five_hour: UsageWindow | null;
    seven_day: UsageWindow | null;
    error: string | null;
  };
  type UsageMonitorStatus = {
    enabled: boolean;
    settings_exists: boolean;
    shim_path: string | null;
    snapshot_path: string | null;
    error: string | null;
  };
  let usageMon = $state<UsageMonitorStatus | null>(null);
  let usageSnap = $state<UsageSnapshot | null>(null);
  let usageBusy = $state(false);

  async function refreshUsage() {
    try {
      usageMon = await invoke<UsageMonitorStatus>("usage_monitor_status");
      usageSnap = await invoke<UsageSnapshot>("read_usage_snapshot");
    } catch (e) {
      error = String(e);
    }
  }

  async function enableUsageMonitor() {
    if (usageBusy) return;
    usageBusy = true;
    try {
      await invoke("enable_usage_monitor");
      await refreshUsage();
    } catch (e) {
      error = `Enable failed: ${e}`;
    } finally {
      usageBusy = false;
    }
  }

  async function disableUsageMonitor() {
    if (usageBusy) return;
    usageBusy = true;
    try {
      await invoke("disable_usage_monitor");
      await refreshUsage();
    } catch (e) {
      error = `Disable failed: ${e}`;
    } finally {
      usageBusy = false;
    }
  }

  function fmtUsageAge(ms: number | null): string {
    if (!ms) return "no data";
    const elapsed = Math.max(0, Date.now() - ms);
    const sec = Math.floor(elapsed / 1000);
    if (sec < 60) return `${sec}s ago`;
    const min = Math.floor(sec / 60);
    if (min < 60) return `${min}m ago`;
    const h = Math.floor(min / 60);
    return `${h}h ${min % 60}m ago`;
  }

  function isUsageStale(ms: number | null): boolean {
    if (!ms) return true;
    return Date.now() - ms > 10 * 60_000; // 10 min
  }

  type NvmInfo = { available: boolean; current: string | null; versions: string[]; error: string | null };
  let nvmMenuOpen = $state(false);
  let nvmInfo = $state<NvmInfo | null>(null);
  let nvmSwitching = $state<string | null>(null);
  let nvmFlash = $state<string | null>(null);

  async function toggleNvmMenu(e: MouseEvent) {
    e.stopPropagation();
    if (nvmMenuOpen) {
      nvmMenuOpen = false;
      return;
    }
    nvmMenuOpen = true;
    nvmInfo = null;
    try {
      // Refresh both in parallel so the menu and the badge agree.
      const [list, ver] = await Promise.all([
        invoke<NvmInfo>("nvm_list"),
        invoke<string | null>("node_version_refresh"),
      ]);
      nvmInfo = list;
      nodeVer = ver;
    } catch (e) {
      nvmInfo = { available: false, current: null, versions: [], error: String(e) };
    }
  }

  async function switchNode(version: string) {
    if (nvmSwitching) return;
    nvmSwitching = version;
    nvmFlash = null;
    try {
      await invoke("nvm_use", { version });
      const v = await invoke<string | null>("node_version_refresh");
      nodeVer = v;
      nvmFlash = `→ ${v ?? version}`;
      setTimeout(() => (nvmFlash = null), 2500);
      // refresh menu state to reflect new "current"
      try {
        nvmInfo = await invoke<NvmInfo>("nvm_list");
      } catch {}
    } catch (e) {
      nvmFlash = `failed: ${String(e).slice(0, 80)}`;
      setTimeout(() => (nvmFlash = null), 4000);
    } finally {
      nvmSwitching = null;
      nvmMenuOpen = false;
    }
  }

  let todoModal = $state<{ projectPath: string; id: string | null } | null>(null);
  let modalTodoTitle = $state("");
  let modalTodoDetails = $state("");
  let modalTodoActionKind = $state<"" | "url" | "folder">("");
  let modalTodoActionTarget = $state("");

  let preview = $state<{ src: string; name: string; openExternal: (() => void) | null } | null>(
    null
  );

  function openPreview(src: string, name: string, openExternal: (() => void) | null = null) {
    preview = { src, name, openExternal };
  }

  function closePreview() {
    preview = null;
  }

  $effect(() => {
    if (!preview) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        closePreview();
      }
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  });

  function isImageName(name: string): boolean {
    return /\.(png|jpe?g|gif|webp|svg)$/i.test(name);
  }

  function thumbKey(projectPath: string, dirName: string, file: string): string {
    return `${projectPath}|${dirName}|${file}`;
  }

  function getMimeFromName(name: string): string {
    const ext = name.split(".").pop()?.toLowerCase() || "";
    switch (ext) {
      case "png": return "image/png";
      case "jpg":
      case "jpeg": return "image/jpeg";
      case "gif": return "image/gif";
      case "webp": return "image/webp";
      case "svg": return "image/svg+xml";
      default: return "application/octet-stream";
    }
  }

  let editingProjectFor = $state<string | null>(null); // project_path
  let editingSessionFor = $state<string | null>(null); // session_id
  let editingValue = $state("");

  let pollTimer: ReturnType<typeof setInterval> | null = null;
  const POLL_MS = 5000;

  let filteredProjects = $derived.by(() => {
    const q = searchQuery.trim().toLowerCase();
    const now = Date.now();
    const startOfToday = new Date();
    startOfToday.setHours(0, 0, 0, 0);
    const todayMs = startOfToday.getTime();
    const threeDaysAgo = now - 3 * 86_400_000;
    const weekAgo = now - 7 * 86_400_000;

    return projects.filter((p) => {
      if (statusFilter === "archived") {
        if (!p.is_archived) return false;
      } else {
        if (p.is_archived) return false;
        if (statusFilter === "starred" && !p.is_starred) return false;
        if (statusFilter === "running" && p.running_count === 0) return false;
      }

      const t = p.last_activity_ms;
      if (timeFilter === "today" && t < todayMs) return false;
      if (timeFilter === "3d" && t < threeDaysAgo) return false;
      if (timeFilter === "week" && t < weekAgo) return false;
      if (timeFilter === "older" && t >= weekAgo) return false;

      if (q) {
        const hay = [
          p.project_name,
          p.project_path,
          ...p.sessions.map((s) => s.first_user_message),
        ]
          .join(" ")
          .toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    });
  });

  let isFiltering = $derived(
    searchQuery.trim() !== "" || statusFilter !== "all" || timeFilter !== "all"
  );

  function isExpanded(p: ProjectGroup): boolean {
    if (p.project_path in expandOverrides) return expandOverrides[p.project_path];
    return p.running_count > 0;
  }

  function toggleExpand(p: ProjectGroup) {
    const next = !isExpanded(p);
    expandOverrides = { ...expandOverrides, [p.project_path]: next };
    try {
      localStorage.setItem(`expand:${p.project_path}`, next ? "1" : "0");
    } catch {}
  }

  function timeAgo(ms: number): string {
    const diff = Date.now() - ms;
    if (diff < 60_000) return "just now";
    if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
    if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
    if (diff < 30 * 86_400_000) return `${Math.floor(diff / 86_400_000)}d ago`;
    return new Date(ms).toLocaleDateString();
  }

  function clearFilters() {
    searchQuery = "";
    statusFilter = "all";
    timeFilter = "all";
  }

  async function notifySessionExits(currentRunningIds: Set<string>) {
    if (!notificationsEnabled) return;
    if (prevRunningSessionIds.size === 0) return; // skip first poll
    for (const id of prevRunningSessionIds) {
      if (currentRunningIds.has(id)) continue;
      let projectName = "";
      let label = "";
      for (const p of projects) {
        const s = p.sessions.find((x) => x.id === id);
        if (s) {
          projectName = p.alias || p.project_name;
          label = s.alias || s.first_user_message || id.slice(0, 8);
          break;
        }
      }
      if (!projectName) continue;
      try {
        sendNotification({
          title: `Claude session ended · ${projectName}`,
          body: label.length > 90 ? label.slice(0, 90) + "…" : label,
        });
      } catch {
        // permission revoked or API error — ignore
      }
    }
  }

  // Fingerprint of the project list: skip reactive update when nothing
  // we visually depend on has changed. Cuts ~90% of useless svelte rerenders
  // on the steady-state 5s poll, which was the dominant cause of input
  // stutter while typing in todos / dragging windows.
  let lastProjectsFingerprint = "";

  function fingerprintProjects(list: ProjectGroup[]): string {
    const parts: string[] = [];
    for (const p of list) {
      parts.push(
        `${p.project_path}|${p.is_starred ? 1 : 0}|${p.is_archived ? 1 : 0}|${p.alias}|${p.running_count}|${p.todo_pending_count}`
      );
      for (const s of p.sessions) {
        parts.push(`${s.id}|${s.last_activity_ms}|${s.message_count}|${s.is_running ? 1 : 0}|${s.alias}`);
      }
    }
    return parts.join("\n");
  }

  async function load(showSpinner = true) {
    if (showSpinner) loading = true;
    try {
      const fresh = await invoke<ProjectGroup[]>("list_projects");
      const fp = fingerprintProjects(fresh);
      const projectsChanged = fp !== lastProjectsFingerprint;
      if (projectsChanged) {
        projects = fresh;
        lastProjectsFingerprint = fp;
      }
      const currentIds = new Set<string>();
      for (const p of fresh) for (const s of p.sessions) if (s.is_running) currentIds.add(s.id);
      await notifySessionExits(currentIds);
      prevRunningSessionIds = currentIds;
      refreshGlobalTodoBadge();
      if (scriptsMenuFor) {
        const p = projects.find((x) => x.project_path === scriptsMenuFor);
        if (p) {
          try {
            const list = await invoke<ScriptItem[]>("list_scripts", {
              projectPath: p.project_path,
            });
            scriptsByPath = { ...scriptsByPath, [p.project_path]: list };
          } catch {
            // ignore secondary failure
          }
        }
      }
      error = null;
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  function startPolling() {
    if (pollTimer) return;
    pollTimer = setInterval(() => load(false), POLL_MS);
  }

  function stopPolling() {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
  }

  function handleVisibility() {
    if (document.hidden) {
      stopPolling();
    } else {
      load(false);
      startPolling();
    }
  }

  function handleDocClick(e: MouseEvent) {
    if (menuOpen && menuRef && !menuRef.contains(e.target as Node)) {
      menuOpen = false;
    }
    if (overflowFor) {
      const el = e.target as HTMLElement;
      if (!el.closest(".overflow-anchor")) {
        overflowFor = null;
      }
    }
    if (sendMenuFor) {
      const el = e.target as HTMLElement;
      if (!el.closest(".send-anchor")) {
        sendMenuFor = null;
      }
    }
    if (inboxStatusMenuFor) {
      const el = e.target as HTMLElement;
      if (!el.closest(".inbox-status-anchor")) {
        inboxStatusMenuFor = null;
      }
    }
    if (nvmMenuOpen) {
      const el = e.target as HTMLElement;
      if (!el.closest(".nvm-anchor")) {
        nvmMenuOpen = false;
      }
    }
  }

  function handleEsc(e: KeyboardEvent) {
    if (e.key !== "Escape") return;
    if (menuOpen) menuOpen = false;
    if (overflowFor) overflowFor = null;
    if (sendMenuFor) sendMenuFor = null;
    if (inboxStatusMenuFor) inboxStatusMenuFor = null;
    if (editingProjectFor || editingSessionFor) cancelEdit();
  }

  // ─── Project-level actions ─────────────────────────────────────────────

  function startEditProject(p: ProjectGroup, e?: MouseEvent) {
    e?.stopPropagation();
    editingSessionFor = null;
    editingProjectFor = p.project_path;
    editingValue = p.alias || "";
  }

  function startEditSession(s: SessionMeta, e?: MouseEvent) {
    e?.stopPropagation();
    editingProjectFor = null;
    editingSessionFor = s.id;
    editingValue = s.alias || "";
  }

  function cancelEdit() {
    editingProjectFor = null;
    editingSessionFor = null;
    editingValue = "";
  }

  async function saveProjectAlias() {
    if (!editingProjectFor) return;
    const path = editingProjectFor;
    const value = editingValue.trim();
    editingProjectFor = null;
    editingValue = "";
    try {
      await invoke("set_project_alias", { projectPath: path, alias: value });
      projects = projects.map((x) =>
        x.project_path === path ? { ...x, alias: value } : x
      );
      load(false);
    } catch (e) {
      error = String(e);
    }
  }

  async function saveSessionAlias() {
    if (!editingSessionFor) return;
    const id = editingSessionFor;
    const value = editingValue.trim();
    editingSessionFor = null;
    editingValue = "";
    try {
      await invoke("set_session_alias", { sessionId: id, alias: value });
      projects = projects.map((p) => ({
        ...p,
        sessions: p.sessions.map((s) =>
          s.id === id ? { ...s, alias: value } : s
        ),
      }));
      load(false);
    } catch (e) {
      error = String(e);
    }
  }

  function handleAliasKey(e: KeyboardEvent, kind: "project" | "session") {
    if (e.key === "Enter") {
      e.preventDefault();
      if (kind === "project") saveProjectAlias();
      else saveSessionAlias();
    } else if (e.key === "Escape") {
      e.preventDefault();
      cancelEdit();
    }
  }

  function toggleOverflow(p: ProjectGroup, e: MouseEvent) {
    e.stopPropagation();
    overflowFor = overflowFor === p.project_path ? null : p.project_path;
  }

  async function toggleArchive(p: ProjectGroup) {
    overflowFor = null;
    try {
      await invoke<boolean>("toggle_archive", { projectPath: p.project_path });
      load(false);
    } catch (e) {
      error = String(e);
    }
  }

  async function toggleStar(p: ProjectGroup) {
    try {
      const nowStarred = await invoke<boolean>("toggle_star", {
        projectPath: p.project_path,
      });
      projects = projects.map((x) =>
        x.project_path === p.project_path ? { ...x, is_starred: nowStarred } : x
      );
      load(false);
    } catch (e) {
      error = String(e);
    }
  }

  async function newSessionInProject(p: ProjectGroup) {
    try {
      await invoke("new_session_in_dir", { path: p.project_path });
      setTimeout(() => load(false), 1500);
    } catch (e) {
      error = String(e);
    }
  }

  async function openProjectFolder(p: ProjectGroup) {
    try {
      await invoke("open_folder", { path: p.project_path });
    } catch (e) {
      error = String(e);
    }
  }

  // ─── Session-level actions ─────────────────────────────────────────────

  async function openSession(s: SessionMeta, p: ProjectGroup) {
    try {
      await invoke("open_session", {
        sessionId: s.id,
        projectPath: p.project_path,
        projectName: p.project_name,
      });
      projects = projects.map((x) =>
        x.project_path === p.project_path
          ? {
              ...x,
              running_count: x.running_count + 1,
              sessions: x.sessions.map((y) =>
                y.id === s.id ? { ...y, is_running: true } : y
              ),
            }
          : x
      );
      setTimeout(() => load(false), 1500);
    } catch (e) {
      error = String(e);
    }
  }

  async function focusSession(s: SessionMeta) {
    try {
      await invoke("focus_session", { sessionId: s.id });
    } catch (e) {
      error = String(e);
    }
  }

  async function closeSession(s: SessionMeta) {
    try {
      await invoke("close_session", { sessionId: s.id });
      projects = projects.map((x) => ({
        ...x,
        running_count: x.sessions.some((y) => y.id === s.id && y.is_running)
          ? Math.max(0, x.running_count - 1)
          : x.running_count,
        sessions: x.sessions.map((y) =>
          y.id === s.id ? { ...y, is_running: false } : y
        ),
      }));
      setTimeout(() => load(false), 1500);
    } catch (e) {
      error = String(e);
    }
  }

  async function openSessionPreview(s: SessionMeta, p: ProjectGroup) {
    const title = s.alias || s.first_user_message || `Session ${s.id.slice(0, 8)}`;
    sessionPreview = { messages: [], title, sessionId: s.id, loading: true };
    try {
      const messages = await invoke<MessagePreview[]>("get_session_preview", {
        encodedDir: p.encoded_dir,
        sessionId: s.id,
        limit: 5,
      });
      sessionPreview = { messages, title, sessionId: s.id, loading: false };
    } catch (e) {
      sessionPreview = { messages: [], title, sessionId: s.id, loading: false };
      error = String(e);
    }
  }

  function closeSessionPreview() {
    sessionPreview = null;
  }

  $effect(() => {
    if (!sessionPreview) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        closeSessionPreview();
      }
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  });

  async function setInboxEntryStatus(
    p: ProjectGroup,
    ie: InboxEntry,
    status: "pending" | "sent" | "applied" | "ignored"
  ) {
    try {
      await invoke("set_inbox_status", {
        projectPath: p.project_path,
        dirName: ie.dir_name,
        status,
      });
      inboxByPath = {
        ...inboxByPath,
        [p.project_path]: (inboxByPath[p.project_path] ?? []).map((x) =>
          x.dir_name === ie.dir_name ? { ...x, status } : x
        ),
      };
    } catch (e) {
      error = String(e);
    }
  }

  async function deleteSessionFile(s: SessionMeta, p: ProjectGroup) {
    if (s.is_running) {
      error = "Stop the running session first before deleting its file.";
      return;
    }
    if (
      !confirm(
        `Permanently delete this session file?\n\n` +
          `"${s.first_user_message || s.id}"\n\n` +
          `(The .jsonl file will be removed from disk.)`
      )
    )
      return;
    try {
      await invoke("delete_session_file", {
        encodedDir: p.encoded_dir,
        sessionId: s.id,
      });
      load(false);
    } catch (e) {
      error = String(e);
    }
  }

  function activateSession(s: SessionMeta) {
    if (s.is_running) {
      focusSession(s);
    }
    // stopped: no-op (user must click ▶ explicitly)
  }

  function activateProject(p: ProjectGroup, e: MouseEvent) {
    // If multi-session: toggle expand. If single-session: focus if running, else no-op.
    if (p.sessions.length > 1) {
      toggleExpand(p);
      return;
    }
    const s = p.sessions[0];
    if (s && s.is_running) {
      focusSession(s);
    }
  }

  // ─── + New top-level menu ──────────────────────────────────────────────

  async function newClaudeDefault() {
    menuOpen = false;
    try {
      await invoke("new_temp_session");
      setTimeout(() => load(false), 1500);
    } catch (e) {
      error = String(e);
    }
  }

  async function newClaudeInDir() {
    menuOpen = false;
    try {
      const path = await openDialog({
        directory: true,
        title: "Choose a working directory",
      });
      if (!path) return;
      await invoke("new_session_in_dir", { path });
      setTimeout(() => load(false), 1500);
    } catch (e) {
      error = String(e);
    }
  }

  async function newTerminalOnly() {
    menuOpen = false;
    try {
      await invoke("new_terminal", { path: null });
    } catch (e) {
      error = String(e);
    }
  }

  // ─── Scripts panel (project-level) ─────────────────────────────────────

  async function refreshScripts(p: ProjectGroup) {
    try {
      const list = await invoke<ScriptItem[]>("list_scripts", {
        projectPath: p.project_path,
      });
      scriptsByPath = { ...scriptsByPath, [p.project_path]: list };
    } catch (e) {
      error = String(e);
    }
  }

  async function toggleScriptsMenu(p: ProjectGroup) {
    if (scriptsMenuFor === p.project_path) {
      scriptsMenuFor = null;
      addingFor = null;
      return;
    }
    scriptsMenuFor = p.project_path;
    inboxOpenFor = null;
    todosOpenFor = null;
    addingFor = null;
    await refreshScripts(p);
  }

  // ─── Todos ────────────────────────────────────────────────────────

  async function refreshTodos(p: ProjectGroup) {
    try {
      const list = await invoke<Todo[]>("list_todos", {
        projectPath: p.project_path,
      });
      todosByPath = { ...todosByPath, [p.project_path]: list };
    } catch (e) {
      error = String(e);
    }
  }

  async function toggleTodosMenu(p: ProjectGroup) {
    if (todosOpenFor === p.project_path) {
      todosOpenFor = null;
      return;
    }
    todosOpenFor = p.project_path;
    scriptsMenuFor = null;
    inboxOpenFor = null;
    await refreshTodos(p);
  }

  async function toggleTodoCompleted(p: ProjectGroup, t: Todo) {
    try {
      await invoke<boolean>("toggle_todo", {
        projectPath: p.project_path,
        id: t.id,
      });
      todosByPath = {
        ...todosByPath,
        [p.project_path]: (todosByPath[p.project_path] ?? []).map((x) =>
          x.id === t.id ? { ...x, completed: !x.completed } : x
        ),
      };
      load(false); // refresh badge
    } catch (e) {
      error = String(e);
    }
  }

  async function deleteTodo(p: ProjectGroup, t: Todo) {
    if (!confirm(`Delete "${t.title}"?`)) return;
    try {
      await invoke("delete_todo", { projectPath: p.project_path, id: t.id });
      await refreshTodos(p);
      load(false);
    } catch (e) {
      error = String(e);
    }
  }

  function handleTodoDragStart(e: DragEvent, idx: number) {
    dragSourceIdx = idx;
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = "move";
      // Required by some browsers
      e.dataTransfer.setData("text/plain", String(idx));
    }
  }

  function handleTodoDragOver(e: DragEvent, idx: number) {
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    if (dragOverIdx !== idx) dragOverIdx = idx;
  }

  function handleTodoDragLeave() {
    dragOverIdx = null;
  }

  async function handleTodoDrop(p: ProjectGroup, e: DragEvent, idx: number) {
    e.preventDefault();
    if (dragSourceIdx === null || dragSourceIdx === idx) {
      dragSourceIdx = null;
      dragOverIdx = null;
      return;
    }
    const list = todosByPath[p.project_path] ?? [];
    const moved = [...list];
    const [item] = moved.splice(dragSourceIdx, 1);
    moved.splice(idx, 0, item);
    todosByPath = { ...todosByPath, [p.project_path]: moved };
    dragSourceIdx = null;
    dragOverIdx = null;
    try {
      await invoke("reorder_todos", {
        projectPath: p.project_path,
        orderedIds: moved.map((t) => t.id),
      });
    } catch (e) {
      error = String(e);
    }
  }

  function handleTodoDragEnd() {
    dragSourceIdx = null;
    dragOverIdx = null;
  }

  function openTodoModalNew(p: ProjectGroup) {
    todoModal = { projectPath: p.project_path, id: null };
    modalTodoTitle = "";
    modalTodoDetails = "";
    modalTodoActionKind = "";
    modalTodoActionTarget = "";
  }

  function openTodoModalEdit(p: ProjectGroup, t: Todo) {
    todoModal = { projectPath: p.project_path, id: t.id };
    modalTodoTitle = t.title;
    modalTodoDetails = t.details;
    modalTodoActionKind = (t.action_kind as "" | "url" | "folder") ?? "";
    modalTodoActionTarget = t.action_target ?? "";
  }

  function closeTodoModal() {
    todoModal = null;
    modalTodoTitle = "";
    modalTodoDetails = "";
    modalTodoActionKind = "";
    modalTodoActionTarget = "";
  }

  // Auto-detect action kind from the target string so the user usually
  // doesn't have to touch the dropdown.
  function autoDetectAction(s: string): "" | "url" | "folder" {
    const v = s.trim();
    if (!v) return "";
    if (/^https?:\/\//i.test(v)) return "url";
    if (/^[a-zA-Z]:[\\/]/.test(v)) return "folder";    // C:\... or D:/...
    if (v.startsWith("\\\\")) return "folder";          // UNC \\server\share
    if (v.startsWith("/")) return "folder";             // POSIX-ish absolute
    if (/^[\w.-]+\.[a-z]{2,}(\/|$)/i.test(v)) return "url"; // bare hostname like google.com or github.com/x
    return "";
  }

  $effect(() => {
    if (modalTodoActionKind === "" && modalTodoActionTarget) {
      const guess = autoDetectAction(modalTodoActionTarget);
      if (guess) modalTodoActionKind = guess;
    }
  });

  async function runTodoAction(t: Todo) {
    if (!t.action_kind || !t.action_target) return;
    try {
      await invoke("open_todo_action", {
        kind: t.action_kind,
        target: t.action_target,
      });
    } catch (e) {
      error = `Open failed: ${e}`;
    }
  }

  async function saveTodoModal() {
    if (!todoModal) return;
    const title = modalTodoTitle.trim();
    if (!title) return;
    const target = todoModal;
    const action_kind = modalTodoActionTarget.trim() ? modalTodoActionKind : "";
    const action_target = modalTodoActionTarget.trim();
    try {
      if (target.id) {
        await invoke("update_todo", {
          projectPath: target.projectPath,
          id: target.id,
          title,
          details: modalTodoDetails,
          actionKind: action_kind,
          actionTarget: action_target,
        });
      } else {
        await invoke("add_todo", {
          projectPath: target.projectPath,
          title,
          details: modalTodoDetails,
          actionKind: action_kind,
          actionTarget: action_target,
        });
      }
      const p = projects.find((x) => x.project_path === target.projectPath);
      if (p) await refreshTodos(p);
      closeTodoModal();
      load(false);
    } catch (e) {
      error = String(e);
    }
  }

  $effect(() => {
    if (!todoModal) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        closeTodoModal();
      }
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  });

  async function runScript(p: ProjectGroup, sc: ScriptItem) {
    try {
      await invoke("run_script", {
        projectPath: p.project_path,
        fileName: sc.file_name,
      });
      setTimeout(() => refreshScripts(p), 500);
    } catch (e) {
      error = String(e);
    }
  }

  async function closeScript(p: ProjectGroup, sc: ScriptItem) {
    try {
      await invoke("close_script", {
        projectPath: p.project_path,
        fileName: sc.file_name,
      });
      scriptsByPath = {
        ...scriptsByPath,
        [p.project_path]: (scriptsByPath[p.project_path] ?? []).map((x) =>
          x.file_name === sc.file_name ? { ...x, is_running: false } : x
        ),
      };
      setTimeout(() => refreshScripts(p), 500);
    } catch (e) {
      error = String(e);
    }
  }

  function startAdd(p: ProjectGroup) {
    addingFor = p.project_path;
    addName = "";
    addTitle = "";
    addContent = "";
  }

  function cancelAdd() {
    addingFor = null;
  }

  async function confirmAdd(p: ProjectGroup) {
    if (!addName.trim() || !addContent.trim()) return;
    try {
      await invoke("add_script", {
        projectPath: p.project_path,
        name: addName,
        title: addTitle,
        content: addContent,
      });
      addingFor = null;
      await refreshScripts(p);
    } catch (e) {
      error = String(e);
    }
  }

  async function deleteScript(p: ProjectGroup, sc: ScriptItem) {
    if (!confirm(`Delete "${sc.file_name}"? This removes the file from disk.`))
      return;
    try {
      await invoke("delete_script", {
        projectPath: p.project_path,
        fileName: sc.file_name,
      });
      await refreshScripts(p);
    } catch (e) {
      error = String(e);
    }
  }

  async function editScript(p: ProjectGroup, sc: ScriptItem) {
    try {
      await invoke("open_script_in_editor", {
        projectPath: p.project_path,
        fileName: sc.file_name,
      });
    } catch (e) {
      error = String(e);
    }
  }

  async function openCcpscriptDir(p: ProjectGroup) {
    try {
      await invoke("open_ccpscript_dir", { projectPath: p.project_path });
    } catch (e) {
      error = String(e);
    }
  }

  async function generateScripts(p: ProjectGroup) {
    try {
      await invoke("generate_scripts", { projectPath: p.project_path });
    } catch (e) {
      error = String(e);
    }
  }

  function startAsk(p: ProjectGroup) {
    askingFor = p.project_path;
    addingFor = null;
    askInstruction = "";
  }

  function cancelAsk() {
    askingFor = null;
    askInstruction = "";
  }

  async function confirmAsk(p: ProjectGroup) {
    const text = askInstruction.trim();
    if (!text) return;
    try {
      await invoke("ask_claude_for_scripts", {
        projectPath: p.project_path,
        instruction: text,
      });
      askingFor = null;
      askInstruction = "";
    } catch (e) {
      error = String(e);
    }
  }

  // ─── Inbox ────────────────────────────────────────────────────────

  async function refreshInbox(p: ProjectGroup) {
    try {
      const list = await invoke<InboxEntry[]>("list_inbox", {
        projectPath: p.project_path,
      });
      inboxByPath = { ...inboxByPath, [p.project_path]: list };
    } catch (e) {
      error = String(e);
    }
  }

  async function loadInboxThumbnails(p: ProjectGroup, ie: InboxEntry) {
    for (const att of ie.attachments) {
      if (!isImageName(att)) continue;
      const key = thumbKey(p.project_path, ie.dir_name, att);
      if (thumbnails[key]) continue;
      try {
        const dataUrl = await invoke<string>("read_inbox_attachment_data_url", {
          projectPath: p.project_path,
          dirName: ie.dir_name,
          fileName: att,
        });
        thumbnails = { ...thumbnails, [key]: dataUrl };
      } catch {
        // skip silently
      }
    }
  }

  async function openInboxAttachment(p: ProjectGroup, ie: InboxEntry, fileName: string) {
    try {
      await invoke("open_inbox_attachment", {
        projectPath: p.project_path,
        dirName: ie.dir_name,
        fileName,
      });
    } catch (e) {
      error = String(e);
    }
  }

  async function toggleInboxMenu(p: ProjectGroup) {
    if (inboxOpenFor === p.project_path) {
      inboxOpenFor = null;
      addingInboxFor = null;
      return;
    }
    inboxOpenFor = p.project_path;
    scriptsMenuFor = null;
    todosOpenFor = null;
    addingInboxFor = null;
    await refreshInbox(p);
  }

  function startAddInbox(p: ProjectGroup) {
    addingInboxFor = p.project_path;
    editingInboxFor = null;
    newInboxTitle = "";
    newInboxDescription = "";
    newInboxAttachments = [];
  }

  function startEditInbox(p: ProjectGroup, ie: InboxEntry) {
    addingInboxFor = p.project_path;
    editingInboxFor = { projectPath: p.project_path, dirName: ie.dir_name };
    newInboxTitle = ie.title;
    newInboxDescription = ie.description;
    newInboxAttachments = []; // existing attachments stay on disk
  }

  function cancelAddInbox() {
    addingInboxFor = null;
    editingInboxFor = null;
    newInboxTitle = "";
    newInboxDescription = "";
    newInboxAttachments = [];
    screenshotWaiting = false;
    if (descEditorPath) {
      invoke("delete_temp_file", { path: descEditorPath }).catch(() => {});
      descEditorPath = null;
    }
    if (editorUnlisten) {
      editorUnlisten();
      editorUnlisten = null;
    }
  }

  async function confirmAddInbox(p: ProjectGroup) {
    const title = newInboxTitle.trim();
    if (!title) return;
    try {
      if (editingInboxFor) {
        await invoke("update_inbox_entry", {
          projectPath: p.project_path,
          dirName: editingInboxFor.dirName,
          title,
          description: newInboxDescription,
        });
      } else {
        await invoke("create_inbox_entry", {
          projectPath: p.project_path,
          title,
          description: newInboxDescription,
          attachments: newInboxAttachments,
        });
      }
      cancelAddInbox();
      await refreshInbox(p);
    } catch (e) {
      error = String(e);
    }
  }

  let editorUnlisten: UnlistenFn | null = null;

  let globalTodoPending = $state(0);

  type GlobalTabSummary = { id: string; name: string; todos: { completed: boolean }[] };

  async function refreshGlobalTodoBadge() {
    try {
      const tabs = await invoke<GlobalTabSummary[]>("list_global_tabs");
      let n = 0;
      for (const t of tabs) for (const x of t.todos) if (!x.completed) n++;
      globalTodoPending = n;
    } catch {
      // ignore
    }
  }

  async function openGlobalTodos() {
    try {
      await invoke("open_global_todos_window");
    } catch (e) {
      error = String(e);
    }
  }

  async function openDescriptionInEditor() {
    try {
      const path = await invoke<string>("open_text_in_editor", {
        content: newInboxDescription,
        suggestedName: "feedback-notes.md",
      });
      descEditorPath = path;

      if (editorUnlisten) {
        editorUnlisten();
        editorUnlisten = null;
      }
      editorUnlisten = await listen<string>("editor-saved", async (e) => {
        if (e.payload === path) {
          await reloadDescriptionFromEditor();
        }
      });
    } catch (e) {
      error = String(e);
    }
  }

  async function reloadDescriptionFromEditor() {
    if (!descEditorPath) return;
    try {
      const content = await invoke<string>("read_temp_file", { path: descEditorPath });
      newInboxDescription = content;
    } catch (e) {
      error = String(e);
    }
  }

  let clipboardCheckLock = false;

  async function tryReadClipboardImage(): Promise<boolean> {
    if (clipboardCheckLock) return false;
    clipboardCheckLock = true;
    try {
      const items = await navigator.clipboard.read();
      for (const item of items) {
        for (const type of item.types) {
          if (type.startsWith("image/")) {
            const blob = await item.getType(type);
            const buf = await blob.arrayBuffer();
            const content_base64 = arrayBufferToBase64(buf);
            const idx =
              newInboxAttachments.filter((a) => /^screenshot-\d+\.png$/.test(a.name)).length + 1;
            newInboxAttachments = [
              ...newInboxAttachments,
              { name: `screenshot-${idx}.png`, content_base64 },
            ];
            screenshotWaiting = false;
            return true;
          }
        }
      }
    } catch {
      // permission denied or empty
    } finally {
      clipboardCheckLock = false;
    }
    return false;
  }

  async function deleteInbox(p: ProjectGroup, ie: InboxEntry) {
    if (!confirm(`Delete feedback "${ie.title}"?\n\nThe folder ${ie.dir_name}/ will be removed.`)) return;
    try {
      await invoke("delete_inbox_entry", {
        projectPath: p.project_path,
        dirName: ie.dir_name,
      });
      await refreshInbox(p);
    } catch (e) {
      error = String(e);
    }
  }

  async function openInboxFolder(p: ProjectGroup, ie: InboxEntry) {
    try {
      await invoke("open_inbox_entry_folder", {
        projectPath: p.project_path,
        dirName: ie.dir_name,
      });
    } catch (e) {
      error = String(e);
    }
  }

  async function sendInboxToClaude(p: ProjectGroup, ie: InboxEntry, mode: "new" | "continue" = "new") {
    sendMenuFor = null;
    try {
      await invoke("send_inbox_to_claude", {
        projectPath: p.project_path,
        dirName: ie.dir_name,
        mode,
      });
    } catch (e) {
      error = String(e);
    }
  }

  function toggleSendMenu(p: ProjectGroup, ie: InboxEntry, e: MouseEvent) {
    e.stopPropagation();
    const key = `${p.project_path}|${ie.dir_name}`;
    sendMenuFor = sendMenuFor === key ? null : key;
  }

  let inboxStatusMenuFor = $state<string | null>(null);

  function toggleInboxStatusMenu(p: ProjectGroup, ie: InboxEntry, e: MouseEvent) {
    e.stopPropagation();
    const key = `${p.project_path}|${ie.dir_name}`;
    inboxStatusMenuFor = inboxStatusMenuFor === key ? null : key;
  }

  async function chooseInboxStatus(p: ProjectGroup, ie: InboxEntry, status: InboxEntry["status"]) {
    inboxStatusMenuFor = null;
    await setInboxEntryStatus(p, ie, status);
  }

  async function takeScreenshot() {
    try {
      await invoke("launch_snipping_tool");
      screenshotWaiting = true;
    } catch (e) {
      error = String(e);
    }
  }

  function arrayBufferToBase64(buf: ArrayBuffer): string {
    const bytes = new Uint8Array(buf);
    const chunkSize = 0x8000;
    const parts: string[] = [];
    for (let i = 0; i < bytes.length; i += chunkSize) {
      parts.push(String.fromCharCode.apply(null, Array.from(bytes.subarray(i, i + chunkSize))));
    }
    return btoa(parts.join(""));
  }

  async function addAttachmentFile(file: File) {
    const buf = await file.arrayBuffer();
    const content_base64 = arrayBufferToBase64(buf);
    let name = file.name;
    if (!name || name === "image.png" || name.startsWith("image (")) {
      const existing = newInboxAttachments.filter((a) => /^screenshot-\d+\.png$/.test(a.name));
      name = `screenshot-${existing.length + 1}.png`;
    }
    newInboxAttachments = [...newInboxAttachments, { name, content_base64 }];
  }

  async function handleInboxPaste(e: ClipboardEvent) {
    if (!e.clipboardData) return;
    let handled = false;
    for (const item of Array.from(e.clipboardData.items)) {
      if (item.kind === "file") {
        const file = item.getAsFile();
        if (file) {
          await addAttachmentFile(file);
          handled = true;
        }
      }
    }
    if (handled) e.preventDefault();
  }

  function handleInboxDragOver(e: DragEvent) {
    e.preventDefault();
  }

  async function handleInboxDrop(e: DragEvent) {
    e.preventDefault();
    if (!e.dataTransfer?.files) return;
    for (const file of Array.from(e.dataTransfer.files)) {
      await addAttachmentFile(file);
    }
  }

  function pickAttachmentFile() {
    const input = document.createElement("input");
    input.type = "file";
    input.multiple = true;
    input.onchange = async (e) => {
      const target = e.target as HTMLInputElement;
      if (target.files) {
        for (const file of Array.from(target.files)) {
          await addAttachmentFile(file);
        }
      }
    };
    input.click();
  }

  function removeAttachment(idx: number) {
    newInboxAttachments = newInboxAttachments.filter((_, i) => i !== idx);
  }

  // Modal-scoped Alt+Q for screenshot.
  $effect(() => {
    if (!addingInboxFor) return;
    const handler = (e: KeyboardEvent) => {
      if (e.altKey && (e.key === "q" || e.key === "Q")) {
        e.preventDefault();
        takeScreenshot();
      }
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  });

  // When the launcher window regains focus while inbox modal is open,
  // try to auto-grab a screenshot from the clipboard.
  // The focus event can fire multiple times as Snipping Tool dismisses
  // (overlay close, taskbar reactivation, etc.), so we flip the
  // screenshotWaiting flag immediately to debounce the schedule.
  $effect(() => {
    if (!addingInboxFor) return;
    const onFocus = () => {
      if (!screenshotWaiting) return;
      screenshotWaiting = false;
      setTimeout(() => {
        tryReadClipboardImage();
      }, 250);
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  });

  // ─── Lifecycle ─────────────────────────────────────────────────────────

  onMount(async () => {
    // Restore expand overrides from localStorage.
    try {
      const overrides: Record<string, boolean> = {};
      for (let i = 0; i < localStorage.length; i++) {
        const k = localStorage.key(i);
        if (k && k.startsWith("expand:")) {
          const path = k.slice("expand:".length);
          overrides[path] = localStorage.getItem(k) === "1";
        }
      }
      expandOverrides = overrides;
    } catch {}

    // Probe notification permission once. Best effort.
    try {
      let granted = await isPermissionGranted();
      if (!granted) {
        const result = await requestPermission();
        granted = result === "granted";
      }
      notificationsEnabled = granted;
    } catch {
      notificationsEnabled = false;
    }

    load();
    if (!document.hidden) startPolling();
    document.addEventListener("visibilitychange", handleVisibility);
    document.addEventListener("mousedown", handleDocClick);
    document.addEventListener("keydown", handleEsc);

    // System info: node version is cached but auto-refreshed every 60s so
    // external `nvm use` from a terminal eventually shows up in the status bar.
    invoke<string | null>("node_version").then((v) => (nodeVer = v)).catch(() => {});
    nodeVersionIntervalId = setInterval(async () => {
      if (document.hidden || probesPaused) return;
      try {
        nodeVer = await invoke<string | null>("node_version_refresh");
      } catch {}
    }, 60_000);
    const tickUptime = setInterval(async () => {
      try {
        uptimeMs = await invoke<number>("app_uptime_ms");
      } catch {}
    }, 5000);
    uptimeIntervalId = tickUptime;
    const tickLat = () => {
      if (document.hidden || probesPaused) return;
      refreshLatency();
    };
    const tickGeo = () => {
      if (document.hidden || probesPaused) return;
      refreshGeo();
    };
    if (!probesPaused) {
      refreshLatency();
      refreshGeo();
      refreshUsage();
    }
    latIntervalId = setInterval(tickLat, 30_000);          // 30s
    geoIntervalId = setInterval(tickGeo, 30 * 60_000);     // 30min
    usageIntervalId = setInterval(() => {
      if (document.hidden || probesPaused) return;
      refreshUsage();
    }, 30_000);                                            // 30s
  });

  let uptimeIntervalId: ReturnType<typeof setInterval> | null = null;
  let latIntervalId: ReturnType<typeof setInterval> | null = null;
  let geoIntervalId: ReturnType<typeof setInterval> | null = null;
  let usageIntervalId: ReturnType<typeof setInterval> | null = null;
  let nodeVersionIntervalId: ReturnType<typeof setInterval> | null = null;

  function fmtUptime(ms: number): string {
    const sec = Math.floor(ms / 1000);
    const h = Math.floor(sec / 3600);
    const m = Math.floor((sec % 3600) / 60);
    const s = sec % 60;
    if (h > 0) return `${h}h ${m}m`;
    if (m > 0) return `${m}m ${s}s`;
    return `${s}s`;
  }

  onDestroy(() => {
    stopPolling();
    if (uptimeIntervalId) clearInterval(uptimeIntervalId);
    if (latIntervalId) clearInterval(latIntervalId);
    if (geoIntervalId) clearInterval(geoIntervalId);
    if (usageIntervalId) clearInterval(usageIntervalId);
    if (nodeVersionIntervalId) clearInterval(nodeVersionIntervalId);
    document.removeEventListener("visibilitychange", handleVisibility);
    document.removeEventListener("mousedown", handleDocClick);
    document.removeEventListener("keydown", handleEsc);
  });
</script>

<svelte:head>
  <title>CCPilot</title>
</svelte:head>

{#if isGlobalTodosWindow}
  <GlobalTodosView />
{:else}

{#if preview}
  <div class="preview-overlay" onclick={closePreview} role="presentation">
    <div class="preview-content" onclick={(e) => e.stopPropagation()} role="presentation">
      <img class="preview-image" src={preview.src} alt={preview.name} />
      <div class="preview-footer">
        <span class="preview-name">{preview.name}</span>
        <div class="preview-actions">
          {#if preview.openExternal}
            <button class="small" onclick={() => preview && preview.openExternal && preview.openExternal()}>Open externally</button>
          {/if}
          <button class="small" onclick={closePreview}>Close (Esc)</button>
        </div>
      </div>
    </div>
  </div>
{/if}

{#if todoModal}
  <div class="preview-overlay" onclick={closeTodoModal} role="presentation">
    <div class="todo-modal" onclick={(e) => e.stopPropagation()} role="presentation">
      <div class="todo-modal-header">
        <span class="todo-modal-title-label">{todoModal.id ? "Edit todo" : "New todo"}</span>
        <button class="icon-tiny" onclick={closeTodoModal} aria-label="Close"><Icon name="x" size={12} /></button>
      </div>
      <input
        class="todo-modal-title"
        bind:value={modalTodoTitle}
        placeholder="Title"
      />
      <textarea
        class="todo-modal-details"
        bind:value={modalTodoDetails}
        placeholder="Details (optional)"
      ></textarea>
      <div class="todo-action-row">
        <label class="todo-action-label">Quick action (optional)</label>
        <div class="todo-action-input-row">
          <select class="todo-action-kind" bind:value={modalTodoActionKind}>
            <option value="">— none —</option>
            <option value="url">🔗 Open URL</option>
            <option value="folder">📁 Open path</option>
          </select>
          <input
            class="todo-action-target"
            bind:value={modalTodoActionTarget}
            placeholder={modalTodoActionKind === "url" ? "https://… or just google.com" : modalTodoActionKind === "folder" ? "C:\\path\\to\\folder or file" : "Paste a URL or a path — type auto-detected"}
          />
        </div>
        {#if modalTodoActionTarget.trim() && modalTodoActionKind}
          <div class="todo-action-hint">Clicking the 🔗/📁 icon on this todo will {modalTodoActionKind === "url" ? "open the URL in your default browser" : "open the path in Explorer"}.</div>
        {/if}
      </div>
      <div class="todo-modal-actions">
        <button class="primary small" onclick={saveTodoModal} disabled={!modalTodoTitle.trim()}>{todoModal.id ? "Save" : "Add"}</button>
        <button class="small" onclick={closeTodoModal}>Cancel (Esc)</button>
      </div>
    </div>
  </div>
{/if}

{#if sessionPreview}
  <div class="preview-overlay" onclick={closeSessionPreview} role="presentation">
    <div class="todo-modal session-preview-modal" onclick={(e) => e.stopPropagation()} role="presentation">
      <div class="todo-modal-header">
        <span class="todo-modal-title-label">Session preview · last user messages</span>
        <button class="icon-tiny" onclick={closeSessionPreview} aria-label="Close"><Icon name="x" size={12} /></button>
      </div>
      <div class="session-preview-title">{sessionPreview.title}</div>
      {#if sessionPreview.loading}
        <div class="empty">Loading…</div>
      {:else if sessionPreview.messages.length === 0}
        <div class="empty">(No user messages yet in this session)</div>
      {:else}
        <ul class="session-preview-list">
          {#each sessionPreview.messages as m, i (i)}
            <li class="session-preview-msg">
              {#if m.timestamp}
                <div class="session-preview-time">{m.timestamp}</div>
              {/if}
              <div class="session-preview-text">{m.text}</div>
            </li>
          {/each}
        </ul>
      {/if}
      <div class="todo-modal-actions">
        <button class="small" onclick={closeSessionPreview}>Close (Esc)</button>
      </div>
    </div>
  </div>
{/if}

<main>
  <header>
    <div class="title">
      <span class="logo">⚡</span>
      <h1>CCPilot</h1>
      <span class="subtitle">
        {#if isFiltering}
          {filteredProjects.length} / {projects.length} projects
        {:else}
          {projects.length} {projects.length === 1 ? "project" : "projects"}
        {/if}
      </span>
    </div>
    <div class="actions">
      <div class="dropdown" bind:this={menuRef}>
        <button class="primary" onclick={() => (menuOpen = !menuOpen)}>
          <Icon name="plus" size={12} /> New <Icon name="chevron-down" size={11} />
        </button>
        {#if menuOpen}
          <div class="menu" role="menu">
            <button class="menu-item" onclick={newClaudeDefault}>
              <span class="menu-icon"><Icon name="play" size={14} /></span>
              <span class="menu-text">
                <span class="menu-title">New Claude session</span>
                <span class="menu-hint">Fresh temp directory</span>
              </span>
            </button>
            <button class="menu-item" onclick={newClaudeInDir}>
              <span class="menu-icon"><Icon name="folder" size={14} /></span>
              <span class="menu-text">
                <span class="menu-title">New Claude session in folder…</span>
                <span class="menu-hint">Pick an existing directory</span>
              </span>
            </button>
            <button class="menu-item" onclick={newTerminalOnly}>
              <span class="menu-icon"><Icon name="terminal" size={14} /></span>
              <span class="menu-text">
                <span class="menu-title">PowerShell only</span>
                <span class="menu-hint">No Claude, just a shell</span>
              </span>
            </button>
          </div>
        {/if}
      </div>
      <button
        class="icon icon-svg has-badge"
        onclick={openGlobalTodos}
        title={globalTodoPending > 0 ? `Global todos · ${globalTodoPending} pending` : "Global todos"}
        aria-label="Global todos"
      >
        <Icon name="list-todo" />
        {#if globalTodoPending > 0}
          <span class="badge">{globalTodoPending}</span>
        {/if}
      </button>
      <button class="icon icon-svg" onclick={() => load()} title="Refresh" aria-label="Refresh"><Icon name="refresh" /></button>
    </div>
  </header>

  {#if error}
    <button type="button" class="error" onclick={() => (error = null)}>{error}</button>
  {/if}

  {#if !loading && projects.length > 0}
    <div class="toolbar">
      <div class="search-row">
        <span class="search-icon">🔍</span>
        <input
          type="text"
          class="search"
          placeholder="Search projects, paths, or messages..."
          bind:value={searchQuery}
        />
        {#if searchQuery}
          <button class="clear" onclick={() => (searchQuery = "")} title="Clear">×</button>
        {/if}
      </div>
      <div class="filter-row">
        <div class="pill-group" role="group" aria-label="Status filter">
          <button class="pill" class:active={statusFilter === "all"} onclick={() => (statusFilter = "all")}>All</button>
          <button class="pill" class:active={statusFilter === "starred"} onclick={() => (statusFilter = "starred")}>
            <Icon name="star" size={11} fill={true} />Starred
          </button>
          <button class="pill" class:active={statusFilter === "running"} onclick={() => (statusFilter = "running")}>
            <span class="status-dot running"></span>Running
          </button>
          <button class="pill" class:active={statusFilter === "archived"} onclick={() => (statusFilter = "archived")}>
            <Icon name="archive" size={11} />Archived
          </button>
        </div>
        <div class="pill-group" role="group" aria-label="Time filter">
          <button class="pill" class:active={timeFilter === "all"} onclick={() => (timeFilter = "all")}>Any</button>
          <button class="pill" class:active={timeFilter === "today"} onclick={() => (timeFilter = "today")}>Today</button>
          <button class="pill" class:active={timeFilter === "3d"} onclick={() => (timeFilter = "3d")}>3d</button>
          <button class="pill" class:active={timeFilter === "week"} onclick={() => (timeFilter = "week")}>Week</button>
          <button class="pill" class:active={timeFilter === "older"} onclick={() => (timeFilter = "older")}>Older</button>
        </div>
      </div>
    </div>
  {/if}

  {#if loading}
    <div class="empty">Loading…</div>
  {:else if projects.length === 0}
    <div class="empty">
      No projects yet.<br />
      Click <b>"+ New"</b> to start one, or run <code>claude</code> in any terminal.
    </div>
  {:else if filteredProjects.length === 0}
    <div class="empty">
      No projects match your filters.<br />
      <button class="link" onclick={clearFilters}>Clear filters</button>
    </div>
  {:else}
    <ul class="projects">
      {#each filteredProjects as p (p.project_path)}
        {@const single = p.sessions.length === 1}
        {@const expanded = !single && isExpanded(p)}
        <li
          class="project"
          class:running={p.running_count > 0}
          class:starred={p.is_starred}
          class:expanded
        >
          <button
            class="corner-star"
            class:starred={p.is_starred}
            onclick={() => toggleStar(p)}
            title={p.is_starred ? "Unstar project" : "Star project"}
            aria-label={p.is_starred ? "Unstar" : "Star"}
          >
            <span class="corner-star-icon"><Icon name="star" size={12} fill={p.is_starred} /></span>
          </button>
          <div class="project-row">
            <button
              class="project-hit"
              onclick={(e) => activateProject(p, e)}
              title={single
                ? p.sessions[0].is_running
                  ? "Focus running window"
                  : ""
                : expanded
                  ? "Collapse"
                  : "Expand"}
            >
              <span class="status" class:running={p.running_count > 0} aria-hidden="true"></span>
              <div class="meta">
                <div class="name-row">
                  {#if editingProjectFor === p.project_path}
                    <input
                      class="alias-input"
                      bind:value={editingValue}
                      placeholder="Project alias (empty to clear)"
                      onkeydown={(e) => handleAliasKey(e, "project")}
                      onblur={saveProjectAlias}
                      onclick={(e) => e.stopPropagation()}
                      autofocus
                    />
                  {:else}
                    <div class="name">{p.alias || p.project_name}</div>
                    <button
                      class="edit-pencil"
                      onclick={(e) => startEditProject(p, e)}
                      title="Rename project"
                      aria-label="Rename"
                    ><Icon name="pencil" size={11} /></button>
                  {/if}
                </div>
                <div class="path">{p.project_path}</div>
                <div class="info" class:info-stacked={single}>
                  {#if single}
                    <div class="msg-line">
                      {#if editingSessionFor === p.sessions[0].id}
                        <input
                          class="alias-input small"
                          bind:value={editingValue}
                          placeholder="Session alias (empty to clear)"
                          onkeydown={(e) => handleAliasKey(e, "session")}
                          onblur={saveSessionAlias}
                          onclick={(e) => e.stopPropagation()}
                          autofocus
                        />
                      {:else}
                        <button
                          class="msg-preview-btn"
                          onclick={(e) => { e.stopPropagation(); openSessionPreview(p.sessions[0], p); }}
                          title="Preview last messages"
                        >
                          {p.sessions[0].alias || p.sessions[0].first_user_message || "(new session)"}
                        </button>
                        <button
                          class="edit-pencil tiny"
                          onclick={(e) => startEditSession(p.sessions[0], e)}
                          title="Rename session"
                          aria-label="Rename session"
                        ><Icon name="pencil" size={10} /></button>
                      {/if}
                    </div>
                    <div class="time-line">{timeAgo(p.last_activity_ms)}</div>
                  {:else}
                    <span>{p.sessions.length} sessions</span>
                    {#if p.running_count > 0}
                      <span class="dot">·</span>
                      <span class="running-tag">{p.running_count} running</span>
                    {/if}
                    <span class="dot">·</span>
                    <span>{timeAgo(p.last_activity_ms)}</span>
                  {/if}
                </div>
              </div>
            </button>
            <div class="row-actions">
              {#if single}
                {#if p.sessions[0].is_running}
                  <button class="icon icon-svg danger" onclick={() => closeSession(p.sessions[0])} title="Close window" aria-label="Close"><Icon name="x" /></button>
                {:else}
                  <button class="primary icon-svg" onclick={() => openSession(p.sessions[0], p)} title="Resume this session" aria-label="Resume"><Icon name="play" size={12} /></button>
                {/if}
              {:else}
                <button
                  class="icon icon-svg expand-btn"
                  onclick={() => toggleExpand(p)}
                  title={expanded ? "Collapse" : "Expand"}
                  aria-label={expanded ? "Collapse" : "Expand"}
                ><Icon name={expanded ? "chevron-up" : "chevron-down"} /></button>
              {/if}
              <button
                class="icon icon-svg"
                class:active-tool={scriptsMenuFor === p.project_path}
                onclick={() => toggleScriptsMenu(p)}
                title="Scripts"
                aria-label="Scripts"
              ><Icon name="terminal" /></button>
              <button
                class="icon icon-svg"
                class:active-tool={inboxOpenFor === p.project_path}
                onclick={() => toggleInboxMenu(p)}
                title="Inbox"
                aria-label="Inbox"
              ><Icon name="inbox" /></button>
              <button
                class="icon icon-svg has-badge"
                class:active-tool={todosOpenFor === p.project_path}
                onclick={() => toggleTodosMenu(p)}
                title={p.todo_pending_count > 0 ? `Todos · ${p.todo_pending_count} pending` : "Todos"}
                aria-label="Todos"
              >
                <Icon name="list-todo" />
                {#if p.todo_pending_count > 0}
                  <span class="badge">{p.todo_pending_count}</span>
                {/if}
              </button>
              <div class="overflow-anchor">
                <button
                  class="icon icon-svg"
                  class:active-tool={overflowFor === p.project_path}
                  onclick={(e) => toggleOverflow(p, e)}
                  title="More actions"
                  aria-label="More actions"
                ><Icon name="more-horizontal" /></button>
                {#if overflowFor === p.project_path}
                  <div class="menu overflow-menu" role="menu">
                    <button class="menu-item" onclick={() => { overflowFor = null; newSessionInProject(p); }}>
                      <span class="menu-icon"><Icon name="plus" size={14} /></span>
                      <span class="menu-text">
                        <span class="menu-title">New session here</span>
                        <span class="menu-hint">Start an additional Claude in this project</span>
                      </span>
                    </button>
                    <button class="menu-item" onclick={() => { overflowFor = null; openProjectFolder(p); }}>
                      <span class="menu-icon"><Icon name="folder" size={14} /></span>
                      <span class="menu-text">
                        <span class="menu-title">Open folder</span>
                        <span class="menu-hint">{p.project_path}</span>
                      </span>
                    </button>
                    <button class="menu-item" onclick={() => toggleArchive(p)}>
                      <span class="menu-icon"><Icon name="archive" size={14} /></span>
                      <span class="menu-text">
                        <span class="menu-title">{p.is_archived ? "Unarchive" : "Archive"}</span>
                        <span class="menu-hint">{p.is_archived ? "Show in normal views again" : "Hide from default views"}</span>
                      </span>
                    </button>
                  </div>
                {/if}
              </div>
            </div>
          </div>

          {#if expanded}
            <ul class="session-list">
              {#each p.sessions as s (s.id)}
                <li class="session-row" class:running={s.is_running}>
                  {#if editingSessionFor === s.id}
                    <input
                      class="alias-input row"
                      bind:value={editingValue}
                      placeholder="Session alias (empty to clear)"
                      onkeydown={(e) => handleAliasKey(e, "session")}
                      onblur={saveSessionAlias}
                      autofocus
                    />
                    <span class="time">{timeAgo(s.last_activity_ms)} <span class="dot">·</span> {s.message_count} msgs</span>
                    <div class="session-actions"></div>
                  {:else}
                    <button
                      class="session-hit"
                      onclick={() => activateSession(s)}
                      title={s.is_running ? "Focus window" : (s.first_user_message || "")}
                    >
                      <span class="status small" class:running={s.is_running}></span>
                      <span class="msg-text">
                        {s.alias || s.first_user_message || "(new session)"}
                      </span>
                    </button>
                    <span class="time">{timeAgo(s.last_activity_ms)} <span class="dot">·</span> {s.message_count} msgs</span>
                    <div class="session-actions">
                      <button class="icon-tiny" onclick={() => openSessionPreview(s, p)} title="Preview last messages" aria-label="Preview"><Icon name="eye" size={12} /></button>
                      <button class="icon-tiny" onclick={(e) => startEditSession(s, e)} title="Rename" aria-label="Rename"><Icon name="pencil" size={11} /></button>
                      {#if s.is_running}
                        <button class="icon-tiny danger" onclick={() => closeSession(s)} title="Close window" aria-label="Close"><Icon name="x" size={12} /></button>
                      {:else}
                        <button class="icon-tiny" onclick={() => openSession(s, p)} title="Resume" aria-label="Resume"><Icon name="play" size={11} /></button>
                      {/if}
                      <button class="icon-tiny danger" onclick={() => deleteSessionFile(s, p)} title="Delete .jsonl" aria-label="Delete"><Icon name="trash" size={11} /></button>
                    </div>
                  {/if}
                </li>
              {/each}
            </ul>
          {/if}

          {#if inboxOpenFor === p.project_path}
            {@const allInbox = inboxByPath[p.project_path] ?? []}
            {@const filteredInbox = inboxStatusFilter === "all" ? allInbox : allInbox.filter((x) => (x.status ?? "pending") === inboxStatusFilter)}
            <div class="inbox-panel">
              {#if allInbox.length > 0}
                <div class="inbox-status-filter">
                  {#each [["all", "All"], ["pending", "Pending"], ["sent", "Sent"], ["applied", "Applied"], ["ignored", "Ignored"]] as [k, label] (k)}
                    {@const cnt = k === "all" ? allInbox.length : allInbox.filter((x) => (x.status ?? "pending") === k).length}
                    <button class="pill {inboxStatusFilter === k ? 'active' : ''}" onclick={() => (inboxStatusFilter = k as typeof inboxStatusFilter)}>
                      {label}{cnt > 0 ? ` (${cnt})` : ""}
                    </button>
                  {/each}
                </div>
              {/if}
              {#if allInbox.length === 0 && addingInboxFor !== p.project_path}
                <div class="scripts-empty">No feedback collected yet.</div>
              {:else if filteredInbox.length === 0 && allInbox.length > 0}
                <div class="scripts-empty">No items in this status.</div>
              {:else if filteredInbox.length > 0}
                <ul class="inbox-list">
                  {#each filteredInbox as ie (ie.dir_name)}
                    {@const sendKey = `${p.project_path}|${ie.dir_name}`}
                    {@const status = ie.status ?? "pending"}
                    <li class="inbox-card">
                      <div class="inbox-card-head">
                        <span class="inbox-icon"><Icon name="mail" size={18} /></span>
                        <div class="inbox-meta">
                          <div class="inbox-title">{ie.title}</div>
                          <div class="inbox-info">
                            <span>{timeAgo(ie.created_at_ms)}</span>
                            {#if ie.attachments.length > 0}
                              <span class="dot">·</span>
                              <span>📎 {ie.attachments.length}</span>
                            {/if}
                            <span class="dot">·</span>
                            <span class="inbox-status-pill {status}">{status}</span>
                          </div>
                          {#if ie.description}
                            <div class="inbox-desc">{ie.description.length > 80 ? ie.description.slice(0, 80) + "…" : ie.description}</div>
                          {/if}
                        </div>
                      </div>
                      <div class="inbox-card-actions">
                        <button class="icon-tiny" onclick={() => openInboxFolder(p, ie)} title="Open folder" aria-label="Open folder"><Icon name="folder" size={12} /></button>
                        <div class="inbox-status-anchor">
                          <button class="icon-tiny" onclick={(e) => toggleInboxStatusMenu(p, ie, e)} title="Set status" aria-label="Set status"><Icon name="more-horizontal" size={12} /></button>
                          {#if inboxStatusMenuFor === sendKey}
                            <div class="menu inbox-status-menu" role="menu">
                              {#each ["pending", "sent", "applied", "ignored"] as opt (opt)}
                                <button class="menu-item" onclick={() => chooseInboxStatus(p, ie, opt as InboxEntry["status"])}>
                                  <span class="menu-text">
                                    <span class="menu-title">Mark {opt}</span>
                                    {#if status === opt}<span class="menu-hint">(current)</span>{/if}
                                  </span>
                                </button>
                              {/each}
                            </div>
                          {/if}
                        </div>
                        <button class="icon-tiny danger" onclick={() => deleteInbox(p, ie)} title="Delete" aria-label="Delete"><Icon name="trash" size={12} /></button>
                        <div class="send-anchor">
                          <button class="primary small send-button" onclick={(e) => toggleSendMenu(p, ie, e)} title="Send options">
                            <Icon name="send" size={12} /> Send <Icon name="chevron-down" size={11} />
                          </button>
                          {#if sendMenuFor === sendKey}
                            <div class="menu send-menu" role="menu">
                              <button class="menu-item" onclick={() => sendInboxToClaude(p, ie, "new")}>
                                <span class="menu-icon"><Icon name="play" size={14} /></span>
                                <span class="menu-text">
                                  <span class="menu-title">Send · new session</span>
                                  <span class="menu-hint">Clean context (recommended)</span>
                                </span>
                              </button>
                              <button class="menu-item" onclick={() => sendInboxToClaude(p, ie, "continue")}>
                                <span class="menu-icon"><Icon name="rotate-ccw" size={14} /></span>
                                <span class="menu-text">
                                  <span class="menu-title">Send · continue most recent</span>
                                  <span class="menu-hint">Resume the latest session in this project</span>
                                </span>
                              </button>
                            </div>
                          {/if}
                        </div>
                      </div>
                    </li>
                  {/each}
                </ul>
              {/if}

              {#if addingInboxFor === p.project_path}
                <div class="script-add inbox-add"
                     onpaste={handleInboxPaste}
                     ondragover={handleInboxDragOver}
                     ondrop={handleInboxDrop}>
                  {#if editingInboxFor}
                    <div class="form-banner">Editing: <code>{editingInboxFor.dirName}</code></div>
                  {/if}
                  <input bind:value={newInboxTitle} placeholder="Title (e.g. login style fixes)" />
                  <div class="textarea-with-actions">
                    <textarea
                      bind:value={newInboxDescription}
                      rows="5"
                      placeholder="Notes / what the customer wants. You can paste images here too."
                    ></textarea>
                    <div class="textarea-actions">
                      {#if descEditorPath}
                        <span class="editor-watching"><Icon name="rotate-ccw" size={11} /> Watching Notepad — saves auto-fill back here</span>
                      {:else}
                        <button class="small" type="button" onclick={openDescriptionInEditor} title="Open in Notepad — saves auto-sync back here"><Icon name="pencil" size={12} /> Open in Notepad</button>
                      {/if}
                    </div>
                  </div>
                  {#if !editingInboxFor}
                    <div class="attachments-zone">
                      <div class="attachments-actions">
                        <button class="small" type="button" onclick={takeScreenshot} title="Snip screen (Alt+Q). I'll auto-grab when you come back."><Icon name="camera" size={12} /> Screenshot (Alt+Q)</button>
                        <button class="small" type="button" onclick={pickAttachmentFile}><Icon name="paperclip" size={12} /> Attach file</button>
                        <span class="hint">or paste / drop here</span>
                      </div>
                      {#if screenshotWaiting}
                        <div class="screenshot-status">
                          ⏳ Snipping Tool launched. Select an area — I'll auto-add the screenshot when you return here.
                          <button class="link" type="button" onclick={tryReadClipboardImage}>Grab now</button>
                        </div>
                      {/if}
                      {#if newInboxAttachments.length > 0}
                        <ul class="attachments-list">
                          {#each newInboxAttachments as att, i (att.name + i)}
                            <li>
                              {#if isImageName(att.name)}
                                <button
                                  type="button"
                                  class="thumb-inline-btn"
                                  onclick={() => openPreview(`data:${getMimeFromName(att.name)};base64,${att.content_base64}`, att.name, null)}
                                  title="Click to preview"
                                >
                                  <img class="thumb-inline" src={`data:${getMimeFromName(att.name)};base64,${att.content_base64}`} alt={att.name} />
                                </button>
                              {:else}
                                <div class="thumb-inline thumb-file"><Icon name="paperclip" size={14} /></div>
                              {/if}
                              <span class="att-name">{att.name}</span>
                              <button class="icon-tiny danger" type="button" onclick={() => removeAttachment(i)} title="Remove" aria-label="Remove"><Icon name="x" size={11} /></button>
                            </li>
                          {/each}
                        </ul>
                      {/if}
                    </div>
                  {:else}
                    <div class="hint" style="font-size: 11px;">
                      Attachments stay on disk and are unchanged when editing. Use [📁 Open folder] on the row to add/remove files manually.
                    </div>
                  {/if}
                  <div class="add-actions">
                    <button class="primary small" onclick={() => confirmAddInbox(p)} disabled={!newInboxTitle.trim()}>{editingInboxFor ? "Update" : "Save"}</button>
                    <button class="small" onclick={cancelAddInbox}>Cancel</button>
                  </div>
                </div>
              {:else}
                <div class="scripts-footer">
                  <button class="primary small" onclick={() => startAddInbox(p)}>+ New feedback</button>
                </div>
              {/if}
            </div>
          {/if}

          {#if todosOpenFor === p.project_path}
            {@const todoList = todosByPath[p.project_path] ?? []}
            <div class="todos-panel">
              {#if todoList.length === 0}
                <div class="scripts-empty">No todos yet.</div>
              {:else if todoList.length > 0}
                <ul class="todo-list">
                  {#each todoList as todo, i (todo.id)}
                    <li
                      class="todo-row"
                      class:completed={todo.completed}
                      class:drop-target={dragOverIdx === i && dragSourceIdx !== i}
                      class:dragging={dragSourceIdx === i}
                      ondragover={(e) => handleTodoDragOver(e, i)}
                      ondragleave={handleTodoDragLeave}
                      ondrop={(e) => handleTodoDrop(p, e, i)}
                    >
                      <span
                        class="drag-handle"
                        title="Drag to reorder"
                        draggable="true"
                        ondragstart={(e) => handleTodoDragStart(e, i)}
                        ondragend={handleTodoDragEnd}
                      ><Icon name="grip-vertical" size={14} /></span>
                      <button
                        class="todo-check"
                        class:checked={todo.completed}
                        onclick={() => toggleTodoCompleted(p, todo)}
                        title={todo.completed ? "Mark not done" : "Mark done"}
                        aria-label={todo.completed ? "Uncheck" : "Check"}
                      >
                        <Icon name={todo.completed ? "check-square" : "square"} size={14} />
                      </button>
                      <div class="todo-body">
                        <div class="todo-title">{todo.title}</div>
                      </div>
                      {#if todo.action_kind && todo.action_target}
                        <button
                          class="icon-tiny todo-launcher"
                          onclick={() => runTodoAction(todo)}
                          title={`${todo.action_kind === "url" ? "Open URL" : "Open path"}: ${todo.action_target}`}
                          aria-label="Run quick action"
                        >{todo.action_kind === "url" ? "🔗" : "📁"}</button>
                      {/if}
                      <button class="icon-tiny" onclick={() => openTodoModalEdit(p, todo)} title="Edit" aria-label="Edit"><Icon name="pencil" size={11} /></button>
                      <button class="icon-tiny danger" onclick={() => deleteTodo(p, todo)} title="Delete" aria-label="Delete"><Icon name="trash" size={11} /></button>
                    </li>
                  {/each}
                </ul>
              {/if}

              <div class="scripts-footer">
                <button class="primary small" onclick={() => openTodoModalNew(p)}><Icon name="plus" size={12} /> New todo</button>
              </div>
            </div>
          {/if}

          {#if scriptsMenuFor === p.project_path}
            <div class="scripts-panel">
              {#if (scriptsByPath[p.project_path] ?? []).length === 0 && addingFor !== p.project_path}
                <div class="scripts-empty">
                  No scripts yet in <code>.ccpscript/</code>
                </div>
              {:else if (scriptsByPath[p.project_path] ?? []).length > 0}
                <ul class="scripts-list">
                  {#each scriptsByPath[p.project_path] as sc (sc.file_name)}
                    <li class="script-row" class:running={sc.is_running}>
                      <button
                        class="script-run"
                        class:running={sc.is_running}
                        onclick={() => runScript(p, sc)}
                        title={sc.is_running ? "Focus running window" : "Run script"}
                      >
                        <span class="script-arrow">
                          {#if sc.is_running}
                            <Icon name="circle-dot" size={11} />
                          {:else}
                            <Icon name="play" size={11} />
                          {/if}
                        </span>
                        <span class="script-title">{sc.title}</span>
                        <span class="script-file">{sc.file_name}</span>
                      </button>
                      {#if sc.is_running}
                        <button class="icon-tiny danger" onclick={() => closeScript(p, sc)} title="Stop (close window)" aria-label="Stop"><Icon name="x" size={12} /></button>
                      {/if}
                      <button class="icon-tiny" onclick={() => editScript(p, sc)} title="Edit in Notepad" aria-label="Edit"><Icon name="pencil" size={11} /></button>
                      <button class="icon-tiny danger" onclick={() => deleteScript(p, sc)} title="Delete file" aria-label="Delete"><Icon name="trash" size={11} /></button>
                    </li>
                  {/each}
                </ul>
              {/if}

              {#if addingFor === p.project_path}
                <div class="script-add">
                  <div class="form-row">
                    <input bind:value={addName} placeholder="File name (e.g. dev) — saved as &lt;name&gt;.ps1" />
                  </div>
                  <div class="form-row">
                    <input bind:value={addTitle} placeholder="Display title (optional)" />
                  </div>
                  <textarea bind:value={addContent} rows="4" placeholder="npm run dev"></textarea>
                  <div class="add-actions">
                    <button
                      class="primary small"
                      onclick={() => confirmAdd(p)}
                      disabled={!addName.trim() || !addContent.trim()}
                    >Save</button>
                    <button class="small" onclick={cancelAdd}>Cancel</button>
                  </div>
                </div>
              {:else if askingFor === p.project_path}
                <div class="script-add">
                  <textarea
                    bind:value={askInstruction}
                    rows="3"
                    placeholder="Describe what you need. e.g. 'a script that runs docker compose with prod env vars'"
                  ></textarea>
                  <div class="add-actions">
                    <button
                      class="primary small"
                      onclick={() => confirmAsk(p)}
                      disabled={!askInstruction.trim()}
                    ><Icon name="sparkles" size={12} /> Ask Claude</button>
                    <button class="small" onclick={cancelAsk}>Cancel</button>
                  </div>
                </div>
              {:else}
                <div class="scripts-footer">
                  <button class="primary small" onclick={() => generateScripts(p)} title="Spawn Claude to detect & generate scripts">
                    <Icon name="wand" size={12} /> Detect &amp; generate
                  </button>
                  <button class="small" onclick={() => startAsk(p)} title="Spawn Claude with a custom instruction">
                    <Icon name="sparkles" size={12} /> Ask Claude
                  </button>
                  <button class="small" onclick={() => startAdd(p)}><Icon name="plus" size={12} /> Manual</button>
                  <button class="small" onclick={() => openCcpscriptDir(p)}><Icon name="folder" size={12} /> Open</button>
                </div>
              {/if}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  <footer class="status-bar">
    <span class="status-item" title="App uptime">⏱ {fmtUptime(uptimeMs)}</span>
    <div class="status-item nvm-anchor">
      <button class="nvm-trigger" onclick={toggleNvmMenu} title="Click to switch via nvm">
        ⬢ node {nodeVer ?? "?"}
        {#if nvmFlash}<span class="nvm-flash">{nvmFlash}</span>{/if}
      </button>
      {#if nvmMenuOpen}
        <div class="menu nvm-menu" role="menu">
          {#if !nvmInfo}
            <div class="menu-empty">loading…</div>
          {:else if !nvmInfo.available}
            <div class="menu-empty">{nvmInfo.error ?? "nvm unavailable"}</div>
          {:else if nvmInfo.versions.length === 0}
            <div class="menu-empty">no nvm versions installed</div>
          {:else}
            {#each nvmInfo.versions as v (v)}
              {@const isCurrent = v === nvmInfo.current}
              <button
                class="menu-item nvm-item"
                class:current={isCurrent}
                disabled={!!nvmSwitching}
                onclick={() => switchNode(v)}
              >
                <span class="menu-text">
                  <span class="menu-title">{isCurrent ? "✓ " : ""}node {v}</span>
                  {#if nvmSwitching === v}<span class="menu-hint">switching…</span>{/if}
                </span>
              </button>
            {/each}
          {/if}
        </div>
      {/if}
    </div>
    <button
      class="status-item probes-toggle"
      class:probes-off={probesPaused}
      onclick={toggleProbes}
      title={probesPaused
        ? "All background probes paused (latency / IP / usage / node). Click to resume."
        : "Pause all background probes — diagnostic toggle. Click to disable probes for A/B testing UI lag."}
    >
      {probesPaused ? "⏸ probes paused" : "⏵ probes on"}
    </button>
    <span class="status-spacer"></span>
    <div class="status-item usage-anchor">
      {#if !usageMon || !usageMon.enabled}
        <button
          class="usage-pill usage-disabled"
          onclick={enableUsageMonitor}
          disabled={usageBusy}
          title="Click to enable Claude Code usage monitoring (modifies ~/.claude/settings.json)"
        >
          📊 enable usage
        </button>
      {:else if !usageSnap || !usageSnap.available}
        <button
          class="usage-pill usage-warn"
          onclick={refreshUsage}
          title="Monitor enabled but no snapshot yet. Open a Claude Code session to populate."
        >
          📊 waiting for claude…
        </button>
      {:else}
        {@const stale = isUsageStale(usageSnap.updated_at_ms)}
        {@const fh = usageSnap.five_hour?.used_percentage ?? null}
        {@const sd = usageSnap.seven_day?.used_percentage ?? null}
        <button
          class="usage-pill"
          class:usage-warn={stale || (sd != null && sd >= 80)}
          onclick={refreshUsage}
          title={`Updated ${fmtUsageAge(usageSnap.updated_at_ms)}\n5h resets: ${usageSnap.five_hour?.resets_at ?? "?"}\n7d resets: ${usageSnap.seven_day?.resets_at ?? "?"}\nRight-click to disable monitor`}
          oncontextmenu={(e) => { e.preventDefault(); disableUsageMonitor(); }}
        >
          📊 5h: {fh != null ? `${Math.round(fh)}%` : "?"} · 7d: {sd != null ? `${Math.round(sd)}%` : "?"}
          {#if stale}<span class="usage-stale-tag">stale</span>{/if}
        </button>
      {/if}
    </div>
    <button
      class="status-item proxy-pill lat-{latTier}"
      class:proxy-checking={latChecking}
      onclick={refreshLatency}
      title={proxyLat?.error ?? "Latency · click to refresh (auto every 30s)"}
    >
      {#if !proxyLat}
        ⚡ …
      {:else if !proxyLat.alive}
        ⛔ down
      {:else if proxyLat.error}
        ⚠ {proxyLat.latency_ms != null ? `${proxyLat.latency_ms}ms` : "?"}
      {:else if latTier === "fast"}
        ⚡ {proxyLat.latency_ms}ms
      {:else if latTier === "slow"}
        🐢 {proxyLat.latency_ms}ms
      {:else if latTier === "very-slow"}
        🔥 {proxyLat.latency_ms}ms
      {:else}
        ⚡ {proxyLat.latency_ms ?? "?"}ms
      {/if}
    </button>
    <button
      class="status-item proxy-pill"
      class:proxy-down={proxyGeo && !proxyGeo.alive}
      class:proxy-warn={proxyGeo && proxyGeo.alive && proxyGeo.error}
      class:proxy-checking={geoChecking}
      onclick={refreshGeo}
      title={proxyGeo?.error ?? "IP location · click to refresh (auto every 30min)"}
    >
      {#if !proxyGeo}
        🌐 …
      {:else if !proxyGeo.alive}
        ⚠ proxy {proxyHost}:{proxyPort} down
      {:else if proxyGeo.error}
        ⚠ {proxyGeo.error.length > 40 ? proxyGeo.error.slice(0, 40) + "…" : proxyGeo.error}
      {:else}
        🌐 {proxyGeo.country ?? "?"}{proxyGeo.region ? ` / ${proxyGeo.region}` : ""}{proxyGeo.city ? ` / ${proxyGeo.city}` : ""}
      {/if}
    </button>
  </footer>
</main>
{/if}

<style>
  :global(:root) {
    --bg: #f7f8fa;
    --card: #ffffff;
    --text: #0f0f10;
    --text-dim: #6b7280;
    --border: #e5e7eb;
    --accent: #3b82f6;
    --accent-hover: #2563eb;
    --running: #10b981;
    --stopped: #d1d5db;
    --star: #f59e0b;
    --shadow: 0 1px 2px rgba(0, 0, 0, 0.04), 0 1px 8px rgba(0, 0, 0, 0.04);
  }

  @media (prefers-color-scheme: dark) {
    :global(:root) {
      --bg: #0f1115;
      --card: #1a1d23;
      --text: #f3f4f6;
      --text-dim: #9ca3af;
      --border: #2a2f38;
      --accent: #3b82f6;
      --accent-hover: #60a5fa;
      --running: #34d399;
      --stopped: #4b5563;
      --star: #fbbf24;
      --shadow: 0 1px 2px rgba(0, 0, 0, 0.2), 0 4px 12px rgba(0, 0, 0, 0.15);
    }
  }

  :global(html, body) {
    margin: 0;
    padding: 0;
    background: var(--bg);
    color: var(--text);
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", "Microsoft YaHei",
      sans-serif;
    font-size: 14px;
    height: 100%;
  }

  main {
    max-width: 760px;
    margin: 0 auto;
    padding: 16px 16px 32px;
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 16px;
  }

  .title {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }

  .logo {
    font-size: 18px;
  }

  h1 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .subtitle {
    color: var(--text-dim);
    font-size: 12px;
  }

  .actions {
    display: flex;
    gap: 8px;
  }

  button {
    background: var(--card);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 6px 12px;
    font-size: 13px;
    cursor: pointer;
    font-family: inherit;
    transition: border-color 0.12s ease, background 0.12s ease;
    line-height: 1.2;
  }

  button:hover {
    border-color: var(--accent);
  }

  button.primary {
    background: var(--accent);
    color: white;
    border-color: var(--accent);
  }

  button.primary:hover {
    background: var(--accent-hover);
    border-color: var(--accent-hover);
  }

  button.icon {
    padding: 6px 10px;
    min-width: 36px;
  }

  button.primary[disabled] {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .empty {
    text-align: center;
    color: var(--text-dim);
    padding: 60px 24px;
    line-height: 1.7;
  }

  .empty code {
    background: var(--border);
    padding: 1px 6px;
    border-radius: 4px;
    font-family: ui-monospace, "Cascadia Code", Consolas, monospace;
    font-size: 12px;
  }

  .error {
    background: #fee2e2;
    color: #991b1b;
    border: 1px solid #fecaca;
    border-radius: 8px;
    padding: 8px 12px;
    margin-bottom: 12px;
    font-size: 13px;
    cursor: pointer;
    width: 100%;
    text-align: left;
    font-family: inherit;
  }

  .error:hover {
    border-color: #fca5a5;
  }

  /* ── Toolbar ───────────────────────────────────────────────────────── */

  .toolbar {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-bottom: 12px;
  }

  .search-row {
    position: relative;
    display: flex;
    align-items: center;
  }

  .search-icon {
    position: absolute;
    left: 10px;
    font-size: 12px;
    opacity: 0.5;
    pointer-events: none;
  }

  .search {
    width: 100%;
    background: var(--card);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 8px 32px 8px 30px;
    font-size: 13px;
    font-family: inherit;
    outline: none;
    transition: border-color 0.12s ease, box-shadow 0.12s ease;
    box-sizing: border-box;
  }

  .search::placeholder {
    color: var(--text-dim);
  }

  .search:focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 18%, transparent);
  }

  .clear {
    position: absolute;
    right: 6px;
    background: transparent;
    border: none;
    color: var(--text-dim);
    font-size: 18px;
    line-height: 1;
    padding: 2px 8px;
    border-radius: 6px;
    cursor: pointer;
  }

  .clear:hover {
    background: var(--border);
    color: var(--text);
  }

  .filter-row {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    justify-content: space-between;
  }

  .pill-group {
    display: flex;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 2px;
    gap: 0;
  }

  .pill {
    background: transparent;
    border: none;
    border-radius: 6px;
    padding: 4px 10px;
    font-size: 12px;
    color: var(--text-dim);
    cursor: pointer;
    font-family: inherit;
    line-height: 1.4;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    transition: background 0.12s ease, color 0.12s ease;
  }

  .pill:hover {
    color: var(--text);
    background: color-mix(in srgb, var(--accent) 8%, transparent);
  }

  .pill.active {
    background: var(--accent);
    color: white;
  }

  .pill.active:hover {
    background: var(--accent-hover);
  }

  .status-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--stopped);
    display: inline-block;
  }

  .status-dot.running {
    background: var(--running);
  }

  .pill.active .status-dot {
    background: rgba(255, 255, 255, 0.85);
  }

  .pill.active .star-glyph {
    color: rgba(255, 255, 255, 0.95);
  }

  .star-glyph {
    color: var(--star);
  }

  .star-glyph.small {
    font-size: 11px;
  }

  .link {
    background: transparent;
    border: none;
    color: var(--accent);
    cursor: pointer;
    text-decoration: underline;
    font-family: inherit;
    font-size: inherit;
    padding: 0;
    margin-top: 4px;
  }

  /* ── Top-level + New dropdown ──────────────────────────────────────── */

  .dropdown {
    position: relative;
  }

  .caret {
    display: inline-block;
    margin-left: 2px;
    transition: transform 0.15s ease;
    font-size: 10px;
    opacity: 0.85;
  }

  .caret.open {
    transform: rotate(180deg);
  }

  .menu {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 50;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.12), 0 1px 4px rgba(0, 0, 0, 0.06);
    padding: 4px;
    min-width: 260px;
    display: flex;
    flex-direction: column;
    gap: 1px;
    animation: menuIn 0.12s ease;
  }

  @keyframes menuIn {
    from { opacity: 0; transform: translateY(-4px); }
    to { opacity: 1; transform: translateY(0); }
  }

  .menu-item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 9px 10px;
    background: transparent;
    border: none;
    border-radius: 6px;
    cursor: pointer;
    text-align: left;
    color: var(--text);
    font-family: inherit;
    font-size: 13px;
    transition: background 0.1s ease;
  }

  .menu-item:hover {
    background: color-mix(in srgb, var(--accent) 10%, transparent);
  }

  .menu-icon {
    font-size: 14px;
    width: 22px;
    text-align: center;
    color: var(--accent);
  }

  .menu-text {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
  }

  .menu-title {
    font-weight: 500;
    line-height: 1.25;
  }

  .menu-hint {
    font-size: 11px;
    color: var(--text-dim);
    line-height: 1.25;
  }

  /* ── Project cards ─────────────────────────────────────────────────── */

  .projects {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .project {
    display: flex;
    flex-direction: column;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: var(--shadow);
    transition: border-color 0.12s ease, background 0.12s ease;
    position: relative;
    /* No overflow:hidden — dropdown menus need to escape vertically.
       Inner panels with their own bg get rounded bottom corners via :last-child. */
  }

  .corner-star {
    position: absolute;
    top: 0;
    right: 0;
    width: 38px;
    height: 38px;
    border: none;
    padding: 0;
    cursor: pointer;
    background: color-mix(in srgb, var(--star) 16%, transparent);
    /* Right-angle triangle pointing into bottom-left of the 38x38 square. */
    clip-path: polygon(100% 0, 100% 100%, 0 0);
    border-top-right-radius: 10px;
    transition: background 0.12s ease;
    z-index: 2;
  }

  .corner-star:hover {
    background: color-mix(in srgb, var(--star) 30%, transparent);
  }

  .corner-star.starred {
    background: color-mix(in srgb, var(--star) 38%, transparent);
  }

  .corner-star.starred:hover {
    background: color-mix(in srgb, var(--star) 52%, transparent);
  }

  .corner-star-icon {
    position: absolute;
    top: 5px;
    right: 5px;
    color: var(--text-dim);
    line-height: 0;
  }

  .corner-star.starred .corner-star-icon {
    color: var(--star);
  }

  .corner-star:hover .corner-star-icon {
    color: var(--star);
  }

  .project > :last-child {
    border-bottom-left-radius: 10px;
    border-bottom-right-radius: 10px;
  }

  .project.running {
    border-color: color-mix(in srgb, var(--running) 35%, var(--border));
  }

  .project.starred {
    background: color-mix(in srgb, var(--star) 4%, var(--card));
  }

  .project-row {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 8px;
    align-items: stretch;
    padding: 4px 8px 4px 4px;
  }

  button.icon.has-badge {
    position: relative;
  }

  button.icon.has-badge .badge {
    position: absolute;
    top: -3px;
    right: -3px;
    background: var(--accent);
    color: white;
    font-size: 9px;
    border-radius: 9px;
    padding: 0 4px;
    min-width: 14px;
    height: 14px;
    line-height: 14px;
    text-align: center;
    font-weight: 700;
    box-shadow: 0 0 0 2px var(--card);
  }

  .project-hit {
    display: grid;
    grid-template-columns: 12px 1fr;
    gap: 10px;
    align-items: center;
    background: transparent;
    border: none;
    padding: 8px 10px;
    border-radius: 8px;
    cursor: pointer;
    text-align: left;
    color: inherit;
    font-family: inherit;
    font-size: inherit;
    width: 100%;
    min-width: 0;
    transition: background 0.12s ease;
  }

  .project-hit:hover {
    background: color-mix(in srgb, var(--accent) 6%, transparent);
  }

  .status {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--stopped);
    box-shadow: 0 0 0 2px var(--card);
  }

  .status.running {
    background: var(--running);
    box-shadow: 0 0 0 2px var(--card), 0 0 8px var(--running);
  }

  .status.small {
    width: 6px;
    height: 6px;
    box-shadow: none;
  }

  .meta {
    min-width: 0;
  }

  .name-row {
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    margin-bottom: 2px;
  }

  .name {
    font-weight: 600;
    font-size: 14px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .edit-pencil {
    background: transparent;
    border: 1px solid transparent;
    border-radius: 6px;
    padding: 2px 6px;
    font-size: 11px;
    color: var(--text-dim);
    cursor: pointer;
    font-family: inherit;
    line-height: 1;
    opacity: 0;
    transition: opacity 0.12s ease, color 0.12s ease, border-color 0.12s ease;
  }

  .project:hover .edit-pencil,
  .session-row:hover .edit-pencil {
    opacity: 0.6;
  }

  .edit-pencil:hover {
    opacity: 1 !important;
    color: var(--accent);
    border-color: var(--border);
  }

  .edit-pencil.tiny {
    padding: 0 4px;
    font-size: 10px;
  }

  .alias-input {
    background: var(--card);
    color: var(--text);
    border: 1px solid var(--accent);
    border-radius: 6px;
    padding: 4px 8px;
    font-size: 14px;
    font-weight: 600;
    font-family: inherit;
    outline: none;
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 18%, transparent);
    flex: 1;
    min-width: 0;
  }

  .alias-input.small {
    font-size: 12px;
    font-weight: 400;
    padding: 2px 6px;
    flex: 0 1 auto;
    width: auto;
    min-width: 180px;
  }

  .alias-input.row {
    font-size: 13px;
    font-weight: 400;
    padding: 4px 8px;
  }

  .path {
    font-size: 11px;
    color: var(--text-dim);
    font-family: ui-monospace, "Cascadia Code", Consolas, monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin-bottom: 4px;
  }

  .info {
    font-size: 11px;
    color: var(--text-dim);
    display: flex;
    gap: 6px;
    align-items: center;
    overflow: hidden;
  }

  .info.info-stacked {
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
  }

  .info-stacked .msg-line {
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    max-width: 100%;
  }

  .info-stacked .time-line {
    color: var(--text-dim);
  }

  button.icon-svg {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--text);
  }

  button.icon-svg svg {
    flex-shrink: 0;
  }

  button.icon.active-tool.icon-svg {
    color: var(--accent);
  }

  .msg-preview-btn {
    color: var(--text);
    font-style: italic;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 380px;
    background: none;
    border: none;
    padding: 0;
    margin: 0;
    text-align: left;
    cursor: pointer;
    font: inherit;
    font-style: italic;
    border-radius: 3px;
  }

  .msg-preview-btn:hover {
    color: var(--accent);
    text-decoration: underline;
  }

  .dot {
    opacity: 0.5;
  }

  .running-tag {
    color: var(--running);
    font-weight: 500;
  }

  .row-actions {
    display: flex;
    gap: 6px;
    align-items: center;
    padding-right: 4px;
  }

  /* ── Star button ───────────────────────────────────────────────────── */

  .star-btn {
    background: transparent;
    border: 1px solid transparent;
    border-radius: 8px;
    padding: 4px 8px;
    font-size: 16px;
    line-height: 1;
    color: var(--text-dim);
    cursor: pointer;
    font-family: inherit;
    transition: color 0.12s ease, transform 0.1s ease, background 0.12s ease;
    min-width: 32px;
  }

  .star-btn:hover {
    color: var(--star);
    background: color-mix(in srgb, var(--star) 10%, transparent);
    transform: scale(1.05);
  }

  .star-btn.starred {
    color: var(--star);
  }

  button.icon.danger {
    color: var(--text-dim);
  }

  button.icon.danger:hover {
    color: #ef4444;
    border-color: #ef4444;
  }

  button.icon.active-tool {
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    border-color: var(--accent);
    color: var(--accent);
  }

  /* ── Session list (expanded) ───────────────────────────────────────── */

  .session-list {
    list-style: none;
    margin: 0;
    padding: 4px 8px 8px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    border-top: 1px solid var(--border);
    background: color-mix(in srgb, var(--accent) 2%, var(--card));
  }

  .session-row {
    display: grid;
    grid-template-columns: 1fr auto auto;
    gap: 8px;
    align-items: center;
    padding: 4px 6px;
    border-radius: 6px;
    transition: background 0.1s ease;
  }

  .session-row:hover {
    background: color-mix(in srgb, var(--accent) 5%, transparent);
  }

  .session-hit {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 8px;
    align-items: center;
    background: transparent;
    border: none;
    padding: 4px 4px;
    border-radius: 4px;
    cursor: default;
    text-align: left;
    color: inherit;
    font-family: inherit;
    font-size: 13px;
    min-width: 0;
  }

  .session-row.running .session-hit {
    cursor: pointer;
  }

  .msg-text {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text);
  }

  .time {
    font-size: 11px;
    color: var(--text-dim);
    white-space: nowrap;
  }

  .session-actions {
    display: flex;
    gap: 4px;
  }

  .icon-tiny {
    background: transparent;
    border: 1px solid transparent;
    border-radius: 6px;
    padding: 4px 7px;
    font-size: 12px;
    color: var(--text-dim);
    cursor: pointer;
    font-family: inherit;
    transition: color 0.1s ease, border-color 0.1s ease;
  }

  .icon-tiny:hover {
    color: var(--text);
    border-color: var(--border);
  }

  .icon-tiny.danger:hover {
    color: #ef4444;
    border-color: #ef4444;
  }

  .expand-btn {
    color: var(--text-dim);
  }

  .overflow-anchor {
    position: relative;
  }

  .overflow-menu {
    top: calc(100% + 4px);
    right: 0;
    min-width: 240px;
  }

  .archive-glyph {
    font-size: 11px;
  }

  .pill.active .archive-glyph {
    filter: brightness(1.2);
  }

  /* ── Scripts panel ─────────────────────────────────────────────────── */

  .scripts-panel {
    border-top: 1px solid var(--border);
    background: color-mix(in srgb, var(--accent) 3%, var(--card));
    padding: 8px 10px 10px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .scripts-empty {
    color: var(--text-dim);
    font-size: 12px;
    padding: 4px 2px;
  }

  .scripts-empty code {
    background: var(--border);
    padding: 1px 5px;
    border-radius: 3px;
    font-family: ui-monospace, "Cascadia Code", Consolas, monospace;
    font-size: 11px;
  }

  .scripts-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .script-row {
    display: grid;
    grid-template-columns: 1fr auto auto auto;
    gap: 4px;
    align-items: center;
  }

  .script-run {
    display: grid;
    grid-template-columns: auto 1fr auto;
    gap: 8px;
    align-items: center;
    text-align: left;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 6px 10px;
    font-size: 13px;
    color: var(--text);
    font-family: inherit;
    cursor: pointer;
    transition: border-color 0.1s ease, background 0.1s ease;
    min-width: 0;
  }

  .script-run:hover {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 6%, var(--card));
  }

  .script-run.running {
    border-color: color-mix(in srgb, var(--running) 50%, var(--border));
    background: color-mix(in srgb, var(--running) 5%, var(--card));
  }

  .script-run.running:hover {
    border-color: var(--running);
    background: color-mix(in srgb, var(--running) 9%, var(--card));
  }

  .script-arrow {
    color: var(--accent);
    font-size: 11px;
    display: inline-flex;
    align-items: center;
  }

  .script-run.running .script-arrow {
    color: var(--running);
  }

  .script-title {
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .script-file {
    color: var(--text-dim);
    font-size: 11px;
    font-family: ui-monospace, "Cascadia Code", Consolas, monospace;
  }

  .scripts-footer {
    display: flex;
    gap: 6px;
    margin-top: 2px;
  }

  button.small {
    font-size: 12px;
    padding: 4px 10px;
  }

  .script-add {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 4px 0;
  }

  .form-row input,
  .script-add textarea {
    width: 100%;
    background: var(--card);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 6px 9px;
    font-size: 13px;
    font-family: inherit;
    box-sizing: border-box;
    outline: none;
    transition: border-color 0.1s ease, box-shadow 0.1s ease;
  }

  .script-add textarea {
    font-family: ui-monospace, "Cascadia Code", Consolas, monospace;
    font-size: 12px;
    resize: vertical;
    min-height: 60px;
  }

  .form-row input:focus,
  .script-add textarea:focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 18%, transparent);
  }

  .add-actions {
    display: flex;
    gap: 6px;
    justify-content: flex-end;
    margin-top: 2px;
  }

  /* ── Todos panel ───────────────────────────────────────────────── */

  .todos-panel {
    border-top: 1px solid var(--border);
    background: color-mix(in srgb, var(--accent) 3%, var(--card));
    padding: 8px 10px 10px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .todo-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .todo-row {
    display: grid;
    grid-template-columns: auto auto 1fr auto auto;
    gap: 6px;
    align-items: center;
    padding: 6px 8px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 6px;
    transition: border-color 0.1s ease, opacity 0.1s ease, background 0.1s ease;
  }

  .todo-row:hover {
    border-color: color-mix(in srgb, var(--accent) 30%, var(--border));
  }

  .todo-row.drop-target {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 8%, var(--card));
  }

  .todo-row.completed {
    opacity: 0.55;
  }

  .todo-row.dragging {
    opacity: 0.4;
  }

  .todo-row.completed .todo-title {
    text-decoration: line-through;
  }

  .drag-handle {
    cursor: grab;
    color: var(--text-dim);
    display: inline-flex;
    align-items: center;
    padding: 4px 4px;
    user-select: none;
    -webkit-user-select: none;
    -webkit-user-drag: element;
    border-radius: 4px;
    transition: background 0.1s ease, color 0.1s ease;
  }

  .drag-handle:hover {
    background: color-mix(in srgb, var(--accent) 10%, transparent);
    color: var(--text);
  }

  .drag-handle:active {
    cursor: grabbing;
  }

  .drag-handle :global(svg) {
    pointer-events: none;
  }

  .todo-check {
    background: transparent;
    border: 1px solid transparent;
    border-radius: 6px;
    padding: 2px;
    cursor: pointer;
    color: var(--text-dim);
    display: inline-flex;
    align-items: center;
    transition: color 0.12s ease, background 0.12s ease;
  }

  .todo-check:hover {
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 10%, transparent);
  }

  .todo-check.checked {
    color: var(--running);
  }

  .todo-body {
    min-width: 0;
  }

  .todo-title {
    font-size: 13px;
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .todo-details {
    font-size: 11px;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin-top: 2px;
  }

  /* ── Inbox panel ────────────────────────────────────────────────── */

  .inbox-panel {
    border-top: 1px solid var(--border);
    background: color-mix(in srgb, var(--accent) 3%, var(--card));
    padding: 8px 10px 10px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .inbox-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .inbox-card {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px 10px 8px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 6px;
    min-width: 0;
  }

  .inbox-card-head {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 10px;
    align-items: start;
    min-width: 0;
  }

  .inbox-card-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    justify-content: flex-end;
    align-items: center;
  }

  .send-button {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    white-space: nowrap;
  }

  .send-button .caret {
    font-size: 10px;
    transition: transform 0.15s ease;
    opacity: 0.85;
  }

  .send-button .caret.open {
    transform: rotate(180deg);
  }

  .send-anchor {
    position: relative;
  }

  .send-menu {
    top: calc(100% + 4px);
    right: 0;
    min-width: 240px;
  }


  .inbox-icon {
    font-size: 18px;
    line-height: 1;
  }

  .inbox-meta {
    min-width: 0;
  }

  .inbox-title {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .inbox-info {
    font-size: 11px;
    color: var(--text-dim);
    display: flex;
    gap: 6px;
    align-items: center;
    margin-top: 2px;
  }

  .inbox-desc {
    font-size: 11px;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin-top: 2px;
    font-style: italic;
  }

  .inbox-add {
    transition: background 0.12s ease;
  }

  .attachments-zone {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .attachments-actions {
    display: flex;
    gap: 6px;
    align-items: center;
    flex-wrap: wrap;
  }

  .attachments-actions .hint {
    color: var(--text-dim);
    font-size: 11px;
  }

  .attachments-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .attachments-list li {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 6px;
    font-size: 12px;
  }

  .att-name {
    font-family: ui-monospace, "Cascadia Code", Consolas, monospace;
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .form-banner {
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    border-radius: 6px;
    padding: 6px 10px;
    font-size: 12px;
    color: var(--text);
  }

  .form-banner code {
    background: var(--card);
    padding: 1px 5px;
    border-radius: 3px;
    font-family: ui-monospace, "Cascadia Code", Consolas, monospace;
    font-size: 11px;
  }

  .textarea-with-actions {
    position: relative;
  }

  .textarea-actions {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
    margin-top: 4px;
    align-items: center;
  }

  .editor-watching {
    font-size: 11px;
    color: var(--running);
    background: color-mix(in srgb, var(--running) 12%, transparent);
    border-radius: 6px;
    padding: 4px 10px;
  }

  .screenshot-status {
    background: color-mix(in srgb, var(--star) 14%, transparent);
    border: 1px solid color-mix(in srgb, var(--star) 35%, transparent);
    border-radius: 6px;
    padding: 6px 10px;
    font-size: 12px;
    color: var(--text);
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
  }

  /* ── Attachment thumbnails ───────────────────────────────────────── */

  .thumbs-row {
    grid-column: 1 / -1;
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
    margin-top: 6px;
  }

  .thumb {
    width: 56px;
    height: 56px;
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
    cursor: pointer;
    background: var(--card);
    padding: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: border-color 0.12s ease, transform 0.1s ease;
  }

  .thumb:hover {
    border-color: var(--accent);
    transform: scale(1.04);
  }

  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }

  .thumb-loading {
    color: var(--text-dim);
    font-size: 14px;
  }

  .thumb-file {
    flex-direction: column;
    gap: 2px;
    color: var(--text-dim);
  }

  .thumb-icon {
    font-size: 18px;
  }

  .thumb-ext {
    font-size: 9px;
    font-family: ui-monospace, "Cascadia Code", Consolas, monospace;
    letter-spacing: 0.5px;
  }

  .thumb-inline {
    width: 32px;
    height: 32px;
    border-radius: 4px;
    border: 1px solid var(--border);
    overflow: hidden;
    object-fit: cover;
    flex: 0 0 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--card);
  }

  img.thumb-inline {
    object-fit: cover;
  }

  .thumb-inline.thumb-file {
    color: var(--text-dim);
  }

  .thumb-inline-btn {
    background: transparent;
    border: none;
    padding: 0;
    cursor: pointer;
    line-height: 0;
  }

  /* ── Preview overlay ─────────────────────────────────────────────── */

  .preview-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.78);
    z-index: 100;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 24px;
    animation: fadeIn 0.12s ease;
  }

  @keyframes fadeIn {
    from { opacity: 0; }
    to { opacity: 1; }
  }

  .preview-content {
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
    max-width: 90vw;
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .preview-image {
    max-width: 100%;
    max-height: 80vh;
    object-fit: contain;
    display: block;
    background: #1a1a1a;
  }

  .preview-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 14px;
    border-top: 1px solid var(--border);
    gap: 12px;
    flex-wrap: wrap;
  }

  .preview-name {
    font-family: ui-monospace, "Cascadia Code", Consolas, monospace;
    font-size: 12px;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
    min-width: 0;
  }

  .preview-actions {
    display: flex;
    gap: 6px;
    flex-shrink: 0;
  }

  /* ── Todo modal ──────────────────────────────────────────────────── */

  .todo-modal {
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
    width: 640px;
    max-width: 90vw;
    max-height: 90vh;
    padding: 14px 16px 12px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .todo-modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .todo-modal-title-label {
    font-size: 11px;
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    font-weight: 600;
  }

  .todo-modal-title {
    width: 100%;
    background: var(--card);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 10px 12px;
    font-size: 16px;
    font-weight: 600;
    font-family: inherit;
    outline: none;
    box-sizing: border-box;
    transition: border-color 0.1s ease, box-shadow 0.1s ease;
  }

  .todo-modal-title:focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 18%, transparent);
  }

  .todo-modal-details {
    width: 100%;
    flex: 1;
    min-height: 280px;
    background: var(--card);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 10px 12px;
    font-size: 13px;
    font-family: inherit;
    outline: none;
    resize: vertical;
    box-sizing: border-box;
    line-height: 1.5;
    transition: border-color 0.1s ease, box-shadow 0.1s ease;
  }

  .todo-modal-details:focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 18%, transparent);
  }

  .todo-modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .todo-action-row {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .todo-action-label {
    font-size: 11px;
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    font-weight: 600;
  }

  .todo-action-input-row {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 6px;
  }

  .todo-action-kind {
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 4px 6px;
    font: inherit;
    font-size: 12px;
    color: var(--text);
  }

  .todo-action-target {
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 4px 8px;
    font: inherit;
    font-size: 12px;
    color: var(--text);
    min-width: 0;
  }

  .todo-action-hint {
    font-size: 10px;
    color: var(--text-dim);
    font-style: italic;
  }

  .todo-launcher {
    font-size: 13px;
    padding: 2px 4px;
  }

  .todo-launcher:hover {
    background: color-mix(in srgb, var(--accent) 14%, transparent);
  }

  /* ── Session preview modal ──────────────────────────────────────── */

  .session-preview-modal {
    width: 600px;
    max-width: 90vw;
  }

  .session-preview-title {
    font-size: 14px;
    color: var(--text-dim);
    font-style: italic;
  }

  .session-preview-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-height: 50vh;
    overflow-y: auto;
  }

  .session-preview-msg {
    background: color-mix(in srgb, var(--accent) 5%, var(--card));
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px 10px;
  }

  .session-preview-time {
    font-size: 10px;
    color: var(--text-dim);
    margin-bottom: 4px;
    font-family: ui-monospace, "Cascadia Code", Consolas, monospace;
  }

  .session-preview-text {
    font-size: 13px;
    color: var(--text);
    white-space: pre-wrap;
    line-height: 1.4;
  }

  /* ── Inbox status pill ──────────────────────────────────────────── */

  .inbox-status-row {
    display: flex;
    gap: 6px;
    align-items: center;
    flex-wrap: wrap;
    padding: 4px 0 6px;
  }

  .inbox-status-pill {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    padding: 2px 8px;
    border-radius: 8px;
    line-height: 1.6;
  }

  .inbox-status-pill.pending {
    background: color-mix(in srgb, var(--text-dim) 14%, transparent);
    color: var(--text-dim);
  }

  .inbox-status-pill.sent {
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    color: var(--accent);
  }

  .inbox-status-pill.applied {
    background: color-mix(in srgb, var(--running) 18%, transparent);
    color: var(--running);
  }

  .inbox-status-pill.ignored {
    background: color-mix(in srgb, var(--star) 16%, transparent);
    color: var(--text-dim);
    text-decoration: line-through;
  }

  .inbox-status-filter {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 0 0 8px;
  }

  .inbox-status-anchor {
    position: relative;
  }

  .inbox-status-menu {
    position: absolute;
    right: 0;
    top: calc(100% + 4px);
    min-width: 140px;
    z-index: 50;
  }

  /* ── Status bar (fixed footer) ──────────────────────────────────── */

  :global(body) {
    padding-bottom: 32px;
  }

  .status-bar {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 6px 12px;
    border-top: 1px solid var(--border);
    background: color-mix(in srgb, var(--bg) 92%, transparent);
    backdrop-filter: blur(6px);
    font-size: 11px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
    z-index: 80;
  }

  .status-item {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    white-space: nowrap;
  }

  .status-spacer {
    flex: 1;
  }

  .proxy-pill {
    background: color-mix(in srgb, var(--accent) 8%, transparent);
    border: 1px solid color-mix(in srgb, var(--accent) 25%, var(--border));
    color: var(--text);
    padding: 3px 8px;
    border-radius: 10px;
    cursor: pointer;
    font-size: 11px;
    font-family: inherit;
    transition: background 0.15s ease;
  }

  .proxy-pill:hover {
    background: color-mix(in srgb, var(--accent) 14%, transparent);
  }

  .proxy-pill.proxy-down,
  .proxy-pill.lat-down {
    background: #d63030;
    border-color: #b21f1f;
    color: #fff;
    font-weight: 700;
    animation: latPulse 1.6s ease-in-out infinite;
  }

  .proxy-pill.proxy-warn,
  .proxy-pill.lat-error {
    background: #f4a300;
    border-color: #cf8a00;
    color: #fff;
    font-weight: 600;
  }

  .proxy-pill.lat-fast {
    background: #1e8a3c;
    border-color: #166a2e;
    color: #fff;
    font-weight: 600;
  }

  .proxy-pill.lat-slow {
    background: #e6a800;
    border-color: #b88500;
    color: #2a1d00;
    font-weight: 600;
  }

  .proxy-pill.lat-very-slow {
    background: #e25822;
    border-color: #b8431b;
    color: #fff;
    font-weight: 700;
    animation: latPulse 2.2s ease-in-out infinite;
  }

  .proxy-pill.lat-idle {
    /* keep default look */
  }

  .proxy-pill.lat-fast:hover { background: #228c41; }
  .proxy-pill.lat-slow:hover { background: #ffb800; }
  .proxy-pill.lat-very-slow:hover { background: #e8612d; }
  .proxy-pill.lat-down:hover { background: #df3838; }

  @keyframes latPulse {
    0%, 100% { box-shadow: 0 0 0 0 currentColor; }
    50%      { box-shadow: 0 0 0 3px color-mix(in srgb, currentColor 28%, transparent); }
  }

  .proxy-pill.proxy-checking {
    opacity: 0.65;
  }

  .probes-toggle {
    background: transparent;
    border: 1px dashed var(--border);
    color: var(--text-dim);
    padding: 2px 8px;
    border-radius: 10px;
    cursor: pointer;
    font: inherit;
    font-size: 11px;
    transition: background 0.15s ease, color 0.15s ease;
  }

  .probes-toggle:hover {
    background: color-mix(in srgb, var(--accent) 10%, transparent);
    color: var(--text);
  }

  .probes-toggle.probes-off {
    background: color-mix(in srgb, var(--star) 22%, transparent);
    border: 1px solid var(--star);
    color: var(--text);
    font-weight: 600;
  }

  .nvm-anchor {
    position: relative;
  }

  .nvm-trigger {
    background: none;
    border: none;
    color: inherit;
    font: inherit;
    padding: 0;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }

  .nvm-trigger:hover {
    color: var(--accent);
  }

  .nvm-flash {
    font-size: 10px;
    color: var(--running);
    font-style: italic;
  }

  .nvm-menu {
    position: absolute;
    top: auto;
    right: auto;
    bottom: calc(100% + 6px);
    left: 0;
    min-width: 180px;
    max-height: 280px;
    overflow-y: auto;
    z-index: 200;
  }

  .nvm-item.current {
    color: var(--running);
  }

  .menu-empty {
    padding: 8px 10px;
    color: var(--text-dim);
    font-size: 12px;
    font-style: italic;
  }

  .usage-anchor { position: relative; }

  .usage-pill {
    background: color-mix(in srgb, var(--running) 10%, transparent);
    border: 1px solid color-mix(in srgb, var(--running) 30%, var(--border));
    color: var(--text);
    padding: 3px 8px;
    border-radius: 10px;
    cursor: pointer;
    font: inherit;
    font-size: 11px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }

  .usage-pill:hover {
    background: color-mix(in srgb, var(--running) 18%, transparent);
  }

  .usage-pill.usage-disabled {
    background: color-mix(in srgb, var(--text-dim) 12%, transparent);
    border-color: color-mix(in srgb, var(--text-dim) 25%, var(--border));
    color: var(--text-dim);
  }

  .usage-pill.usage-warn {
    background: color-mix(in srgb, var(--star) 16%, transparent);
    border-color: color-mix(in srgb, var(--star) 40%, var(--border));
  }

  .usage-stale-tag {
    font-size: 9px;
    background: var(--star);
    color: var(--card);
    padding: 1px 5px;
    border-radius: 6px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    font-weight: 700;
  }
</style>
