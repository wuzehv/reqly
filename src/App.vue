<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, toRaw, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import ResponseViewer from "./components/ResponseViewer.vue";
import ScriptEditor from "./components/ScriptEditor.vue";
import {
  AlertTriangle,
  Archive,
  ArchiveRestore,
  ArrowDown,
  ArrowUp,
  Check,
  ChevronDown,
  ChevronRight,
  ClipboardPaste,
  Code2,
  Copy,
  Download,
  FileJson,
  FileInput,
  Folder,
  FolderPlus,
  Moon,
  Network,
  Pencil,
  Pin,
  Plus,
  Save,
  Search,
  Send,
  Settings2,
  Sun,
  Terminal,
  Trash2,
  X,
} from "@lucide/vue";

const uid = () => crypto.randomUUID();
const cloneData = (value) => JSON.parse(JSON.stringify(toRaw(value)));
const pair = (key = "", value = "") => ({ id: uid(), key, value, enabled: true });
const defaultPreScript = "// 发送集合内的请求前运行\n// pm.environment.set(\"token\", \"your-token\");";
const defaultPostScript = "// 收到响应后运行\n// if (pm.response.code === 401) { /* 刷新凭证 */ }";
const legacyBatchData = "[\n  {\n    \"id\": 1\n  },\n  {\n    \"id\": 2\n  }\n]";
const themeColors = ["#d65f35", "#3276c3", "#27805f", "#7654b5", "#b94b68"];
const makeRequest = (name = "新请求", url = "") => ({
  id: uid(),
  name,
  method: "GET",
  url,
  params: [pair()],
  headers: [pair("Accept", "application/json")],
  bodyType: "none",
  body: "",
  formBody: [pair()],
  formData: [pair()],
  postScript: defaultPostScript,
  batchData: "",
  pinned: false,
});

const defaultWorkspace = () => {
  const request = makeRequest("示例请求", "https://httpbin.org/get");
  return {
    collections: [{ id: uid(), name: "我的接口", expanded: true, preScript: defaultPreScript, requests: [request], groups: [] }],
    selectedRequestId: request.id,
    variables: [pair("baseUrl", "https://httpbin.org")],
    proxy: { enabled: false, url: "http://127.0.0.1:7890", username: "", password: "" },
    mcp: { enabled: true, port: 3765 },
    appearance: { mode: "light", accent: themeColors[0], sidebarWidth: 252, requestEditorHeight: 430, consoleHeight: 300, settingsWidth: 520 },
  };
};

const workspace = reactive(defaultWorkspace());
const ready = ref(false);
const activeTab = ref("params");
const responseTab = ref("body");
const response = ref(null);
const responseFullscreen = ref(false);
const sending = ref(false);
const saveState = ref("saved");
const settingsOpen = ref(false);
const mcpRuntime = reactive({ running: false, port: null, applying: false, error: "" });
const mcpAddresses = ref([]);
const mcpAddressesOpen = ref(false);
const copiedMcpAddress = ref("");
const artifactImportInput = ref(null);
const importingArtifact = ref(false);
const searchText = ref("");
const errorMessage = ref("");
const bodyFormatError = ref("");
const formImportOpen = ref(false);
const formImportText = ref("");
const formImportMode = ref("auto");
const formImportStrategy = ref("replace");
const formImportError = ref("");
const batchOpen = ref(false);
const batchRunning = ref(false);
const batchStopping = ref(false);
const batchError = ref("");
const batchResults = ref([]);
const batchPlaceholders = ref([]);
const selectedBatchResultId = ref("");
const batchResponseTab = ref("body");
const batchResponseFullscreen = ref(false);
const batchProgress = reactive({ current: 0, total: 0 });
const editingCollectionId = ref("");
const scriptCollectionId = ref("");
const scriptLogs = ref([]);
const consoleOpen = ref(false);
const consoleUnread = ref(0);
const environmentCreateMode = ref("variable");
const variableDraftKey = ref("");
const variableDraftValue = ref("");
const namespaceDraft = ref("");
const collapsedNamespaces = reactive({});
const collapsedSettings = reactive({ theme: true, accent: true, mcp: true, proxy: true, variables: false });
const confirmDialog = reactive({
  open: false,
  title: "",
  message: "",
  confirmLabel: "确认",
  resolve: null,
});
const resizingSidebar = ref(false);
const resizingRequest = ref(false);
const resizingConsole = ref(false);
const resizingSettings = ref(false);
const requestSessions = new Map();
const jsonValueEditors = reactive({});
const noticeMessage = ref("");
let saveTimer;
let noticeTimer;
let stopWorkspaceListener;
let stopMcpRequestListener;
let mcpRequestQueue = Promise.resolve();

function appendConsole(entry) {
  scriptLogs.value.push({
    id: uid(),
    time: new Date().toLocaleTimeString([], { hour12: false }),
    requestName: selectedRequest.value?.name || "Workspace",
    ...entry,
  });
  if (scriptLogs.value.length > 500) scriptLogs.value.splice(0, scriptLogs.value.length - 500);
  if (!consoleOpen.value) consoleUnread.value = Math.min(99, consoleUnread.value + 1);
}

function toggleConsole() {
  consoleOpen.value = !consoleOpen.value;
  if (consoleOpen.value) consoleUnread.value = 0;
}

function askConfirm({ title, message, confirmLabel = "确认" }) {
  if (confirmDialog.resolve) confirmDialog.resolve(false);
  confirmDialog.title = title;
  confirmDialog.message = message;
  confirmDialog.confirmLabel = confirmLabel;
  confirmDialog.open = true;
  return new Promise((resolve) => {
    confirmDialog.resolve = resolve;
    nextTick(() => document.querySelector(".confirm-dialog .danger-confirm")?.focus());
  });
}

function resolveConfirm(result) {
  if (!confirmDialog.open) return;
  confirmDialog.open = false;
  const resolve = confirmDialog.resolve;
  confirmDialog.resolve = null;
  resolve?.(result);
}

const isTauri = () => Boolean(window.__TAURI_INTERNALS__);

const requestContainers = (collection) => [collection, ...(collection.groups || [])];
const collectionRequests = (collection) => requestContainers(collection).flatMap((group) => group.requests || []);

function findRequestLocation(requestId) {
  for (const collection of workspace.collections) {
    for (const group of requestContainers(collection)) {
      const request = group.requests.find((item) => item.id === requestId);
      if (request) return { collection, group, request };
    }
  }
  return null;
}

function resolveRequestGroup(group) {
  for (const collection of workspace.collections) {
    if (collection.id === group.id) return collection;
    const child = (collection.groups || []).find((item) => item.id === group.id);
    if (child) return child;
  }
  return group;
}

const selectedRequestLocation = computed(() => findRequestLocation(workspace.selectedRequestId));
const selectedRequest = computed(() => selectedRequestLocation.value?.request || null);

const selectedCollection = computed(() => selectedRequestLocation.value?.collection || null);
const selectedRequestGroup = computed(() => selectedRequestLocation.value?.group || null);

const scriptCollection = computed(() =>
  workspace.collections.find((collection) => collection.id === scriptCollectionId.value),
);

const filteredCollections = computed(() => {
  const query = searchText.value.trim().toLowerCase();
  if (!query) return workspace.collections;
  return workspace.collections
    .map((collection) => {
      const collectionMatches = collection.name.toLowerCase().includes(query);
      const requests = collectionMatches ? collection.requests : collection.requests.filter(
        (request) => request.name.toLowerCase().includes(query) || request.url.toLowerCase().includes(query),
      );
      const groups = (collection.groups || []).map((group) => {
        const groupMatches = group.name.toLowerCase().includes(query);
        const groupRequests = collectionMatches || groupMatches ? group.requests : group.requests.filter(
          (request) => request.name.toLowerCase().includes(query) || request.url.toLowerCase().includes(query),
        );
        return { ...group, expanded: true, requests: groupRequests };
      }).filter((group) => collectionMatches || group.name.toLowerCase().includes(query) || group.requests.length);
      return { ...collection, expanded: true, requests, groups };
    })
    .filter((collection) => collection.name.toLowerCase().includes(query) || collection.requests.length || collection.groups.length);
});

const activeRequests = (collection) => collection.requests
  .filter((request) => !request.archived)
  .sort((left, right) => Number(Boolean(right.pinned)) - Number(Boolean(left.pinned)));
const archivedRequests = (collection) => collection.requests.filter((request) => request.archived);

const variableHierarchy = computed(() => {
  const roots = [];
  const groups = new Map();
  for (const variable of workspace.variables) {
    if (!variable.key && variable.namespace) {
      const namespaceParts = variable.namespace.split(".");
      if (!groups.has(namespaceParts[0])) {
        groups.set(namespaceParts[0], { name: namespaceParts[0], path: namespaceParts[0], variables: [], groups: new Map() });
      }
      const draftGroup = groups.get(namespaceParts[0]);
      if (namespaceParts.length === 1) {
        draftGroup.variables.push(variable);
      } else {
        if (!draftGroup.groups.has(namespaceParts[1])) {
          draftGroup.groups.set(namespaceParts[1], { name: namespaceParts[1], path: variable.namespace, variables: [] });
        }
        draftGroup.groups.get(namespaceParts[1]).variables.push(variable);
      }
      continue;
    }
    const parts = variable.key.split(".");
    if (parts.length === 1 || parts.length > 3 || parts.some((part) => !part)) {
      roots.push(variable);
      continue;
    }
    if (!groups.has(parts[0])) {
      groups.set(parts[0], { name: parts[0], path: parts[0], variables: [], groups: new Map() });
    }
    const group = groups.get(parts[0]);
    if (parts.length === 2) {
      group.variables.push(variable);
      continue;
    }
    if (!group.groups.has(parts[1])) {
      group.groups.set(parts[1], { name: parts[1], path: `${parts[0]}.${parts[1]}`, variables: [] });
    }
    group.groups.get(parts[1]).variables.push(variable);
  }
  return {
    roots,
    groups: [...groups.values()].map((group) => ({ ...group, groups: [...group.groups.values()] })),
  };
});

const methodClass = (method) => `method-${method.toLowerCase()}`;

const selectedBatchResult = computed(() =>
  batchResults.value.find((item) => item.id === selectedBatchResultId.value) || null,
);

function addCollection() {
  const collection = { id: uid(), name: `新集合 ${workspace.collections.length + 1}`, expanded: true, preScript: defaultPreScript, requests: [], groups: [] };
  workspace.collections.push(collection);
  beginRenameCollection(collection);
}

function addSubgroup(collection) {
  const target = workspace.collections.find((item) => item.id === collection.id);
  if (!target) return;
  const group = { id: uid(), name: `新分组 ${(target.groups || []).length + 1}`, expanded: true, archiveExpanded: false, requests: [] };
  target.groups ||= [];
  target.groups.push(group);
  target.expanded = true;
  beginRenameCollection(group);
}

async function removeSubgroup(collection, group) {
  const target = workspace.collections.find((item) => item.id === collection.id);
  const actual = target?.groups?.find((item) => item.id === group.id);
  if (!actual) return;
  const confirmed = await askConfirm({
    title: "删除二级分组",
    message: `将删除“${actual.name}”及其中所有请求，此操作无法撤销。`,
    confirmLabel: "删除分组",
  });
  if (!confirmed) return;
  const requestIds = actual.requests.map((request) => request.id);
  target.groups = target.groups.filter((item) => item.id !== actual.id);
  if (requestIds.includes(workspace.selectedRequestId)) selectFirstRequest();
  requestIds.forEach((id) => requestSessions.delete(id));
}

function beginRenameCollection(collection) {
  editingCollectionId.value = collection.id;
  nextTick(() => {
    const input = document.querySelector(`[data-collection-edit="${collection.id}"]`);
    input?.focus();
    input?.select();
  });
}

function finishRenameCollection(collection) {
  collection.name = collection.name.trim() || "未命名集合";
  editingCollectionId.value = "";
}

function openCollectionScript(collection) {
  saveCurrentRequestSession();
  scriptCollectionId.value = collection.id;
}

async function removeCollection(collection) {
  const confirmed = await askConfirm({
    title: "删除集合",
    message: `将删除“${collection.name}”及其中所有请求，此操作无法撤销。`,
    confirmLabel: "删除集合",
  });
  if (!confirmed) return;
  const removedRequestIds = collectionRequests(collection).map((request) => request.id);
  const wasSelected = removedRequestIds.includes(workspace.selectedRequestId);
  workspace.collections = workspace.collections.filter((item) => item.id !== collection.id);
  if (wasSelected) selectFirstRequest();
  removedRequestIds.forEach((id) => requestSessions.delete(id));
}

function addRequest(group = selectedRequestGroup.value || selectedCollection.value || workspace.collections[0]) {
  if (group) {
    group = resolveRequestGroup(group);
  }
  if (!group) {
    group = { id: uid(), name: "我的接口", expanded: true, preScript: defaultPreScript, requests: [], groups: [] };
    workspace.collections.push(group);
  }
  const previousRequest = activeRequests(group).at(-1);
  const request = makeRequest("新请求", previousRequest?.url || "");
  if (previousRequest) {
    request.method = previousRequest.method;
    syncParamsFromUrl(request);
  }
  group.requests.push(request);
  group.expanded = true;
  changeSelectedRequest(request.id);
  nextTick(() => document.querySelector(".request-name")?.select());
}

function archiveRequest(group, request) {
  const target = resolveRequestGroup(group);
  const targetRequest = target?.requests.find((item) => item.id === request.id);
  if (!targetRequest) return;
  targetRequest.archived = true;
  target.archiveExpanded = true;
}

function restoreRequest(group, request) {
  const target = resolveRequestGroup(group);
  const targetRequest = target?.requests.find((item) => item.id === request.id);
  if (targetRequest) targetRequest.archived = false;
}

function toggleArchive(group) {
  const target = resolveRequestGroup(group);
  if (target) target.archiveExpanded = !target.archiveExpanded;
}

async function removeRequest(group, request) {
  group = resolveRequestGroup(group);
  const confirmed = await askConfirm({
    title: "删除请求",
    message: `将永久删除请求“${request.name}”。`,
    confirmLabel: "删除请求",
  });
  if (!confirmed) return;
  group.requests = group.requests.filter((item) => item.id !== request.id);
  if (workspace.selectedRequestId === request.id) selectFirstRequest();
  requestSessions.delete(request.id);
}

function moveRequest(group, request, direction) {
  const target = resolveRequestGroup(group);
  if (!target) return;
  const siblings = activeRequests(target);
  const siblingIndex = siblings.findIndex((item) => item.id === request.id);
  const nextSibling = siblings[siblingIndex + direction];
  if (siblingIndex < 0 || !nextSibling || Boolean(nextSibling.pinned) !== Boolean(request.pinned)) return;
  const index = target.requests.findIndex((item) => item.id === request.id);
  const nextIndex = target.requests.findIndex((item) => item.id === nextSibling.id);
  [target.requests[index], target.requests[nextIndex]] = [target.requests[nextIndex], target.requests[index]];
}

function canMoveRequest(group, request, direction) {
  const siblings = activeRequests(group);
  const index = siblings.findIndex((item) => item.id === request.id);
  const next = siblings[index + direction];
  return Boolean(next) && Boolean(next.pinned) === Boolean(request.pinned);
}

function togglePinRequest(group, request) {
  const target = resolveRequestGroup(group)?.requests.find((item) => item.id === request.id);
  if (target) target.pinned = !target.pinned;
}

function selectFirstRequest() {
  const requests = workspace.collections.flatMap(collectionRequests);
  changeSelectedRequest(requests.find((request) => !request.archived)?.id || requests[0]?.id || "");
}

function selectRequest(request) {
  changeSelectedRequest(request.id);
}

function saveCurrentRequestSession() {
  const id = workspace.selectedRequestId;
  if (!id) return;
  requestSessions.set(id, {
    response: response.value,
    errorMessage: errorMessage.value,
    activeTab: activeTab.value,
    responseTab: responseTab.value,
    batchResults: batchResults.value,
    selectedBatchResultId: selectedBatchResultId.value,
    batchResponseTab: batchResponseTab.value,
  });
}

function changeSelectedRequest(id) {
  scriptCollectionId.value = "";
  batchOpen.value = false;
  if (id === workspace.selectedRequestId) return;
  saveCurrentRequestSession();
  batchResults.value = [];
  selectedBatchResultId.value = "";
  responseFullscreen.value = false;
  batchResponseFullscreen.value = false;
  bodyFormatError.value = "";
  formImportOpen.value = false;
  formImportText.value = "";
  formImportError.value = "";
  workspace.selectedRequestId = id;
  const session = requestSessions.get(id);
  response.value = session?.response || null;
  errorMessage.value = session?.errorMessage || "";
  activeTab.value = session?.activeTab || "params";
  responseTab.value = session?.responseTab || "body";
  batchResults.value = session?.batchResults || [];
  selectedBatchResultId.value = session?.selectedBatchResultId || batchResults.value.at(-1)?.id || "";
  batchResponseTab.value = session?.batchResponseTab || "body";
}

function addRow(rows) {
  const item = pair();
  rows.push(item);
  nextTick(() => document.querySelector(`[data-row-key="${item.id}"]`)?.focus());
}

function isStructuredJson(value) {
  if (!value?.trim()) return false;
  try {
    const parsed = JSON.parse(value);
    return parsed !== null && typeof parsed === "object";
  } catch {
    return false;
  }
}

function usesJsonEditor(item) {
  return Boolean(jsonValueEditors[item.id]) || isStructuredJson(item.value);
}

function updateJsonValue(item, value, request) {
  jsonValueEditors[item.id] = true;
  item.value = value;
  if (request) syncUrlFromParams(request);
}

function decodeFormComponent(value) {
  return decodeURIComponent(value.replaceAll("+", " "));
}

function parseFormImport() {
  formImportError.value = "";
  let source = formImportText.value.trim();
  if (!source) return [];
  let decode = formImportMode.value === "encoded"
    || (formImportMode.value === "auto" && (/%[0-9a-f]{2}/i.test(source) || source.includes("+")));

  try {
    if (decode && !source.includes("=") && /%3d/i.test(source)) {
      source = decodeFormComponent(source);
      decode = false;
    }
    if (source.startsWith("{")) {
      const object = JSON.parse(source);
      if (!object || Array.isArray(object) || typeof object !== "object") {
        throw new Error("JSON 必须是键值对象");
      }
      return Object.entries(object).map(([key, value]) => pair(key, typeof value === "object" ? JSON.stringify(value) : String(value ?? "")));
    }

    return source
      .replaceAll("\r", "")
      .split(/[&\n]/)
      .map((part) => part.trim())
      .filter(Boolean)
      .map((part) => {
        const separator = part.indexOf("=");
        const rawKey = separator < 0 ? part : part.slice(0, separator);
        const rawValue = separator < 0 ? "" : part.slice(separator + 1);
        return pair(decode ? decodeFormComponent(rawKey) : rawKey, decode ? decodeFormComponent(rawValue) : rawValue);
      })
      .filter((item) => item.key);
  } catch (error) {
    formImportError.value = formatError(error);
    return [];
  }
}

function importFormData() {
  const rows = parseFormImport();
  if (!rows.length) {
    if (!formImportError.value) formImportError.value = "没有解析到有效字段";
    return;
  }
  const target = selectedRequest.value.bodyType === "form"
    ? selectedRequest.value.formBody
    : selectedRequest.value.formData;
  if (formImportStrategy.value === "replace") target.splice(0, target.length, ...rows);
  else target.push(...rows);
  formImportText.value = "";
  formImportOpen.value = false;
}

function clearFormData() {
  const target = selectedRequest.value.bodyType === "form"
    ? selectedRequest.value.formBody
    : selectedRequest.value.formData;
  target.splice(0, target.length, pair());
  formImportError.value = "";
}

function removeRow(rows, index) {
  const [removed] = rows.splice(index, 1);
  if (removed) delete jsonValueEditors[removed.id];
  if (!rows.length) rows.push(pair());
}

function encodeQueryPart(value) {
  return encodeURIComponent(value)
    .replace(/%7B%7B/gi, "{{")
    .replace(/%7D%7D/gi, "}}");
}

function syncUrlFromParams(request) {
  const hashIndex = request.url.indexOf("#");
  const hash = hashIndex >= 0 ? request.url.slice(hashIndex) : "";
  const withoutHash = hashIndex >= 0 ? request.url.slice(0, hashIndex) : request.url;
  const base = withoutHash.split("?")[0];
  const query = request.params
    .filter((item) => item.enabled && item.key)
    .map((item) => `${encodeQueryPart(item.key)}=${encodeQueryPart(item.value)}`)
    .join("&");
  request.url = `${base}${query ? `?${query}` : ""}${hash}`;
}

function syncParamsFromUrl(request) {
  const query = request.url.split("#")[0].split("?")[1];
  if (query === undefined) {
    request.params = [pair()];
    return;
  }
  const parsed = [...new URLSearchParams(query).entries()].map(([key, value]) => pair(key, value));
  request.params = parsed.length ? parsed : [pair()];
}

function normalizeWorkspace(saved) {
  saved.mcp ||= { enabled: true, port: 3765 };
  saved.mcp.enabled ??= true;
  saved.mcp.port ||= 3765;
  saved.appearance ||= { mode: "light", accent: themeColors[0] };
  saved.appearance.mode ||= "light";
  saved.appearance.sidebarWidth ||= 252;
  saved.appearance.requestEditorHeight ||= 430;
  saved.appearance.consoleHeight ||= 300;
  saved.appearance.settingsWidth ||= 520;
  saved.collections.forEach((collection) => {
    collection.expanded ??= true;
    collection.archiveExpanded ??= false;
    collection.requests ||= [];
    collection.groups ||= [];
    collection.groups.forEach((group) => {
      group.expanded ??= true;
      group.archiveExpanded ??= false;
      group.requests ||= [];
      delete group.preScript;
    });
    collection.preScript ||= collectionRequests(collection).find((request) => request.preScript)?.preScript || defaultPreScript;
    const inheritedPostScript = collection.postScript;
    collectionRequests(collection).forEach((request) => {
      request.archived ??= false;
      request.pinned ??= false;
      request.params ||= [pair()];
      request.headers ||= [pair()];
      request.bodyType ||= "none";
      request.body ||= "";
      request.formBody ||= [pair()];
      request.formData ||= [pair()];
      request.postScript ??= inheritedPostScript || defaultPostScript;
      request.batchData ??= "";
    });
    delete collection.postScript;
  });
  return saved;
}

async function applyMcpConfig() {
  mcpRuntime.error = "";
  mcpAddressesOpen.value = false;
  const port = Number(workspace.mcp.port);
  if (!Number.isInteger(port) || port < 1024 || port > 65535) {
    mcpRuntime.error = "端口必须是 1024 到 65535 之间的整数";
    return;
  }
  if (!isTauri()) {
    mcpRuntime.error = "MCP 服务仅在桌面应用中可用";
    return;
  }
  mcpRuntime.applying = true;
  try {
    const status = await invoke("configure_mcp", { enabled: workspace.mcp.enabled, port });
    mcpRuntime.running = status.running;
    mcpRuntime.port = status.port;
  } catch (error) {
    mcpRuntime.running = false;
    workspace.mcp.enabled = false;
    mcpRuntime.error = formatError(error);
  } finally {
    mcpRuntime.applying = false;
  }
}

async function toggleMcpAddresses() {
  if (mcpAddressesOpen.value) {
    mcpAddressesOpen.value = false;
    return;
  }
  mcpRuntime.error = "";
  if (!isTauri()) {
    mcpRuntime.error = "MCP 地址仅在桌面应用中可用";
    return;
  }
  try {
    mcpAddresses.value = await invoke("get_mcp_addresses", { port: Number(workspace.mcp.port) });
    mcpAddressesOpen.value = true;
  } catch (error) {
    mcpRuntime.error = formatError(error);
  }
}

async function copyMcpAddress(address) {
  await navigator.clipboard.writeText(address);
  copiedMcpAddress.value = address;
  window.setTimeout(() => {
    if (copiedMcpAddress.value === address) copiedMcpAddress.value = "";
  }, 1200);
}

async function downloadArtifact(format, data, name) {
  const payload = { format, version: 1, exportedAt: new Date().toISOString(), data: cloneData(data) };
  const content = JSON.stringify(payload, null, 2);
  const filename = `${name.replace(/[\\/:*?"<>|]/g, "-")}.reqly.json`;
  if (isTauri()) {
    const { save } = await import("@tauri-apps/plugin-dialog");
    const path = await save({
      defaultPath: filename,
      filters: [{ name: "Reqly JSON", extensions: ["json"] }],
    });
    if (!path) {
      showNotice("已取消导出");
      return;
    }
    await invoke("write_export_file", { path, content });
    showNotice(`导出成功：${path}`);
    return;
  }
  const url = URL.createObjectURL(new Blob([content], { type: "application/json" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  document.body.appendChild(link);
  link.click();
  link.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 0);
  showNotice(`已导出：${filename}`);
}

async function exportCollection(collection) {
  const source = workspace.collections.find((item) => item.id === collection.id) || collection;
  try {
    await downloadArtifact("reqly-collection", source, source.name);
  } catch (error) {
    showNotice(`导出失败：${formatError(error)}`);
  }
}

async function exportSubgroup(group) {
  const source = resolveRequestGroup(group);
  try {
    await downloadArtifact("reqly-subgroup", source, source.name);
  } catch (error) {
    showNotice(`导出失败：${formatError(error)}`);
  }
}

function openArtifactImport() {
  artifactImportInput.value?.click();
}

function cloneImportedRequest(source) {
  if (!source || typeof source !== "object" || typeof source.name !== "string") throw new Error("接口数据格式无效");
  const request = { ...makeRequest(source.name, source.url || ""), ...cloneData(source), id: uid() };
  for (const field of ["params", "headers", "formBody", "formData"]) {
    request[field] = (request[field] || []).map((item) => ({ ...item, id: uid() }));
    if (!request[field].length) request[field].push(pair());
  }
  return request;
}

function cloneImportedSubgroup(source) {
  if (!source || typeof source.name !== "string" || !Array.isArray(source.requests || [])) throw new Error("二级分组数据格式无效");
  return {
    ...cloneData(source),
    id: uid(),
    name: `${source.name} 副本`,
    expanded: true,
    archiveExpanded: false,
    requests: (source.requests || []).map(cloneImportedRequest),
  };
}

function cloneImportedCollection(source) {
  if (!source || typeof source.name !== "string" || !Array.isArray(source.requests || [])) throw new Error("分组数据格式无效");
  return {
    ...cloneData(source),
    id: uid(),
    name: `${source.name} 副本`,
    expanded: true,
    archiveExpanded: false,
    preScript: source.preScript || defaultPreScript,
    requests: (source.requests || []).map(cloneImportedRequest),
    groups: (source.groups || []).map(cloneImportedSubgroup),
  };
}

function parentCollectionForGroup(group) {
  return workspace.collections.find((collection) => collection.id === group?.id || (collection.groups || []).some((item) => item.id === group?.id));
}

async function importArtifactFile(event) {
  const [file] = event.target.files || [];
  event.target.value = "";
  if (!file || importingArtifact.value) return;
  importingArtifact.value = true;
  try {
    const payload = JSON.parse(await file.text());
    const targetGroup = selectedRequestGroup.value;
    if (payload?.format === "reqly-request") {
      let target = targetGroup || workspace.collections[0];
      if (!target) {
        target = { id: uid(), name: "我的接口", expanded: true, preScript: defaultPreScript, requests: [], groups: [] };
        workspace.collections.push(target);
      }
      const request = cloneImportedRequest(payload.data);
      request.name = `${request.name} 副本`;
      target.requests.push(request);
      target.expanded = true;
      changeSelectedRequest(request.id);
    } else if (payload?.format === "reqly-subgroup") {
      const parent = parentCollectionForGroup(targetGroup) || selectedCollection.value || workspace.collections[0];
      if (!parent) throw new Error("请先创建一个一级分组");
      parent.groups ||= [];
      parent.groups.push(cloneImportedSubgroup(payload.data));
      parent.expanded = true;
    } else if (payload?.format === "reqly-collection") {
      const collection = cloneImportedCollection(payload.data);
      workspace.collections.push(collection);
      workspace.selectedRequestId = collectionRequests(collection).find((request) => !request.archived)?.id || "";
    } else {
      throw new Error("不支持的文件类型，请选择 Reqly 分组或接口导出文件");
    }
    showNotice("导入成功，已创建新副本");
  } catch (error) {
    showNotice(`导入失败：${formatError(error)}`);
  } finally {
    importingArtifact.value = false;
  }
}

function resizeSidebar(event) {
  if (!resizingSidebar.value) return;
  workspace.appearance.sidebarWidth = Math.min(520, Math.max(190, event.clientX));
}

function stopSidebarResize() {
  resizingSidebar.value = false;
  document.body.classList.remove("resizing-sidebar");
  window.removeEventListener("pointermove", resizeSidebar);
  window.removeEventListener("pointerup", stopSidebarResize);
}

function startSidebarResize(event) {
  event.preventDefault();
  resizingSidebar.value = true;
  document.body.classList.add("resizing-sidebar");
  window.addEventListener("pointermove", resizeSidebar);
  window.addEventListener("pointerup", stopSidebarResize);
}

function resizeRequestPanels(event) {
  if (!resizingRequest.value) return;
  const top = document.querySelector(".request-workbench")?.getBoundingClientRect().top || 48;
  const available = window.innerHeight - top;
  workspace.appearance.requestEditorHeight = Math.min(available - 190, Math.max(240, event.clientY - top));
}

function stopRequestResize() {
  resizingRequest.value = false;
  document.body.classList.remove("resizing-row");
  window.removeEventListener("pointermove", resizeRequestPanels);
  window.removeEventListener("pointerup", stopRequestResize);
}

function startRequestResize(event) {
  event.preventDefault();
  resizingRequest.value = true;
  document.body.classList.add("resizing-row");
  window.addEventListener("pointermove", resizeRequestPanels);
  window.addEventListener("pointerup", stopRequestResize);
}

function resizeConsolePanel(event) {
  if (!resizingConsole.value) return;
  workspace.appearance.consoleHeight = Math.min(window.innerHeight * 0.8, Math.max(150, window.innerHeight - event.clientY));
}

function stopConsoleResize() {
  resizingConsole.value = false;
  document.body.classList.remove("resizing-row");
  window.removeEventListener("pointermove", resizeConsolePanel);
  window.removeEventListener("pointerup", stopConsoleResize);
}

function startConsoleResize(event) {
  event.preventDefault();
  resizingConsole.value = true;
  document.body.classList.add("resizing-row");
  window.addEventListener("pointermove", resizeConsolePanel);
  window.addEventListener("pointerup", stopConsoleResize);
}

function resizeSettingsPanel(event) {
  if (!resizingSettings.value) return;
  workspace.appearance.settingsWidth = Math.min(window.innerWidth * 0.9, Math.max(380, window.innerWidth - event.clientX));
}

function stopSettingsResize() {
  resizingSettings.value = false;
  document.body.classList.remove("resizing-sidebar");
  window.removeEventListener("pointermove", resizeSettingsPanel);
  window.removeEventListener("pointerup", stopSettingsResize);
}

function startSettingsResize(event) {
  event.preventDefault();
  resizingSettings.value = true;
  document.body.classList.add("resizing-sidebar");
  window.addEventListener("pointermove", resizeSettingsPanel);
  window.addEventListener("pointerup", stopSettingsResize);
}

function interpolate(value, variables) {
  return String(value || "").replace(/\{\{\s*([^{}]+?)\s*\}\}/g, (match, key) =>
    Object.prototype.hasOwnProperty.call(variables, key) ? variables[key] : match,
  );
}

function validVariableKey(key) {
  if (!key) return true;
  const segments = key.split(".");
  return segments.length <= 3 && segments.every((segment) => segment.trim().length > 0);
}

function variableLeafName(variable) {
  return variable.key.split(".").at(-1);
}

function renameVariableLeaf(variable, namespace, value) {
  const leaf = value.trim();
  if (!leaf || leaf.includes(".")) return;
  variable.key = namespace ? `${namespace}.${leaf}` : leaf;
  delete variable.namespace;
}

function renameVariableKey(variable, value) {
  const key = value.trim();
  if (key && validVariableKey(key)) {
    variable.key = key;
    delete variable.namespace;
  }
}

function removeVariable(variable) {
  workspace.variables = workspace.variables.filter((item) => item.id !== variable.id);
  if (!workspace.variables.length) workspace.variables.push(pair());
}

function addVariableAt(namespace = "") {
  const variable = pair();
  if (namespace) variable.namespace = namespace;
  workspace.variables.push(variable);
  collapsedNamespaces[namespace] = false;
}

function addDirectVariable() {
  const key = variableDraftKey.value.trim();
  if (!key || !validVariableKey(key)) return;
  const existing = workspace.variables.find((item) => item.key === key);
  if (existing) {
    existing.value = variableDraftValue.value;
    existing.enabled = true;
    delete existing.namespace;
  } else {
    workspace.variables.push(pair(key, variableDraftValue.value));
  }
  variableDraftKey.value = "";
  variableDraftValue.value = "";
}

function addNamespace() {
  const namespace = namespaceDraft.value.trim();
  const segments = namespace.split(".");
  if (!namespace || segments.length > 2 || segments.some((segment) => !segment)) return;
  addVariableAt(namespace);
  namespaceDraft.value = "";
}

function toggleNamespace(path) {
  collapsedNamespaces[path] = !collapsedNamespaces[path];
}

function variablesMap() {
  return Object.fromEntries(
    workspace.variables.filter((item) => item.enabled && item.key).map((item) => [item.key, item.value]),
  );
}

function variablesMapFrom(rows) {
  return Object.fromEntries(
    rows.filter((item) => item.enabled && item.key).map((item) => [item.key, item.value]),
  );
}

function batchVariables(overrides) {
  const rows = cloneData(workspace.variables);
  for (const [key, rawValue] of Object.entries(overrides)) {
    const value = batchVariableValue(rawValue);
    const existing = rows.find((item) => item.key === key);
    if (existing) {
      existing.value = value;
      existing.enabled = true;
    } else {
      rows.push({ id: uid(), key, value, enabled: true });
    }
  }
  return rows;
}

function batchVariableValue(value) {
  return value !== null && typeof value === "object" ? JSON.stringify(value) : String(value ?? "");
}

function applyBatchOverrides(request, overrides) {
  const output = cloneData(request);
  const variables = Object.fromEntries(
    Object.entries(overrides).map(([key, value]) => [key, batchVariableValue(value)]),
  );
  const applyRows = (rows = []) => rows.forEach((item) => {
    item.key = interpolate(item.key, variables);
    item.value = interpolate(item.value, variables);
  });

  output.url = interpolate(output.url, variables);
  applyRows(output.params);
  applyRows(output.headers);
  applyRows(output.formBody);
  applyRows(output.formData);
  output.body = interpolate(output.body, variables);
  return output;
}

function multipartBody(rows, variables) {
  const boundary = `----Reqly${uid().replaceAll("-", "")}`;
  const content = rows
    .filter((item) => item.enabled && item.key && item.type !== "file")
    .map((item) => {
      const key = interpolate(item.key, variables).replaceAll('"', "%22");
      return `--${boundary}\r\nContent-Disposition: form-data; name="${key}"\r\n\r\n${interpolate(item.value, variables)}\r\n`;
    })
    .join("") + `--${boundary}--\r\n`;
  return { content, boundary };
}

function prepareInput(request, variables) {
  const resolvedUrl = interpolate(request.url, variables);
  const unresolved = [...resolvedUrl.matchAll(/\{\{\s*([^{}]+?)\s*\}\}/g)].map((match) => match[1]);
  if (unresolved.length) throw new Error(`未找到环境变量：${[...new Set(unresolved)].join("、")}`);

  const url = new URL(resolvedUrl);
  (request.params || [])
    .filter((item) => item.enabled && item.key)
    .forEach((item) => url.searchParams.set(interpolate(item.key, variables), interpolate(item.value, variables)));
  let headers = request.headers
    .filter((item) => item.enabled && item.key)
    .map((item) => ({ ...item, key: interpolate(item.key, variables), value: interpolate(item.value, variables) }));
  if (request.bodyType === "json" && !headers.some((item) => item.key.toLowerCase() === "content-type")) {
    headers.push({ id: uid(), key: "Content-Type", value: "application/json", enabled: true });
  }
  if (request.bodyType === "form" && !headers.some((item) => item.key.toLowerCase() === "content-type")) {
    headers.push({ id: uid(), key: "Content-Type", value: "application/x-www-form-urlencoded", enabled: true });
  }
  let body = request.bodyType === "form"
    ? new URLSearchParams(
        request.formBody
          .filter((item) => item.enabled && item.key)
          .map((item) => [interpolate(item.key, variables), interpolate(item.value, variables)]),
      ).toString()
    : request.bodyType === "none"
      ? ""
      : interpolate(request.body, variables);
  if (request.bodyType === "formdata") {
    const multipart = multipartBody(request.formData || [], variables);
    body = multipart.content;
    headers = headers.filter((item) => item.key.toLowerCase() !== "content-type");
    headers.push({ id: uid(), key: "Content-Type", value: `multipart/form-data; boundary=${multipart.boundary}`, enabled: true });
  }
  return {
    method: request.method,
    url: url.toString(),
    headers,
    body,
    proxy: cloneData(workspace.proxy),
  };
}

async function executeHttp(input) {
  if (isTauri()) return invoke("send_request", { input });
  const started = performance.now();
  const result = await fetch(input.url, {
    method: input.method,
    headers: Object.fromEntries(input.headers.map((item) => [item.key, item.value])),
    body: ["GET", "HEAD"].includes(input.method) ? undefined : input.body,
  });
  const body = await result.text();
  return {
    status: result.status,
    statusText: result.statusText,
    durationMs: Math.round(performance.now() - started),
    size: new Blob([body]).size,
    headers: [...result.headers.entries()],
    body,
  };
}

async function sendSandboxRequest(nestedRequest) {
  appendConsole({ phase: "request", level: "info", args: [`${nestedRequest.method} ${nestedRequest.url}`] });
  const result = await executeHttp({
    ...prepareInput({ ...nestedRequest, params: [] }, variablesMap()),
    proxy: cloneData(workspace.proxy),
  });
  appendConsole({ phase: "response", level: result.status >= 400 ? "error" : "info", args: [`HTTP ${result.status} ${result.statusText}`, `${result.durationMs} ms`] });
  return {
    code: result.status,
    status: result.statusText,
    header: result.headers.map(([key, value]) => ({ key, value })),
    body: result.body,
    responseTime: result.durationMs,
  };
}

function applyScriptVariables(variables) {
  const disabledVariables = workspace.variables.filter((item) => !item.enabled);
  workspace.variables = [...disabledVariables, ...variables];
}

async function buildInput(source, variables = cloneData(workspace.variables), persistVariables = true, collection = selectedCollection.value) {
  const { runPreRequest } = await import("./services/postmanSandbox");
  const execution = await runPreRequest({
    script: collection?.preScript || "",
    request: cloneData(source),
    variables,
    onConsole: (entry) => appendConsole({ phase: "pre", ...entry }),
    onRequest: sendSandboxRequest,
  });
  if (persistVariables) applyScriptVariables(execution.variables);
  return {
    input: prepareInput(execution.request, variablesMapFrom(execution.variables)),
    request: execution.request,
    variables: execution.variables,
  };
}

async function executeRequestLifecycle(source, collection, variables = cloneData(workspace.variables), persistVariables = true) {
  let currentVariables = variables;
  let result;
  let retryCount = 0;
  while (true) {
    const execution = await buildInput(source, currentVariables, persistVariables, collection);
    currentVariables = execution.variables;
    appendConsole({ requestName: source.name, phase: "request", level: "info", args: [`${execution.input.method} ${execution.input.url}`] });
    result = await executeHttp(execution.input);
    appendConsole({
      requestName: source.name,
      phase: "response",
      level: result.status >= 400 ? "error" : "info",
      args: [`HTTP ${result.status} ${result.statusText}`, `${result.durationMs} ms`],
    });
    const postExecution = await runResponseScript(execution.request, result, currentVariables, persistVariables);
    currentVariables = postExecution.variables;
    if (!postExecution.retryRequested) break;
    if (retryCount >= 1) {
      appendConsole({ requestName: source.name, phase: "post", level: "warn", args: ["已达到自动重试上限（1 次）"] });
      break;
    }
    retryCount += 1;
    appendConsole({ requestName: source.name, phase: "post", level: "info", args: ["凭证已刷新，正在重新执行请求"] });
  }
  return result;
}

async function runResponseScript(request, result, variables = cloneData(workspace.variables), persistVariables = true) {
  const script = request.postScript?.trim();
  if (!script || script === defaultPostScript.trim()) return { retryRequested: false, variables };
  const { runPostResponse } = await import("./services/postmanSandbox");
  const execution = await runPostResponse({
    script,
    request: cloneData(request),
    response: cloneData(result),
    variables,
    onConsole: (entry) => appendConsole({ phase: "post", ...entry }),
    onRequest: sendSandboxRequest,
  });
  if (persistVariables) applyScriptVariables(execution.variables);
  const next = execution.nextRequest;
  return {
    retryRequested: next === request.name || next === request.id,
    variables: execution.variables,
  };
}

function formatError(error) {
  if (typeof error === "string") return error.replace(/^Error:\s*/, "");
  if (error instanceof Error && error.message) return error.message;
  if (error && typeof error === "object") {
    for (const key of ["message", "error", "reason", "detail", "details"]) {
      const value = error[key];
      if (typeof value === "string" && value.trim()) return value.replace(/^Error:\s*/, "");
    }
    try {
      return stringifyObject(error, 2);
    } catch {
      return "未知请求错误";
    }
  }
  return String(error ?? "未知请求错误");
}

function showNotice(message) {
  noticeMessage.value = message;
  clearTimeout(noticeTimer);
  noticeTimer = window.setTimeout(() => (noticeMessage.value = ""), 3200);
}

async function sendRequest() {
  if (!selectedRequest.value || sending.value) return;
  sending.value = true;
  errorMessage.value = "";
  response.value = null;
  try {
    response.value = await executeRequestLifecycle(selectedRequest.value, selectedCollection.value);
  } catch (error) {
    errorMessage.value = formatError(error);
    appendConsole({ phase: "request", level: "error", args: [errorMessage.value] });
  } finally {
    sending.value = false;
  }
}

async function executeMcpRequest({ requestId, variables = {} }) {
  const location = findRequestLocation(requestId);
  const collection = location?.collection;
  const request = location?.request;
  if (!request) throw new Error(`未找到请求：${requestId}`);
  const hasOverrides = variables && Object.keys(variables).length > 0;
  const source = hasOverrides ? applyBatchOverrides(request, variables) : request;
  const executionVariables = hasOverrides ? batchVariables(variables) : cloneData(workspace.variables);
  const result = await executeRequestLifecycle(source, collection, executionVariables, !hasOverrides);
  setRequestExecutionResult(requestId, result, "");
  return { response: result, scriptsExecuted: true };
}

function setRequestExecutionResult(requestId, result, error) {
  if (workspace.selectedRequestId === requestId) {
    response.value = result;
    errorMessage.value = error;
    responseTab.value = "body";
    return;
  }
  const session = requestSessions.get(requestId) || {};
  requestSessions.set(requestId, {
    ...session,
    response: result,
    errorMessage: error,
    activeTab: session.activeTab || "params",
    responseTab: "body",
  });
}

function requestPlaceholders(request) {
  const values = [];
  for (const item of request.params || []) {
    if (item.enabled) values.push(item.key, item.value);
  }
  if (request.bodyType === "form") {
    for (const item of request.formBody || []) if (item.enabled) values.push(item.key, item.value);
  } else if (request.bodyType === "formdata") {
    for (const item of request.formData || []) if (item.enabled) values.push(item.key, item.value);
  } else if (request.bodyType !== "none") {
    values.push(request.body);
  }

  const names = new Set();
  for (const value of values) {
    for (const match of String(value || "").matchAll(/\{\{\s*([^{}]+?)\s*\}\}/g)) names.add(match[1].trim());
  }
  return [...names];
}

function suggestedBatchData(placeholders) {
  if (placeholders.length === 1) return JSON.stringify([1, 2], null, 2);
  return JSON.stringify([
    Object.fromEntries(placeholders.map((name) => [name, ""])),
  ], null, 2);
}

function batchValue(source, name) {
  if (Object.prototype.hasOwnProperty.call(source, name)) return { found: true, value: source[name] };
  let value = source;
  for (const segment of name.split(".")) {
    if (!value || typeof value !== "object" || !Object.prototype.hasOwnProperty.call(value, segment)) {
      return { found: false, value: undefined };
    }
    value = value[segment];
  }
  return { found: true, value };
}

function variablesForBatchItem(item, placeholders, index) {
  const variables = {};
  const missing = [];
  for (const name of placeholders) {
    const resolved = batchValue(item, name);
    if (resolved.found) variables[name] = resolved.value;
    else missing.push(name);
  }
  if (missing.length) throw new Error(`第 ${index + 1} 项缺少参数：${missing.join("、")}`);
  return variables;
}

function parseBatchData() {
  batchError.value = "";
  try {
    const placeholders = requestPlaceholders(selectedRequest.value);
    batchPlaceholders.value = placeholders;
    if (!placeholders.length) throw new Error("当前请求中没有可替换的 {{变量}} 占位符");
    const rows = JSON.parse(selectedRequest.value.batchData);
    if (!Array.isArray(rows) || !rows.length) throw new Error("请输入至少包含一项的 JSON 数组");
    if (rows.length > 500) throw new Error("单次最多执行 500 项");
    if (placeholders.length === 1) {
      const name = placeholders[0];
      return rows.map((item, index) => {
        if (!item || Array.isArray(item) || typeof item !== "object") {
          return { source: item, variables: { [name]: item } };
        }
        return { source: item, variables: variablesForBatchItem(item, placeholders, index) };
      });
    }

    return rows.map((item, index) => {
      if (!item || Array.isArray(item) || typeof item !== "object") {
        throw new Error(`检测到多个占位符，第 ${index + 1} 项必须是 JSON 对象`);
      }
      return { source: item, variables: variablesForBatchItem(item, placeholders, index) };
    });
  } catch (error) {
    batchError.value = formatError(error);
    return null;
  }
}

function openBatchRunner() {
  const placeholders = requestPlaceholders(selectedRequest.value);
  if (!placeholders.length) {
    showNotice("请先在 Query 参数或请求 Body 中添加 {{变量}} 占位符");
    return;
  }
  batchPlaceholders.value = placeholders;
  if (!selectedRequest.value.batchData.trim() || selectedRequest.value.batchData === legacyBatchData) {
    selectedRequest.value.batchData = suggestedBatchData(placeholders);
  }
  batchError.value = "";
  batchOpen.value = true;
}

function stopBatchRun() {
  batchStopping.value = true;
}

function showBatchResponse(item) {
  selectedBatchResultId.value = item.id;
}

async function runBatch() {
  if (!selectedRequest.value || batchRunning.value) return;
  const rows = parseBatchData();
  if (!rows) return;

  const source = cloneData(selectedRequest.value);
  batchResults.value = [];
  selectedBatchResultId.value = "";
  batchProgress.current = 0;
  batchProgress.total = rows.length;
  batchRunning.value = true;
  batchStopping.value = false;

  for (let index = 0; index < rows.length; index += 1) {
    if (batchStopping.value) break;
    const iteration = rows[index];
    const item = { id: uid(), index, data: iteration.source, status: "running", response: null, error: "" };
    batchResults.value.push(item);
    selectedBatchResultId.value = item.id;
    try {
      let variables = batchVariables(iteration.variables);
      const iterationRequest = applyBatchOverrides(source, iteration.variables);
      let retryCount = 0;
      while (true) {
        const execution = await buildInput(iterationRequest, variables, false);
        variables = execution.variables;
        appendConsole({ phase: "batch", level: "info", args: [`#${index + 1}`, `${execution.input.method} ${execution.input.url}`] });
        item.response = await executeHttp(execution.input);
        const postExecution = await runResponseScript(execution.request, item.response, variables, false);
        variables = postExecution.variables;
        if (!postExecution.retryRequested || retryCount >= 1) break;
        retryCount += 1;
      }
      item.status = "completed";
      appendConsole({
        phase: "batch",
        level: item.response.status >= 400 ? "error" : "info",
        args: [`#${index + 1}`, `HTTP ${item.response.status} ${item.response.statusText}`, `${item.response.durationMs} ms`],
      });
    } catch (error) {
      item.status = "failed";
      item.error = formatError(error);
      appendConsole({ phase: "batch", level: "error", args: [`#${index + 1}`, item.error] });
    }
    batchProgress.current = index + 1;
  }

  batchRunning.value = false;
  batchStopping.value = false;
}

function stringifyObject(value, spacing = 0) {
  const visited = new WeakSet();
  const result = JSON.stringify(
    value,
    (_key, item) => {
      if (typeof item === "bigint") return item.toString();
      if (typeof item === "function") return `[Function: ${item.name || "anonymous"}]`;
      if (item && typeof item === "object") {
        if (visited.has(item)) return "[Circular]";
        visited.add(item);
      }
      return item;
    },
    spacing,
  );
  return result === undefined ? String(value) : result;
}

function formatLogArg(value) {
  if (typeof value === "string") return value;
  try {
    return stringifyObject(value, 2);
  } catch {
    return String(value);
  }
}

async function persist() {
  saveState.value = "saving";
  const snapshot = cloneData(workspace);
  try {
    if (isTauri()) await invoke("save_workspace", { workspace: snapshot });
    else localStorage.setItem("reqly-workspace", JSON.stringify(snapshot));
    saveState.value = "saved";
  } catch (error) {
    saveState.value = "error";
    errorMessage.value = `保存失败：${formatError(error)}`;
  }
}

watch(
  workspace,
  () => {
    if (!ready.value) return;
    saveState.value = "dirty";
    clearTimeout(saveTimer);
    saveTimer = window.setTimeout(persist, 500);
  },
  { deep: true },
);

watch(
  () => selectedRequest.value?.body,
  () => {
    bodyFormatError.value = "";
  },
);

function handleKeydown(event) {
  if (event.key === "Escape" && confirmDialog.open) {
    resolveConfirm(false);
    return;
  }
  if (event.key === "Escape" && responseFullscreen.value) {
    responseFullscreen.value = false;
    return;
  }
  if (event.key === "Escape" && batchResponseFullscreen.value) {
    batchResponseFullscreen.value = false;
    return;
  }
  if (event.key === "Escape" && batchOpen.value && !batchRunning.value) {
    batchOpen.value = false;
    return;
  }
  if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
    event.preventDefault();
    sendRequest();
  }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
    event.preventDefault();
    persist();
  }
}

onMounted(async () => {
  window.addEventListener("keydown", handleKeydown);
  try {
    if (isTauri()) {
      stopWorkspaceListener = await listen("workspace-updated", (event) => {
        const saved = event.payload;
        if (!saved?.collections || !saved?.variables || !saved?.proxy) return;
        Object.assign(workspace, normalizeWorkspace(cloneData(saved)));
        showNotice("已同步 MCP 工作区变更");
      });
    }
    const saved = isTauri()
      ? await invoke("load_workspace")
      : JSON.parse(localStorage.getItem("reqly-workspace") || "null");
    if (saved?.collections && saved?.variables && saved?.proxy) Object.assign(workspace, normalizeWorkspace(saved));
    else await persist();
    if (isTauri()) {
      const status = await invoke("get_mcp_status");
      mcpRuntime.running = status.running;
      mcpRuntime.port = status.port;
      stopMcpRequestListener = await listen("mcp-run-request", (event) => {
        mcpRequestQueue = mcpRequestQueue.catch(() => {}).then(async () => {
          const { callId, requestId, variables } = event.payload;
          try {
            const result = await executeMcpRequest({ requestId, variables });
            await invoke("complete_mcp_request", { callId, result, error: null });
          } catch (error) {
            const message = formatError(error);
            setRequestExecutionResult(requestId, null, message);
            await invoke("complete_mcp_request", { callId, result: null, error: message });
          }
        });
      });
    }
  } catch (error) {
    errorMessage.value = `读取工作区失败：${formatError(error)}`;
  }
  ready.value = true;
});

onBeforeUnmount(() => {
  stopWorkspaceListener?.();
  stopMcpRequestListener?.();
  window.removeEventListener("keydown", handleKeydown);
  stopSidebarResize();
  stopRequestResize();
  stopConsoleResize();
  stopSettingsResize();
  clearTimeout(saveTimer);
  clearTimeout(noticeTimer);
});
</script>

<template>
  <div class="app-shell" :class="`theme-${workspace.appearance.mode}`" :style="{ '--accent': workspace.appearance.accent }">
    <header class="topbar">
      <div class="brand"><span class="brand-mark">R</span><span>Reqly</span></div>
      <div class="topbar-actions">
        <span class="save-status" :class="saveState">
          <Check v-if="saveState === 'saved'" :size="14" />
          <Save v-else :size="14" />
          {{ saveState === "saved" ? "已保存" : saveState === "saving" ? "保存中" : saveState === "error" ? "保存失败" : "未保存" }}
        </span>
        <button class="icon-button console-toggle" title="全局 Console" @click="toggleConsole"><Terminal :size="18" /><span v-if="consoleUnread">{{ consoleUnread }}</span></button>
        <button class="icon-button" title="代理与环境变量" @click="settingsOpen = true"><Settings2 :size="18" /></button>
      </div>
    </header>
    <div v-if="noticeMessage" class="app-notice"><AlertTriangle :size="16" /><span>{{ noticeMessage }}</span></div>

    <div class="workspace-layout" :style="{ '--sidebar-width': `${workspace.appearance.sidebarWidth || 252}px` }">
      <aside class="sidebar">
        <div class="sidebar-heading">
          <div><span class="eyebrow">WORKSPACE</span><strong>接口集合</strong></div>
          <button class="icon-button" title="新建集合" @click="addCollection"><FolderPlus :size="17" /></button>
        </div>
        <input ref="artifactImportInput" class="artifact-import-input" type="file" accept="application/json,.json" @change="importArtifactFile" />
        <label class="search-box"><Search :size="15" /><input v-model="searchText" placeholder="搜索请求" /></label>
        <div class="collection-list">
          <section v-for="collection in filteredCollections" :key="collection.id" class="collection" :class="{ 'has-selected-request': collectionRequests(collection).some((request) => request.id === workspace.selectedRequestId) }">
            <div class="collection-row">
              <button class="collapse-button" @click="collection.expanded = !collection.expanded">
                <ChevronDown v-if="collection.expanded" :size="15" /><ChevronRight v-else :size="15" />
              </button>
              <Folder :size="16" class="folder-icon" />
              <input
                v-if="editingCollectionId === collection.id"
                v-model="collection.name"
                class="collection-name-input"
                :data-collection-edit="collection.id"
                @blur="finishRenameCollection(collection)"
                @keydown.enter.prevent="$event.currentTarget.blur()"
              />
              <button v-else class="collection-name" @dblclick="beginRenameCollection(collection)">{{ collection.name }}</button>
              <span class="count">{{ collectionRequests(collection).filter((request) => !request.archived).length }}</span>
              <button class="row-action" title="重命名集合" @click="beginRenameCollection(collection)"><Pencil :size="13" /></button>
              <button class="row-action" title="集合脚本" @click="openCollectionScript(collection)"><Code2 :size="14" /></button>
              <button class="row-action" title="添加二级分组" @click="addSubgroup(collection)"><FolderPlus :size="14" /></button>
              <button class="row-action" title="添加请求" @click="addRequest(collection)"><Plus :size="15" /></button>
              <button class="row-action" title="导出分组" @click="exportCollection(collection)"><Download :size="13" /></button>
              <button class="row-action danger" title="删除集合" @click="removeCollection(collection)"><Trash2 :size="14" /></button>
            </div>
            <div v-if="collection.expanded" class="request-list">
              <button
                v-for="request in activeRequests(collection)"
                :key="request.id"
                class="request-item"
                :class="{ active: request.id === workspace.selectedRequestId, pinned: request.pinned }"
                @click="selectRequest(request)"
              >
                <span class="method-label" :class="methodClass(request.method)">{{ request.method }}</span>
                <span class="request-item-name"><span>{{ request.name }}</span><Pin v-if="request.pinned" :size="11" class="pinned-indicator" /></span>
                <span class="request-actions">
                  <span class="request-action" :class="{ disabled: !canMoveRequest(collection, request, -1) }" title="上移" @click.stop="moveRequest(collection, request, -1)"><ArrowUp :size="13" /></span>
                  <span class="request-action" :class="{ disabled: !canMoveRequest(collection, request, 1) }" title="下移" @click.stop="moveRequest(collection, request, 1)"><ArrowDown :size="13" /></span>
                  <span class="request-action pin-action" :class="{ active: request.pinned }" :title="request.pinned ? '取消置顶' : '置顶请求'" @click.stop="togglePinRequest(collection, request)"><Pin :size="13" /></span>
                  <span class="request-action" title="归档请求" @click.stop="archiveRequest(collection, request)"><Archive :size="13" /></span>
                  <span class="request-action danger" title="删除请求" @click.stop="removeRequest(collection, request)"><X :size="13" /></span>
                </span>
              </button>
              <div v-if="!activeRequests(collection).length && !collection.groups.length" class="empty-group">暂无活跃请求</div>
              <div v-if="archivedRequests(collection).length" class="archive-section">
                <button class="archive-heading" @click="toggleArchive(collection)">
                  <ChevronDown v-if="collection.archiveExpanded || searchText.trim()" :size="13" />
                  <ChevronRight v-else :size="13" />
                  <Archive :size="13" />
                  <span>归档</span>
                  <span class="archive-count">{{ archivedRequests(collection).length }}</span>
                </button>
                <div v-if="collection.archiveExpanded || searchText.trim()" class="archive-list">
                  <button
                    v-for="request in archivedRequests(collection)"
                    :key="request.id"
                    class="request-item archived"
                    :class="{ active: request.id === workspace.selectedRequestId }"
                    @click="selectRequest(request)"
                  >
                    <span class="method-label" :class="methodClass(request.method)">{{ request.method }}</span>
                    <span class="request-item-name">{{ request.name }}</span>
                    <span class="request-actions">
                      <span class="request-action" title="恢复请求" @click.stop="restoreRequest(collection, request)"><ArchiveRestore :size="13" /></span>
                      <span class="request-action danger" title="删除请求" @click.stop="removeRequest(collection, request)"><X :size="13" /></span>
                    </span>
                  </button>
                </div>
              </div>
              <section v-for="group in collection.groups" :key="group.id" class="subcollection" :class="{ active: group.requests.some((request) => request.id === workspace.selectedRequestId) }">
                <div class="subcollection-row">
                  <button class="collapse-button" @click="group.expanded = !group.expanded"><ChevronDown v-if="group.expanded" :size="15" /><ChevronRight v-else :size="15" /></button>
                  <Folder :size="16" class="folder-icon" />
                  <input
                    v-if="editingCollectionId === group.id"
                    v-model="group.name"
                    class="collection-name-input"
                    :data-collection-edit="group.id"
                    @blur="finishRenameCollection(group)"
                    @keydown.enter.prevent="$event.currentTarget.blur()"
                  />
                  <button v-else class="collection-name" @dblclick="beginRenameCollection(group)">{{ group.name }}</button>
                  <span class="count">{{ activeRequests(group).length }}</span>
                  <button class="row-action" title="重命名分组" @click="beginRenameCollection(group)"><Pencil :size="12" /></button>
                  <button class="row-action" title="添加请求" @click="addRequest(group)"><Plus :size="14" /></button>
                  <button class="row-action" title="导出二级分组" @click="exportSubgroup(group)"><Download :size="13" /></button>
                  <button class="row-action danger" title="删除二级分组" @click="removeSubgroup(collection, group)"><Trash2 :size="13" /></button>
                </div>
                <div v-if="group.expanded" class="subrequest-list">
                  <button
                    v-for="request in activeRequests(group)"
                    :key="request.id"
                    class="request-item"
                    :class="{ active: request.id === workspace.selectedRequestId, pinned: request.pinned }"
                    @click="selectRequest(request)"
                  >
                    <span class="method-label" :class="methodClass(request.method)">{{ request.method }}</span>
                    <span class="request-item-name"><span>{{ request.name }}</span><Pin v-if="request.pinned" :size="11" class="pinned-indicator" /></span>
                    <span class="request-actions">
                      <span class="request-action" :class="{ disabled: !canMoveRequest(group, request, -1) }" title="上移" @click.stop="moveRequest(group, request, -1)"><ArrowUp :size="13" /></span>
                      <span class="request-action" :class="{ disabled: !canMoveRequest(group, request, 1) }" title="下移" @click.stop="moveRequest(group, request, 1)"><ArrowDown :size="13" /></span>
                      <span class="request-action pin-action" :class="{ active: request.pinned }" :title="request.pinned ? '取消置顶' : '置顶请求'" @click.stop="togglePinRequest(group, request)"><Pin :size="13" /></span>
                      <span class="request-action" title="归档请求" @click.stop="archiveRequest(group, request)"><Archive :size="13" /></span>
                      <span class="request-action danger" title="删除请求" @click.stop="removeRequest(group, request)"><X :size="13" /></span>
                    </span>
                  </button>
                  <div v-if="!activeRequests(group).length" class="empty-group">暂无活跃请求</div>
                  <div v-if="archivedRequests(group).length" class="archive-section">
                    <button class="archive-heading" @click="toggleArchive(group)">
                      <ChevronDown v-if="group.archiveExpanded || searchText.trim()" :size="13" /><ChevronRight v-else :size="13" />
                      <Archive :size="13" /><span>归档</span><span class="archive-count">{{ archivedRequests(group).length }}</span>
                    </button>
                    <div v-if="group.archiveExpanded || searchText.trim()" class="archive-list">
                      <button v-for="request in archivedRequests(group)" :key="request.id" class="request-item archived" :class="{ active: request.id === workspace.selectedRequestId }" @click="selectRequest(request)">
                        <span class="method-label" :class="methodClass(request.method)">{{ request.method }}</span>
                        <span class="request-item-name"><span>{{ request.name }}</span></span>
                        <span class="request-actions">
                          <span class="request-action" title="恢复请求" @click.stop="restoreRequest(group, request)"><ArchiveRestore :size="13" /></span>
                          <span class="request-action danger" title="删除请求" @click.stop="removeRequest(group, request)"><X :size="13" /></span>
                        </span>
                      </button>
                    </div>
                  </div>
                </div>
              </section>
            </div>
          </section>
        </div>
        <div class="sidebar-footer-actions">
          <button class="import-artifact-button" :disabled="importingArtifact" @click="openArtifactImport"><FileInput :size="16" />{{ importingArtifact ? '导入中' : '导入' }}</button>
          <button class="new-request-button" @click="addRequest()"><Plus :size="16" />新建请求</button>
        </div>
      </aside>

      <div class="sidebar-resizer" title="拖动调整集合栏宽度，双击恢复" @pointerdown="startSidebarResize" @dblclick="workspace.appearance.sidebarWidth = 252"></div>

      <main v-if="scriptCollection" class="collection-script-workbench">
        <header class="collection-script-header">
          <div><span class="eyebrow">COLLECTION PRE-REQUEST</span><h2>{{ scriptCollection.name }}</h2></div>
          <span>脚本自动保存</span>
        </header>
        <div class="collection-script-content">
          <div class="editor-caption"><Code2 :size="15" /><span>Pre-request JavaScript</span><span class="hint">发送集合内任意请求前运行</span></div>
          <ScriptEditor :key="`pre-${workspace.appearance.mode}`" v-model="scriptCollection.preScript" :theme="workspace.appearance.mode" :variables="workspace.variables" />
          <p>集合只提供 Pre-request；Post-response 请在具体请求的 <code>Post Script</code> 标签中配置。</p>
        </div>
      </main>

      <main v-else-if="selectedRequest" class="request-workbench" :style="{ '--request-editor-height': `${workspace.appearance.requestEditorHeight || 430}px` }">
        <section class="request-editor">
          <div class="request-title-row">
            <label class="editable-title request-title-edit" title="点击修改接口名称">
              <Pencil :size="13" class="edit-indicator" />
              <input v-model="selectedRequest.name" class="request-name" aria-label="接口名称" placeholder="接口名称" @focus="$event.currentTarget.select()" />
            </label>
            <span class="title-divider"></span>
            <label v-if="selectedRequestGroup" class="editable-title collection-title-edit" title="点击修改分组名称">
              <Pencil :size="13" class="edit-indicator" />
              <span v-if="selectedRequestGroup.id !== selectedCollection.id" class="group-parent-path">{{ selectedCollection.name }} /</span>
              <input v-model="selectedRequestGroup.name" class="collection-path" aria-label="分组名称" placeholder="分组名称" @focus="$event.currentTarget.select()" />
            </label>
            <span v-if="selectedRequest.archived" class="archived-badge"><Archive :size="13" />已归档</span>
          </div>

          <div class="url-bar">
            <select v-model="selectedRequest.method" class="method-select" :class="methodClass(selectedRequest.method)">
              <option v-for="method in ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS']" :key="method" :class="methodClass(method)">{{ method }}</option>
            </select>
            <input v-model="selectedRequest.url" class="url-input" placeholder="https://api.example.com/users" @blur="syncParamsFromUrl(selectedRequest)" @keydown.enter="sendRequest" />
            <button class="send-button" :disabled="sending" @click="sendRequest">
              <span v-if="sending" class="spinner"></span><Send v-else :size="17" />{{ sending ? "发送中" : "发送" }}
            </button>
            <button class="batch-button" :disabled="sending" title="使用数据列表批量运行" @click="openBatchRunner"><span class="batch-flight-icon"><Send :size="14" /><Send :size="14" /></span></button>
          </div>

          <nav class="tabs" aria-label="请求配置">
            <button v-for="tab in [{id:'params',label:'参数'}, {id:'headers',label:'请求头'}, {id:'body',label:'Body'}, {id:'post',label:'Post Script'}]" :key="tab.id" :class="{ active: activeTab === tab.id }" @click="activeTab = tab.id">
              {{ tab.label }}
              <span v-if="tab.id === 'params'" class="tab-count">{{ selectedRequest.params.filter((item) => item.enabled && item.key).length }}</span>
              <span v-if="tab.id === 'headers'" class="tab-count">{{ selectedRequest.headers.filter((item) => item.enabled && item.key).length }}</span>
            </button>
          </nav>

          <div class="request-panel">
            <div v-if="activeTab === 'params' || activeTab === 'headers'" class="kv-editor">
              <div class="kv-head"><span></span><span>KEY</span><span>VALUE</span><span></span></div>
              <div v-for="(item, index) in activeTab === 'params' ? selectedRequest.params : selectedRequest.headers" :key="item.id" class="kv-row">
                <input v-model="item.enabled" type="checkbox" aria-label="启用" @change="activeTab === 'params' && syncUrlFromParams(selectedRequest)" />
                <input v-model="item.key" :data-row-key="item.id" placeholder="键" @input="activeTab === 'params' && syncUrlFromParams(selectedRequest)" />
                <div v-if="activeTab === 'params'" class="kv-value-editor" :class="{ json: usesJsonEditor(item) }">
                  <div v-if="usesJsonEditor(item)" class="kv-json-editor">
                    <ScriptEditor
                      :key="`param-json-${item.id}-${workspace.appearance.mode}`"
                      :model-value="item.value"
                      language="json"
                      :theme="workspace.appearance.mode"
                      @update:model-value="updateJsonValue(item, $event, selectedRequest)"
                    />
                  </div>
                  <textarea v-else v-model="item.value" rows="1" placeholder="值" @input="syncUrlFromParams(selectedRequest)"></textarea>
                </div>
                <input v-else v-model="item.value" placeholder="值" />
                <button class="row-action" title="删除" @click="removeRow(activeTab === 'params' ? selectedRequest.params : selectedRequest.headers, index); activeTab === 'params' && syncUrlFromParams(selectedRequest)"><X :size="14" /></button>
              </div>
              <button class="add-row" @click="addRow(activeTab === 'params' ? selectedRequest.params : selectedRequest.headers)"><Plus :size="14" />添加一行</button>
            </div>

            <div v-else-if="activeTab === 'body'" class="body-editor">
              <div class="body-toolbar">
                <div class="segmented">
                  <button v-for="type in [{id:'none',label:'None'}, {id:'json',label:'JSON'}, {id:'form',label:'x-www-form-urlencoded'}, {id:'formdata',label:'form-data'}, {id:'text',label:'Text'}]" :key="type.id" :class="{ active: selectedRequest.bodyType === type.id }" @click="selectedRequest.bodyType = type.id; bodyFormatError = ''">{{ type.label }}</button>
                </div>
                <div v-if="selectedRequest.bodyType === 'form' || selectedRequest.bodyType === 'formdata'" class="form-toolbar-actions">
                  <button class="format-button" title="导入表单数据" @click="formImportOpen = !formImportOpen; formImportError = ''"><ClipboardPaste :size="14" />导入</button>
                  <button class="format-button danger-button" title="清空全部表单参数" @click="clearFormData"><Trash2 :size="14" />清空参数</button>
                </div>
              </div>
              <div v-if="formImportOpen && (selectedRequest.bodyType === 'form' || selectedRequest.bodyType === 'formdata')" class="form-importer">
                <div class="form-import-heading"><span>批量导入参数</span><button class="icon-button small" title="关闭导入框" @click="formImportOpen = false; formImportError = ''"><X :size="15" /></button></div>
                <textarea v-model="formImportText" spellcheck="false" placeholder="粘贴 a=1&b=2、逐行 key=value，或 JSON 对象"></textarea>
                <div class="form-import-options">
                  <div class="segmented compact-segmented">
                    <button v-for="option in [{id:'auto',label:'自动识别'}, {id:'encoded',label:'URL 解码'}, {id:'raw',label:'保留原文'}]" :key="option.id" :class="{ active: formImportMode === option.id }" @click="formImportMode = option.id">{{ option.label }}</button>
                  </div>
                  <div class="segmented compact-segmented">
                    <button :class="{ active: formImportStrategy === 'replace' }" @click="formImportStrategy = 'replace'">替换</button>
                    <button :class="{ active: formImportStrategy === 'append' }" @click="formImportStrategy = 'append'">追加</button>
                  </div>
                  <button class="primary-button import-confirm" :disabled="!formImportText.trim()" @click="importFormData">生成表格</button>
                </div>
                <div v-if="formImportError" class="body-format-error">导入失败：{{ formImportError }}</div>
              </div>
              <div v-if="selectedRequest.bodyType === 'form' || selectedRequest.bodyType === 'formdata'" class="kv-editor form-editor">
                <div class="kv-head"><span></span><span>KEY</span><span>VALUE</span><span></span></div>
                <div v-for="(item, index) in selectedRequest.bodyType === 'form' ? selectedRequest.formBody : selectedRequest.formData" :key="item.id" class="kv-row">
                  <input v-model="item.enabled" type="checkbox" aria-label="启用" />
                  <input v-model="item.key" :data-row-key="item.id" placeholder="字段名" />
                  <div class="kv-value-editor" :class="{ json: usesJsonEditor(item) }">
                    <div v-if="usesJsonEditor(item)" class="kv-json-editor">
                      <ScriptEditor
                        :key="`form-json-${item.id}-${workspace.appearance.mode}`"
                        :model-value="item.value"
                        language="json"
                        :theme="workspace.appearance.mode"
                        @update:model-value="updateJsonValue(item, $event)"
                      />
                    </div>
                    <textarea v-else v-model="item.value" rows="1" placeholder="字段值"></textarea>
                  </div>
                  <button class="row-action" title="删除" @click="removeRow(selectedRequest.bodyType === 'form' ? selectedRequest.formBody : selectedRequest.formData, index)"><X :size="14" /></button>
                </div>
                <button class="add-row" @click="addRow(selectedRequest.bodyType === 'form' ? selectedRequest.formBody : selectedRequest.formData)"><Plus :size="14" />添加字段</button>
              </div>
              <ScriptEditor
                v-else-if="selectedRequest.bodyType === 'json'"
                :key="`body-json-${selectedRequest.id}-${workspace.appearance.mode}`"
                v-model="selectedRequest.body"
                language="json"
                :theme="workspace.appearance.mode"
              />
              <textarea v-else-if="selectedRequest.bodyType !== 'none'" v-model="selectedRequest.body" spellcheck="false" placeholder="请求体" @input="bodyFormatError = ''"></textarea>
              <div v-else class="empty-editor"><FileJson :size="26" /><span>此请求不发送 Body</span></div>
              <div v-if="bodyFormatError" class="body-format-error">JSON 格式错误：{{ bodyFormatError }}</div>
            </div>

            <div v-else-if="activeTab === 'post'" class="request-script-editor">
              <div class="editor-caption"><Code2 :size="15" /><span>Post-response JavaScript</span><span class="hint">收到当前请求响应后运行</span></div>
              <ScriptEditor
                :key="`request-post-${selectedRequest.id}-${workspace.appearance.mode}`"
                v-model="selectedRequest.postScript"
                :theme="workspace.appearance.mode"
                :variables="workspace.variables"
              />
            </div>

          </div>
        </section>

        <div class="request-resizer" title="拖动调整请求与响应区域高度" @pointerdown="startRequestResize" @dblclick="workspace.appearance.requestEditorHeight = 430"></div>

        <ResponseViewer v-model:tab="responseTab" v-model:fullscreen="responseFullscreen" :response="response" :error-message="errorMessage" :theme="workspace.appearance.mode" />
      </main>

      <main v-else class="no-selection"><div class="empty-logo">R</div><strong>还没有请求</strong><span>从左侧新建一个集合或请求</span><button class="send-button" @click="addRequest()"><Plus :size="17" />新建请求</button></main>
    </div>

    <div v-if="batchOpen" class="batch-backdrop" @click.self="!batchRunning && (batchOpen = false)">
      <section class="batch-runner" role="dialog" aria-modal="true" aria-label="批量运行">
        <header>
          <div><span class="eyebrow">DATA RUNNER</span><h2>批量运行</h2></div>
          <button class="icon-button" title="关闭" :disabled="batchRunning" @click="batchOpen = false"><X :size="19" /></button>
        </header>
        <div class="batch-content">
          <section class="batch-input-section">
            <div class="batch-section-heading">
              <div>
                <strong>迭代数据</strong>
                <span v-if="batchPlaceholders.length === 1">请求模板使用 <code v-pre>{{参数名}}</code><br />单参数可直接输入值数组，如 <code>[1, 2]</code></span>
                <span v-else>请求模板使用 <code v-pre>{{参数名}}</code><br />多参数请输入包含全部参数的对象数组</span>
              </div>
              <div class="batch-heading-actions">
                <span v-if="batchRunning" class="batch-progress">{{ batchProgress.current }} / {{ batchProgress.total }}</span>
              </div>
            </div>
            <div class="batch-placeholder-list">
              <div class="batch-placeholder-label"><strong>替换参数</strong><span>{{ batchPlaceholders.length }}</span></div>
              <div class="batch-placeholder-tags"><code v-for="name in batchPlaceholders" :key="name">{{ name }}</code></div>
            </div>
            <div class="batch-editor">
              <ScriptEditor
                :key="`batch-${selectedRequest.id}-${workspace.appearance.mode}`"
                v-model="selectedRequest.batchData"
                language="json"
                :theme="workspace.appearance.mode"
              />
            </div>
            <div v-if="batchError" class="body-format-error">数据格式错误：{{ batchError }}</div>
          </section>

          <section class="batch-results-section">
            <div class="batch-section-heading">
              <div><strong>执行结果</strong><span v-if="batchResults.length">{{ batchResults.filter((item) => item.status === 'completed').length }} 完成，{{ batchResults.filter((item) => item.status === 'failed').length }} 失败</span></div>
            </div>
            <div v-if="batchResults.length" class="batch-results">
              <div class="batch-result-head"><span>#</span><span>迭代数据</span><span>状态</span><span>耗时</span></div>
              <div v-for="item in batchResults" :key="item.id" class="batch-result-row" :class="{ selected: item.id === selectedBatchResultId }" @click="showBatchResponse(item)">
                <span>{{ item.index + 1 }}</span>
                <code :title="stringifyObject(item.data)">{{ stringifyObject(item.data) }}</code>
                <span v-if="item.status === 'running'" class="batch-status running"><span class="spinner"></span>执行中</span>
                <span v-else-if="item.status === 'failed'" class="batch-status failed" :title="item.error">失败</span>
                <span v-else class="batch-status" :class="item.response.status < 400 ? 'success' : 'failed'">HTTP {{ item.response.status }}</span>
                <span>{{ item.response ? `${item.response.durationMs} ms` : '—' }}</span>
              </div>
            </div>
            <div v-else class="batch-results-empty"><Send :size="26" /><span>运行后将在这里汇总每次请求</span></div>
            <ResponseViewer
              v-if="selectedBatchResult"
              v-model:tab="batchResponseTab"
              v-model:fullscreen="batchResponseFullscreen"
              class="batch-response-viewer"
              :response="selectedBatchResult.response"
              :error-message="selectedBatchResult.error"
              :theme="workspace.appearance.mode"
              empty-text="当前批次请求执行完成后，结果会显示在这里"
            />
          </section>
        </div>
        <footer>
          <span v-if="batchStopping" class="batch-stop-hint">当前请求完成后停止</span>
          <button v-if="batchRunning" class="dialog-button danger-confirm" :disabled="batchStopping" @click="stopBatchRun">{{ batchStopping ? '正在停止' : '停止' }}</button>
          <button v-else class="dialog-button" @click="batchOpen = false">关闭</button>
          <button class="primary-button" :disabled="batchRunning || !selectedRequest.batchData.trim()" @click="runBatch"><Send :size="16" />{{ batchResults.length ? '重新运行' : '开始运行' }}</button>
        </footer>
      </section>
    </div>

    <div v-if="settingsOpen" class="modal-backdrop" @click.self="settingsOpen = false">
      <div class="settings-resizer" title="拖动调整设置面板宽度" @pointerdown="startSettingsResize"></div>
      <section class="settings-panel" :style="{ width: `${workspace.appearance.settingsWidth || 520}px` }">
        <header><div><span class="eyebrow">WORKSPACE</span><h2>设置</h2></div><button class="icon-button" title="关闭" @click="settingsOpen = false"><X :size="19" /></button></header>
        <div class="settings-content">
          <div class="setting-section" :class="{ collapsed: collapsedSettings.theme }">
            <div class="setting-title"><button class="setting-title-toggle" @click="collapsedSettings.theme = !collapsedSettings.theme"><ChevronRight v-if="collapsedSettings.theme" :size="16" /><ChevronDown v-else :size="16" /><div><h3>主题模式</h3><p>默认使用柔和浅色主题</p></div></button></div>
            <div v-if="!collapsedSettings.theme" class="theme-mode-control">
              <button :class="{ active: workspace.appearance.mode === 'light' }" @click="workspace.appearance.mode = 'light'"><Sun :size="16" />浅色</button>
              <button :class="{ active: workspace.appearance.mode === 'dark' }" @click="workspace.appearance.mode = 'dark'"><Moon :size="16" />深色</button>
            </div>
          </div>
          <div class="setting-section" :class="{ collapsed: collapsedSettings.accent }">
            <div class="setting-title"><button class="setting-title-toggle" @click="collapsedSettings.accent = !collapsedSettings.accent"><ChevronRight v-if="collapsedSettings.accent" :size="16" /><ChevronDown v-else :size="16" /><div><h3>主题色</h3><p>不会改变请求方法的语义颜色</p></div></button></div>
            <div v-if="!collapsedSettings.accent" class="color-picker-row">
              <button
                v-for="color in themeColors"
                :key="color"
                class="color-swatch"
                :class="{ active: workspace.appearance.accent === color }"
                :style="{ backgroundColor: color }"
                :title="color"
                @click="workspace.appearance.accent = color"
              ><Check v-if="workspace.appearance.accent === color" :size="14" /></button>
              <label class="custom-color" title="自定义主题色">
                <input v-model="workspace.appearance.accent" type="color" />
                <span>自定义</span>
              </label>
            </div>
          </div>
          <div class="setting-section" :class="{ collapsed: collapsedSettings.mcp }">
            <div class="setting-title">
              <button class="setting-title-toggle" @click="collapsedSettings.mcp = !collapsedSettings.mcp"><ChevronRight v-if="collapsedSettings.mcp" :size="16" /><ChevronDown v-else :size="16" /><div><h3>MCP 服务</h3><p>允许本机 AI 客户端管理分组、创建并调用接口</p></div></button>
              <label class="switch" @click.stop><input v-model="workspace.mcp.enabled" type="checkbox" :disabled="mcpRuntime.applying" @change="applyMcpConfig" /><span></span></label>
            </div>
            <template v-if="!collapsedSettings.mcp">
            <div class="mcp-status-row" :class="{ running: mcpRuntime.running, error: mcpRuntime.error }">
              <span class="mcp-status-dot"></span>
              <strong>{{ mcpRuntime.applying ? '正在应用' : mcpRuntime.running ? '运行中' : '已关闭' }}</strong>
              <code>http://0.0.0.0:{{ workspace.mcp.port }}/mcp</code>
              <button class="mcp-address-trigger" :disabled="!workspace.mcp.enabled" :title="mcpAddressesOpen ? '收起地址' : '查看所有地址'" @click="toggleMcpAddresses"><Network :size="14" /><span>{{ mcpAddressesOpen ? '收起' : '地址' }}</span></button>
            </div>
            <div v-if="mcpAddressesOpen" class="mcp-address-list">
              <div v-for="address in mcpAddresses" :key="address" class="mcp-address-row">
                <code>{{ address }}</code>
                <button :title="copiedMcpAddress === address ? '已复制' : '复制地址'" @click="copyMcpAddress(address)"><Check v-if="copiedMcpAddress === address" :size="14" /><Copy v-else :size="14" /></button>
              </div>
            </div>
            <label class="field"><span>监听端口</span><input v-model.number="workspace.mcp.port" type="number" min="1024" max="65535" step="1" :disabled="mcpRuntime.applying" @change="workspace.mcp.enabled && applyMcpConfig()" /></label>
            <div v-if="mcpRuntime.error" class="mcp-setting-error">{{ mcpRuntime.error }}</div>
            </template>
          </div>
          <div class="setting-section" :class="{ collapsed: collapsedSettings.proxy }">
            <div class="setting-title"><button class="setting-title-toggle" @click="collapsedSettings.proxy = !collapsedSettings.proxy"><ChevronRight v-if="collapsedSettings.proxy" :size="16" /><ChevronDown v-else :size="16" /><div><h3>网络代理</h3><p>HTTP 与 HTTPS 请求均通过此代理发送</p></div></button><label class="switch" @click.stop><input v-model="workspace.proxy.enabled" type="checkbox" /><span></span></label></div>
            <template v-if="!collapsedSettings.proxy">
            <label class="field"><span>代理地址</span><input v-model="workspace.proxy.url" :disabled="!workspace.proxy.enabled" placeholder="http://127.0.0.1:7890" /></label>
            <div class="field-grid">
              <label class="field"><span>用户名</span><input v-model="workspace.proxy.username" :disabled="!workspace.proxy.enabled" autocomplete="off" /></label>
              <label class="field"><span>密码</span><input v-model="workspace.proxy.password" :disabled="!workspace.proxy.enabled" type="password" autocomplete="new-password" /></label>
            </div>
            </template>
          </div>
          <div class="setting-section variables-section" :class="{ collapsed: collapsedSettings.variables }">
            <div class="setting-title"><button class="setting-title-toggle" @click="collapsedSettings.variables = !collapsedSettings.variables"><ChevronRight v-if="collapsedSettings.variables" :size="16" /><ChevronDown v-else :size="16" /><div><h3>环境变量</h3><p>可直接添加全局变量，或创建命名空间后添加；支持 <code>token</code>、<code>dev.token</code>、<code>dev.wms.token</code>，最多三级</p></div></button></div>
            <div v-if="!collapsedSettings.variables" class="variable-tree">
              <div class="namespace-row root"><span>全局</span><button class="row-action" title="添加全局变量" @click="addVariableAt()"><Plus :size="14" /></button></div>
              <div v-for="item in variableHierarchy.roots" :key="item.id" class="kv-row variable-row level-0">
                <input v-model="item.enabled" type="checkbox" aria-label="启用" />
                <input :value="item.key" :class="{ invalid: !validVariableKey(item.key) }" placeholder="变量名" @change="renameVariableKey(item, $event.target.value)" />
                <input v-model="item.value" placeholder="值" />
                <button class="row-action" title="删除" @click="removeVariable(item)"><X :size="14" /></button>
              </div>

              <section v-for="group in variableHierarchy.groups" :key="group.path" class="namespace-group">
                <div class="namespace-row" @click="toggleNamespace(group.path)">
                  <ChevronRight v-if="collapsedNamespaces[group.path]" :size="14" /><ChevronDown v-else :size="14" />
                  <Folder :size="15" /><strong>{{ group.name }}</strong>
                  <button class="row-action" title="在此命名空间添加变量" @click.stop="addVariableAt(group.path)"><Plus :size="14" /></button>
                </div>
                <template v-if="!collapsedNamespaces[group.path]">
                  <div v-for="item in group.variables" :key="item.id" class="kv-row variable-row level-1">
                    <input v-model="item.enabled" type="checkbox" aria-label="启用" />
                    <input :value="variableLeafName(item)" placeholder="变量名" @change="renameVariableLeaf(item, group.path, $event.target.value)" />
                    <input v-model="item.value" placeholder="值" />
                    <button class="row-action" title="删除" @click="removeVariable(item)"><X :size="14" /></button>
                  </div>
                  <section v-for="subgroup in group.groups" :key="subgroup.path" class="namespace-group nested">
                    <div class="namespace-row level-1" @click="toggleNamespace(subgroup.path)">
                      <ChevronRight v-if="collapsedNamespaces[subgroup.path]" :size="14" /><ChevronDown v-else :size="14" />
                      <Folder :size="14" /><strong>{{ subgroup.name }}</strong>
                      <button class="row-action" title="在此命名空间添加变量" @click.stop="addVariableAt(subgroup.path)"><Plus :size="14" /></button>
                    </div>
                    <div v-if="!collapsedNamespaces[subgroup.path]">
                      <div v-for="item in subgroup.variables" :key="item.id" class="kv-row variable-row level-2">
                        <input v-model="item.enabled" type="checkbox" aria-label="启用" />
                        <input :value="variableLeafName(item)" placeholder="变量名" @change="renameVariableLeaf(item, subgroup.path, $event.target.value)" />
                        <input v-model="item.value" placeholder="值" />
                        <button class="row-action" title="删除" @click="removeVariable(item)"><X :size="14" /></button>
                      </div>
                    </div>
                  </section>
                </template>
              </section>

              <div class="environment-create">
                <div class="segmented compact-segmented environment-create-mode">
                  <button :class="{ active: environmentCreateMode === 'variable' }" @click="environmentCreateMode = 'variable'">直接赋值</button>
                  <button :class="{ active: environmentCreateMode === 'namespace' }" @click="environmentCreateMode = 'namespace'">命名空间</button>
                </div>
                <div v-if="environmentCreateMode === 'variable'" class="environment-create-fields direct-variable-fields">
                  <input v-model="variableDraftKey" :class="{ invalid: variableDraftKey.trim() && !validVariableKey(variableDraftKey.trim()) }" placeholder="变量名，如 token" @keydown.enter="addDirectVariable" />
                  <input v-model="variableDraftValue" placeholder="值" @keydown.enter="addDirectVariable" />
                  <button :disabled="!variableDraftKey.trim() || !validVariableKey(variableDraftKey.trim())" @click="addDirectVariable"><Plus :size="14" />添加变量</button>
                </div>
                <div v-else class="environment-create-fields namespace-fields">
                  <input v-model="namespaceDraft" placeholder="新命名空间，如 dev 或 dev.wms" @keydown.enter="addNamespace" />
                  <button :disabled="!namespaceDraft.trim()" @click="addNamespace"><FolderPlus :size="14" />添加命名空间</button>
                </div>
              </div>
            </div>
          </div>
        </div>
        <footer><button class="primary-button" @click="settingsOpen = false"><Check :size="16" />完成</button></footer>
      </section>
    </div>

    <div v-if="confirmDialog.open" class="confirm-backdrop" @click.self="resolveConfirm(false)">
      <form class="confirm-dialog" @submit.prevent="resolveConfirm(true)">
        <div class="confirm-icon"><AlertTriangle :size="20" /></div>
        <div class="confirm-copy"><h3>{{ confirmDialog.title }}</h3><p>{{ confirmDialog.message }}</p></div>
        <div class="confirm-actions">
          <button type="button" class="dialog-button" @click="resolveConfirm(false)">取消</button>
          <button type="submit" class="dialog-button danger-confirm">{{ confirmDialog.confirmLabel }}</button>
        </div>
      </form>
    </div>

    <section v-if="consoleOpen" class="global-console" :style="{ height: `${workspace.appearance.consoleHeight || 300}px` }">
      <div class="console-resizer" title="拖动调整 Console 高度" @pointerdown="startConsoleResize" @dblclick="workspace.appearance.consoleHeight = 300"></div>
      <header>
        <div><Terminal :size="16" /><strong>Console</strong><span>{{ scriptLogs.length }} 条</span></div>
        <div>
          <button class="console-command" :disabled="!scriptLogs.length" @click="scriptLogs = []"><Trash2 :size="14" />清空</button>
          <button class="icon-button small" title="关闭 Console" @click="consoleOpen = false"><X :size="16" /></button>
        </div>
      </header>
      <div v-if="scriptLogs.length" class="global-console-logs">
        <div v-for="entry in scriptLogs" :key="entry.id" class="global-console-line" :class="entry.level">
          <time>{{ entry.time }}</time>
          <span class="console-source">{{ entry.requestName }}</span>
          <span class="console-phase">{{ entry.phase }} · {{ entry.level }}</span>
          <code>{{ entry.args.map(formatLogArg).join(' ') }}</code>
        </div>
      </div>
      <div v-else class="global-console-empty">暂无日志</div>
    </section>
  </div>
</template>

<style src="./styles.css"></style>
