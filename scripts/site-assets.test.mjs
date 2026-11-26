// The website's own files: every picture, font, stylesheet and script that web/index.html and
// web/style.css point at, and the issue form the footer links to, exists in the repository.
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

const WEB = fileURLToPath(new URL("../web/", import.meta.url));
const REPO = fileURLToPath(new URL("../", import.meta.url));

/** A path on the site itself, not a link elsewhere, an in-page anchor or inline data. */
function isLocal(ref) {
  return !/^(https?:|mailto:|data:|#)/.test(ref) && ref !== "/";
}

// Fails when a commit leaves out a file the page uses (a new picture or font never added), or a
// rename leaves the page pointing at the old name; the page would show a broken picture or font.
test("every file the website points at exists", () => {
  const html = readFileSync(join(WEB, "index.html"), "utf8");
  const css = readFileSync(join(WEB, "style.css"), "utf8");
  const refs = new Set();
  for (const [, ref] of html.matchAll(/\s(?:src|href)="([^"]+)"/g)) refs.add(ref);
  for (const [, list] of html.matchAll(/\ssrcset="([^"]+)"/g)) {
    for (const candidate of list.split(",")) refs.add(candidate.trim().split(/\s+/)[0]);
  }
  for (const [, ref] of css.matchAll(/url\("?([^")]+)"?\)/g)) refs.add(ref);

  const local = [...refs].filter(isLocal);
  assert.ok(local.length >= 8, `expected the page's pictures, fonts, stylesheet and script, found ${local.length}`);
  const missing = local.filter((ref) => !existsSync(join(WEB, ref.replace(/^\//, ""))));
  assert.deepEqual(missing, []);

  const forms = [...html.matchAll(/issues\/new\?template=([\w.-]+)/g)].map((m) => m[1]);
  assert.ok(forms.length > 0, "the footer links an issue form");
  for (const form of forms) assert.ok(existsSync(join(REPO, ".github", "ISSUE_TEMPLATE", form)), form);
});
