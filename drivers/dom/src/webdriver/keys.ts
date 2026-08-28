export const WEBDRIVER_KEYS: Record<string, string> = {
  Cancel: "\uE001",
  Help: "\uE002",
  Backspace: "\uE003",
  Tab: "\uE004",
  Clear: "\uE005",
  Return: "\uE006",
  Enter: "\uE007",
  Shift: "\uE008",
  Control: "\uE009",
  Alt: "\uE00A",
  Pause: "\uE00B",
  Escape: "\uE00C",
  Space: "\uE00D",
  PageUp: "\uE00E",
  PageDown: "\uE00F",
  End: "\uE010",
  Home: "\uE011",
  ArrowLeft: "\uE012",
  ArrowUp: "\uE013",
  ArrowRight: "\uE014",
  ArrowDown: "\uE015",
  Insert: "\uE016",
  Delete: "\uE017",
  Semicolon: "\uE018",
  Equal: "\uE019",
  Multiply: "\uE024",
  Add: "\uE025",
  Separator: "\uE026",
  Subtract: "\uE027",
  Decimal: "\uE028",
  Divide: "\uE029",
  F1: "\uE031",
  F2: "\uE032",
  F3: "\uE033",
  F4: "\uE034",
  F5: "\uE035",
  F6: "\uE036",
  F7: "\uE037",
  F8: "\uE038",
  F9: "\uE039",
  F10: "\uE03A",
  F11: "\uE03B",
  F12: "\uE03C",
  Meta: "\uE03D",
};

const MODIFIERS = new Set(["Shift", "Control", "Alt", "Meta"]);

export interface KeyChord {
  modifiers: string[];
  key: string;
}

export function parseChord(input: string): KeyChord {
  const parts = input.split("+").filter((part) => part.length > 0);
  if (parts.length <= 1) return { modifiers: [], key: input };
  const key = parts[parts.length - 1]!;
  const modifiers: string[] = [];
  for (const part of parts.slice(0, -1)) {
    const normalized = part === "Ctrl" ? "Control" : part === "Cmd" ? "Meta" : part;
    if (!MODIFIERS.has(normalized)) return { modifiers: [], key: input };
    modifiers.push(normalized);
  }
  return { modifiers, key };
}

export function toKeyValue(key: string): string {
  const mapped = WEBDRIVER_KEYS[key];
  if (mapped) return mapped;
  if (key.length === 1) return key;
  const capitalized = key.charAt(0).toUpperCase() + key.slice(1);
  const alias = WEBDRIVER_KEYS[capitalized];
  if (alias) return alias;
  throw new Error(`unsupported key "${key}" for the tauri surface`);
}

export function keyActions(chord: KeyChord): unknown[] {
  const down = chord.modifiers.map((modifier) => ({
    type: "keyDown",
    value: toKeyValue(modifier),
  }));
  const up = chord.modifiers
    .slice()
    .reverse()
    .map((modifier) => ({ type: "keyUp", value: toKeyValue(modifier) }));
  const value = toKeyValue(chord.key);
  return [
    {
      type: "key",
      id: "uibox-keyboard",
      actions: [...down, { type: "keyDown", value }, { type: "keyUp", value }, ...up],
    },
  ];
}
