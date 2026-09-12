# CrossLatch (xlatch)

**Capability**: A named, typed action offered to a person or agent. Its manifest describes the input, output, and execution binding.

**Revision**: The content digest of a manifest. Approval and device grants refer to this exact digest.

**Approval**: The local operator's decision to activate a reviewed revision. Registration alone grants no execution authority.

**Device**: A paired client with its own signing key and explicit grants. It is not an Oqto Account or OS Principal.

**Grant**: Permission for a device to invoke one capability revision.

**Job**: A durable invocation, owned by its requester, with a terminal result or error.

**Local operator**: The trusted OS user running the daemon. Local agents sharing that identity share its authority in v0.
