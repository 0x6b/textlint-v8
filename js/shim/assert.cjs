function fail(message) {
  throw new Error(message || "assertion failed");
}

function assert(value, message) {
  if (!value) fail(message);
}

assert.ok = assert;
assert.strictEqual = function strictEqual(actual, expected, message) {
  if (actual !== expected) {
    fail(message || `expected ${String(actual)} to equal ${String(expected)}`);
  }
};
assert.doesNotThrow = function doesNotThrow(block) {
  block();
};

module.exports = assert;
