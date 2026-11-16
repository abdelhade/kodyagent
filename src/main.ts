import { invoke } from "@tauri-apps/api/core";

type AgentSettings = {
  server_url: string;
  dbname: string;
  main_printer: string;
  extra_printer: string;
  group_printers: Record<string, string>;
  agent_port: number;
};

type GroupItem = { id: number; name: string };
type DbItem = { name: string; label: string; is_current: boolean };

type TestConnectionResult = {
  ok: boolean;
  message: string;
  dbname: string;
  databases: DbItem[];
  groups: GroupItem[];
};

const el = {
  serverUrl: document.getElementById("serverUrl") as HTMLInputElement,
  dbname: document.getElementById("dbname") as HTMLSelectElement,
  mainPrinter: document.getElementById("mainPrinter") as HTMLSelectElement,
  extraPrinter: document.getElementById("extraPrinter") as HTMLSelectElement,
  groupsBox: document.getElementById("groupsBox") as HTMLDivElement,
  connMsg: document.getElementById("connMsg") as HTMLParagraphElement,
  saveMsg: document.getElementById("saveMsg") as HTMLSpanElement,
  agentBadge: document.getElementById("agentBadge") as HTMLDivElement,
  btnTest: document.getElementById("btnTest") as HTMLButtonElement,
  btnSave: document.getElementById("btnSave") as HTMLButtonElement,
  btnRefreshPrinters: document.getElementById(
    "btnRefreshPrinters",
  ) as HTMLButtonElement,
};

let printers: string[] = [];
let groups: GroupItem[] = [];
let databases: DbItem[] = [];
let groupMap: Record<string, string> = {};

function setMsg(node: HTMLElement, text: string, kind: "" | "ok" | "err" = "") {
  node.textContent = text;
  node.classList.remove("ok", "err");
  if (kind) node.classList.add(kind);
}

function fillSelect(
  select: HTMLSelectElement,
  selected: string,
  includeEmptyLabel: string,
) {
  select.innerHTML = "";
  const empty = document.createElement("option");
  empty.value = "";
  empty.textContent = includeEmptyLabel;
  select.appendChild(empty);
  for (const name of printers) {
    const opt = document.createElement("option");
    opt.value = name;
    opt.textContent = name;
    if (name === selected) opt.selected = true;
    select.appendChild(opt);
  }
}

function fillDatabases(selected: string) {
  el.dbname.innerHTML = "";
  if (!databases.length) {
    const opt = document.createElement("option");
    opt.value = selected || "";
    opt.textContent = selected
      ? selected
      : "— بعد اختبار الاتصال —";
    el.dbname.appendChild(opt);
    return;
  }
  for (const d of databases) {
    const opt = document.createElement("option");
    opt.value = d.name;
    opt.textContent = d.is_current
      ? `${d.label} (النشطة الآن)`
      : d.label || d.name;
    if (d.name === selected) opt.selected = true;
    el.dbname.appendChild(opt);
  }
  if (selected && !databases.some((d) => d.name === selected)) {
    const opt = document.createElement("option");
    opt.value = selected;
    opt.textContent = selected;
    opt.selected = true;
    el.dbname.appendChild(opt);
  }
}

function renderGroups() {
  el.groupsBox.innerHTML = "";
  if (!groups.length) {
    el.groupsBox.innerHTML =
      '<div class="empty">لا توجد مجموعات بعد — اعمل اختبار اتصال أولاً</div>';
    return;
  }

  for (const g of groups) {
    const row = document.createElement("div");
    row.className = "group-row";

    const name = document.createElement("div");
    name.className = "group-name";
    name.textContent = g.name;

    const select = document.createElement("select");
    fillSelect(select, groupMap[String(g.id)] || "", "— الطابعة العامة —");
    select.dataset.groupId = String(g.id);
    select.addEventListener("change", () => {
      const id = select.dataset.groupId!;
      if (select.value) groupMap[id] = select.value;
      else delete groupMap[id];
    });

    row.appendChild(name);
    row.appendChild(select);
    el.groupsBox.appendChild(row);
  }
}

async function loadPrinters() {
  printers = await invoke<string[]>("list_printers");
}

async function runTest(preferredDb?: string) {
  setMsg(el.connMsg, "جاري الاختبار...", "");
  const res = await invoke<TestConnectionResult>("test_connection", {
    serverUrl: el.serverUrl.value,
    dbname: preferredDb ?? el.dbname.value ?? "",
  });
  setMsg(el.connMsg, res.message, res.ok ? "ok" : "err");
  if (!res.ok) return res;

  databases = res.databases || [];
  groups = res.groups || [];
  fillDatabases(res.dbname || preferredDb || "");
  renderGroups();
  return res;
}

async function bootstrap() {
  const settings = await invoke<AgentSettings>("get_settings");
  const endpoint = await invoke<string>("agent_endpoint");
  el.agentBadge.textContent = endpoint.replace("http://", "");

  el.serverUrl.value = settings.server_url || "";
  groupMap = { ...(settings.group_printers || {}) };

  await loadPrinters();
  fillSelect(el.mainPrinter, settings.main_printer || "", "— اختر طابعة —");
  fillSelect(
    el.extraPrinter,
    settings.extra_printer || "",
    "— بدون طابعة إضافية —",
  );

  if (settings.server_url.trim()) {
    try {
      await runTest(settings.dbname || "");
    } catch {
      fillDatabases(settings.dbname || "");
      setMsg(el.connMsg, "تعذر التحقق من الاتصال المحفوظ", "err");
    }
  } else {
    fillDatabases(settings.dbname || "");
  }

  renderGroups();
}

el.btnRefreshPrinters.addEventListener("click", async () => {
  try {
    await loadPrinters();
    fillSelect(el.mainPrinter, el.mainPrinter.value, "— اختر طابعة —");
    fillSelect(
      el.extraPrinter,
      el.extraPrinter.value,
      "— بدون طابعة إضافية —",
    );
    renderGroups();
    setMsg(el.saveMsg, `تم تحديث الطابعات (${printers.length})`, "ok");
  } catch (e) {
    setMsg(el.saveMsg, String(e), "err");
  }
});

el.btnTest.addEventListener("click", async () => {
  try {
    // اختبار بدون إجبار db أولاً عشان نعرف النشطة من السيرفر لو القائمة فاضية
    await runTest(el.dbname.value || "");
  } catch (e) {
    setMsg(el.connMsg, String(e), "err");
  }
});

el.dbname.addEventListener("change", async () => {
  if (!el.serverUrl.value.trim() || !el.dbname.value) return;
  try {
    await runTest(el.dbname.value);
  } catch (e) {
    setMsg(el.connMsg, String(e), "err");
  }
});

el.btnSave.addEventListener("click", async () => {
  el.groupsBox.querySelectorAll("select[data-group-id]").forEach((node) => {
    const select = node as HTMLSelectElement;
    const id = select.dataset.groupId!;
    if (select.value) groupMap[id] = select.value;
    else delete groupMap[id];
  });

  const payload: AgentSettings = {
    server_url: el.serverUrl.value.trim().replace(/\/+$/, ""),
    dbname: el.dbname.value.trim(),
    main_printer: el.mainPrinter.value,
    extra_printer: el.extraPrinter.value,
    group_printers: groupMap,
    agent_port: 17890,
  };

  try {
    await invoke("save_settings", { settings: payload });
    setMsg(
      el.saveMsg,
      payload.dbname
        ? `تم الحفظ — القاعدة: ${payload.dbname}`
        : "تم الحفظ",
      "ok",
    );
  } catch (e) {
    setMsg(el.saveMsg, String(e), "err");
  }
});

bootstrap().catch((e) => {
  setMsg(el.connMsg, String(e), "err");
});
