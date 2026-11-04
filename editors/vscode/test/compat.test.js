const { test } = require("node:test");
const assert = require("node:assert/strict");
const { releaseMismatch } = require("../compat");

test("the same release is fine, and a different one is called out with both versions", () => {
  assert.equal(releaseMismatch("0.0.1-alpha.10", "0.0.1-alpha.10"), null);
  const message = releaseMismatch("0.0.1-alpha.11", "0.0.1-alpha.10");
  assert.match(message, /0\.0\.1-alpha\.11/);
  assert.match(message, /0\.0\.1-alpha\.10/);
});

test("a development build has nothing to compare", () => {
  assert.equal(releaseMismatch(null, "0.0.1-alpha.10"), null);
  assert.equal(releaseMismatch("0.0.1-alpha.10", undefined), null);
});
