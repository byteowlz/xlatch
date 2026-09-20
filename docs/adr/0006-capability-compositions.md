# Revision-bound linear compositions

A composition is an independently approved capability containing the complete manifests, revisions and input mappings of 2–16 leaf steps. Its grant delegates only that reviewed sequence. It does not grant direct access to its children. Every child must remain active at the pinned revision; changing a dependency stops subsequent execution until the composition is reviewed again.

The broker schedules ordinary durable child jobs and uses the existing local or protected executor. Completed receipts are checkpointed before the next step is enqueued. Restart may advance a completed step, but interrupted execution fails rather than replaying uncertain side effects. Cancellation propagates to children. External events and recent-job listings describe the parent; authorized result retrieval exposes individual step receipts.

Direct connections require conservative schema compatibility. Explicit mappings are reviewed as part of the manifest and validated against the next input schema at runtime. The final result must satisfy the composition output schema, which may require a stronger delivery receipt than the final leaf advertises. Existing capabilities and grants retain their serialization and revisions.

Nested compositions and fan-out are excluded initially. This keeps delegated authority and recovery understandable while preserving an extension path. Phone gesture-based assembly must use these approval and grant boundaries, not an unrestricted client-provided execution plan.


One-off phone chains use job-local manifests and a transient parent marker, not active capability registrations. The authenticated requester must hold a direct grant for every leaf. The broker constructs the manifest from its own registry and rechecks every leaf grant and revision before execution/advancement. Saved compositions retain their separate parent-grant model. This distinction lets a swipe submit an already-authorized sequence without silently broadening reusable authority.

`chain_candidates` checks direct schema compatibility on the server. `invoke_chain` accepts exact ID/revision references, original input and a stable retry key. `save_chain` creates a fresh pending proposal; it does not activate or grant it. Existing protected local-transport restrictions remain in force. One-off chains do not currently enter the optional routing-history export; their operational jobs and step receipts remain durable.
