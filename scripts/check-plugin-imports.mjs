#!/usr/bin/env node
// Enforces the plugin import rule:
// a first-party plugin may import only from "@mesovibe/sdk" (or its subpaths)
// and from files inside its own folder. Anything else fails the build.
//
// This scans source text, so it also sees type-only imports, which the bundler erases.
// It is a fast first check, not a JavaScript parser, so it fails closed: it scans both the
// raw text and the text with comments removed, and flags what either finds. An import left
// in a comment therefore still fails the build; delete it instead. Triple-slash directives
// (/// <reference ...>) are refused outright, since they can pull in types or files.
//
// Usage: node scripts/check-plugin-imports.mjs <plugins-src-dir>

import { readdirSync, readFileSync, realpathSync, statSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const SOURCE_EXT = /\.(?:[cm]?[jt]sx?)$/;
const SPECIFIER_PATTERNS = [
  // import x from "y", import{a}from"y", import { "a-b" as c } from "y", export * from "y", import type T from "y"
  /\b(?:import|export)\b(?:"[^"\n]*"|'[^'\n]*'|[^'"`;])*?\bfrom\s*(['"`])([^'"`\n]+)\1/g,
  /\bimport\s*(['"`])([^'"`\n]+)\1/g, // import "y"
  /\b(?:import|require)\s*\(\s*(['"`])([^'"`\n]+)\1/g, // import("y"), import(`y`, options), require("y")
];
const DIRECTIVE = /^[ \t]*\/\/\/[ \t]*<(?:reference|amd-module)\b[^\n]*/gm;

function listFiles(dir) {
  const out = [];
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) out.push(...listFiles(path));
    else if (SOURCE_EXT.test(name)) out.push(path);
  }
  return out;
}

/** Replaces comments with whitespace, leaving strings and template literals intact. */
function stripComments(source) {
  let out = "";
  let quote = null; // the open string's quote character, if inside one
  for (let i = 0; i < source.length; i++) {
    const c = source[i];
    if (quote) {
      out += c;
      if (c === "\\") out += source[++i] ?? "";
      else if (c === quote || (c === "\n" && quote !== "`")) quote = null;
    } else if (c === "/" && source[i + 1] === "/") {
      while (i < source.length && source[i] !== "\n") i++;
      out += "\n";
    } else if (c === "/" && source[i + 1] === "*") {
      const end = source.indexOf("*/", i + 2);
      i = end === -1 ? source.length : end + 1;
      out += " ";
    } else {
      if (c === '"' || c === "'" || c === "`") quote = c;
      out += c;
    }
  }
  return out;
}

export function importSpecifiers(source) {
  const found = new Set();
  // Raw text catches what a misread comment (a regex literal like /\/*$/, say) would hide;
  // stripped text catches imports split by comments.
  for (const code of [source, stripComments(source)]) {
    for (const pattern of SPECIFIER_PATTERNS) {
      for (const match of code.matchAll(pattern)) found.add(match[2]);
    }
  }
  // A directive is never allowed, so it is reported as written.
  for (const match of source.matchAll(DIRECTIVE)) found.add(match[0].trim());
  return [...found];
}

function isAllowed(specifier, file, pluginRoot) {
  // An escape sequence means something different to the bundler than to path.resolve.
  if (specifier.includes("\\")) return false;
  if (specifier === "@mesovibe/sdk" || specifier.startsWith("@mesovibe/sdk/")) return true;
  if (specifier === "." || specifier === ".." || specifier.startsWith("./") || specifier.startsWith("../")) {
    const target = resolve(dirname(file), specifier);
    return target === pluginRoot || target.startsWith(pluginRoot + sep);
  }
  return false;
}

/** Returns a list of violations: { file, specifier } for every disallowed import. */
export function checkPlugins(srcDir) {
  const root = resolve(srcDir);
  const violations = [];
  for (const name of readdirSync(root)) {
    const pluginRoot = join(root, name);
    if (!statSync(pluginRoot).isDirectory()) continue;
    for (const file of listFiles(pluginRoot)) {
      for (const specifier of importSpecifiers(readFileSync(file, "utf8"))) {
        if (!isAllowed(specifier, file, pluginRoot)) {
          violations.push({ file: relative(process.cwd(), file), specifier });
        }
      }
    }
  }
  return violations;
}

// Compare real paths so the CLI still runs when invoked through a symlink.
function isMain() {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

if (process.argv[1] && isMain()) {
  const dir = process.argv[2];
  if (!dir) {
    console.error("usage: check-plugin-imports.mjs <plugins-src-dir>");
    process.exit(2);
  }
  const violations = checkPlugins(dir);
  if (violations.length > 0) {
    console.error("Plugins may import only from @mesovibe/sdk or their own folder:");
    for (const v of violations) console.error(`  ${v.file}: "${v.specifier}"`);
    process.exit(1);
  }
  console.log("plugin imports ok");
}
