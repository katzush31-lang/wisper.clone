import { describe, expect, it } from "vitest";
import {
  autoTitle,
  insertAtCaret,
  previewDictionaryCsv,
  previewSnippetsJson,
  relativeTime,
  slugify,
  splitList,
  truncate,
  uniqueId,
} from "./helpers";

describe("slugify / uniqueId", () => {
  it("makes a safe slug", () => {
    expect(slugify("WhatsApp to family!")).toBe("whatsapp_to_family");
    expect(slugify("  Café  ")).toBe("cafe");
    expect(slugify("!!!")).toBe("untitled");
  });
  it("keeps ids unique", () => {
    expect(uniqueId("custom_", "Family", [])).toBe("custom_family");
    expect(uniqueId("custom_", "Family", ["custom_family"])).toBe("custom_family_2");
    expect(uniqueId("custom_", "Family", ["custom_family", "custom_family_2"])).toBe("custom_family_3");
  });
});

describe("splitList", () => {
  it("splits, trims and de-duplicates", () => {
    expect(splitList("Zeus, zechus ,, ZEUS\nfoo")).toEqual(["Zeus", "zechus", "foo"]);
    expect(splitList("")).toEqual([]);
  });
});

describe("previews", () => {
  it("counts csv lines", () => {
    expect(previewDictionaryCsv("zechus\nzeus,zechus\n\n  \nShabbos")).toEqual({ words: 2, corrections: 1 });
  });
  it("counts snippets in json", () => {
    expect(previewSnippetsJson('[{"name":"a","text":"b"},{"trigger":"c","expansion":"d"},{"x":1}]')).toBe(2);
    expect(previewSnippetsJson("nope")).toBeNull();
    expect(previewSnippetsJson('{"a":1}')).toBeNull();
    expect(previewSnippetsJson("")).toBeNull();
  });
});

describe("autoTitle", () => {
  it("uses the first three words", () => {
    expect(autoTitle("  Buy milk and eggs tomorrow")).toBe("Buy milk and");
    expect(autoTitle("Hello.")).toBe("Hello");
    expect(autoTitle("   ")).toBe("");
  });
});

describe("insertAtCaret", () => {
  it("inserts at the caret", () => {
    expect(insertAtCaret("Hello world", "big ", 6, 6)).toEqual({ value: "Hello big world", caret: 10 });
  });
  it("replaces a selection", () => {
    expect(insertAtCaret("Hello world", "there", 6, 11).value).toBe("Hello there");
  });
  it("appends without a caret", () => {
    expect(insertAtCaret("Hello", "world", null, null).value).toBe("Hello world");
    expect(insertAtCaret("", "world", null, null).value).toBe("world");
  });
});

describe("relativeTime / truncate", () => {
  const now = 1_700_000_000_000;
  it("formats", () => {
    expect(relativeTime(now - 10_000, now)).toBe("Just now");
    expect(relativeTime(now - 5 * 60_000, now)).toBe("5 min ago");
    expect(relativeTime(now - 3 * 3_600_000, now)).toBe("3 h ago");
    expect(relativeTime(now - 30 * 3_600_000, now)).toBe("Yesterday");
  });
  it("truncates to one line", () => {
    expect(truncate("a\n b   c", 10)).toBe("a b c");
    expect(truncate("abcdefghij", 5)).toBe("abcd…");
  });
});
