<script setup>
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { Braces } from "@lucide/vue";
import beautify from "js-beautify";
import { autocompletion, snippetCompletion } from "@codemirror/autocomplete";
import { HighlightStyle, indentOnInput, indentUnit, syntaxHighlighting } from "@codemirror/language";
import { javascript } from "@codemirror/lang-javascript";
import { json } from "@codemirror/lang-json";
import { oneDark } from "@codemirror/theme-one-dark";
import { basicSetup } from "codemirror";
import { indentWithTab } from "@codemirror/commands";
import { EditorState, Prec } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { tags } from "@lezer/highlight";

const props = defineProps({
  modelValue: { type: String, default: "" },
  theme: { type: String, default: "light" },
  variables: { type: Array, default: () => [] },
  language: { type: String, default: "javascript" },
  showToolbar: { type: Boolean, default: true },
});
const emit = defineEmits(["update:modelValue"]);
const host = ref(null);
let view;

function formatCode() {
  if (!view) return;
  const source = view.state.doc.toString();
  let formatted;
  if (props.language === "json") {
    try {
      formatted = JSON.stringify(JSON.parse(source), null, 2);
    } catch {
      return;
    }
  } else {
    formatted = beautify.js(source, {
      indent_size: 2,
      indent_with_tabs: false,
      preserve_newlines: true,
      max_preserve_newlines: 2,
      brace_style: "collapse",
      end_with_newline: false,
    });
  }
  if (formatted === source) return;
  const cursor = Math.min(view.state.selection.main.head, formatted.length);
  view.dispatch({
    changes: { from: 0, to: view.state.doc.length, insert: formatted },
    selection: { anchor: cursor },
  });
  view.focus();
}

const lightHighlight = HighlightStyle.define([
  { tag: [tags.keyword, tags.controlKeyword, tags.moduleKeyword], color: "#7442a8", fontWeight: "600" },
  { tag: [tags.name, tags.variableName], color: "#263b4a" },
  { tag: [tags.definition(tags.variableName), tags.function(tags.variableName)], color: "#1769a6" },
  { tag: [tags.propertyName, tags.attributeName], color: "#a04f18" },
  { tag: [tags.string, tags.special(tags.string)], color: "#287a42" },
  { tag: [tags.number, tags.bool, tags.null], color: "#b35808" },
  { tag: [tags.operator, tags.punctuation], color: "#59666f" },
  { tag: [tags.comment, tags.lineComment, tags.blockComment], color: "#77848b", fontStyle: "italic" },
  { tag: [tags.typeName, tags.className], color: "#7650a7" },
  { tag: tags.invalid, color: "#bd343c", textDecoration: "underline" },
]);

const variableMethods = [
  { label: "get", type: "function", detail: "get(name)", apply: 'get("${name}")' },
  { label: "set", type: "function", detail: "set(name, value)", apply: 'set("${name}", "${value}")' },
  { label: "unset", type: "function", detail: "unset(name)", apply: 'unset("${name}")' },
  { label: "has", type: "function", detail: "has(name)", apply: 'has("${name}")' },
  { label: "replaceIn", type: "function", detail: "replaceIn(template)", apply: 'replaceIn("{{${name}}}")' },
];

const headerMethods = [
  { label: "get", type: "function", detail: "get(name)", apply: 'get("${name}")' },
  { label: "has", type: "function", detail: "has(name)", apply: 'has("${name}")' },
  { label: "remove", type: "function", detail: "remove(name)", apply: 'remove("${name}")' },
  { label: "add", type: "function", detail: "add({ key, value })", apply: 'add({ key: "${key}", value: "${value}" })' },
  { label: "upsert", type: "function", detail: "upsert({ key, value })", apply: 'upsert({ key: "${key}", value: "${value}" })' },
];

const propertyListMethods = [
  { label: "each", type: "function", detail: "each(callback)", apply: "each((${item}) => {\n\t${body}\n})" },
  { label: "upsert", type: "function", detail: "upsert({ key, value })", apply: 'upsert({ key: "${key}", value: "${value}" })' },
  { label: "add", type: "function", detail: "add({ key, value })", apply: 'add({ key: "${key}", value: "${value}" })' },
  { label: "remove", type: "function", detail: "remove(key)", apply: 'remove("${key}")' },
  { label: "get", type: "function", detail: "get(key)", apply: 'get("${key}")' },
  { label: "all", type: "function", detail: "all()", apply: "all()" },
];

function asSnippets(items) {
  return items.map(({ apply, ...info }) => snippetCompletion(apply, info));
}

function completionResult(context, match, options) {
  const lastDot = match.text.lastIndexOf(".");
  return {
    from: lastDot >= 0 ? match.from + lastDot + 1 : match.from,
    options,
    validFor: /^\w*$/,
  };
}

function pmCompletions(context) {
  const scopeString = context.matchBefore(/pm\.(?:environment|variables)\.(?:get|set)\(\s*["'][^"']*$/);
  if (scopeString) {
    const quoteIndex = Math.max(scopeString.text.lastIndexOf('"'), scopeString.text.lastIndexOf("'"));
    return {
      from: scopeString.from + quoteIndex + 1,
      options: props.variables
        .filter((item) => item.enabled && item.key)
        .map((item) => ({ label: item.key, type: "constant", detail: item.value || "环境变量" })),
      validFor: /^[^"']*$/,
    };
  }

  const match = context.matchBefore(/[$\w.]*$/);
  if (!match || (!context.explicit && !match.text)) return null;
  const text = match.text;

  if (/^pm\.(environment|variables)\.\w*$/.test(text)) {
    return completionResult(context, match, asSnippets(variableMethods));
  }
  if (/^env(?:\.[$\w]*){1,3}$/.test(text)) {
    const typedParts = text.slice(4).split(".");
    const partial = typedParts.pop();
    const parent = typedParts;
    const options = new Map();
    for (const variable of props.variables.filter((item) => item.enabled && item.key)) {
      const parts = variable.key.split(".");
      if (parts.length > 3 || parent.some((part, index) => parts[index] !== part)) continue;
      const next = parts[parent.length];
      if (!next || !next.startsWith(partial)) continue;
      options.set(next, {
        label: next,
        type: parts.length > parent.length + 1 ? "namespace" : "constant",
        detail: parts.length > parent.length + 1 ? `${[...parent, next].join(".")}.*` : variable.value || variable.key,
      });
    }
    return completionResult(context, match, [...options.values()]);
  }
  if (/^pm\.request\.headers\.\w*$/.test(text)) {
    return completionResult(context, match, asSnippets(headerMethods));
  }
  if (/^pm\.response\.headers\.\w*$/.test(text)) {
    return completionResult(context, match, asSnippets(headerMethods));
  }
  if (/^pm\.request\.body\.(urlencoded|formdata)\.\w*$/.test(text)) {
    return completionResult(context, match, asSnippets(propertyListMethods));
  }
  if (/^pm\.request\.body\.\w*$/.test(text)) {
    return completionResult(context, match, [
      { label: "mode", type: "property", detail: "raw | urlencoded | formdata" },
      { label: "raw", type: "property", detail: "原始请求体" },
      { label: "urlencoded", type: "property", detail: "URL-encoded 参数列表" },
      { label: "formdata", type: "property", detail: "Form-data 参数列表" },
    ]);
  }
  if (/^CryptoJS\.enc\.\w*$/.test(text)) {
    return completionResult(context, match, ["Hex", "Base64", "Utf8"].map((label) => ({ label, type: "class" })));
  }
  if (/^CryptoJS\.\w*$/.test(text)) {
    return completionResult(context, match, [
      { label: "SHA256", type: "function", detail: "SHA-256" },
      { label: "HmacSHA256", type: "function", detail: "HMAC SHA-256" },
      { label: "MD5", type: "function", detail: "MD5" },
      { label: "enc", type: "property", detail: "编码器" },
    ]);
  }
  if (/^pm\.request\.\w*$/.test(text)) {
    return completionResult(context, match, [
      { label: "method", type: "property", detail: "请求方法" },
      { label: "url", type: "property", detail: "请求 URL" },
      { label: "body", type: "property", detail: "请求体" },
      { label: "headers", type: "property", detail: "请求头 API" },
    ]);
  }
  if (/^pm\.response\.\w*$/.test(text)) {
    return completionResult(context, match, [
      { label: "code", type: "property", detail: "HTTP 状态码" },
      { label: "status", type: "property", detail: "HTTP 状态文本" },
      { label: "headers", type: "property", detail: "响应头" },
      { label: "json", type: "function", detail: "json()" },
      { label: "text", type: "function", detail: "text()" },
      { label: "responseTime", type: "property", detail: "响应耗时" },
    ]);
  }
  if (/^pm\.execution\.\w*$/.test(text)) {
    return completionResult(context, match, [
      snippetCompletion("setNextRequest(pm.info.requestName)", {
        label: "setNextRequest",
        type: "function",
        detail: "重新执行当前请求",
      }),
      { label: "skipRequest", type: "function", detail: "跳过当前请求" },
    ]);
  }
  if (/^pm\.info\.\w*$/.test(text)) {
    return completionResult(context, match, [
      { label: "eventName", type: "property", detail: "prerequest | test" },
      { label: "requestName", type: "property", detail: "当前请求名称" },
      { label: "requestId", type: "property", detail: "当前请求 ID" },
    ]);
  }
  if (/^pm\.\w*$/.test(text)) {
    return completionResult(context, match, [
      { label: "environment", type: "variable", detail: "持久化环境变量" },
      { label: "variables", type: "variable", detail: "当前变量 API" },
      { label: "request", type: "variable", detail: "当前请求" },
      { label: "sendRequest", type: "function", detail: "发送辅助请求" },
      { label: "response", type: "variable", detail: "当前响应（Post-response）" },
      { label: "execution", type: "variable", detail: "执行控制" },
      { label: "info", type: "variable", detail: "脚本上下文" },
    ]);
  }

  return {
    from: match.from,
    options: [
      snippetCompletion('pm.environment.get("${name}")', {
        label: "pm.environment.get",
        type: "function",
        detail: "读取环境变量",
      }),
      snippetCompletion('pm.environment.set("${name}", "${value}");', {
        label: "pm.environment.set",
        type: "function",
        detail: "保存环境变量",
      }),
      snippetCompletion('pm.request.headers.upsert({ key: "${key}", value: "${value}" });', {
        label: "pm.request.headers.upsert",
        type: "function",
        detail: "添加或更新请求头",
      }),
      snippetCompletion("env.${variable}", {
        label: "env.variable",
        type: "variable",
        detail: "直接引用环境变量",
      }),
      { label: "CryptoJS", type: "class", detail: "Postman 加密兼容 API" },
    ],
    validFor: /^[\w.]*$/,
  };
}

onMounted(() => {
  const dark = props.theme === "dark";
  view = new EditorView({
    parent: host.value,
    state: EditorState.create({
      doc: props.modelValue,
      extensions: [
        basicSetup,
        Prec.highest(keymap.of([indentWithTab])),
        EditorState.tabSize.of(2),
        indentUnit.of("  "),
        indentOnInput(),
        props.language === "json" ? json() : javascript(),
        ...(dark ? [oneDark] : []),
        ...(!dark ? [syntaxHighlighting(lightHighlight)] : []),
        ...(props.language === "javascript"
          ? [autocompletion({ override: [pmCompletions], activateOnTyping: true })]
          : []),
        EditorView.lineWrapping,
        EditorView.updateListener.of((update) => {
          if (update.docChanged) emit("update:modelValue", update.state.doc.toString());
        }),
        EditorView.theme({
          "&": { height: "100%", color: dark ? "#d7dce1" : "#29343c", backgroundColor: dark ? "#16181a" : "#f3f5f5" },
          ".cm-scroller": { fontFamily: '"Cascadia Code", "Cascadia Mono", Consolas, monospace', fontSize: "13px" },
          ".cm-content": { caretColor: dark ? "#d7dce1" : "#29343c", tabSize: "2", padding: "8px 0" },
          ".cm-gutters": { color: dark ? "#7f8790" : "#8a949c", backgroundColor: dark ? "#191b1e" : "#e8ecee", borderRight: `1px solid ${dark ? "#30343a" : "#cbd2d7"}` },
          ".cm-activeLine, .cm-activeLineGutter": { backgroundColor: dark ? "#202328" : "#e4e9ec" },
          "&.cm-focused .cm-selectionBackground, .cm-selectionBackground": { backgroundColor: dark ? "#46617c" : "#b8d7f0" },
          ".cm-content ::selection": { backgroundColor: dark ? "#46617c" : "#b8d7f0" },
          ".cm-tooltip": { color: dark ? "#d7dce1" : "#29343c", backgroundColor: dark ? "#24272b" : "#f3f5f5", border: `1px solid ${dark ? "#464c54" : "#b9c2c9"}` },
          ".cm-tooltip-autocomplete ul li[aria-selected]": { color: dark ? "#f0f2f3" : "#24303a", backgroundColor: dark ? "#39414a" : "#d9e3ea" },
        }),
      ],
    }),
  });
});

watch(
  () => props.modelValue,
  (value) => {
    if (!view || value === view.state.doc.toString()) return;
    view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: value } });
  },
);

onBeforeUnmount(() => view?.destroy());
</script>

<template>
  <div class="script-editor-shell">
    <div v-if="showToolbar" class="script-editor-toolbar">
      <button class="format-button" :title="language === 'json' ? '格式化 JSON' : '格式化 JavaScript'" @click="formatCode"><Braces :size="14" />格式化</button>
    </div>
    <div ref="host" class="script-editor-host"></div>
  </div>
</template>
