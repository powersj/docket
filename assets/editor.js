// Markdown editor helpers for textarea.md-editor. Edits go through
// execCommand('insertText') so native undo works and an `input` event fires.
(function () {
  function isEditor(el) {
    return el && el.tagName === "TEXTAREA" && el.classList.contains("md-editor");
  }
  function insert(el, text, selectFrom, selectTo) {
    el.focus();
    const start = el.selectionStart;
    document.execCommand("insertText", false, text);
    if (selectFrom !== undefined) {
      el.setSelectionRange(start + selectFrom, start + (selectTo === undefined ? selectFrom : selectTo));
    }
  }
  function wrap(el, before, after, placeholder) {
    const sel = el.value.slice(el.selectionStart, el.selectionEnd) || placeholder;
    insert(el, before + sel + after, before.length, before.length + sel.length);
  }

  document.addEventListener("keydown", function (e) {
    const el = e.target;
    if (!isEditor(el)) return;
    const mod = e.metaKey || e.ctrlKey;
    if (e.key === "Tab" && !mod && !e.altKey) {
      e.preventDefault();
      insert(el, "    ");
    } else if (mod && !e.shiftKey && e.key.toLowerCase() === "b") {
      e.preventDefault();
      wrap(el, "**", "**", "bold");
    } else if (mod && !e.shiftKey && e.key.toLowerCase() === "i") {
      e.preventDefault();
      wrap(el, "_", "_", "italic");
    } else if (mod && !e.shiftKey && e.key.toLowerCase() === "k") {
      e.preventDefault();
      const sel = el.value.slice(el.selectionStart, el.selectionEnd) || "text";
      const text = "[" + sel + "](url)";
      insert(el, text, sel.length + 3, sel.length + 6);
    } else if (mod && e.shiftKey && e.key.toLowerCase() === "s") {
      e.preventDefault();
      const lines = el.dataset.suggest || "";
      const atLineStart = el.selectionStart === 0 || el.value[el.selectionStart - 1] === "\n";
      const prefix = atLineStart ? "" : "\n";
      const text = prefix + "```suggestion\n" + lines + "\n```\n";
      insert(el, text, prefix.length + 14, prefix.length + 14 + lines.length);
    }
  }, true);

  document.addEventListener("paste", function (e) {
    const el = e.target;
    if (!isEditor(el) || el.selectionStart === el.selectionEnd) return;
    const text = (e.clipboardData || window.clipboardData).getData("text");
    if (/^https?:\/\/\S+$/.test(text.trim())) {
      e.preventDefault();
      const sel = el.value.slice(el.selectionStart, el.selectionEnd);
      insert(el, "[" + sel + "](" + text.trim() + ")");
    }
  }, true);

  // Keep the preview roughly aligned with the editor in split view.
  document.addEventListener("scroll", function (e) {
    const el = e.target;
    if (!isEditor(el)) return;
    const split = el.closest(".md-split");
    const preview = split && split.querySelector(".md-preview");
    if (!preview) return;
    const ratio = el.scrollTop / Math.max(1, el.scrollHeight - el.clientHeight);
    preview.scrollTop = ratio * (preview.scrollHeight - preview.clientHeight);
  }, true);
})();
