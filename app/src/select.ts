// A dropdown we draw ourselves.
//
// A native <select> is the one control CSS cannot reach: `color-scheme` decides
// whether its popup is black or white and that is the whole of the API. Ours is
// shaped like the station and mode panels, because a list of choices already
// looks like something in this app.
//
// Behaviour follows the ARIA select-only combobox pattern — the point of writing
// it out rather than reaching for a library is that a settings page you cannot
// drive from the keyboard is worse than an ugly one.

export interface Option {
  value: string;
  label: string;
}

export interface Select {
  /** Currently chosen value. Setting it does not fire `onChange`. */
  get value(): string;
  set value(v: string);
  /** Replace the list. Keeps the current value selected if it still exists. */
  setOptions(options: Option[]): void;
  setDisabled(disabled: boolean): void;
}

const CHEVRON = `<svg class="select-chevron" viewBox="0 0 24 24" aria-hidden="true"><path d="m6 9 6 6 6-6"/></svg>`;
const TICK = `<svg class="select-tick" viewBox="0 0 24 24" aria-hidden="true"><path d="m4 12.5 5 5L20 6.5"/></svg>`;

/**
 * Turn `root` into a dropdown. `root` should be an empty element with class
 * `select`; its contents are replaced.
 */
export function createSelect(
  root: HTMLElement,
  options: Option[],
  opts: { label: string; onChange: (value: string) => void },
): Select {
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "select-btn field";
  btn.setAttribute("role", "combobox");
  btn.setAttribute("aria-haspopup", "listbox");
  btn.setAttribute("aria-expanded", "false");
  btn.setAttribute("aria-label", opts.label);

  const valueEl = document.createElement("span");
  valueEl.className = "select-value";
  btn.appendChild(valueEl);
  btn.insertAdjacentHTML("beforeend", CHEVRON);

  const panel = document.createElement("div");
  panel.className = "select-panel";
  panel.setAttribute("role", "listbox");
  panel.setAttribute("aria-label", opts.label);
  panel.hidden = true;

  const listId = `${root.id || "select"}-list`;
  panel.id = listId;
  btn.setAttribute("aria-controls", listId);

  root.replaceChildren(btn, panel);

  let items = options.slice();
  let value = items[0]?.value ?? "";
  let hl = 0;
  let typed = "";
  let typedAt = 0;

  const isOpen = () => !panel.hidden;

  function renderButton() {
    valueEl.textContent = items.find((o) => o.value === value)?.label ?? "";
  }

  function renderPanel() {
    panel.replaceChildren();
    items.forEach((o, i) => {
      const el = document.createElement("div");
      el.className = "select-opt" + (i === hl ? " hl" : "");
      el.id = `${listId}-${i}`;
      el.setAttribute("role", "option");
      el.setAttribute("aria-selected", String(o.value === value));
      el.insertAdjacentHTML("afterbegin", TICK);
      const text = document.createElement("span");
      text.textContent = o.label;
      el.appendChild(text);
      // mousedown, not click: the button's own blur/close would otherwise race it.
      el.addEventListener("mousedown", (e) => {
        e.preventDefault();
        choose(i);
      });
      el.addEventListener("mousemove", () => highlight(i));
      panel.appendChild(el);
    });
    btn.setAttribute("aria-activedescendant", isOpen() ? `${listId}-${hl}` : "");
  }

  function highlight(i: number) {
    if (!items.length) return;
    hl = (i + items.length) % items.length;
    panel.querySelectorAll(".select-opt").forEach((el, n) => el.classList.toggle("hl", n === hl));
    btn.setAttribute("aria-activedescendant", `${listId}-${hl}`);
    panel.children[hl]?.scrollIntoView({ block: "nearest" });
  }

  function open() {
    if (isOpen() || btn.disabled) return;
    hl = Math.max(
      0,
      items.findIndex((o) => o.value === value),
    );
    panel.hidden = false;
    root.dataset.open = "true";
    btn.setAttribute("aria-expanded", "true");
    renderPanel();
    panel.children[hl]?.scrollIntoView({ block: "nearest" });
  }

  function close() {
    if (!isOpen()) return;
    panel.hidden = true;
    delete root.dataset.open;
    btn.setAttribute("aria-expanded", "false");
    btn.removeAttribute("aria-activedescendant");
  }

  function choose(i: number) {
    const picked = items[i];
    close();
    btn.focus();
    if (!picked || picked.value === value) return;
    value = picked.value;
    renderButton();
    opts.onChange(value);
  }

  btn.addEventListener("click", () => (isOpen() ? close() : open()));

  // Attached to the root rather than the window so an Escape that closes this list
  // does not also close the Settings page behind it — the root is on the bubble
  // path first, and stopPropagation() there ends the journey.
  root.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      if (!isOpen()) return;
      e.stopPropagation();
      e.preventDefault();
      close();
      return;
    }
    if (e.key === "Tab") {
      close();
      return;
    }
    if (!isOpen()) {
      if (["Enter", " ", "ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) {
        e.preventDefault();
        open();
      }
      return;
    }
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        highlight(hl + 1);
        break;
      case "ArrowUp":
        e.preventDefault();
        highlight(hl - 1);
        break;
      case "Home":
        e.preventDefault();
        highlight(0);
        break;
      case "End":
        e.preventDefault();
        highlight(items.length - 1);
        break;
      case "Enter":
      case " ":
        e.preventDefault();
        choose(hl);
        break;
      default: {
        // Type-ahead. Printable keys only; the buffer expires so that typing
        // "d" twice a minute apart means "the first d" both times.
        if (e.key.length !== 1 || e.ctrlKey || e.altKey || e.metaKey) return;
        typed = performance.now() - typedAt > 800 ? e.key : typed + e.key;
        typedAt = performance.now();
        const at = items.findIndex((o) => o.label.toLowerCase().startsWith(typed.toLowerCase()));
        if (at >= 0) highlight(at);
      }
    }
  });

  // A click anywhere else means "not this one", including on another control.
  window.addEventListener("mousedown", (e) => {
    if (isOpen() && !root.contains(e.target as Node)) close();
  });

  renderButton();

  return {
    get value() {
      return value;
    },
    set value(v: string) {
      value = v;
      renderButton();
      if (isOpen()) renderPanel();
    },
    setOptions(next: Option[]) {
      items = next.slice();
      renderButton();
      if (isOpen()) renderPanel();
    },
    setDisabled(disabled: boolean) {
      btn.disabled = disabled;
      if (disabled) close();
    },
  };
}
