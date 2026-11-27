"use strict";

function sparkSlot(now, slotMs) {
  return Math.floor(now / slotMs);
}

function emptySparkBucket() {
  return { queries: 0, blocked: 0, cache: 0, ms: 0 };
}

function sparkPoint(bucket, seconds) {
  const queries = bucket.queries;
  return {
    queries: queries / seconds,
    blocked: queries ? (bucket.blocked / queries) * 100 : 0,
    cache: queries ? (bucket.cache / queries) * 100 : 0,
    latency: queries ? bucket.ms / queries : 0,
  };
}

function createSpark(size, slotMs, now) {
  return {
    size,
    slotMs,
    slot: sparkSlot(now, slotMs),
    bucket: emptySparkBucket(),
    series: {
      queries: new Array(size).fill(0),
      blocked: new Array(size).fill(0),
      cache: new Array(size).fill(0),
      latency: new Array(size).fill(0),
    },
  };
}

function pushSparkPoint(spark, point) {
  for (const [name, values] of Object.entries(spark.series)) {
    values.push(point[name]);
    if (values.length > spark.size) values.shift();
  }
}

function rollSpark(spark, now) {
  const slot = sparkSlot(now, spark.slotMs);
  if (slot <= spark.slot) return false;
  pushSparkPoint(spark, sparkPoint(spark.bucket, spark.slotMs / 1000));
  const empty = sparkPoint(emptySparkBucket(), 1);
  const gap = Math.min(slot - spark.slot - 1, spark.size);
  for (let i = 0; i < gap; i += 1) pushSparkPoint(spark, empty);
  spark.slot = slot;
  spark.bucket = emptySparkBucket();
  return true;
}

function countSpark(spark, event, now) {
  const rolled = rollSpark(spark, now);
  spark.bucket.queries += 1 + (event.skipped ?? 0);
  if (event.blocked) spark.bucket.blocked += 1;
  if (event.source === "cache") spark.bucket.cache += 1;
  spark.bucket.ms += Number(event.ms ?? 0);
  return rolled;
}
