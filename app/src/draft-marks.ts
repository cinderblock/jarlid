// Marking up the words of a lyric line: the ones the speech engine was unsure of, in a
// draft, and each word on its own, for lyrics timed word by word.
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

/// Fill `el` with `text` one `<span class="w">` per word, for lyrics timed word by word: the
/// pane lights each as it is sung. Words at positions `doubt` are marked as well.
export function fillWords(el: HTMLElement, text: string, doubt: readonly number[]) {
  el.replaceChildren(
    ...text
      .trim()
      .split(/\s+/)
      .flatMap((word, i) => {
        const span = document.createElement("span");
        span.className = doubt.includes(i) ? "w doubt" : "w";
        span.textContent = word;
        return i ? [document.createTextNode(" "), span] : [span];
      })
  );
}
