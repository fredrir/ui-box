import { SelectorError } from "./errors.js";

export interface TextMatch {
  source: string;
  regex: boolean;
  flags: string;
  exact: boolean;
}

export interface RoleOptions {
  name?: TextMatch;
  exact: boolean;
  level?: number;
  checked?: boolean;
  disabled?: boolean;
  expanded?: boolean;
  pressed?: boolean;
  selected?: boolean;
  includeHidden: boolean;
}

export type ParsedSelector =
  | { kind: "css"; raw: string; value: string }
  | { kind: "role"; raw: string; role: string; options: RoleOptions }
  | { kind: "text"; raw: string; match: TextMatch };

const TUI_ONLY = new Set(["re", "cell"]);
const SUPPORTED = "css=, role=, text=";

const ROLE_FLAGS = new Set(["exact", "includeHidden"]);
const ROLE_BOOLEANS = new Set(["checked", "disabled", "expanded", "pressed", "selected"]);

export function parseSelector(input: unknown): ParsedSelector {
  if (typeof input !== "string" || input.trim().length === 0) {
    throw new SelectorError(`selector must be a non-empty string, got ${JSON.stringify(input)}`);
  }
  const raw = input.trim();
  const eq = raw.indexOf("=");
  if (eq <= 0) {
    throw new SelectorError(
      `selector "${raw}" has no engine prefix; the dom driver supports ${SUPPORTED}`,
    );
  }
  const prefix = raw.slice(0, eq).trim();
  const body = raw.slice(eq + 1).trim();

  if (TUI_ONLY.has(prefix)) {
    throw new SelectorError(
      `selector "${prefix}=" is TUI-only and is not available on the dom driver; supported here: ${SUPPORTED}`,
    );
  }

  if (body.length === 0) {
    throw new SelectorError(`selector "${raw}" has an empty ${prefix}= body`);
  }

  switch (prefix) {
    case "css":
      return { kind: "css", raw, value: body };
    case "text":
      return { kind: "text", raw, match: parseTextMatch(body, raw, false) };
    case "role":
      return parseRole(body, raw);
    default:
      throw new SelectorError(
        `unknown selector engine "${prefix}=" in "${raw}"; the dom driver supports ${SUPPORTED}`,
      );
  }
}

function parseTextMatch(body: string, raw: string, defaultExact: boolean): TextMatch {
  if (body.startsWith("/")) {
    const end = findRegexEnd(body);
    if (end === -1) throw new SelectorError(`unterminated regular expression in "${raw}"`);
    const source = body.slice(1, end);
    const flags = body.slice(end + 1).trim();
    assertRegex(source, flags, raw);
    return { source, regex: true, flags, exact: false };
  }
  const quote = body[0];
  if (quote === '"' || quote === "'") {
    const value = unquote(body, quote, raw);
    return { source: value, regex: false, flags: "", exact: true };
  }
  return { source: body, regex: false, flags: "", exact: defaultExact };
}

function findRegexEnd(body: string): number {
  let escaped = false;
  let inClass = false;
  for (let i = 1; i < body.length; i += 1) {
    const ch = body[i];
    if (escaped) {
      escaped = false;
      continue;
    }
    if (ch === "\\") {
      escaped = true;
      continue;
    }
    if (ch === "[") inClass = true;
    else if (ch === "]") inClass = false;
    else if (ch === "/" && !inClass) return i;
  }
  return -1;
}

function assertRegex(source: string, flags: string, raw: string): void {
  try {
    new RegExp(source, flags);
  } catch (err) {
    throw new SelectorError(`invalid regular expression in "${raw}": ${(err as Error).message}`);
  }
}

function unquote(body: string, quote: string, raw: string): string {
  let out = "";
  let escaped = false;
  for (let i = 1; i < body.length; i += 1) {
    const ch = body[i]!;
    if (escaped) {
      out += ch;
      escaped = false;
      continue;
    }
    if (ch === "\\") {
      escaped = true;
      continue;
    }
    if (ch === quote) {
      if (i !== body.length - 1) {
        throw new SelectorError(`trailing characters after quoted value in "${raw}"`);
      }
      return out;
    }
    out += ch;
  }
  throw new SelectorError(`unterminated quoted value in "${raw}"`);
}

function parseRole(body: string, raw: string): ParsedSelector {
  const bracket = body.indexOf("[");
  const role = (bracket === -1 ? body : body.slice(0, bracket)).trim();
  if (!/^[a-zA-Z][a-zA-Z-]*$/.test(role)) {
    throw new SelectorError(`invalid ARIA role "${role}" in "${raw}"`);
  }
  const options: RoleOptions = { exact: false, includeHidden: false };
  if (bracket === -1) return { kind: "role", raw, role, options };

  for (const [key, value] of parseAttributes(body.slice(bracket), raw)) {
    if (value === null) {
      if (ROLE_FLAGS.has(key)) {
        if (key === "exact") options.exact = true;
        else options.includeHidden = true;
        continue;
      }
      if (ROLE_BOOLEANS.has(key)) {
        setRoleBoolean(options, key, true);
        continue;
      }
      throw new SelectorError(`role attribute "${key}" in "${raw}" requires a value`);
    }
    if (key === "name") {
      options.name = parseTextMatch(value, raw, false);
      continue;
    }
    if (key === "level") {
      const level = Number.parseInt(value, 10);
      if (!Number.isFinite(level)) throw new SelectorError(`level must be a number in "${raw}"`);
      options.level = level;
      continue;
    }
    if (ROLE_BOOLEANS.has(key)) {
      setRoleBoolean(options, key, parseBoolean(value, key, raw));
      continue;
    }
    if (key === "exact") {
      options.exact = parseBoolean(value, key, raw);
      continue;
    }
    if (key === "includeHidden") {
      options.includeHidden = parseBoolean(value, key, raw);
      continue;
    }
    throw new SelectorError(
      `unknown role attribute "${key}" in "${raw}"; supported: name, exact, level, checked, disabled, expanded, pressed, selected, includeHidden`,
    );
  }
  if (options.name && options.exact) options.name.exact = true;
  return { kind: "role", raw, role, options };
}

function setRoleBoolean(options: RoleOptions, key: string, value: boolean): void {
  if (key === "checked") options.checked = value;
  else if (key === "disabled") options.disabled = value;
  else if (key === "expanded") options.expanded = value;
  else if (key === "pressed") options.pressed = value;
  else if (key === "selected") options.selected = value;
}

function parseBoolean(value: string, key: string, raw: string): boolean {
  const normalized = stripQuotes(value).toLowerCase();
  if (normalized === "true") return true;
  if (normalized === "false") return false;
  throw new SelectorError(`role attribute "${key}" in "${raw}" must be true or false`);
}

function stripQuotes(value: string): string {
  const quote = value[0];
  if ((quote === '"' || quote === "'") && value.endsWith(quote) && value.length > 1) {
    return value.slice(1, -1);
  }
  return value;
}

function parseAttributes(section: string, raw: string): Array<[string, string | null]> {
  const out: Array<[string, string | null]> = [];
  let i = 0;
  while (i < section.length) {
    if (section[i] !== "[") {
      if (/\s/.test(section[i] ?? "")) {
        i += 1;
        continue;
      }
      throw new SelectorError(`unexpected character "${section[i]}" in role selector "${raw}"`);
    }
    let depth = 0;
    let quote: string | null = null;
    let escaped = false;
    let end = -1;
    for (let j = i; j < section.length; j += 1) {
      const ch = section[j]!;
      if (escaped) {
        escaped = false;
        continue;
      }
      if (ch === "\\") {
        escaped = true;
        continue;
      }
      if (quote) {
        if (ch === quote) quote = null;
        continue;
      }
      if (ch === '"' || ch === "'") {
        quote = ch;
        continue;
      }
      if (ch === "[") depth += 1;
      else if (ch === "]") {
        depth -= 1;
        if (depth === 0) {
          end = j;
          break;
        }
      }
    }
    if (end === -1) throw new SelectorError(`unterminated "[" in role selector "${raw}"`);
    const inner = section.slice(i + 1, end).trim();
    const eq = splitAttributeAt(inner);
    if (eq === -1) out.push([inner, null]);
    else out.push([inner.slice(0, eq).trim(), inner.slice(eq + 1).trim()]);
    i = end + 1;
  }
  return out;
}

function splitAttributeAt(inner: string): number {
  let quote: string | null = null;
  let escaped = false;
  for (let i = 0; i < inner.length; i += 1) {
    const ch = inner[i]!;
    if (escaped) {
      escaped = false;
      continue;
    }
    if (ch === "\\") {
      escaped = true;
      continue;
    }
    if (quote) {
      if (ch === quote) quote = null;
      continue;
    }
    if (ch === '"' || ch === "'") {
      quote = ch;
      continue;
    }
    if (ch === "=") return i;
  }
  return -1;
}
