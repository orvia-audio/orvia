import assert from "node:assert/strict";
import test from "node:test";
import {matchingPulls, pullMergeBase} from "./pulls.mjs";

test("finds a fork head on a later page and preserves distinct target choices", async () => {
  const fork = {head: {sha: "ab12cde9", repo: {full_name: "example/orvia"}}};
  const matches = [{...fork, number: 2}, {...fork, number: 5}];
  const rows = Array.from({length: 100}, () => ({head: {sha: "0000"}}));
  const result = await matchingPulls(async path => {
    assert.match(path, /state=open/);
    return path.endsWith("page=1") ? rows : matches;
  }, "ab12cde");
  assert.deepEqual(result, matches);
});

test("finds closed PRs and returns no candidates for an unrelated hash", async () => {
  const pr = {head: {sha: "ab12cde9"}};
  const request = async path => path.includes("state=open") ? [] : [pr];
  assert.deepEqual(await matchingPulls(request, "ab12cde"), [pr]);
  assert.deepEqual(await matchingPulls(request, "7654321"), []);
});

test("compares against the merge base of the supplied event SHAs after a force-push", async () => {
  const common = await pullMergeBase(async path => {
    assert.equal(path, "compare/target-at-event...head-at-event?per_page=1");
    return {merge_base_commit: {sha: "fork-point"}};
  }, {head: {sha: "new-head"}}, "head-at-event", "target-at-event");
  assert.equal(common, "fork-point");
});

test("recovers a merged PR's original range only for a verified merge parent", async () => {
  const pr = {merged_at: "2026-01-01", merge_commit_sha: "merge"};
  const responses = {
    "compare/current...head?per_page=1": {merge_base_commit: {sha: "head"}},
    "git/commits/merge": {parents: [{sha: "before-merge"}, {sha: "head"}]},
    "compare/before-merge...head?per_page=1": {merge_base_commit: {sha: "fork-point"}},
  };
  const request = async path => responses[path];
  assert.equal(await pullMergeBase(request, pr, "head", "current"), "fork-point");
  responses["git/commits/merge"] = {parents: [{sha: "unrelated"}]};
  assert.equal(await pullMergeBase(request, pr, "head", "current"), null);
});
