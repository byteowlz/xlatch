# Optional session bridge

`xlatch-sessions` lets xlatch send text, or text with one file, to live coding-agent
sessions (Claude Code, Codex, pi, ...) that are not registered as capabilities.
It ships with xlatch but does nothing until an operator registers it. The safety
rules below are deliberate: the grant authorizes the bridge, and every send
re-checks its target against the live session list.

| Capability | Does | Input |
|---|---|---|
| `sessions.list` | Lists live agent sessions: id, provider, address, agent, status, cwd, title. Reads no screen content. | `{}` |
| `sessions.send` | Re-checks the target, then pastes the text (hc places a file first and names its landed path). | `{"target":"hc:t7","text":"...","file":{...}}` |

Providers:

- **hc** (`herdr-core`): terminals whose detected agent is not a shell. Addresses
  may name other machines (`machine/t7`). Files use `hc put`.
- **herdr** (legacy, during migration): `herdr agent list`/`prompt`. Text only;
  blocked agents are refused. hc's attach mirrors of herdr panes are shells, so
  each session is listed once.

Never: plain shells, forced input into a blocked program, keys or interrupts,
reading screens, creating sessions, automatic resends.

## Enable

Install the adapter where the executor runs (in protected mode: an
administrator-owned directory, not Homebrew or `~/.local`), then let it write
manifests bound to its own path and hash and to your provider programs:

```sh
just sessions-manifests ~/.local/bin/hc ~/.local/bin/herdr target/sessions-manifests
xlatch register target/sessions-manifests/sessions.list.json
xlatch register target/sessions-manifests/sessions.send.json
xlatch approve sessions.list --revision REV --allow-host-execution
xlatch approve sessions.send --revision REV --allow-host-execution
```

Omit `--herdr` (pass `""` to the recipe) once migration to hc is complete; that
changes the manifests, so re-register and approve the new revisions. Updating the
adapter binary changes its hash and likewise needs new revisions.

## Use

```sh
echo '{}' > list.json
xlatch invoke sessions.list --revision REV --input list.json --wait
echo '{"target":"herdr:w2:p88","text":"Please review the latest diff."}' > send.json
xlatch invoke sessions.send --revision REV --input send.json --wait
```

The receipt records provider, target, agent, the SHA-256 of the exact text typed
and any landed file path. A paired device can use it only after an explicit
`xlatch grant` of that revision; prompting an agent can make it run code as the
executor's user.

The adapter runs as the executor's ordinary user and restores `HOME` from the
account database, because command execution clears the environment.
