import "@xterm/xterm/css/xterm.css";
import "./styles/variables.css";
import "./styles/layout.css";
import "./styles/sidebar.css";
import "./styles/terminal.css";
import "./styles/changes.css";
import "./styles/status-bar.css";
import "./styles/mobile.css";

import { createSidebar } from "./components/sidebar";
import { createTerminalPanel } from "./components/terminal-panel";
import { createChangesPanel } from "./components/changes-panel";
import { createStatusBar } from "./components/status-bar";
import { refresh } from "./state";

const REFRESH_INTERVAL_MS = 5000;

function bootstrap(): void {
  const appNode = document.getElementById("app");
  if (!(appNode instanceof HTMLDivElement)) {
    throw new Error("missing #app root");
  }

  const shell = document.createElement("div");
  shell.className = "app-shell";

  // Mobile top bar with burger
  const mobileBar = document.createElement("div");
  mobileBar.className = "mobile-bar";

  const burgerBtn = document.createElement("button");
  burgerBtn.className = "burger-btn";
  burgerBtn.setAttribute("aria-label", "Toggle sidebar");
  burgerBtn.innerHTML = `<svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor"><path d="M3 5h14v1.5H3V5zm0 4.25h14v1.5H3v-1.5zm0 4.25h14V15H3v-1.5z"/></svg>`;

  const mobileTitle = document.createElement("span");
  mobileTitle.className = "mobile-title";
  mobileTitle.textContent = "Arbor";

  const changesBtn = document.createElement("button");
  changesBtn.className = "changes-toggle-btn";
  changesBtn.setAttribute("aria-label", "Toggle changes");
  changesBtn.innerHTML = `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/></svg>`;

  mobileBar.append(burgerBtn, mobileTitle, changesBtn);

  // Overlay backdrop
  const overlay = document.createElement("div");
  overlay.className = "sidebar-overlay";

  const mainLayout = document.createElement("div");
  mainLayout.className = "main-layout";

  const sidebar = createSidebar();
  const leftHandle = createResizeHandle(sidebar, "left");
  const terminalPanel = createTerminalPanel();
  const rightHandle = createResizeHandle(null, "right");
  const changesPanel = createChangesPanel();
  rightHandle.dataset["target"] = "right";

  mainLayout.append(sidebar, leftHandle, terminalPanel, rightHandle, changesPanel);

  const statusBar = createStatusBar();

  shell.append(mobileBar, overlay, mainLayout, statusBar);
  appNode.append(shell);

  // Sidebar toggle
  function toggleSidebar(): void {
    sidebar.classList.toggle("open");
    overlay.classList.toggle("visible");
  }

  burgerBtn.addEventListener("click", toggleSidebar);
  overlay.addEventListener("click", () => {
    sidebar.classList.remove("open");
    overlay.classList.remove("visible");
  });

  // Changes panel toggle on mobile
  changesBtn.addEventListener("click", () => {
    changesPanel.classList.toggle("open");
  });

  // Close sidebar on worktree selection (mobile)
  sidebar.addEventListener("click", (e) => {
    const target = e.target as HTMLElement;
    if (target.closest(".wt-card") && window.innerWidth <= 768) {
      sidebar.classList.remove("open");
      overlay.classList.remove("visible");
    }
  });

  // Setup resize handles
  setupResize(leftHandle, sidebar, "left");
  setupResize(rightHandle, changesPanel, "right");

  // Initial data fetch
  void refresh();
  setInterval(() => { void refresh(); }, REFRESH_INTERVAL_MS);
}

function createResizeHandle(_target: HTMLElement | null, _side: string): HTMLDivElement {
  const handle = document.createElement("div");
  handle.className = "resize-handle";
  return handle;
}

function setupResize(handle: HTMLElement, target: HTMLElement, side: "left" | "right"): void {
  let startX = 0;
  let startWidth = 0;

  function onMouseDown(event: MouseEvent): void {
    event.preventDefault();
    startX = event.clientX;
    startWidth = target.getBoundingClientRect().width;
    handle.classList.add("dragging");
    document.addEventListener("mousemove", onMouseMove);
    document.addEventListener("mouseup", onMouseUp);
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
  }

  function onMouseMove(event: MouseEvent): void {
    const delta = event.clientX - startX;
    const newWidth = side === "left" ? startWidth + delta : startWidth - delta;
    const clamped = Math.max(200, Math.min(400, newWidth));
    target.style.width = `${clamped}px`;
  }

  function onMouseUp(): void {
    handle.classList.remove("dragging");
    document.removeEventListener("mousemove", onMouseMove);
    document.removeEventListener("mouseup", onMouseUp);
    document.body.style.cursor = "";
    document.body.style.userSelect = "";
  }

  handle.addEventListener("mousedown", onMouseDown);
}

bootstrap();
