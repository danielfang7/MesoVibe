// Protects tooltip-seed.mjs, which unpacks the font the tooltip reader's starting letter shapes
// are drawn from. A wrong file or an unchecked download would draw them from the wrong font.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";

import { checked, untar } from "./tooltip-seed.mjs";

/** A tar entry: a 512-byte header (name, octal size, type) and the content padded to 512. */
function entry(name, content, type = "0") {
  const header = Buffer.alloc(512);
  header.write(name, 0, "latin1");
  header.write(content.length.toString(8).padStart(11, "0"), 124, "latin1");
  header.write(type, 156, "latin1");
  header.write("ustar\u000000", 257, "latin1");
  const body = Buffer.alloc(Math.ceil(content.length / 512) * 512);
  Buffer.from(content).copy(body);
  return Buffer.concat([header, body]);
}

// The release is a tar of plain files beside folders; each file comes out whole, by its path.
test("untar gives each plain file by its path and skips folders", () => {
  const archive = Buffer.concat([
    entry("fonts/", "", "5"),
    entry("fonts/LICENSE", "SIL Open Font License"),
    entry("fonts/Bold.ttf", "x".repeat(700)),
    Buffer.alloc(1024),
  ]);
  const files = untar(archive);
  assert.deepEqual([...files.keys()], ["fonts/LICENSE", "fonts/Bold.ttf"]);
  assert.equal(files.get("fonts/LICENSE").toString(), "SIL Open Font License");
  assert.equal(files.get("fonts/Bold.ttf").length, 700);
});

// A download that isn't the pinned release must stop the script before anything is drawn.
test("checked passes the expected bytes and refuses any others", () => {
  const bytes = Buffer.from("font");
  const sha = createHash("sha256").update(bytes).digest("hex");
  assert.equal(checked(bytes, sha, "release"), bytes);
  assert.throws(() => checked(Buffer.from("other"), sha, "release"), /^Error: release: sha256 [0-9a-f]{64}, expected/);
});
