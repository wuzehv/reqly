<script setup>
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { css } from "@codemirror/lang-css";
import { html } from "@codemirror/lang-html";
import { javascript } from "@codemirror/lang-javascript";
import { json } from "@codemirror/lang-json";
import { xml } from "@codemirror/lang-xml";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { tags } from "@lezer/highlight";
import { oneDark } from "@codemirror/theme-one-dark";
import { basicSetup } from "codemirror";

const props = defineProps({
  content: { type: [String, Object, Array, Number, Boolean], default: "" },
  language: { type: String, default: "text" },
  theme: { type: String, default: "light" },
});
const host = ref(null);
let view;

const lightHighlight = HighlightStyle.define([
  { tag: [tags.tagName, tags.typeName, tags.className], color: "#7952a8" },
  { tag: [tags.attributeName, tags.propertyName], color: "#9a4d19" },
  { tag: [tags.string, tags.attributeValue], color: "#287a42" },
  { tag: [tags.number, tags.bool, tags.null], color: "#1769a6" },
  { tag: [tags.keyword, tags.controlKeyword], color: "#7442a8", fontWeight: "600" },
  { tag: [tags.comment, tags.meta], color: "#77848b", fontStyle: "italic" },
  { tag: [tags.bracket, tags.punctuation, tags.operator], color: "#59666f" },
]);

function contentText(value) {
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value ?? "");
  }
}

function languageExtension(language) {
  if (language === "html") return html();
  if (language === "xml") return xml();
  if (language === "json") return json();
  if (language === "javascript") return javascript();
  if (language === "css") return css();
  return [];
}

onMounted(() => {
  const dark = props.theme === "dark";
  view = new EditorView({
    parent: host.value,
    state: EditorState.create({
      doc: contentText(props.content),
      extensions: [
        basicSetup,
        EditorState.readOnly.of(true),
        EditorView.editable.of(false),
        languageExtension(props.language),
        ...(dark ? [oneDark] : [syntaxHighlighting(lightHighlight)]),
        EditorView.lineWrapping,
        EditorView.theme({
          "&": { height: "100%", color: dark ? "#d7dce1" : "#29343c", backgroundColor: dark ? "#202327" : "#f6f7f7" },
          ".cm-scroller": { fontFamily: '"Cascadia Code", "Cascadia Mono", Consolas, monospace', fontSize: "13px", lineHeight: "1.65" },
          ".cm-content": { padding: "10px 0 22px" },
          ".cm-gutters": { color: dark ? "#737c84" : "#8a949c", backgroundColor: dark ? "#1e2124" : "#eef1f2", borderRight: `1px solid ${dark ? "#30353a" : "#d5dde1"}` },
          ".cm-activeLine, .cm-activeLineGutter": { backgroundColor: "transparent" },
          ".cm-cursor": { display: "none" },
        }),
      ],
    }),
  });
});

watch(
  () => props.content,
  (content) => {
    if (!view) return;
    const next = contentText(content);
    if (next === view.state.doc.toString()) return;
    view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: next } });
  },
);

onBeforeUnmount(() => view?.destroy());
</script>

<template>
  <div ref="host" class="response-code-viewer"></div>
</template>
