export interface RuntimeConfig {
  seed: number;
  fixedTimeMs: number | null;
  disableAnimations: boolean;
  probeConsole: boolean;
  probeNetwork: boolean;
  captureAll: boolean;
  maxEvents: number;
}

export interface SnapshotConfig {
  maxLines: number;
  maxTextLength: number;
  includeHidden: boolean;
}

export interface ReadinessReport {
  ready: boolean;
  reason: string;
  url: string;
  title: string;
  readyState: string;
  textLength: number;
  paintedElements: number;
  bodyHeight: number;
  pageErrors: number;
  lastPageError: string | null;
}

export interface RuntimeConsoleEntry {
  ts: number;
  type: "error" | "warning" | "log" | "pageerror";
  text: string;
  location?: string;
}

export interface RuntimeNetworkEntry {
  ts: number;
  method: string;
  url: string;
  status?: number;
  failure?: string;
}

export interface RuntimeDrain {
  console: RuntimeConsoleEntry[];
  network: RuntimeNetworkEntry[];
}

export interface RuntimeSelectorSpec {
  kind: "css" | "role" | "text";
  value?: string;
  role?: string;
  match?: { source: string; regex: boolean; flags: string; exact: boolean };
  options?: {
    exact?: boolean;
    level?: number;
    checked?: boolean;
    disabled?: boolean;
    expanded?: boolean;
    pressed?: boolean;
    selected?: boolean;
    includeHidden?: boolean;
  };
}

export interface UiboxRuntimeApi {
  version: number;
  snapshot(config: SnapshotConfig): string;
  readiness(): ReadinessReport;
  mark(spec: RuntimeSelectorSpec, token: string): number;
  clearMarks(token: string): void;
  textOf(spec: RuntimeSelectorSpec, limit: number): string[];
  drain(): RuntimeDrain;
  pageErrors(): string[];
}

declare global {
  interface Window {
    __uibox?: UiboxRuntimeApi;
  }
}

export function uiboxRuntime(config: RuntimeConfig): void {
  if (window.__uibox) return;

  const SKIP_TAGS: Record<string, boolean> = {
    SCRIPT: true,
    STYLE: true,
    NOSCRIPT: true,
    TEMPLATE: true,
    HEAD: true,
    LINK: true,
    META: true,
    TITLE: true,
    BR: true,
  };

  const NAME_FROM_CONTENT: Record<string, boolean> = {
    button: true,
    checkbox: true,
    link: true,
    menuitem: true,
    menuitemcheckbox: true,
    menuitemradio: true,
    option: true,
    radio: true,
    switch: true,
    tab: true,
    treeitem: true,
    heading: true,
    cell: true,
    columnheader: true,
    rowheader: true,
    gridcell: true,
    tooltip: true,
    caption: true,
    term: true,
    definition: true,
  };

  const VALUE_ROLES: Record<string, boolean> = {
    textbox: true,
    searchbox: true,
    combobox: true,
    spinbutton: true,
    slider: true,
  };

  const TRANSPARENT: Record<string, boolean> = {
    generic: true,
    none: true,
    presentation: true,
    label: true,
    paragraph: true,
    code: true,
  };

  const consoleEntries: RuntimeConsoleEntry[] = [];
  const networkEntries: RuntimeNetworkEntry[] = [];
  const pageErrorList: string[] = [];
  const maxEvents = config.maxEvents > 0 ? config.maxEvents : 500;

  function normalize(value: string | null | undefined): string {
    if (!value) return "";
    return value.replace(/\s+/g, " ").trim();
  }

  function attr(el: Element, name: string): string | null {
    return el.getAttribute(name);
  }

  function isHidden(el: Element): boolean {
    if (el.getAttribute("aria-hidden") === "true") return true;
    if ((el as HTMLElement).hasAttribute("hidden")) return true;
    const style = window.getComputedStyle(el);
    if (style.display === "none") return true;
    if (style.visibility === "hidden" || style.visibility === "collapse") return true;
    return false;
  }

  function isVisible(el: Element): boolean {
    if (isHidden(el)) return false;
    let current: Element | null = el;
    while (current) {
      if (isHidden(current)) return false;
      const parent: Node | null =
        current.parentElement ?? (current.getRootNode() as ShadowRoot).host ?? null;
      current = parent && (parent as Element).tagName ? (parent as Element) : null;
    }
    return true;
  }

  function isTopLevelLandmark(el: Element): boolean {
    let parent = el.parentElement;
    while (parent) {
      const tag = parent.tagName.toLowerCase();
      if (
        tag === "article" ||
        tag === "aside" ||
        tag === "main" ||
        tag === "nav" ||
        tag === "section"
      ) {
        return false;
      }
      parent = parent.parentElement;
    }
    return true;
  }

  function hasOwnName(el: Element): boolean {
    return (
      el.hasAttribute("aria-label") ||
      el.hasAttribute("aria-labelledby") ||
      el.hasAttribute("title")
    );
  }

  function implicitRole(el: Element): string {
    const tag = el.tagName.toLowerCase();
    if (tag === "a" || tag === "area") return el.hasAttribute("href") ? "link" : "generic";
    if (tag === "button") return "button";
    if (tag === "input") {
      const type = (attr(el, "type") || "text").toLowerCase();
      if (type === "checkbox") return "checkbox";
      if (type === "radio") return "radio";
      if (type === "range") return "slider";
      if (type === "number") return "spinbutton";
      if (type === "search") return el.hasAttribute("list") ? "combobox" : "searchbox";
      if (type === "submit" || type === "reset" || type === "button" || type === "image")
        return "button";
      if (type === "hidden") return "none";
      return el.hasAttribute("list") ? "combobox" : "textbox";
    }
    if (tag === "textarea") return "textbox";
    if (tag === "select") {
      const size = Number.parseInt(attr(el, "size") || "0", 10);
      return el.hasAttribute("multiple") || size > 1 ? "listbox" : "combobox";
    }
    if (tag === "option") return "option";
    if (tag === "optgroup") return "group";
    if (/^h[1-6]$/.test(tag)) return "heading";
    if (tag === "img") return el.getAttribute("alt") === "" ? "none" : "img";
    if (tag === "svg") return "img";
    if (tag === "ul" || tag === "ol" || tag === "menu" || tag === "dl") return "list";
    if (tag === "li") return "listitem";
    if (tag === "dt") return "term";
    if (tag === "dd") return "definition";
    if (tag === "table") return "table";
    if (tag === "thead" || tag === "tbody" || tag === "tfoot") return "rowgroup";
    if (tag === "tr") return "row";
    if (tag === "td") return "cell";
    if (tag === "th")
      return (attr(el, "scope") || "").toLowerCase() === "row" ? "rowheader" : "columnheader";
    if (tag === "caption") return "caption";
    if (tag === "nav") return "navigation";
    if (tag === "main") return "main";
    if (tag === "aside") return "complementary";
    if (tag === "header") return isTopLevelLandmark(el) ? "banner" : "generic";
    if (tag === "footer") return isTopLevelLandmark(el) ? "contentinfo" : "generic";
    if (tag === "form") return "form";
    if (tag === "search") return "search";
    if (tag === "section") return hasOwnName(el) ? "region" : "generic";
    if (tag === "article") return "article";
    if (tag === "dialog") return "dialog";
    if (tag === "fieldset") return "group";
    if (tag === "details") return "group";
    if (tag === "figure") return "figure";
    if (tag === "hr") return "separator";
    if (tag === "p") return "paragraph";
    if (tag === "blockquote") return "blockquote";
    if (tag === "progress") return "progressbar";
    if (tag === "meter") return "meter";
    if (tag === "output") return "status";
    if (tag === "summary") return "button";
    if (tag === "iframe" || tag === "frame") return "iframe";
    if (tag === "canvas" || tag === "video" || tag === "audio") return tag;
    if (tag === "code" || tag === "pre") return "code";
    if (tag === "label" || tag === "legend") return "label";
    return "generic";
  }

  function roleOf(el: Element): string {
    const explicit = normalize(attr(el, "role")).split(" ")[0];
    if (explicit) return explicit.toLowerCase();
    return implicitRole(el);
  }

  function textFromReferences(el: Element, ids: string): string {
    const root = el.getRootNode() as Document | ShadowRoot;
    const parts: string[] = [];
    for (const id of ids.split(/\s+/)) {
      if (!id) continue;
      const target = root.getElementById ? root.getElementById(id) : document.getElementById(id);
      if (target) parts.push(normalize(target.textContent));
    }
    return normalize(parts.join(" "));
  }

  function labelText(el: Element): string {
    const id = el.getAttribute("id");
    if (id) {
      const root = el.getRootNode() as Document | ShadowRoot;
      const escaped = window.CSS?.escape ? window.CSS.escape(id) : id;
      const label = root.querySelector(`label[for="${escaped}"]`);
      if (label) return normalize(label.textContent);
    }
    const wrapper = el.closest("label");
    if (wrapper) return normalize(wrapper.textContent);
    return "";
  }

  function accessibleName(el: Element, role: string): string {
    const labelledBy = attr(el, "aria-labelledby");
    if (labelledBy) {
      const referenced = textFromReferences(el, labelledBy);
      if (referenced) return referenced;
    }
    const ariaLabel = normalize(attr(el, "aria-label"));
    if (ariaLabel) return ariaLabel;

    const tag = el.tagName.toLowerCase();
    if (tag === "input" || tag === "textarea" || tag === "select") {
      const type = normalize(attr(el, "type")).toLowerCase();
      if (tag === "input" && (type === "submit" || type === "reset" || type === "button")) {
        const buttonValue = normalize((el as HTMLInputElement).value);
        if (buttonValue) return buttonValue;
      }
      const fromLabel = labelText(el);
      if (fromLabel) return fromLabel;
      const placeholder = normalize(attr(el, "placeholder"));
      if (placeholder) return placeholder;
      const alt = normalize(attr(el, "alt"));
      if (alt) return alt;
      return normalize(attr(el, "title"));
    }
    if (tag === "img" || tag === "area") {
      const alt = normalize(attr(el, "alt"));
      if (alt) return alt;
    }
    if (tag === "svg") {
      const title = el.querySelector("title");
      if (title) return normalize(title.textContent);
    }
    if (tag === "figure") {
      const caption = el.querySelector("figcaption");
      if (caption) return normalize(caption.textContent);
    }
    if (tag === "table") {
      const caption = el.querySelector("caption");
      if (caption) return normalize(caption.textContent);
    }
    if (tag === "fieldset") {
      const legend = el.querySelector("legend");
      if (legend) return normalize(legend.textContent);
    }
    if (NAME_FROM_CONTENT[role]) {
      const content = normalize(el.textContent);
      if (content) return content;
    }
    return normalize(attr(el, "title"));
  }

  function headingLevel(el: Element): number | null {
    const explicit = attr(el, "aria-level");
    if (explicit) {
      const parsed = Number.parseInt(explicit, 10);
      if (Number.isFinite(parsed)) return parsed;
    }
    const tag = el.tagName.toLowerCase();
    if (/^h[1-6]$/.test(tag)) return Number.parseInt(tag.slice(1), 10);
    return null;
  }

  function isChecked(el: Element, role: string): boolean | "mixed" | null {
    const aria = attr(el, "aria-checked");
    if (aria === "mixed") return "mixed";
    if (aria === "true") return true;
    if (aria === "false") return false;
    if (role === "checkbox" || role === "radio" || role === "switch") {
      const input = el as HTMLInputElement;
      if (
        typeof input.checked === "boolean" &&
        (el.tagName === "INPUT" || el.tagName === "OPTION")
      ) {
        return input.checked;
      }
    }
    return null;
  }

  function isDisabled(el: Element): boolean {
    if (attr(el, "aria-disabled") === "true") return true;
    const input = el as HTMLInputElement;
    if (typeof input.disabled === "boolean" && input.disabled) return true;
    return el.closest("fieldset[disabled]") !== null;
  }

  function stateAttrs(el: Element, role: string): string[] {
    const attrs: string[] = [];
    if (role === "heading") {
      const level = headingLevel(el);
      if (level !== null) attrs.push(`level=${level}`);
    }
    const checked = isChecked(el, role);
    if (checked === "mixed") attrs.push("checked=mixed");
    else if (checked === true) attrs.push("checked");
    const expanded = attr(el, "aria-expanded");
    if (expanded === "true") attrs.push("expanded");
    else if (expanded === "false") attrs.push("collapsed");
    if (attr(el, "aria-pressed") === "true") attrs.push("pressed");
    if (attr(el, "aria-selected") === "true") attrs.push("selected");
    else if (role === "option" && (el as HTMLOptionElement).selected) attrs.push("selected");
    if (isDisabled(el)) attrs.push("disabled");
    if (attr(el, "aria-required") === "true" || (el as HTMLInputElement).required === true)
      attrs.push("required");
    if (attr(el, "aria-invalid") === "true") attrs.push("invalid");
    if (el === document.activeElement && el !== document.body) attrs.push("focused");
    return attrs;
  }

  function fieldValue(el: Element, role: string, clamp: (value: string) => string): string | null {
    if (!VALUE_ROLES[role]) return null;
    const tag = el.tagName.toLowerCase();
    if (tag === "input" || tag === "textarea") {
      const type = normalize(attr(el, "type")).toLowerCase();
      const raw = (el as HTMLInputElement).value;
      if (!raw) return null;
      if (type === "password") return "•••";
      return clamp(normalize(raw));
    }
    if (tag === "select") {
      const option = (el as HTMLSelectElement).selectedOptions[0];
      return option ? clamp(normalize(option.textContent)) : null;
    }
    const owned = normalize(attr(el, "aria-valuetext") || attr(el, "aria-valuenow"));
    return owned ? owned : null;
  }

  function snapshot(snapConfig: SnapshotConfig): string {
    const maxLines = snapConfig.maxLines > 0 ? snapConfig.maxLines : 1200;
    const maxTextLength = snapConfig.maxTextLength > 0 ? snapConfig.maxTextLength : 160;
    const includeHidden = snapConfig.includeHidden === true;
    let budget = maxLines;
    let overflow = 0;

    function clamp(value: string): string {
      if (value.length <= maxTextLength) return value;
      return `${value.slice(0, maxTextLength)}…`;
    }

    interface ElementNode {
      kind: "element";
      role: string;
      name: string;
      attrs: string[];
      value: string | null;
      children: SnapNode[];
    }
    interface TextNode {
      kind: "text";
      text: string;
    }
    type SnapNode = ElementNode | TextNode;

    function mergeText(nodes: SnapNode[]): SnapNode[] {
      const out: SnapNode[] = [];
      for (const node of nodes) {
        const previous = out[out.length - 1];
        if (node.kind === "text" && previous && previous.kind === "text") {
          previous.text = clamp(`${previous.text} ${node.text}`);
          continue;
        }
        out.push(node);
      }
      return out;
    }

    function collectChildren(el: Element, skipText = false): SnapNode[] {
      const out: SnapNode[] = [];
      if (el.shadowRoot) {
        for (const child of Array.from(el.shadowRoot.childNodes)) {
          for (const collected of collect(child, skipText)) out.push(collected);
        }
      }
      for (const child of Array.from(el.childNodes)) {
        for (const collected of collect(child, skipText)) out.push(collected);
      }
      return mergeText(out);
    }

    function labelTextIsConsumed(el: Element): boolean {
      const control = (el as HTMLLabelElement).control;
      if (!control) return false;
      const own = normalize(el.textContent);
      if (!own) return true;
      const name = accessibleName(control, roleOf(control));
      return name.length > 0 && (name === own || own.indexOf(name) !== -1);
    }

    function collect(node: Node, skipText = false): SnapNode[] {
      if (budget <= 0) {
        overflow += 1;
        return [];
      }
      if (node.nodeType === Node.TEXT_NODE) {
        if (skipText) return [];
        const text = normalize(node.textContent);
        if (!text) return [];
        budget -= 1;
        return [{ kind: "text", text: clamp(text) }];
      }
      if (node.nodeType !== Node.ELEMENT_NODE) return [];

      const el = node as Element;
      if (SKIP_TAGS[el.tagName]) return [];
      if (!includeHidden && isHidden(el)) return [];
      if (el.tagName === "LABEL" && labelTextIsConsumed(el)) return collectChildren(el, true);

      const role = roleOf(el);
      if (role === "none" || role === "presentation") return collectChildren(el, skipText);
      if (role === "iframe") {
        budget -= 1;
        const src = accessibleName(el, role) || normalize(attr(el, "src"));
        return [
          {
            kind: "element",
            role: "iframe",
            name: clamp(src),
            attrs: [],
            value: null,
            children: [],
          },
        ];
      }

      const name = accessibleName(el, role);
      const attrs = stateAttrs(el, role);
      const value = fieldValue(el, role, clamp);
      if (TRANSPARENT[role] && !name && attrs.length === 0) return collectChildren(el, skipText);

      budget -= 1;
      const children = NAME_FROM_CONTENT[role] && name ? [] : collectChildren(el);
      return [{ kind: "element", role, name: clamp(name), attrs, value, children }];
    }

    function render(nodes: SnapNode[], depth: number, out: string[]): void {
      const pad = "  ".repeat(depth);
      for (const node of nodes) {
        if (node.kind === "text") {
          out.push(`${pad}- text: ${node.text}`);
          continue;
        }
        let line = `${pad}- ${node.role}`;
        if (node.name) line += ` "${node.name.replace(/"/g, '\\"')}"`;
        for (const item of node.attrs) line += ` [${item}]`;
        if (node.value !== null) line += `: ${node.value}`;
        else if (node.children.length > 0) line += ":";
        out.push(line);
        render(node.children, depth + 1, out);
      }
    }

    if (!document.body) return "- document: (no body)";
    const lines: string[] = [];
    render(collectChildren(document.body), 0, lines);
    if (overflow > 0) lines.push(`- … ${overflow} more nodes omitted (line budget reached)`);
    if (lines.length === 0) return "- document: (empty body)";
    return lines.join("\n");
  }

  function allElements(): Element[] {
    const out: Element[] = [];
    function walk(root: Document | ShadowRoot | Element): void {
      const children = root.querySelectorAll("*");
      for (const el of Array.from(children)) {
        out.push(el);
        if (el.shadowRoot) walk(el.shadowRoot);
      }
    }
    walk(document);
    return out;
  }

  function matchText(candidate: string, match: RuntimeSelectorSpec["match"]): boolean {
    if (!match) return true;
    const normalized = normalize(candidate);
    if (match.regex) {
      try {
        return new RegExp(match.source, match.flags).test(normalized);
      } catch {
        return false;
      }
    }
    const needle = normalize(match.source);
    if (match.exact) return normalized === needle;
    return normalized.toLowerCase().indexOf(needle.toLowerCase()) !== -1;
  }

  function visibleText(el: Element): string {
    const rendered = (el as HTMLElement).innerText;
    if (typeof rendered === "string") return normalize(rendered);
    return normalize(el.textContent);
  }

  function ownText(el: Element): string {
    const tag = el.tagName.toLowerCase();
    if (tag === "input") {
      const type = normalize(attr(el, "type")).toLowerCase();
      if (type === "submit" || type === "reset" || type === "button") {
        return normalize((el as HTMLInputElement).value);
      }
      return "";
    }
    return visibleText(el);
  }

  function resolve(spec: RuntimeSelectorSpec): Element[] {
    const includeHidden = spec.options?.includeHidden === true;
    if (spec.kind === "css") {
      const found: Element[] = [];
      try {
        for (const el of Array.from(document.querySelectorAll(spec.value || ""))) found.push(el);
      } catch {
        return [];
      }
      return includeHidden ? found : found.filter(isVisible);
    }

    const candidates = allElements();

    if (spec.kind === "text") {
      const matched = candidates.filter((el) => {
        if (SKIP_TAGS[el.tagName]) return false;
        if (!includeHidden && !isVisible(el)) return false;
        return matchText(ownText(el), spec.match);
      });
      return matched.filter((el) => !matched.some((other) => other !== el && el.contains(other)));
    }

    const wantedRole = (spec.role || "").toLowerCase();
    const options = spec.options || {};
    return candidates.filter((el) => {
      if (SKIP_TAGS[el.tagName]) return false;
      if (!includeHidden && !isVisible(el)) return false;
      if (roleOf(el) !== wantedRole) return false;
      if (spec.match && !matchText(accessibleName(el, wantedRole), spec.match)) return false;
      if (options.level !== undefined && headingLevel(el) !== options.level) return false;
      if (options.checked !== undefined && isChecked(el, wantedRole) !== options.checked)
        return false;
      if (options.disabled !== undefined && isDisabled(el) !== options.disabled) return false;
      if (
        options.expanded !== undefined &&
        (attr(el, "aria-expanded") === "true") !== options.expanded
      )
        return false;
      if (
        options.pressed !== undefined &&
        (attr(el, "aria-pressed") === "true") !== options.pressed
      )
        return false;
      if (options.selected !== undefined) {
        const selected =
          attr(el, "aria-selected") === "true" || (el as HTMLOptionElement).selected === true;
        if (selected !== options.selected) return false;
      }
      return true;
    });
  }

  function readiness(): ReadinessReport {
    const body = document.body;
    const text = body ? visibleText(body) : "";
    const painted = body
      ? body.querySelectorAll("img,svg,canvas,video,input,button,select,textarea,a,[role]").length
      : 0;
    const height = body ? body.getBoundingClientRect().height : 0;
    const report: ReadinessReport = {
      ready: false,
      reason: "",
      url: location.href,
      title: document.title,
      readyState: document.readyState,
      textLength: text.length,
      paintedElements: painted,
      bodyHeight: height,
      pageErrors: pageErrorList.length,
      lastPageError: pageErrorList.length > 0 ? pageErrorList[pageErrorList.length - 1]! : null,
    };
    if (!body) {
      report.reason = "document has no body element";
      return report;
    }
    if (document.readyState === "loading") {
      report.reason = "document is still loading";
      return report;
    }
    if (pageErrorList.length > 0) {
      report.reason = `uncaught page error: ${pageErrorList[pageErrorList.length - 1]}`;
      return report;
    }
    if (height <= 0) {
      report.reason = "body has zero height";
      return report;
    }
    if (text.length === 0 && painted === 0) {
      report.reason = "body rendered no text and no visual elements";
      return report;
    }
    report.ready = true;
    report.reason = "ok";
    return report;
  }

  function pushConsole(entry: RuntimeConsoleEntry): void {
    if (consoleEntries.length >= maxEvents) consoleEntries.shift();
    consoleEntries.push(entry);
  }

  function pushNetwork(entry: RuntimeNetworkEntry): void {
    if (networkEntries.length >= maxEvents) networkEntries.shift();
    networkEntries.push(entry);
  }

  function stringifyArgs(args: unknown[]): string {
    const parts: string[] = [];
    for (const arg of args) {
      if (typeof arg === "string") parts.push(arg);
      else if (arg instanceof Error) parts.push(`${arg.name}: ${arg.message}`);
      else {
        try {
          parts.push(JSON.stringify(arg));
        } catch {
          parts.push(String(arg));
        }
      }
    }
    return parts.join(" ").slice(0, 4000);
  }

  function installConsoleProbe(): void {
    if (config.probeConsole) installConsolePatches();
    window.addEventListener("error", (event) => {
      const text =
        event.error instanceof Error
          ? `${event.error.name}: ${event.error.message}`
          : event.message;
      const location = event.filename
        ? `${event.filename}:${event.lineno}:${event.colno}`
        : undefined;
      pageErrorList.push(text);
      if (config.probeConsole) {
        pushConsole({ ts: Date.now(), type: "pageerror", text, ...(location ? { location } : {}) });
      }
    });
    window.addEventListener("unhandledrejection", (event) => {
      const reason = (event as PromiseRejectionEvent).reason;
      const text = reason instanceof Error ? `${reason.name}: ${reason.message}` : String(reason);
      pageErrorList.push(text);
      if (config.probeConsole) {
        pushConsole({ ts: Date.now(), type: "pageerror", text: `unhandled rejection: ${text}` });
      }
    });
  }

  function installConsolePatches(): void {
    const levels: Array<["error" | "warning" | "log", "error" | "warn" | "log"]> = config.captureAll
      ? [
          ["error", "error"],
          ["warning", "warn"],
          ["log", "log"],
        ]
      : [
          ["error", "error"],
          ["warning", "warn"],
        ];
    for (const [type, method] of levels) {
      const original = console[method];
      console[method] = function patched(...args: unknown[]): void {
        pushConsole({ ts: Date.now(), type, text: stringifyArgs(args) });
        original.apply(console, args as never);
      } as never;
    }
  }

  function installNetworkProbe(): void {
    if (!config.probeNetwork) return;
    const originalFetch = window.fetch;
    if (typeof originalFetch === "function") {
      window.fetch = function patchedFetch(
        input: RequestInfo | URL,
        init?: RequestInit,
      ): Promise<Response> {
        const method = init?.method || (input instanceof Request ? input.method : "GET");
        const url = input instanceof Request ? input.url : String(input);
        return originalFetch.call(window, input as never, init as never).then(
          (response: Response) => {
            if (!response.ok) pushNetwork({ ts: Date.now(), method, url, status: response.status });
            return response;
          },
          (error: unknown) => {
            pushNetwork({ ts: Date.now(), method, url, failure: String(error) });
            throw error;
          },
        );
      } as typeof window.fetch;
    }

    const OriginalXhr = window.XMLHttpRequest;
    if (typeof OriginalXhr === "function") {
      const open = OriginalXhr.prototype.open;
      OriginalXhr.prototype.open = function patchedOpen(
        this: XMLHttpRequest,
        method: string,
        url: string | URL,
        ...rest: unknown[]
      ) {
        this.addEventListener("load", () => {
          if (this.status >= 400) {
            pushNetwork({ ts: Date.now(), method, url: String(url), status: this.status });
          }
        });
        this.addEventListener("error", () => {
          pushNetwork({ ts: Date.now(), method, url: String(url), failure: "network error" });
        });
        return (open as any).call(this, method, url, ...rest);
      } as typeof open;
    }
  }

  function installDeterminism(): void {
    let state = config.seed >>> 0 || 0x9e3779b9;
    Math.random = function seeded(): number {
      state = (state + 0x6d2b79f5) >>> 0;
      let t = state;
      t = Math.imul(t ^ (t >>> 15), t | 1);
      t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };

    if (config.fixedTimeMs !== null) {
      const fixed = config.fixedTimeMs;
      const OriginalDate = Date;
      const patched: any = function PatchedDate(...args: unknown[]) {
        if (!new.target) return new OriginalDate(fixed).toString();
        if (args.length === 0) return new OriginalDate(fixed);
        return new (OriginalDate as any)(...args);
      };
      patched.prototype = OriginalDate.prototype;
      patched.now = function now() {
        return fixed;
      };
      patched.parse = OriginalDate.parse;
      patched.UTC = OriginalDate.UTC;
      window.Date = patched as DateConstructor;
    }

    if (!config.disableAnimations) return;

    const css = [
      "*,*::before,*::after{",
      "animation-delay:-1ms!important;",
      "animation-duration:1ms!important;",
      "animation-iteration-count:1!important;",
      "transition-delay:0s!important;",
      "transition-duration:0s!important;",
      "scroll-behavior:auto!important;",
      "caret-color:transparent!important;",
      "}",
    ].join("");

    function injectStyle(): void {
      if (document.getElementById("uibox-determinism")) return;
      const target = document.head || document.documentElement;
      if (!target) return;
      const style = document.createElement("style");
      style.id = "uibox-determinism";
      style.textContent = css;
      target.appendChild(style);
    }

    injectStyle();
    document.addEventListener("DOMContentLoaded", injectStyle);
    if (typeof Element.prototype.animate === "function") {
      const originalAnimate = Element.prototype.animate;
      Element.prototype.animate = function patchedAnimate(this: Element, ...args: unknown[]) {
        const animation = originalAnimate.apply(this, args as never);
        try {
          animation.finish();
        } catch {
          animation.cancel();
        }
        return animation;
      } as typeof Element.prototype.animate;
    }
  }

  installConsoleProbe();
  installNetworkProbe();
  installDeterminism();

  window.__uibox = {
    version: 1,
    snapshot,
    readiness,
    mark(spec: RuntimeSelectorSpec, token: string): number {
      const matches = resolve(spec);
      for (let i = 0; i < matches.length; i += 1) matches[i]!.setAttribute("data-uibox-hit", token);
      return matches.length;
    },
    clearMarks(token: string): void {
      for (const el of Array.from(document.querySelectorAll(`[data-uibox-hit="${token}"]`))) {
        el.removeAttribute("data-uibox-hit");
      }
    },
    textOf(spec: RuntimeSelectorSpec, limit: number): string[] {
      return resolve(spec)
        .slice(0, limit > 0 ? limit : 10)
        .map(visibleText);
    },
    drain(): RuntimeDrain {
      return { console: consoleEntries.splice(0), network: networkEntries.splice(0) };
    },
    pageErrors(): string[] {
      return pageErrorList.slice();
    },
  };
}
