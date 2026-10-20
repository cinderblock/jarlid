// The confirmation card in front of every public, one-way lyrics action: publishing to
// LRCLIB and flagging a record. Shared by the editor and the lyrics pane, so both ask the
// same way.

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const confirmEl = $("lyric-confirm");
const titleEl = $("lyric-confirm-title");
const msgEl = $("lyric-confirm-msg");
const goBtn = $<HTMLButtonElement>("lyric-confirm-go");
const reasonEl = $<HTMLInputElement>("lyric-reason");

/// Settles the question on screen: the reason typed (or "") on go, null on cancel.
let answer: ((reason: string | null) => void) | null = null;

export const isOpen = () => !confirmEl.hidden;

/// Ask, and resolve to the optional reason when the action is confirmed or to null when it
/// isn't. Asking again while a question is open cancels the first one.
export function ask(opts: {
  title: string;
  message: string;
  go: string;
  reason?: boolean;
}): Promise<string | null> {
  settle(null);
  titleEl.textContent = opts.title;
  msgEl.textContent = opts.message;
  goBtn.textContent = opts.go;
  reasonEl.hidden = !opts.reason;
  reasonEl.value = "";
  confirmEl.hidden = false;
  (opts.reason ? reasonEl : goBtn).focus();
  return new Promise((resolve) => (answer = resolve));
}

function settle(reason: string | null) {
  confirmEl.hidden = true;
  const a = answer;
  answer = null;
  a?.(reason);
}

/// Close without acting, as Cancel does.
export const cancel = () => settle(null);

$("lyric-confirm-cancel").addEventListener("click", cancel);
goBtn.addEventListener("click", () => settle(reasonEl.value));

// Capture phase, so Escape closes the card and stops there rather than also reaching the
// editor, where it would end a timing pass.
window.addEventListener(
  "keydown",
  (e) => {
    if (e.key !== "Escape" || !isOpen()) return;
    e.stopPropagation();
    cancel();
  },
  true
);
