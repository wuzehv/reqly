<script setup>
import { computed, reactive, ref, watch } from "vue";
import {
  Braces,
  Check,
  ChevronDown,
  ChevronRight,
  Clock3,
  Copy,
  Gauge,
  Maximize2,
  Minimize2,
} from "@lucide/vue";

const props = defineProps({
  response: { type: Object, default: null },
  errorMessage: { type: String, default: "" },
  tab: { type: String, default: "body" },
  fullscreen: { type: Boolean, default: false },
  emptyText: { type: String, default: "发送请求后，结果会显示在这里" },
});
const emit = defineEmits(["update:tab", "update:fullscreen"]);
const copied = ref(false);
const collapsedJsonPaths = reactive({});

const activeTab = computed({
  get: () => props.tab,
  set: (value) => emit("update:tab", value),
});
const isFullscreen = computed({
  get: () => props.fullscreen,
  set: (value) => emit("update:fullscreen", value),
});
const statusClass = computed(() => {
  if (!props.response) return "";
  if (props.response.status < 300) return "status-ok";
  if (props.response.status < 400) return "status-warn";
  return "status-error";
});
const parsedResponseJson = computed(() => {
  if (!props.response) return { valid: false, value: null };
  try {
    const value = typeof props.response.body === "string"
      ? JSON.parse(props.response.body)
      : props.response.body;
    return { valid: true, value };
  } catch {
    return { valid: false, value: null };
  }
});
const jsonHasContainers = computed(() =>
  parsedResponseJson.value.valid
    && parsedResponseJson.value.value !== null
    && typeof parsedResponseJson.value.value === "object",
);
const jsonRootCollapsed = computed(() => Boolean(collapsedJsonPaths.$));
const jsonRows = computed(() => {
  if (!parsedResponseJson.value.valid) return [];
  const rows = [];
  const walk = (value, key, path, depth, last) => {
    const container = value !== null && typeof value === "object";
    if (!container) {
      rows.push({ id: path, kind: "value", key, value, depth, last });
      return;
    }
    const array = Array.isArray(value);
    const entries = Object.entries(value);
    const collapsed = Boolean(collapsedJsonPaths[path]);
    rows.push({ id: `${path}:open`, kind: "open", key, path, depth, last, array, count: entries.length, collapsed });
    if (collapsed) return;
    entries.forEach(([childKey, childValue], index) => {
      const escapedKey = childKey.replaceAll("~", "~0").replaceAll("/", "~1");
      walk(childValue, array ? null : childKey, `${path}/${escapedKey}`, depth + 1, index === entries.length - 1);
    });
    rows.push({ id: `${path}:close`, kind: "close", depth, last, array });
  };
  walk(parsedResponseJson.value.value, null, "$", 0, true);
  return rows;
});

function jsonValueText(value) {
  return typeof value === "string" ? JSON.stringify(value) : String(value);
}

function jsonValueClass(value) {
  if (value === null) return "json-null";
  return `json-${typeof value}`;
}

function toggleJsonPath(path) {
  collapsedJsonPaths[path] = !collapsedJsonPaths[path];
}

function setAllJsonCollapsed(collapsed) {
  const visit = (value, path) => {
    if (value === null || typeof value !== "object") return;
    collapsedJsonPaths[path] = collapsed;
    Object.entries(value).forEach(([key, child]) => {
      const escapedKey = key.replaceAll("~", "~0").replaceAll("/", "~1");
      visit(child, `${path}/${escapedKey}`);
    });
  };
  visit(parsedResponseJson.value.value, "$");
}

function escapeHtml(value) {
  return String(value).replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");
}

function stringifyObject(value, spacing = 0) {
  const visited = new WeakSet();
  const result = JSON.stringify(value, (_key, item) => {
    if (typeof item === "bigint") return item.toString();
    if (typeof item === "function") return `[Function: ${item.name || "anonymous"}]`;
    if (item && typeof item === "object") {
      if (visited.has(item)) return "[Circular]";
      visited.add(item);
    }
    return item;
  }, spacing);
  return result === undefined ? String(value) : result;
}

function highlightBody(body) {
  let content;
  if (typeof body !== "string") content = stringifyObject(body, 2);
  else {
    try {
      content = JSON.stringify(JSON.parse(body), null, 2);
    } catch {
      return escapeHtml(body);
    }
  }
  const tokenPattern = /("(?:\\.|[^"\\])*")(?=\s*:)|("(?:\\.|[^"\\])*")|\b(true|false)\b|\bnull\b|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/g;
  let html = "";
  let offset = 0;
  for (const match of content.matchAll(tokenPattern)) {
    html += escapeHtml(content.slice(offset, match.index));
    const token = match[0];
    const type = match[1] ? "key" : match[2] ? "string" : token === "true" || token === "false" ? "boolean" : token === "null" ? "null" : "number";
    html += `<span class="json-${type}">${escapeHtml(token)}</span>`;
    offset = match.index + token.length;
  }
  return html + escapeHtml(content.slice(offset));
}

function formatBytes(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

async function copyResponse() {
  if (!props.response) return;
  const content = typeof props.response.body === "string"
    ? props.response.body
    : stringifyObject(props.response.body, 2);
  await navigator.clipboard.writeText(content);
  copied.value = true;
  window.setTimeout(() => (copied.value = false), 1200);
}

watch(
  () => props.response?.body,
  () => Object.keys(collapsedJsonPaths).forEach((path) => delete collapsedJsonPaths[path]),
);
</script>

<template>
  <section class="response-viewer" :class="{ fullscreen: isFullscreen }">
    <div class="response-heading">
      <nav class="tabs response-tabs">
        <button :class="{ active: activeTab === 'body' }" @click="activeTab = 'body'">响应体</button>
        <button :class="{ active: activeTab === 'headers' }" @click="activeTab = 'headers'">响应头 <span v-if="response" class="tab-count">{{ response.headers.length }}</span></button>
      </nav>
      <div v-if="response" class="response-meta">
        <button v-if="activeTab === 'body' && jsonHasContainers" class="json-view-command" :title="jsonRootCollapsed ? '全部展开' : '全部折叠'" @click="setAllJsonCollapsed(!jsonRootCollapsed)"><ChevronDown v-if="jsonRootCollapsed" :size="13" /><ChevronRight v-else :size="13" />{{ jsonRootCollapsed ? '展开' : '折叠' }}</button>
        <span :class="statusClass">{{ response.status }} {{ response.statusText }}</span>
        <span><Clock3 :size="13" />{{ response.durationMs }} ms</span>
        <span><Gauge :size="13" />{{ formatBytes(response.size) }}</span>
        <button class="icon-button small" :title="copied ? '已复制' : '复制响应'" @click="copyResponse"><Check v-if="copied" :size="15" /><Copy v-else :size="15" /></button>
        <button class="icon-button small" :title="isFullscreen ? '退出全屏' : '全屏查看'" @click="isFullscreen = !isFullscreen"><Minimize2 v-if="isFullscreen" :size="15" /><Maximize2 v-else :size="15" /></button>
      </div>
    </div>
    <div v-if="errorMessage" class="error-state"><strong>请求失败</strong><span>{{ errorMessage }}</span></div>
    <template v-else-if="response">
      <div v-if="activeTab === 'body' && parsedResponseJson.valid" class="response-json-tree">
        <div v-for="row in jsonRows" :key="row.id" class="json-tree-row" :style="{ paddingLeft: `${12 + row.depth * 18}px` }">
          <template v-if="row.kind === 'open'">
            <button class="json-toggle" :title="row.collapsed ? '展开' : '折叠'" @click="toggleJsonPath(row.path)"><ChevronRight v-if="row.collapsed" :size="14" /><ChevronDown v-else :size="14" /></button>
            <span v-if="row.key !== null" class="json-key">{{ JSON.stringify(row.key) }}: </span><span class="json-bracket">{{ row.array ? '[' : '{' }}</span>
            <template v-if="row.collapsed"><span class="json-ellipsis"> … </span><span class="json-bracket">{{ row.array ? ']' : '}' }}{{ row.last ? '' : ',' }}</span></template><span class="json-count">{{ row.count }} {{ row.array ? '项' : '字段' }}</span>
          </template>
          <template v-else-if="row.kind === 'close'"><span class="json-close-spacer"></span><span class="json-bracket">{{ row.array ? ']' : '}' }}{{ row.last ? '' : ',' }}</span></template>
          <template v-else><span class="json-close-spacer"></span><span v-if="row.key !== null" class="json-key">{{ JSON.stringify(row.key) }}: </span><span :class="jsonValueClass(row.value)">{{ jsonValueText(row.value) }}</span><span>{{ row.last ? '' : ',' }}</span></template>
        </div>
      </div>
      <pre v-else-if="activeTab === 'body'" class="response-body" v-html="highlightBody(response.body)"></pre>
      <div v-else class="response-headers"><div v-for="([key, value], index) in response.headers" :key="`${key}-${index}`"><strong>{{ key }}</strong><span>{{ value }}</span></div></div>
    </template>
    <div v-else class="response-empty"><Braces :size="30" /><strong>等待响应</strong><span>{{ emptyText }}</span></div>
  </section>
</template>
