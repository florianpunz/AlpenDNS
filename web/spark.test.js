"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const context = {};
vm.createContext(context);
vm.runInContext(fs.readFileSync(path.join(__dirname, "spark.js"), "utf8"), context);
const { createSpark, rollSpark, countSpark } = context;

const SIZE = 30;
const SLOT = 2000;
const T0 = 1_700_000_000_000;

function filled() {
  const spark = createSpark(SIZE, SLOT, T0);
  for (let slot = 0; slot < SIZE + 5; slot += 1) {
    const start = T0 + slot * SLOT;
    for (let n = 0; n < (slot * 7) % 11; n += 1) {
      countSpark(
        spark,
        { blocked: n % 3 === 0, source: n % 2 ? "cache" : "upstream", ms: n * 4 + slot },
        start + n * 100,
      );
    }
  }
  return spark;
}

function snapshot(spark) {
  return Object.fromEntries(
    Object.entries(spark.series).map(([name, values]) => [name, [...values]]),
  );
}

test("a slot change shifts every finished point by exactly one", () => {
  const spark = filled();
  const before = snapshot(spark);
  const now = (spark.slot + 1) * SLOT;
  assert.equal(rollSpark(spark, now), true);
  for (const [name, values] of Object.entries(spark.series)) {
    assert.equal(values.length, SIZE, name);
    for (let i = 0; i < SIZE - 1; i += 1) {
      assert.equal(values[i], before[name][i + 1], `${name}[${i}]`);
    }
  }
});

test("events within the same slot leave every finished point alone", () => {
  const spark = filled();
  const before = snapshot(spark);
  const counted = spark.bucket.queries;
  const start = spark.slot * SLOT;
  for (let offset = 0; offset < SLOT; offset += 250) {
    assert.equal(countSpark(spark, { blocked: true, ms: 900 }, start + offset), false);
    assert.equal(rollSpark(spark, start + offset), false);
  }
  assert.deepEqual(snapshot(spark), before);
  assert.equal(spark.bucket.queries, counted + SLOT / 250);
});

test("three skipped slots become exactly three zeros", () => {
  const spark = filled();
  const start = (spark.slot + 1) * SLOT;
  countSpark(spark, { ms: 10 }, start);
  countSpark(spark, { ms: 30 }, start + 1);
  const before = snapshot(spark);
  assert.equal(rollSpark(spark, start + 4 * SLOT), true);
  for (const [name, values] of Object.entries(snapshot(spark))) {
    assert.equal(values.length, SIZE, name);
    assert.deepEqual(values.slice(0, SIZE - 4), before[name].slice(4), name);
    assert.deepEqual(values.slice(SIZE - 3), [0, 0, 0], name);
  }
  assert.equal(spark.series.queries[SIZE - 4], 2 / (SLOT / 1000));
  assert.equal(spark.series.latency[SIZE - 4], 20);
});

test("an event exactly on the slot boundary lands in the new slot", () => {
  const spark = createSpark(SIZE, SLOT, T0);
  const boundary = (spark.slot + 1) * SLOT;
  countSpark(spark, {}, boundary - 1);
  assert.equal(countSpark(spark, { blocked: true }, boundary), true);
  assert.equal(spark.series.queries[SIZE - 1], 1 / (SLOT / 1000));
  assert.equal(spark.series.blocked[SIZE - 1], 0);
  assert.equal(spark.bucket.queries, 1);
  assert.equal(spark.bucket.blocked, 1);
});
