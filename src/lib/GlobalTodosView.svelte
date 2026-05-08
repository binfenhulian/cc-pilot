<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount, onDestroy } from "svelte";
  import Icon from "$lib/Icon.svelte";

  type Todo = {
    id: string;
    title: string;
    details: string;
    completed: boolean;
  };

  type GlobalTab = {
    id: string;
    name: string;
    todos: Todo[];
  };

  let tabs = $state<GlobalTab[]>([]);
  let activeTabId = $state<string | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);

  let editingTabId = $state<string | null>(null);
  let editingTabName = $state("");
  let addingTabName = $state("");
  let showAddTab = $state(false);

  let todoModal = $state<{ id: string | null } | null>(null);
  let modalTitle = $state("");
  let modalDetails = $state("");

  let dragSourceIdx = $state<number | null>(null);
  let dragOverIdx = $state<number | null>(null);

  let activeTab = $derived(tabs.find((t) => t.id === activeTabId) ?? null);

  async function load() {
    loading = true;
    try {
      tabs = await invoke<GlobalTab[]>("list_global_tabs");
      if (!activeTabId || !tabs.find((t) => t.id === activeTabId)) {
        activeTabId = tabs[0]?.id ?? null;
      }
      error = null;
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  function pendingCount(t: GlobalTab): number {
    return t.todos.filter((x) => !x.completed).length;
  }

  // ─── Tabs ────────────────────────────────────────────────────────

  function startAddTab() {
    showAddTab = true;
    addingTabName = "";
  }

  function cancelAddTab() {
    showAddTab = false;
    addingTabName = "";
  }

  async function confirmAddTab() {
    const name = addingTabName.trim();
    if (!name) return;
    try {
      const tab = await invoke<GlobalTab>("create_global_tab", { name });
      tabs = [...tabs, tab];
      activeTabId = tab.id;
      showAddTab = false;
      addingTabName = "";
    } catch (e) {
      error = String(e);
    }
  }

  function startRenameTab(t: GlobalTab) {
    editingTabId = t.id;
    editingTabName = t.name;
  }

  function cancelRenameTab() {
    editingTabId = null;
    editingTabName = "";
  }

  async function confirmRenameTab() {
    if (!editingTabId) return;
    const name = editingTabName.trim();
    if (!name) return;
    try {
      await invoke("rename_global_tab", { id: editingTabId, name });
      tabs = tabs.map((t) => (t.id === editingTabId ? { ...t, name } : t));
      cancelRenameTab();
    } catch (e) {
      error = String(e);
    }
  }

  async function deleteTab(t: GlobalTab) {
    if (tabs.length <= 1) {
      error = "Can't delete the last tab.";
      return;
    }
    if (!confirm(`Delete tab "${t.name}" and its ${t.todos.length} todo(s)?`)) return;
    try {
      await invoke("delete_global_tab", { id: t.id });
      tabs = tabs.filter((x) => x.id !== t.id);
      if (activeTabId === t.id) activeTabId = tabs[0]?.id ?? null;
    } catch (e) {
      error = String(e);
    }
  }

  // ─── Todos ───────────────────────────────────────────────────────

  function openModalNew() {
    if (!activeTabId) return;
    todoModal = { id: null };
    modalTitle = "";
    modalDetails = "";
  }

  function openModalEdit(t: Todo) {
    todoModal = { id: t.id };
    modalTitle = t.title;
    modalDetails = t.details;
  }

  function closeModal() {
    todoModal = null;
    modalTitle = "";
    modalDetails = "";
  }

  async function saveModal() {
    if (!todoModal || !activeTabId) return;
    const title = modalTitle.trim();
    if (!title) return;
    try {
      if (todoModal.id) {
        await invoke("update_global_todo", {
          tabId: activeTabId,
          id: todoModal.id,
          title,
          details: modalDetails,
        });
      } else {
        await invoke("add_global_todo", {
          tabId: activeTabId,
          title,
          details: modalDetails,
        });
      }
      closeModal();
      await load();
    } catch (e) {
      error = String(e);
    }
  }

  async function toggleCompleted(t: Todo) {
    if (!activeTabId) return;
    try {
      await invoke<boolean>("toggle_global_todo", {
        tabId: activeTabId,
        id: t.id,
      });
      tabs = tabs.map((tab) =>
        tab.id === activeTabId
          ? { ...tab, todos: tab.todos.map((x) => (x.id === t.id ? { ...x, completed: !x.completed } : x)) }
          : tab
      );
    } catch (e) {
      error = String(e);
    }
  }

  async function deleteTodo(t: Todo) {
    if (!activeTabId) return;
    if (!confirm(`Delete "${t.title}"?`)) return;
    try {
      await invoke("delete_global_todo", { tabId: activeTabId, id: t.id });
      await load();
    } catch (e) {
      error = String(e);
    }
  }

  function handleDragStart(e: DragEvent, idx: number) {
    dragSourceIdx = idx;
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", String(idx));
    }
  }

  function handleDragOver(e: DragEvent, idx: number) {
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    if (dragOverIdx !== idx) dragOverIdx = idx;
  }

  function handleDragLeave() {
    dragOverIdx = null;
  }

  async function handleDrop(e: DragEvent, idx: number) {
    e.preventDefault();
    if (!activeTabId || dragSourceIdx === null || dragSourceIdx === idx) {
      dragSourceIdx = null;
      dragOverIdx = null;
      return;
    }
    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab) return;
    const moved = [...tab.todos];
    const [item] = moved.splice(dragSourceIdx, 1);
    moved.splice(idx, 0, item);
    tabs = tabs.map((t) => (t.id === activeTabId ? { ...t, todos: moved } : t));
    dragSourceIdx = null;
    dragOverIdx = null;
    try {
      await invoke("reorder_global_todos", {
        tabId: activeTabId,
        orderedIds: moved.map((x) => x.id),
      });
    } catch (e) {
      error = String(e);
    }
  }

  function handleDragEnd() {
    dragSourceIdx = null;
    dragOverIdx = null;
  }

  $effect(() => {
    if (!todoModal) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        closeModal();
      }
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  });

  onMount(() => {
    load();
  });
</script>

<svelte:head>
  <title>CCPilot · Global Todos</title>
</svelte:head>

{#if todoModal}
  <div class="modal-overlay" onclick={closeModal} role="presentation">
    <div class="todo-modal" onclick={(e) => e.stopPropagation()} role="presentation">
      <div class="todo-modal-header">
        <span class="todo-modal-title-label">{todoModal.id ? "Edit todo" : "New todo"}</span>
        <button class="icon-tiny" onclick={closeModal} aria-label="Close"><Icon name="x" size={12} /></button>
      </div>
      <input class="todo-modal-title" bind:value={modalTitle} placeholder="Title" />
      <textarea class="todo-modal-details" bind:value={modalDetails} placeholder="Details (optional)"></textarea>
      <div class="todo-modal-actions">
        <button class="primary small" onclick={saveModal} disabled={!modalTitle.trim()}>
          {todoModal.id ? "Save" : "Add"}
        </button>
        <button class="small" onclick={closeModal}>Cancel (Esc)</button>
      </div>
    </div>
  </div>
{/if}

<main>
  <header>
    <h1>Global Todos</h1>
    <button class="primary small" onclick={openModalNew} disabled={!activeTabId}>
      <Icon name="plus" size={12} /> New todo
    </button>
  </header>

  {#if error}
    <button type="button" class="error" onclick={() => (error = null)}>{error}</button>
  {/if}

  <div class="tabs-strip">
    {#each tabs as tab (tab.id)}
      {@const pending = pendingCount(tab)}
      <div class="tab-wrapper">
        {#if editingTabId === tab.id}
          <input
            class="tab-rename"
            bind:value={editingTabName}
            onkeydown={(e) => {
              if (e.key === "Enter") confirmRenameTab();
              if (e.key === "Escape") cancelRenameTab();
            }}
            onblur={confirmRenameTab}
            autofocus
          />
        {:else}
          <button
            class="tab"
            class:active={activeTabId === tab.id}
            onclick={() => (activeTabId = tab.id)}
            ondblclick={() => startRenameTab(tab)}
            title="Double-click to rename"
          >
            {tab.name}
            {#if pending > 0}
              <span class="tab-badge">{pending}</span>
            {/if}
          </button>
          {#if activeTabId === tab.id}
            <button class="tab-action" onclick={() => startRenameTab(tab)} title="Rename" aria-label="Rename">
              <Icon name="pencil" size={11} />
            </button>
            <button class="tab-action danger" onclick={() => deleteTab(tab)} title="Delete tab" aria-label="Delete tab">
              <Icon name="x" size={11} />
            </button>
          {/if}
        {/if}
      </div>
    {/each}
    {#if showAddTab}
      <input
        class="tab-rename"
        bind:value={addingTabName}
        placeholder="Tab name"
        onkeydown={(e) => {
          if (e.key === "Enter") confirmAddTab();
          if (e.key === "Escape") cancelAddTab();
        }}
        onblur={() => {
          if (addingTabName.trim()) confirmAddTab();
          else cancelAddTab();
        }}
        autofocus
      />
    {:else}
      <button class="tab tab-add" onclick={startAddTab} title="New tab">
        <Icon name="plus" size={12} />
      </button>
    {/if}
  </div>

  {#if loading}
    <div class="empty">Loading…</div>
  {:else if !activeTab}
    <div class="empty">No tabs yet.</div>
  {:else if activeTab.todos.length === 0}
    <div class="empty">
      No todos in <b>{activeTab.name}</b> yet.<br />
      Click <b>+ New todo</b> to add one.
    </div>
  {:else}
    <ul class="todo-list">
      {#each activeTab.todos as todo, i (todo.id)}
        <li
          class="todo-row"
          class:completed={todo.completed}
          class:drop-target={dragOverIdx === i && dragSourceIdx !== i}
          class:dragging={dragSourceIdx === i}
          ondragover={(e) => handleDragOver(e, i)}
          ondragleave={handleDragLeave}
          ondrop={(e) => handleDrop(e, i)}
        >
          <span
            class="drag-handle"
            title="Drag to reorder"
            draggable="true"
            ondragstart={(e) => handleDragStart(e, i)}
            ondragend={handleDragEnd}
          ><Icon name="grip-vertical" size={14} /></span>
          <button
            class="todo-check"
            class:checked={todo.completed}
            onclick={() => toggleCompleted(todo)}
            title={todo.completed ? "Mark not done" : "Mark done"}
            aria-label="Toggle"
          >
            <Icon name={todo.completed ? "check-square" : "square"} size={14} />
          </button>
          <div class="todo-body">
            <div class="todo-title">{todo.title}</div>
          </div>
          <button class="icon-tiny" onclick={() => openModalEdit(todo)} title="Edit" aria-label="Edit">
            <Icon name="pencil" size={11} />
          </button>
          <button class="icon-tiny danger" onclick={() => deleteTodo(todo)} title="Delete" aria-label="Delete">
            <Icon name="trash" size={11} />
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</main>

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
    }
  }

  :global(html, body) {
    margin: 0;
    padding: 0;
    background: var(--bg);
    color: var(--text);
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", "Microsoft YaHei", sans-serif;
    font-size: 14px;
    height: 100%;
  }

  main {
    padding: 12px 14px 24px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  h1 {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
  }

  button {
    background: var(--card);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 4px 10px;
    font-size: 12px;
    cursor: pointer;
    font-family: inherit;
    line-height: 1.2;
    display: inline-flex;
    align-items: center;
    gap: 4px;
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

  button[disabled] {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .small {
    font-size: 12px;
  }

  .empty {
    color: var(--text-dim);
    text-align: center;
    padding: 40px 16px;
    line-height: 1.6;
  }

  .error {
    background: #fee2e2;
    color: #991b1b;
    border: 1px solid #fecaca;
    border-radius: 6px;
    padding: 6px 10px;
    width: 100%;
    text-align: left;
    cursor: pointer;
    font-size: 13px;
  }

  /* ── Tabs ───────────────────────────────────────────────────────── */

  .tabs-strip {
    display: flex;
    gap: 4px;
    border-bottom: 1px solid var(--border);
    padding-bottom: 6px;
    flex-wrap: wrap;
    align-items: center;
  }

  .tab-wrapper {
    display: inline-flex;
    align-items: center;
    gap: 2px;
  }

  .tab {
    background: transparent;
    border: 1px solid transparent;
    border-bottom: none;
    border-radius: 6px 6px 0 0;
    padding: 6px 12px;
    font-size: 13px;
    color: var(--text-dim);
    cursor: pointer;
    margin-bottom: -1px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }

  .tab:hover {
    color: var(--text);
    background: color-mix(in srgb, var(--accent) 6%, transparent);
  }

  .tab.active {
    color: var(--text);
    background: var(--card);
    border-color: var(--border);
    border-bottom: 1px solid var(--card);
    font-weight: 600;
  }

  .tab-badge {
    background: var(--accent);
    color: white;
    font-size: 10px;
    font-weight: 700;
    border-radius: 8px;
    padding: 1px 6px;
    min-width: 16px;
    text-align: center;
    line-height: 1.4;
  }

  .tab-action {
    background: transparent;
    border: none;
    color: var(--text-dim);
    padding: 4px 6px;
    border-radius: 4px;
    cursor: pointer;
  }

  .tab-action:hover {
    color: var(--text);
    background: var(--border);
  }

  .tab-action.danger:hover {
    color: #ef4444;
    background: color-mix(in srgb, #ef4444 12%, transparent);
  }

  .tab-add {
    color: var(--text-dim);
  }

  .tab-rename {
    background: var(--card);
    color: var(--text);
    border: 1px solid var(--accent);
    border-radius: 6px;
    padding: 5px 10px;
    font-size: 13px;
    font-weight: 600;
    font-family: inherit;
    outline: none;
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 18%, transparent);
    width: 140px;
  }

  /* ── Todo rows (mirrors main page) ───────────────────────────────── */

  .todo-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
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

  .todo-row.completed .todo-title {
    text-decoration: line-through;
  }

  .todo-row.dragging {
    opacity: 0.4;
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

  .drag-handle :global(svg) {
    pointer-events: none;
  }

  .todo-check {
    background: transparent;
    border: 1px solid transparent;
    border-radius: 6px;
    padding: 2px;
    color: var(--text-dim);
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

  .icon-tiny {
    background: transparent;
    border: 1px solid transparent;
    border-radius: 6px;
    padding: 4px 7px;
    font-size: 12px;
    color: var(--text-dim);
    cursor: pointer;
  }

  .icon-tiny:hover {
    color: var(--text);
    border-color: var(--border);
  }

  .icon-tiny.danger:hover {
    color: #ef4444;
    border-color: #ef4444;
  }

  /* ── Modal ────────────────────────────────────────────────────── */

  .modal-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.78);
    z-index: 100;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 16px;
  }

  .todo-modal {
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
    width: 600px;
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
  }

  .todo-modal-title:focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 18%, transparent);
  }

  .todo-modal-details {
    width: 100%;
    flex: 1;
    min-height: 240px;
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
</style>
