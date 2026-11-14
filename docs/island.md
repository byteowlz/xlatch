# Optional anchr island

`xlatch-island` shows xlatch job activity on the anchr desktop island. It ships with xlatch, is never started automatically, and xlatch works the
same without it or without anchr.

What appears:

| anchr activity | When | Shows |
|---|---|---|
| `jobs` | Jobs are queued or running | Title of the newest running (else queued) action, `xlatch · running · N more`. Leased, refreshed every interval, withdrawn when idle. |
| `result` | A job the companion saw, or one created after it started, finishes | Its action title, `xlatch · done`, `failed` or `cancelled`, for 6 s (failed: 12 s). One outcome per refresh, the newest. |

Only action titles and job states cross to anchr. Shared content, targets,
results, error text, file names and paths never do. History from before the
companion started is not announced. If xlatch is unreachable, in-flight work is
withdrawn rather than guessed; if the companion stops, the leases expire on their
own. Expiry is not success.

## Run

```sh
xlatch-island run                          # local server, refresh every 3 s
xlatch-island --client-config PATH run     # paired: this device's own jobs, every 10 s
xlatch-island --dry-run run                # print frames as JSON lines, send nothing
xlatch-island once                         # one refresh; prints and sends its frames
```

It runs in the foreground as your user: start it in an hc/herdr terminal or under
dpty. Do not create a launchd agent for it on managed Macs. It uses anchr's socket
(`--socket`, `ANCHR_SOCKET`, then anchr's XDG location). When anchr is not running
it reports that once and keeps trying.

## Scope and limits

This uses anchr's implemented status-only transport (`publish`/`withdraw`). The
`source` is a claim, not an identity, and the island cannot act on xlatch: there
are no actions. When anchr's surface protocol host lands (session, grants,
declared actions), an xlatch contributor manifest could offer `open_app` and a
`control` cancel with parity `xlatch cancel JOB`; sending stays in xlatch, since
anchr rejects `outbound` effects.

Verified: unit tests for every transition and the wire exchange; a real
loopback xlatch server with a slow command capability, every emitted frame
accepted by `anchr validate`. Not yet verified: a live anchr host showing them,
and the paired mode against a real paired server.
