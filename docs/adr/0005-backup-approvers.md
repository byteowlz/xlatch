# Backup approvers require an existing approver signature

An active client can propose its own P-256 approval key, with proof of possession bound to its device and server. That proposal grants nothing. An existing, different approver signs the exact server-generated, expiring review to promote it. Reviews bind the request key, approval key, operation and server identity, and are consumed transactionally.

A surviving approver can revoke another approver through the same review protocol. Revocation also revokes device access and cancels outstanding jobs. Self-removal is forbidden, ensuring an authenticated reviewer remains. Configure a backup before losing the original phone. If all approval keys are lost, recovery still requires a new server identity and explicit re-pairing; local operator access does not substitute for a biometric approval.

This extends ADR-0001's single-approver limitation. Protected installation migration still intentionally accepts exactly one verified approver; configure additional approvers after migration. iOS uses the existing Secure Enclave biometric key; the protocol remains independent of client platform. A signature proves key possession, not remote attestation of biometric hardware.
