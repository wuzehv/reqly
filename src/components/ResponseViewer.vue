<script setup>
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import {
  AlertTriangle,
  ArrowDownLeft,
  ArrowUpRight,
  Braces,
  Check,
  ChevronDown,
  ChevronRight,
  CircleX,
  Clock3,
  Copy,
  ExternalLink,
  FileText,
  Gauge,
  Maximize2,
  Minimize2,
  Plus,
  Timer,
} from "@lucide/vue";
import ResponseCodeViewer from "./ResponseCodeViewer.vue";

const props = defineProps({
  response: { type: Object, default: null },
  errorMessage: { type: String, default: "" },
  tab: { type: String, default: "body" },
  fullscreen: { type: Boolean, default: false },
  emptyText: { type: String, default: "发送请求后，结果会显示在这里" },
  theme: { type: String, default: "light" },
  hideStatusAndDuration: { type: Boolean, default: false },
});
const emit = defineEmits(["update:tab", "update:fullscreen", "create-request-from-url", "open-url"]);
const copied = ref(false);
const collapsedJsonPaths = reactive({});
const urlMenu = reactive({ open: false, url: "", top: 0, left: 0 });
const responseUrlPattern = /https?:\/\/[^\s<>"'`]*[^\s<>"'`.,;:!?)}\]]/g;

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
const displayedResponseBody = computed(() => {
  const body = props.response?.body;
  if (typeof body !== "string" || parsedResponseJson.value.valid) return body;
  return body.replace(/(?<!\\)\\u([0-9a-f]{4})/gi, (_match, code) => String.fromCharCode(Number.parseInt(code, 16)));
});
const responseContentType = computed(() => {
  const header = props.response?.headers?.find(([key]) => key.toLowerCase() === "content-type");
  return String(header?.[1] || "").toLowerCase();
});
const responseFormat = computed(() => {
  if (!props.response) return "text";
  const contentType = responseContentType.value;
  const body = typeof props.response.body === "string" ? props.response.body.trimStart() : "";
  if (contentType.includes("html") || /^<!doctype\s+html|^<html[\s>]/i.test(body)) return "html";
  if (contentType.includes("xml") || /^<\?xml[\s>]/i.test(body) || /^<[A-Za-z_][\w:.-]*(?:\s|>)/.test(body)) return "xml";
  if (contentType.includes("javascript") || contentType.includes("ecmascript")) return "javascript";
  if (contentType.includes("css")) return "css";
  if (contentType.includes("json") || parsedResponseJson.value.valid) return "json";
  return "text";
});
const responseFormatLabel = computed(() => ({
  json: "JSON",
  html: "HTML",
  xml: "XML",
  javascript: "JavaScript",
  css: "CSS",
  text: "Text",
}[responseFormat.value]));
const requestHeaders = computed(() => props.response?.requestHeaders || []);
const rawResponse = computed(() => {
  if (!props.response) return "";
  const statusLine = props.response.statusLine
    || `HTTP ${props.response.status} ${props.response.statusText || ""}`.trimEnd();
  const headers = (props.response.headers || [])
    .map(([key, value]) => `${key}: ${value}`)
    .join("\r\n");
  const body = typeof props.response.body === "string"
    ? props.response.body
    : stringifyObject(props.response.body, 2);
  const request = props.response.rawRequest?.trimEnd() || "请求报文不可用";
  return `===== 请求报文 =====\r\n${request}\r\n\r\n===== 响应报文 =====\r\n${statusLine}\r\n${headers}\r\n\r\n${body}`;
});
const scriptWarnings = computed(() => props.response?.scriptWarnings || []);
const timingStages = computed(() => {
  const timings = props.response?.timings;
  if (!timings) return [];
  const definitions = [
    ["dnsMs", "DNS 查询", "域名解析"],
    ["tcpMs", "TCP 连接", "建立网络连接"],
    ["sslMs", "TLS 握手", "证书校验与密钥协商"],
    ["requestMs", "协议准备", "连接就绪到开始传输"],
    ["waitingMs", "等待首字节", "发送请求、服务器处理与网络等待"],
    ["downloadMs", "响应下载", "首字节到响应体接收完成"],
  ];
  let offset = 0;
  const total = Math.max(Number(timings.totalMs) || 0, 0.001);
  return definitions.map(([key, label, description], index) => {
    const raw = timings[key];
    const duration = raw === null || raw === undefined ? null : Math.max(Number(raw) || 0, 0);
    const stage = {
      key,
      label,
      description,
      duration,
      style: duration === null ? {} : {
        left: `${Math.min(offset / total * 100, 100)}%`,
        width: `${Math.min(duration / total * 100, 100)}%`,
      },
      color: index + 1,
    };
    if (duration !== null) offset += duration;
    return stage;
  });
});
const connectionDetails = computed(() => {
  const connection = props.response?.connection;
  if (!connection) return [];
  return [
    ["最终地址", connection.effectiveUrl],
    ["远端地址", connection.remoteAddress],
    ["本地地址", connection.localAddress],
  ].filter(([, value]) => value);
});
const displayedErrorMessage = computed(() => props.response
  ? props.errorMessage.replace(/^Post Script 执行失败[：:]\s*/, "")
  : props.errorMessage,
);
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

function jsonValueSegments(value) {
  const text = JSON.stringify(value);
  const segments = [];
  let cursor = 0;
  for (const match of text.matchAll(responseUrlPattern)) {
    if (match.index > cursor) segments.push({ type: "text", text: text.slice(cursor, match.index) });
    segments.push({ type: "url", text: match[0] });
    cursor = match.index + match[0].length;
  }
  if (cursor < text.length) segments.push({ type: "text", text: text.slice(cursor) });
  return segments;
}

function showUrlMenu({ url, x, y }) {
  urlMenu.url = url;
  urlMenu.left = Math.max(8, Math.min(x, window.innerWidth - 210));
  urlMenu.top = Math.max(8, Math.min(y, window.innerHeight - 92));
  urlMenu.open = true;
}

function showJsonUrlMenu(event, url) {
  event.stopPropagation();
  showUrlMenu({ url, x: event.clientX, y: event.clientY });
}

function createRequestFromUrl() {
  emit("create-request-from-url", urlMenu.url);
  urlMenu.open = false;
}

function openUrl() {
  emit("open-url", urlMenu.url);
  urlMenu.open = false;
}

function handleEscape(event) {
  if (event.key === "Escape") urlMenu.open = false;
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

function formatBytes(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function formatTiming(value) {
  if (value === null || value === undefined) return "不可用";
  if (value < 1) return `${value.toFixed(2)} ms`;
  if (value < 1000) return `${value.toFixed(value < 10 ? 2 : 1)} ms`;
  return `${(value / 1000).toFixed(2)} s`;
}

async function copyResponse() {
  if (!props.response) return;
  const content = activeTab.value === "raw"
    ? rawResponse.value
    : typeof displayedResponseBody.value === "string"
      ? displayedResponseBody.value
      : stringifyObject(displayedResponseBody.value, 2);
  await navigator.clipboard.writeText(content);
  copied.value = true;
  window.setTimeout(() => (copied.value = false), 1200);
}

watch(
  () => props.response?.body,
  () => Object.keys(collapsedJsonPaths).forEach((path) => delete collapsedJsonPaths[path]),
);

onMounted(() => window.addEventListener("keydown", handleEscape));
onBeforeUnmount(() => window.removeEventListener("keydown", handleEscape));
</script>

<template>
  <section class="response-viewer" :class="{ fullscreen: isFullscreen }">
    <div class="response-heading">
      <nav class="tabs response-tabs">
        <button :class="{ active: activeTab === 'body' }" @click="activeTab = 'body'"><Braces :size="14" />响应体</button>
        <button :class="{ active: activeTab === 'timing' }" @click="activeTab = 'timing'"><Timer :size="14" />耗时</button>
        <button :class="{ active: activeTab === 'request-headers' }" @click="activeTab = 'request-headers'"><ArrowUpRight :size="14" />请求头 <span v-if="response" class="tab-count">{{ requestHeaders.length }}</span></button>
        <button :class="{ active: activeTab === 'headers' }" @click="activeTab = 'headers'"><ArrowDownLeft :size="14" />响应头 <span v-if="response" class="tab-count">{{ response.headers.length }}</span></button>
        <button :class="{ active: activeTab === 'raw' }" @click="activeTab = 'raw'"><FileText :size="14" />原始报文</button>
      </nav>
      <div v-if="response" class="response-meta">
        <button v-if="activeTab === 'body' && responseFormat === 'json' && jsonHasContainers" class="json-view-command" :title="jsonRootCollapsed ? '全部展开' : '全部折叠'" @click="setAllJsonCollapsed(!jsonRootCollapsed)"><ChevronDown v-if="jsonRootCollapsed" :size="13" /><ChevronRight v-else :size="13" />{{ jsonRootCollapsed ? '展开' : '折叠' }}</button>
        <span v-if="activeTab === 'body'" class="response-format">{{ responseFormatLabel }}</span>
        <span v-if="!hideStatusAndDuration" class="response-status" :class="statusClass">{{ response.status }} {{ response.statusText }}</span>
        <span v-if="!hideStatusAndDuration" class="response-duration"><Clock3 :size="13" />{{ response.durationMs }} ms</span>
        <span><Gauge :size="13" />{{ formatBytes(response.size) }}</span>
        <div v-if="activeTab === 'body'" class="response-actions">
          <button class="icon-button small" :title="copied ? '已复制' : '复制响应'" @click="copyResponse"><Check v-if="copied" :size="15" /><Copy v-else :size="15" /></button>
          <button class="icon-button small" :title="isFullscreen ? '退出全屏' : '全屏查看'" @click="isFullscreen = !isFullscreen"><Minimize2 v-if="isFullscreen" :size="15" /><Maximize2 v-else :size="15" /></button>
        </div>
      </div>
    </div>
    <div v-if="scriptWarnings.length" class="pre-script-warning-state" :title="scriptWarnings.join('\n')"><AlertTriangle :size="14" /><strong>Pre Script 环境变量警告</strong><span>{{ scriptWarnings.join('；') }}</span></div>
    <div v-if="errorMessage && response" class="script-error-state"><CircleX :size="14" /><strong>Post Script 执行失败</strong><span>{{ displayedErrorMessage }}</span></div>
    <div v-if="errorMessage && !response" class="error-state"><strong>请求失败</strong><span>{{ displayedErrorMessage }}</span></div>
    <template v-else-if="response">
      <div v-if="activeTab === 'body' && responseFormat === 'json' && parsedResponseJson.valid" class="response-json-tree">
        <div v-for="row in jsonRows" :key="row.id" class="json-tree-row" :style="{ paddingLeft: `${12 + row.depth * 18}px` }">
          <template v-if="row.kind === 'open'">
            <button class="json-toggle" :title="row.collapsed ? '展开' : '折叠'" @click="toggleJsonPath(row.path)"><ChevronRight v-if="row.collapsed" :size="14" /><ChevronDown v-else :size="14" /></button>
            <span v-if="row.key !== null" class="json-key">{{ JSON.stringify(row.key) }}: </span><span class="json-bracket">{{ row.array ? '[' : '{' }}</span>
            <template v-if="row.collapsed"><span class="json-ellipsis"> … </span><span class="json-bracket">{{ row.array ? ']' : '}' }}{{ row.last ? '' : ',' }}</span></template><span class="json-count">{{ row.count }} {{ row.array ? '项' : '字段' }}</span>
          </template>
          <template v-else-if="row.kind === 'close'"><span class="json-close-spacer"></span><span class="json-bracket">{{ row.array ? ']' : '}' }}{{ row.last ? '' : ',' }}</span></template>
          <template v-else><span class="json-close-spacer"></span><span v-if="row.key !== null" class="json-key">{{ JSON.stringify(row.key) }}: </span><span v-if="typeof row.value === 'string'" class="json-string"><template v-for="(segment, index) in jsonValueSegments(row.value)" :key="index"><button v-if="segment.type === 'url'" class="response-url-link" @click.stop="showJsonUrlMenu($event, segment.text)">{{ segment.text }}</button><span v-else>{{ segment.text }}</span></template></span><span v-else :class="jsonValueClass(row.value)">{{ jsonValueText(row.value) }}</span><span>{{ row.last ? '' : ',' }}</span></template>
        </div>
      </div>
      <ResponseCodeViewer v-else-if="activeTab === 'body'" :key="`${responseFormat}-${theme}`" :content="displayedResponseBody" :language="responseFormat" :theme="theme" @url-click="showUrlMenu" />
      <ResponseCodeViewer v-else-if="activeTab === 'raw'" :key="`raw-${theme}`" :content="rawResponse" language="text" :theme="theme" @url-click="showUrlMenu" />
      <div v-else-if="activeTab === 'timing'" class="response-timing">
        <div v-if="response.timings" class="timing-summary">
          <div><span>总耗时</span><strong>{{ formatTiming(response.timings.totalMs) }}</strong></div>
          <div><span>首字节</span><strong>{{ formatTiming(response.timings.totalMs - response.timings.downloadMs) }}</strong></div>
          <div><span>重定向</span><strong>{{ response.timings.redirectCount ? `${response.timings.redirectCount} 次 / ${formatTiming(response.timings.redirectMs)}` : '无' }}</strong></div>
        </div>
        <div v-if="timingStages.length" class="timing-waterfall">
          <div v-for="stage in timingStages" :key="stage.key" class="timing-row">
            <div class="timing-label"><strong>{{ stage.label }}</strong><span>{{ stage.description }}</span></div>
            <div class="timing-track"><span v-if="stage.duration !== null" :class="`timing-color-${stage.color}`" :style="stage.style"></span></div>
            <code>{{ formatTiming(stage.duration) }}</code>
          </div>
        </div>
        <div v-else class="timing-unavailable">当前运行模式无法读取底层网络阶段耗时。</div>
        <div v-if="connectionDetails.length" class="timing-connection">
          <h3>连接信息</h3>
          <div v-for="([label, value]) in connectionDetails" :key="label"><strong>{{ label }}</strong><span>{{ value }}</span></div>
        </div>
      </div>
      <div v-else-if="activeTab === 'request-headers'" class="response-headers"><div v-for="([key, value], index) in requestHeaders" :key="`${key}-${index}`"><strong>{{ key }}</strong><span>{{ value }}</span></div></div>
      <div v-else class="response-headers"><div v-for="([key, value], index) in response.headers" :key="`${key}-${index}`"><strong>{{ key }}</strong><span>{{ value }}</span></div></div>
    </template>
    <div v-else class="response-empty"><Braces :size="30" /><strong>等待响应</strong><span>{{ emptyText }}</span></div>
    <Teleport to="body">
      <button v-if="urlMenu.open" class="response-url-backdrop" aria-label="关闭 URL 操作菜单" @click="urlMenu.open = false"></button>
      <div v-if="urlMenu.open" class="response-url-menu" :class="`theme-${theme}`" :style="{ top: `${urlMenu.top}px`, left: `${urlMenu.left}px` }">
        <div :title="urlMenu.url">{{ urlMenu.url }}</div>
        <button @click="createRequestFromUrl"><Plus :size="14" />创建接口</button>
        <button @click="openUrl"><ExternalLink :size="14" />浏览器打开</button>
      </div>
    </Teleport>
  </section>
</template>
