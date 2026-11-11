// A small fake of the DOM the built-in interface uses, for tests only (bun
// has no DOM, and the package takes no dependency for one). It records what
// matters to the tests: the tree, attributes, text, inline style and
// listeners. Parsing markup is not faked: setting `innerHTML` throws, so a
// test fails if any path tries it.

export class FakeNode extends EventTarget {
  readonly tagName: string;
  readonly ownerDocument: FakeDocument;
  parentNode: FakeNode | null = null;
  children: FakeNode[] = [];
  readonly attributes = new Map<string, string>();
  readonly listeners = new Set<unknown>();
  readonly style = new FakeStyle();
  data = "";
  hidden = false;
  open = false;
  type = "";

  constructor(doc: FakeDocument, tag: string) {
    super();
    this.ownerDocument = doc;
    this.tagName = tag.toUpperCase();
  }
  override addEventListener(
    type: string,
    listener: EventListenerOrEventListenerObject | null,
  ): void {
    this.listeners.add(listener);
    super.addEventListener(type, listener);
  }
  override removeEventListener(
    type: string,
    listener: EventListenerOrEventListenerObject | null,
  ): void {
    this.listeners.delete(listener);
    super.removeEventListener(type, listener);
  }
  get className(): string {
    return this.attributes.get("class") ?? "";
  }
  set className(value: string) {
    this.attributes.set("class", value);
  }
  readonly classList = {
    add: (name: string) => this.classList.toggle(name, true),
    remove: (name: string) => this.classList.toggle(name, false),
    contains: (name: string) => this.className.split(" ").includes(name),
    toggle: (name: string, on?: boolean) => {
      const names = new Set(this.className.split(" ").filter(Boolean));
      const want = on ?? !names.has(name);
      if (want) {
        names.add(name);
      } else {
        names.delete(name);
      }
      this.className = [...names].join(" ");
      return want;
    },
  };
  setAttribute(name: string, value: string): void {
    this.attributes.set(name, String(value));
  }
  getAttribute(name: string): string | null {
    return this.attributes.get(name) ?? null;
  }
  toggleAttribute(name: string, on: boolean): void {
    if (on) {
      this.attributes.set(name, "");
    } else {
      this.attributes.delete(name);
    }
  }
  removeAttribute(name: string): void {
    this.attributes.delete(name);
  }
  append(...nodes: Array<FakeNode | string>): void {
    for (const node of nodes) {
      const child = typeof node === "string" ? this.ownerDocument.text(node) : node;
      child.remove();
      child.parentNode = this;
      this.children.push(child);
    }
  }
  replaceChildren(...nodes: Array<FakeNode | string>): void {
    // `remove` replaces the array, so this one can be walked.
    for (const child of this.children) {
      child.remove();
    }
    this.append(...nodes);
  }
  remove(): void {
    const parent = this.parentNode;
    if (parent !== null) {
      parent.children = parent.children.filter((c) => c !== this);
      this.parentNode = null;
    }
  }
  contains(node: unknown): boolean {
    return node === this || this.children.some((c) => c.contains(node));
  }
  get firstChild(): FakeNode | null {
    return this.children[0] ?? null;
  }
  get childElementCount(): number {
    return this.children.filter((c) => c.tagName !== "#TEXT").length;
  }
  get textContent(): string {
    return this.tagName === "#TEXT" ? this.data : this.children.map((c) => c.textContent).join("");
  }
  set textContent(value: string) {
    this.replaceChildren(String(value));
  }
  set innerHTML(_value: string) {
    throw new Error("innerHTML is never used");
  }
  get offsetWidth(): number {
    return 200;
  }
  get offsetHeight(): number {
    return 80;
  }
  getBoundingClientRect(): { left: number; top: number } {
    return { left: 0, top: 0 };
  }
  /** Every node below this one, in tree order. */
  *walk(): Generator<FakeNode> {
    for (const child of this.children) {
      yield child;
      yield* child.walk();
    }
  }
  /** The descendants whose `part` includes `part`. */
  parts(part: string): FakeNode[] {
    return [...this.walk()].filter((n) => (n.getAttribute("part") ?? "").split(" ").includes(part));
  }
  get isConnected(): boolean {
    const top = this.top();
    return top === this.ownerDocument || top === this.ownerDocument.body;
  }
  /** The node at the top of this one's tree. */
  top(): FakeNode {
    return this.parentNode === null ? this : this.parentNode.top();
  }
  focus(): void {
    this.ownerDocument.activeElement = this;
  }
  click(): void {
    this.dispatchEvent(new Event("click"));
  }
}

class FakeStyle {
  readonly values = new Map<string, string>();
  setProperty(name: string, value: string): void {
    this.values.set(name, value);
  }
  removeProperty(name: string): void {
    this.values.delete(name);
  }
  getPropertyValue(name: string): string {
    return this.values.get(name) ?? "";
  }
}

export class FakeDocument extends FakeNode {
  readonly documentElement: FakeNode;
  readonly body: FakeNode;
  readonly created: FakeNode[] = [];
  activeElement: FakeNode | null = null;
  constructor() {
    super(undefined as unknown as FakeDocument, "#document");
    (this as { ownerDocument: FakeDocument }).ownerDocument = this;
    this.documentElement = this.createElement("html");
    this.body = this.createElement("body");
  }
  createElement(tag: string): FakeNode {
    const node = new FakeNode(this, tag);
    this.created.push(node);
    return node;
  }
  createElementNS(_ns: string, tag: string): FakeNode {
    return this.createElement(tag);
  }
  text(data: string): FakeNode {
    const node = new FakeNode(this, "#text");
    node.data = data;
    return node;
  }
}
