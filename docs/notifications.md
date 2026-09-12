# Notifications

## Apprise

The daemon optionally reads `$XDG_CONFIG_HOME/xlatch/notifications.toml`
(default `~/.config/xlatch/notifications.toml` on macOS/Linux) at startup:

```toml
endpoint = "http://127.0.0.1:8000/notify/xlatch"
tag = "phone"
# token_env = "XLATCH_APPRISE_TOKEN"
```

Run [Apprise API](https://github.com/caronc/apprise-api) separately and configure
its `xlatch` key with your destinations and `phone` tag. Destination credentials
stay in Apprise. The optional bearer token authenticates an API proxy that
supports bearer authentication; it is not an Apprise destination credential.
Never expose an unauthenticated Apprise API publicly. Use HTTPS for non-loopback
endpoints. xlatch refuses redirects and does not use environment HTTP proxies.

Restart xlatch after changing configuration. With no file, this integration is
disabled. Removing it and restarting disables delivery. This is an operator-wide
notification destination: it receives generic completion, failure and cancellation
alerts for all jobs on this daemon. No job input, output, identifier, filename,
capability name or error detail is transmitted.

A new endpoint/tag combination starts with future events. Its acknowledged event
cursor is persisted in `notifications.sqlite3` under the daemon data directory.
An unavailable destination retries with backoff up to five minutes between
attempts and a 15-second HTTP timeout. Pending events survive restart; a failed
notification blocks later notifications for that destination, never job execution.
Disabling and re-enabling the same destination resumes its prior cursor.
Apprise acceptance is not proof of phone delivery. An uncertain HTTP outcome or
restart after delivery but before cursor commit can cause duplicate alerts.
No notification payload is stored in the cursor database; the existing job/event
retention is unchanged. Apprise and downstream services have their own retention.

Validate the HTTP adapter without sending real notifications using
`cargo build -p xlatch` followed by
`python3 crates/xlatch-cli/tests/apprise_delivery.py` from the repository root.

## Native iPhone push: remaining implementation

Native push is not connected yet. The current app uses local notifications while
refreshing foreground results. Apprise notifications arrive in the destination
app, such as ntfy.

1. Enable Push Notifications for `com.byteowlz.xlatch` in the Apple developer
   account, regenerate provisioning and include the APS entitlement. Register
   with APNs each launch and forward refreshed tokens over an authenticated channel.
2. Deploy a small push gateway with a topic/environment-scoped APNs key in secret
   storage. Separate development and production. Do not distribute its signing
   key with xlatch servers or apps.
3. Have the phone authorize each paired server to notify it. Store revocable,
   narrowly scoped routing grants, not an unrestricted public forwarding API.
   Add token rotation, invalid-token cleanup, quotas, replay protection and bounded
   payloads. Jobs stay on the user's server.
4. Send generic alert pushes with opaque references, no message content. On tap,
   open the correct server/job and retrieve results using the existing device
   authorization. Reconcile events on app resume even if no push arrived.
5. Validate background, locked-phone, offline, revoked-server and production
   TestFlight delivery on a physical iPhone. APNs is best-effort.
6. Optional encrypted previews need a notification service extension and a
   reviewed encryption protocol with separate recipient encryption keys. Do not
   repurpose Ed25519 device signing keys as encryption keys. Gateway no-payload
   retention is distinct from Apple's delivery storage and infrastructure metadata.

Hosting remains undecided. Verify actual Worker-to-APNs HTTP/2 delivery before
choosing Cloudflare Workers; otherwise use a small VPS. No gateway has been deployed.
