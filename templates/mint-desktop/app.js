const desktop = document.querySelector("#desktop");
const startButton = document.querySelector("#startButton");
const startMenu = document.querySelector("#startMenu");
const launcherSearch = document.querySelector("#launcherSearch");
const taskList = document.querySelector("#taskList");
const clock = document.querySelector("#clock");
const windows = [...document.querySelectorAll(".window")];
let z = 10;

const appTitles = {
  files: "Files",
  terminal: "Terminal",
  editor: "Text Editor",
  monitor: "Process Monitor",
  settings: "Settings",
  browser: "Browser",
};

function focusWindow(win) {
  windows.forEach(w => w.classList.remove("focused", "active"));
  win.classList.add("open", "focused", "active");
  win.classList.remove("minimized");
  win.style.zIndex = ++z;
  renderTasks();
}

function openApp(app) {
  const win = document.querySelector(`[data-app="${app}"]`);
  if (!win) return;
  focusWindow(win);
  startMenu.classList.remove("open");
}

function renderTasks() {
  taskList.innerHTML = "";
  windows.filter(w => w.classList.contains("open") || w.classList.contains("active")).forEach(win => {
    const app = win.dataset.app;
    const task = document.createElement("button");
    task.textContent = appTitles[app] || app;
    task.className = win.classList.contains("focused") ? "active" : "";
    task.addEventListener("click", () => focusWindow(win));
    taskList.append(task);
  });
}

document.querySelectorAll("[data-open]").forEach(button => {
  button.addEventListener("click", () => openApp(button.dataset.open));
});

startButton.addEventListener("click", event => {
  event.stopPropagation();
  startMenu.classList.toggle("open");
  launcherSearch.focus();
});

desktop.addEventListener("click", event => {
  if (!startMenu.contains(event.target) && event.target !== startButton) {
    startMenu.classList.remove("open");
  }
});

launcherSearch.addEventListener("input", () => {
  const term = launcherSearch.value.trim().toLowerCase();
  document.querySelectorAll(".launcher-list button").forEach(button => {
    button.hidden = term && !button.textContent.toLowerCase().includes(term);
  });
});

windows.forEach(win => {
  win.addEventListener("mousedown", () => focusWindow(win));

  const bar = win.querySelector(".titlebar");
  let drag = null;

  bar.addEventListener("mousedown", event => {
    if (event.target.closest(".controls")) return;
    drag = {
      x: event.clientX,
      y: event.clientY,
      left: parseInt(win.style.left, 10),
      top: parseInt(win.style.top, 10),
    };
    focusWindow(win);
  });

  window.addEventListener("mousemove", event => {
    if (!drag) return;
    const left = Math.max(0, Math.min(window.innerWidth - 120, drag.left + event.clientX - drag.x));
    const top = Math.max(0, Math.min(window.innerHeight - 80, drag.top + event.clientY - drag.y));
    win.style.left = `${left}px`;
    win.style.top = `${top}px`;
  });

  window.addEventListener("mouseup", () => {
    drag = null;
  });

  win.querySelector("[data-close]").addEventListener("click", event => {
    event.stopPropagation();
    win.classList.remove("open", "active", "focused", "minimized");
    renderTasks();
  });

  win.querySelector("[data-minimize]").addEventListener("click", event => {
    event.stopPropagation();
    win.classList.remove("active", "focused");
    win.classList.add("minimized");
    renderTasks();
  });

  win.querySelector("[data-maximize]").addEventListener("click", event => {
    event.stopPropagation();
    const maximized = win.dataset.maximized === "true";
    if (maximized) {
      win.style.left = win.dataset.left;
      win.style.top = win.dataset.top;
      win.style.width = win.dataset.width;
      win.style.height = win.dataset.height;
      win.dataset.maximized = "false";
    } else {
      win.dataset.left = win.style.left;
      win.dataset.top = win.style.top;
      win.dataset.width = win.style.width;
      win.dataset.height = win.style.height;
      win.style.left = "10px";
      win.style.top = "10px";
      win.style.width = "calc(100vw - 20px)";
      win.style.height = "calc(100vh - 68px)";
      win.dataset.maximized = "true";
    }
    focusWindow(win);
  });
});

function updateClock() {
  const now = new Date();
  clock.textContent = now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

updateClock();
setInterval(updateClock, 1000);
focusWindow(document.querySelector("#window-files"));
