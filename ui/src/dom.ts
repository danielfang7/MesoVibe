// Builds elements with text nodes only. Nothing in the panel is ever parsed as HTML, so text read
// from the screen or sent by a plugin can't become markup.

type Child = Node | string | number | null | undefined | false;

export interface Props {
  class?: string;
  /** Attributes, like aria-label, role, title or type. `false` and `null` leave one out. */
  attrs?: Record<string, string | boolean | null | undefined>;
  on?: Partial<Record<keyof HTMLElementEventMap, (event: Event) => void>>;
}

export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  props: Props = {},
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const element = document.createElement(tag);
  if (props.class) element.className = props.class;
  for (const [name, value] of Object.entries(props.attrs ?? {})) {
    if (value === true) element.setAttribute(name, "");
    else if (typeof value === "string") element.setAttribute(name, value);
  }
  for (const [name, handler] of Object.entries(props.on ?? {})) {
    if (handler) element.addEventListener(name, handler);
  }
  append(element, ...children);
  return element;
}

export function append(parent: Node, ...children: Child[]): void {
  for (const child of children) {
    if (child === null || child === undefined || child === false) continue;
    parent.appendChild(typeof child === "object" ? child : document.createTextNode(String(child)));
  }
}

/** Replaces `parent`'s children, keeping keyboard focus on the element with the same data-key. */
export function replace(parent: Element, ...children: Child[]): void {
  const focused = document.activeElement;
  const key = focused instanceof HTMLElement && parent.contains(focused) ? focused.dataset.key : undefined;
  parent.replaceChildren();
  append(parent, ...children);
  if (key !== undefined) {
    const again = parent.querySelector<HTMLElement>(`[data-key="${CSS.escape(key)}"]`);
    again?.focus();
  }
}

/** A small line icon, drawn with SVG elements (no markup strings). */
export function icon(name: "session" | "chat" | "sees" | "settings" | "reset" | "end"): SVGSVGElement {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("class", "icon");
  const shapes: Record<typeof name, [string, Record<string, string>][]> = {
    session: [
      ["circle", { cx: "12", cy: "13", r: "8" }],
      ["path", { d: "M12 9v4l3 2M9 3h6" }],
    ],
    // A speech bubble with a tail: the Chat Hub.
    chat: [["path", { d: "M4 5h16v11H10l-5 4v-4H4z" }]],
    sees: [
      ["path", { d: "M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12z" }],
      ["circle", { cx: "12", cy: "12", r: "3" }],
    ],
    settings: [
      ["circle", { cx: "12", cy: "12", r: "3" }],
      [
        "path",
        {
          d: "M12 2v3M12 19v3M4.2 4.2l2.1 2.1M17.7 17.7l2.1 2.1M2 12h3M19 12h3M4.2 19.8l2.1-2.1M17.7 6.3l2.1-2.1",
        },
      ],
    ],
    reset: [["path", { d: "M4 12a8 8 0 1 0 2.3-5.7M4 4v4h4" }]],
    // A flag on a pole: finish and keep (End).
    end: [["path", { d: "M6 21V4M6 4h11l-2.5 4L17 12H6" }]],
  };
  for (const [tag, attrs] of shapes[name]) {
    const shape = document.createElementNS(ns, tag);
    for (const [k, v] of Object.entries(attrs)) shape.setAttribute(k, v);
    svg.appendChild(shape);
  }
  return svg;
}
