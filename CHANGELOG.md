# Changelog

All notable changes to this project will be documented in this file.

## Unreleased

### Changed

- Extracted store-independent Ed25519 enrollment/request proofs into `xlatch-wire-auth`, preserving existing signing bytes and adding a separate Oqto personal-host domain. Ticket consumption, grants, replay storage and device revocation remain owned by the embedding service; no personal-host transport is implied by this extraction.

