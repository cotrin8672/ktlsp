# Performance targets for the next release comparison

Latest user priority: warm startup is sufficiently fast; prioritize resolving
valid-code Unknown results and missing useful completion. Further warm optimization
is paused. Under1s remains a preference, not a blocker for that semantic work.
Cold startup is lower priority. The former3s warm target below is superseded.
These are prospective editing targets on the originating Windows machine, not
semantic correctness deadlines or universal hardware requirements. Record results
even when a target is missed; a slow useful answer is not a semantic failure and
an empty fast answer is not a successful completion. Do not change expectations,
retry content requests or kill live builds to make a target pass.

The baseline is release18938, documented in fork-cem-startup120.md: CEM first useful
completion takes52.547s with a fresh LSP cache and1.689s with the same cache warm
(one observation each). Thirty warm requests have RPC p50/p95 of10.421/16.547ms;
input-to-menu handoff is44.026/51.686ms. Retrofit's original incomplete case still
takes1689.496ms in a quiet replay. These results precede lazy binary navigation
and the recovered-index file-lookup fix.

| Measured behavior | Next-release target | Required interpretation |
| --- | --- | --- |
| CEM cold first useful completion | <=30s, lower-priority aspiration | Fresh LSP cache, existing Gradle artifact home; also report full-index time separately |
| CEM warm first useful completion | <1s preference; optimization paused | Reuse the exact cache from the cold run; previously <=3s |
| CEM warm useful RPCs, n=30 | p95 <=50ms | Keep every observation and report p50/p95/max |
| CEM input-to-menu handoff, n=30 | p95 <=100ms | Inspect actual accepted text and restore the whole buffer; this is not pixel-display timing |
| Original five-project completion cases | Each <=100ms after readiness | First response only, unchanged semantic expectations; report startup/producer waiting separately |

Use the same uninstrumented release binary, pinned projects, JDK/Gradle homes,
cache policy and existing harness for comparisons. Start timing runs with no
other owned heavy workload; retain the process inventory, binary/input identities
and normal shutdown evidence. Sampling profiles run separately. Polling limits
the resolution of startup observations and must be reported. One startup run
does not establish a startup p95 or reproducible attainment of the cold target.

The old cold baseline and Retrofit outlier miss these targets. Release120-6's
quiet replay of the unchanged Retrofit case now observes6.613ms with a useful
first response, versus1689.496ms before (n=1 each, same genuine cache). It closes
normally with unchanged guards and no owned residuals. This demonstrates this
case's improvement, not a latency distribution or the remaining cases' targets.
Evidence: oracle120/retrofit1752-release6-quiet1/receipt.json.

The new CEM run observes35.969s cold (above the lower-priority aspiration),1.700s
warm (met the old3s target, misses the new1s goal), warm30
RPC p95 17.046ms and menu p95 51.228ms (within targets). All expected results,
actual UI insertions, unchanged guards and normal shutdown are retained. These
finite observations establish neither a startup distribution nor the entire
five-project row. The changed observed-result paragraph was captured before the
actual run, and the target table and conditions match the preparation bytes.
Evidence: cem-speed120-6/{performance-summary.json,budget-guard-audit.json}.
Release120-9 observes30.194s cold (n=1) and warm first-use samples of598.058,
593.061,601.994,1177.902 and602.495ms using the unchanged500ms polling and cache.
The finite warm n=5 has p50/p95/max of601.994/1062.820/1177.902ms; four samples
are under1s. This does not establish consistent subsecond startup or a population
percentile. The original30 RPCs have p50/p95/max10.298/18.577/23.163ms, and menu
handoff44.253/52.326/57.886ms. All31 actual insertions restore the whole buffer;
all original8 content checks pass in each startup run. All owned servers close
normally with unchanged439 input guards and zero forced cleanup. The four extra
warm runs omit only the later, separate UI server and preserve the original UI
evidence. Evidence: cem-speed120-9/{performance-summary.json,warm-startup-n5.json,
budget-guard-audit.json}.

Memory reductions must preserve useful results and latency.
