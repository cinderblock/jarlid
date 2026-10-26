// Marking the words of a draft line the speech engine was unsure of.
//
// A draft is lyrics the engine wrote for a song LRCLIB has nothing for (see
// plans/draft-lyrics.md). Its mistakes cluster in the words it heard with low confidence,
// so those are marked wherever a draft is shown — in the pane, and in the editor where it
// is reviewed — to point the reviewer's ear at them.

/// Fill `el` with `text`, wrapping the words at positions `doubt` (counted by spaces, as the
/// drafter joined them) in `<span class="doubt">`.
export function fillMarked(el: HTMLElement, text: string, doubt: readonly number[]) {
  if (!doubt.length) {
    el.textContent = text;
    return;
  }
  el.replaceChildren(
    ...text.split(" ").flatMap((word, i) => {
      const sep = i ? [document.createTextNode(" ")] : [];
      if (!doubt.includes(i)) return [...sep, document.createTextNode(word)];
      const span = document.createElement("span");
      span.className = "doubt";
      span.textContent = word;
      return [...sep, span];
    })
  );
}
