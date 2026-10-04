# CrossLatch (xlatch)

**Capability**: A named, typed action offered to a person or agent. Its manifest describes the input, output, and execution binding.

**Revision**: The content digest of a manifest. Approval and device grants refer to this exact digest.

**Approval**: The local operator's decision to activate a reviewed revision. Registration alone grants no execution authority.

**Device**: A paired client with its own signing key and explicit grants. It is not an Oqto Account or OS Principal.

**Grant**: Permission for a device to invoke one capability revision.

**Job**: A durable invocation, owned by its requester, with a terminal result or error.

**Resend**: An explicit new Job created from the retained input of a completed owned Job. It uses a fresh idempotency key and rechecks the selected target's current revision, grant, input contract, and artifact retention.

**Local operator**: The trusted OS user running the daemon. Local agents sharing that identity share its authority in v0.

**Outbox item**: Content saved on a client for delivery to one exact capability revision, with a stable invocation ID. It is not a server Job until accepted.

**Parked item**: Owner-scoped content saved durably on the server before any final target is selected. Dispatch rechecks the current grant and contract, creates a Job, and removes the parked item.

**Preparation**: An optional exact granted capability invoked when content is parked. Its durable Job remains linked to the parked item; successful typed output becomes cached context while failure never replaces or removes the original content.

**Composition**: A capability that passes results through an ordered set of exact capability revisions, with its own approval and grants.
