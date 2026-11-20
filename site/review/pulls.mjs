/** Find PRs by their published head, including fork heads omitted by GitHub's commit association API. */
export async function matchingPulls(request, prefix) {
  for (const state of ["open", "closed"]) {
    const matches = [];
    for (let page = 1; ; page++) {
      const rows = await request("pulls?state=" + state + "&per_page=100&page=" + page);
      matches.push(...rows.filter(pr => pr.head.sha.startsWith(prefix)));
      if (rows.length < 100) break;
    }
    if (matches.length) return matches;
  }
  return [];
}

/** Resolve the same three-dot comparison as a PR, keeping target-only changes out of the diff. */
export async function pullMergeBase(request, pr, head, target) {
  async function ancestor(left) {
    const comparison = await request("compare/" + left + "..." + head + "?per_page=1");
    return comparison.merge_base_commit.sha;
  }
  const common = await ancestor(target);
  if (common !== head || !pr.merged_at) return common;
  const merge = await request("git/commits/" + pr.merge_commit_sha);
  // When the target already contains the head, recover its pre-merge tip if available.
  if (merge.parents[1]?.sha !== head) return null;
  return ancestor(merge.parents[0].sha);
}
