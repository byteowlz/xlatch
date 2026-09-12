# Protected service setup

The development user service remains the default. Protected mode separates the approval broker from executable capability handlers. It is not a sandbox for actions: the executor retains your ordinary user's access to files and credentials.

1. Update xlatch and the iPhone app. Refresh the existing connection so the app remembers its server identity.
2. In Server → Device approvals, follow the displayed `xlatch enrollment-bootstrap DEVICE_ID` command, scan its terminal QR with **Scan setup code**, and enable approval with biometrics. Existing devices do not become approvers automatically.
3. Tap **Verify installation fingerprint** on the approver phone. The fingerprint comes from that phone's actual keys and remembered server identity, not from the server database.
4. Review the installation plan, using your usual listen address:

   ```sh
   xlatch service enable --protected --executor-user YOUR_USER --listen 0.0.0.0:7898 --dry-run
   ```

5. Finish outstanding jobs, stop the existing user daemon, and run the same installation with administrator privileges, the original data directory, and the phone's fingerprint:

   ```sh
   sudo /absolute/path/to/xlatch --data-dir /absolute/path/to/original/data service enable --protected --executor-user YOUR_USER --listen 0.0.0.0:7898 --approver-fingerprint FINGERPRINT
   ```

   The installer refuses a running daemon, outstanding jobs, an existing protected installation/account, or a mismatched fingerprint. It creates `_xlatch` and two system services. The original data stays in place. Failed installation leaves its partial files for inspection; it does not delete accounts or silently overwrite an existing installation.

6. On macOS, display the new public certificate identity using the installed binary:

   ```sh
   "/Library/Application Support/xlatch/xlatch" --control-dir "/Library/Application Support/xlatch/run" identity
   ```

   On the approver phone, open Server → Update server identity, scan the QR, review and authenticate. The app checks the remembered server identity, verifies connectivity with its existing device key, and retains the existing approval key. Other devices must pair again and receive phone approval. On Linux the corresponding paths are `/var/lib/xlatch/xlatch` and `/var/lib/xlatch/run`.

Use `xlatch service status --protected` to inspect the broker and executor. Start, stop, restart and disable accept the same flag; operations that change a system service need administrator privileges. Disabling stops both services and retains their data and definitions.

Local registry/pairing commands use `--control-dir` with the installed service's run directory. The protected local socket denies capability approval, grant changes, revocation and invocation. Those operations must not be restored as local shortcuts: phone workflows for them are still pending. Only the verified approver survives the initial protected migration, and already approved capabilities retain their revisions.

Losing the sole approver or changing its enrolled biometrics currently requires an explicit new-identity setup, not a CLI unlock. Additional approvers and signed recovery/replacement remain tracked work. A compromised administrator/root account can replace the broker; that is outside the boundary.
